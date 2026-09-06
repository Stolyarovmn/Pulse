//! HTTP-сервер экспорта.
//!
//! Тонкий, без внешних зависимостей: один поток-акцептор, последовательная обработка
//! соединений, `Connection: close`. Безопасность: bearer-токен из файла (сравнение за
//! константное время), rate-limit по IP с ограниченной таблицей, жёсткие лимиты
//! запроса. Отказы не собирают тело метрик.

use std::io::{Error as IoError, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pulse_core::config::Export;
use pulse_core::snapshot::SnapshotSource;

use crate::limits::RateLimiter;
use crate::render::render_openmetrics;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_REQUEST_BYTES: usize = 8 * 1024;
const MAX_TRACKED_ADDRESSES: usize = 1024;

/// Максимум одновременно обрабатываемых соединений.
///
/// Диагностическому эндпоинту больше не нужно: реальных потребителей единицы
/// (Prometheus, оператор с curl). Лимит превращает неограниченное создание
/// потоков в предсказуемый отказ 503.
const MAX_INFLIGHT: usize = 8;

/// Полный бюджет времени на чтение заголовка одного запроса.
///
/// Сервер обрабатывает соединения последовательно, поэтому ограничивать нужно
/// не только паузу между чтениями (таймаут сокета), но и суммарную длительность:
/// иначе один медленный клиент занимает единственный поток произвольно долго.
const REQUEST_BUDGET: Duration = Duration::from_secs(10);

/// Бюджет на вычитывание запроса на пути отказа (429): короче обычного.
const DRAIN_BUDGET: Duration = Duration::from_millis(1_000);

const INDEX_HTML: &str = "<!doctype html>\n<html><head><title>Pulse</title></head><body><p>Pulse export endpoint. <a href=\"/metrics\">/metrics</a>, <a href=\"/healthz\">/healthz</a>.</p></body></html>\n";

/// Счётчики работы сервера. Обновляются атомарно через `Mutex` — объём работы мал,
/// простота важнее скорости.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportRuntimeStats {
    /// Обработанные HTTP-запросы (включая отказы после прохождения rate-limit).
    pub requests: u64,
    /// Отклонённые запросы (401/404/405/413/414/429).
    pub rejected: u64,
    /// Серии в последнем успешно рендеренном ответе.
    pub series: usize,
    /// Серии, отброшенные в последнем рендере.
    pub dropped: u64,
}

/// Дескриптор запущенного сервера. `shutdown` завершает поток-акцептор.
pub struct ExportHandle {
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<ExportRuntimeStats>>,
    addr: std::net::SocketAddr,
}

impl ExportHandle {
    /// Фактический адрес, к которому сервер привязался (при `port = 0` — реальный порт).
    #[must_use]
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Текущая статистика.
    #[must_use]
    pub fn stats(&self) -> ExportRuntimeStats {
        self.stats.lock().map(|s| *s).unwrap_or_default()
    }

    /// Останавливает сервер. Поток завершается в течение ~10 мс.
    pub fn shutdown(self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Запускает экспорт-сервер в фоновом потоке.
///
/// Отказ старта:
/// * экспорт отключён в конфигурации;
/// * не-loopback bind без токена (нарушение политики `pulse-core`);
/// * токен короче 16 байт или файл не читается.
pub fn spawn(cfg: &Export, source: SnapshotSource) -> std::io::Result<ExportHandle> {
    if !cfg.enabled {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "экспорт отключён в конфигурации",
        ));
    }
    let token = load_token(cfg)?;
    if !cfg.bind.ip().is_loopback() && token.is_none() {
        return Err(IoError::new(
            ErrorKind::PermissionDenied,
            "токен обязателен для не-loopback адреса",
        ));
    }

    let listener = TcpListener::bind(cfg.bind)?;
    let addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;

    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(Mutex::new(ExportRuntimeStats::default()));
    let stop_thread = Arc::clone(&stop);
    let stats_thread = Arc::clone(&stats);
    let config = cfg.clone();

    std::thread::Builder::new()
        .name("pulse-export".into())
        .spawn(move || {
            serve(listener, source, config, token, stats_thread, stop_thread);
        })
        .map_err(|e| IoError::other(format!("не удалось запустить поток экспорта: {e}")))?;

    Ok(ExportHandle { stop, stats, addr })
}

/// Читает токен из файла через единый загрузчик [`crate::auth::load_token`].
fn load_token(cfg: &Export) -> std::io::Result<Option<Vec<u8>>> {
    let Some(path) = &cfg.token_file else {
        return Ok(None);
    };
    let token = crate::auth::load_token(path).map_err(|e| {
        IoError::new(
            ErrorKind::InvalidData,
            format!("не удалось прочитать токен {}: {e}", path.display()),
        )
    })?;
    Ok(Some(token))
}

/// Главный цикл: accept → rate-limit → обработка соединения.
///
/// Соединения обрабатываются в отдельных потоках с жёстким лимитом
/// одновременных обработчиков. Последовательная обработка означала бы
/// head-of-line blocking: одно медленное соединение задерживало бы scrape всем
/// остальным, не предъявив ни токена, ни валидного запроса. Лимит нужен, чтобы
/// защита от блокировки не превратилась в неограниченное создание потоков.
fn serve(
    listener: TcpListener,
    source: SnapshotSource,
    cfg: Export,
    token: Option<Vec<u8>>,
    stats: Arc<Mutex<ExportRuntimeStats>>,
    stop: Arc<AtomicBool>,
) {
    let mut limiter = RateLimiter::new(cfg.rate_limit_per_minute, MAX_TRACKED_ADDRESSES);
    let inflight = Arc::new(AtomicUsize::new(0));
    let token = Arc::new(token);
    let cfg = Arc::new(cfg);

    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !limiter.try_acquire(peer.ip()) {
                    bump(&stats, 1, 1);
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    // Запрос нужно вычитать до ответа: если закрыть сокет, оставив
                    // непрочитанные данные в очереди приёма, ядро пошлёт RST и
                    // клиент получит «сброс соединения» вместо кода 429.
                    let _ = drain_request(&mut stream);
                    let _ = write_response(
                        &mut stream,
                        true,
                        429,
                        "Too Many Requests",
                        "text/plain; charset=utf-8",
                        "rate limited\n",
                        None,
                    );
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    continue;
                }

                if inflight.load(Ordering::Acquire) >= MAX_INFLIGHT {
                    // Отказ немедленный и без чтения тела: очередь занята, и
                    // тратить на этого клиента время обработки нельзя.
                    bump(&stats, 1, 1);
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let _ = write_response(
                        &mut stream,
                        true,
                        503,
                        "Service Unavailable",
                        "text/plain; charset=utf-8",
                        "too many connections\n",
                        None,
                    );
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    continue;
                }

                bump(&stats, 1, 0);
                inflight.fetch_add(1, Ordering::AcqRel);

                let worker_source = Arc::clone(&source);
                let worker_cfg = Arc::clone(&cfg);
                let worker_token = Arc::clone(&token);
                let worker_stats = Arc::clone(&stats);
                let worker_inflight = Arc::clone(&inflight);
                let spawned = std::thread::Builder::new()
                    .name("pulse-export-conn".into())
                    .spawn(move || {
                        handle_connection(
                            stream,
                            &worker_source,
                            &worker_cfg,
                            &worker_token,
                            &worker_stats,
                        );
                        worker_inflight.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    // Поток не создался (лимит ОС): счётчик обязан вернуться,
                    // иначе сервер навсегда решит, что очередь занята.
                    inflight.fetch_sub(1, Ordering::AcqRel);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            // Ошибка акцепта (например, listener закрыт) — останавливаем сервер.
            Err(_) => return,
        }
    }
}

/// Вычитывает запрос до конца заголовка либо до `MAX_HEADER_BYTES`.
///
/// Используется на путях отказа: ответ без чтения запроса приводит к RST.
/// Ограничен и по объёму, и по общему времени: путь отказа не должен давать
/// клиенту способ занять однопоточный сервер надолго.
fn drain_request(stream: &mut TcpStream) -> std::io::Result<()> {
    let deadline = Instant::now() + DRAIN_BUDGET;
    let mut buf = [0u8; 1024];
    let mut total = 0usize;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                total = total.saturating_add(n);
                if total >= MAX_HEADER_BYTES || Instant::now() >= deadline {
                    return Ok(());
                }
                if buf.get(..n).is_some_and(|s| find_header_end(s).is_some()) {
                    return Ok(());
                }
            }
            Err(_) => return Ok(()),
        }
    }
}

/// Обработка одного соединения. Читаем только заголовок; тело не читается
/// (GET/HEAD не должны нести полезную нагрузку).
fn handle_connection(
    mut stream: TcpStream,
    source: &SnapshotSource,
    cfg: &Export,
    token: &Option<Vec<u8>>,
    stats: &Arc<Mutex<ExportRuntimeStats>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let header = match read_headers(&mut stream, Instant::now() + REQUEST_BUDGET) {
        Ok(buf) => buf,
        Err(RequestError::TooLarge) => {
            reject(
                &mut stream,
                stats,
                false,
                413,
                "Payload Too Large",
                "payload too large\n",
                None,
            );
            return;
        }
        Err(RequestError::Bad) => return,
    };

    let req = match parse_request(&header) {
        Ok(r) => r,
        Err(ParseError::MethodNotAllowed) => {
            reject(
                &mut stream,
                stats,
                false,
                405,
                "Method Not Allowed",
                "method not allowed\n",
                None,
            );
            return;
        }
        Err(ParseError::TooLong) => {
            reject(
                &mut stream,
                stats,
                false,
                414,
                "URI Too Long",
                "uri too long\n",
                None,
            );
            return;
        }
        Err(ParseError::TooLarge) => {
            reject(
                &mut stream,
                stats,
                false,
                413,
                "Payload Too Large",
                "payload too large\n",
                None,
            );
            return;
        }
        Err(ParseError::Bad) => {
            reject(
                &mut stream,
                stats,
                false,
                400,
                "Bad Request",
                "bad request\n",
                None,
            );
            return;
        }
    };

    let include_body = req.method == "GET";
    // `Content-Length` проверен при разборе; само тело не используется.
    let _ = req.content_length;

    match req.path.as_str() {
        "/healthz" => {
            let _ = write_response(
                &mut stream,
                include_body,
                200,
                "OK",
                "text/plain; charset=utf-8",
                "ok\n",
                None,
            );
        }
        "/" => {
            let _ = write_response(
                &mut stream,
                include_body,
                200,
                "OK",
                "text/html; charset=utf-8",
                INDEX_HTML,
                None,
            );
        }
        "/metrics" => {
            // Проверка токена до сборки ответа: отказ не платит за рендер.
            if let Some(expected) = token {
                let ok = req
                    .authorization
                    .as_deref()
                    .and_then(crate::auth::parse_bearer)
                    .map(|provided| crate::auth::constant_time_equal(expected, provided.as_bytes()))
                    .unwrap_or(false);
                if !ok {
                    reject(
                        &mut stream,
                        stats,
                        include_body,
                        401,
                        "Unauthorized",
                        "authorization required\n",
                        Some(("WWW-Authenticate", "Bearer")),
                    );
                    return;
                }
            }

            let snapshot = source();
            let (body, render_stats) = render_openmetrics(&snapshot, cfg);
            if let Ok(mut s) = stats.lock() {
                s.series = render_stats.series;
                s.dropped = render_stats.dropped;
            }
            let _ = write_response(
                &mut stream,
                include_body,
                200,
                "OK",
                "text/plain; version=0.0.4; charset=utf-8",
                &body,
                None,
            );
        }
        _ => {
            reject(
                &mut stream,
                stats,
                include_body,
                404,
                "Not Found",
                "not found\n",
                None,
            );
        }
    }
}

/// Отказ с учётом статистики и опциональным заголовком (401 → `WWW-Authenticate`).
fn reject(
    stream: &mut TcpStream,
    stats: &Arc<Mutex<ExportRuntimeStats>>,
    include_body: bool,
    status: u16,
    reason: &str,
    body: &str,
    extra: Option<(&str, &str)>,
) {
    bump(stats, 0, 1);
    let _ = write_response(
        stream,
        include_body,
        status,
        reason,
        "text/plain; charset=utf-8",
        body,
        extra,
    );
}

/// Атомарное (через Mutex) увеличение счётчиков.
fn bump(stats: &Arc<Mutex<ExportRuntimeStats>>, requests: u64, rejected: u64) {
    if let Ok(mut s) = stats.lock() {
        s.requests = s.requests.saturating_add(requests);
        s.rejected = s.rejected.saturating_add(rejected);
    }
}

/// Читает заголовок HTTP до пустой строки.
///
/// Два независимых ограничения. `MAX_HEADER_BYTES` — против гигантского
/// заголовка. `deadline` — против медленного клиента: сервер однопоточный, и
/// клиент, присылающий по байту чуть быстрее таймаута чтения, иначе держал бы
/// `/metrics` заблокированным неограниченно долго, не предъявив токена.
fn read_headers(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, RequestError> {
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(buf),
            Ok(n) => {
                let n = n.min(chunk.len());
                buf.extend_from_slice(&chunk[..n]);
                if Instant::now() >= deadline {
                    return Err(RequestError::Bad);
                }
                if buf.len() > MAX_HEADER_BYTES {
                    return Err(RequestError::TooLarge);
                }
                if let Some(pos) = find_header_end(&buf) {
                    let end = pos + 4;
                    if end <= buf.len() {
                        buf.truncate(end);
                    }
                    return Ok(buf);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                return Err(RequestError::Bad);
            }
            Err(_) => return Err(RequestError::Bad),
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Ошибка чтения сырого запроса из сокета.
///
/// Разделена с [`ParseError`], потому что на этом этапе ещё неизвестно, какой
/// метод и путь запрашивались: отвечать можно только кодом транспортного уровня.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestError {
    /// Заголовок превысил `MAX_HEADER_BYTES` — вероятная попытка исчерпать память.
    TooLarge,
    /// Соединение оборвалось или дало нечитаемые данные: молча закрываем.
    Bad,
}

/// Ошибка разбора строки запроса и заголовков.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseError {
    /// Метод известен, но не поддерживается: ответ 405, а не 400.
    MethodNotAllowed,
    /// URI длиннее `MAX_URL_BYTES`: ответ 414.
    TooLong,
    /// Заявленное тело больше `MAX_REQUEST_BYTES`: ответ 413.
    TooLarge,
    /// Синтаксис запроса непригоден: ответ 400.
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedRequest {
    method: &'static str,
    path: String,
    content_length: Option<u64>,
    authorization: Option<String>,
}

fn parse_request(buf: &[u8]) -> Result<ParsedRequest, ParseError> {
    let text = String::from_utf8_lossy(buf);
    let mut lines = text.split("\r\n");
    let Some(first) = lines.next() else {
        return Err(ParseError::Bad);
    };

    let mut parts = first.splitn(3, ' ');
    let raw_method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");

    if target.len() > MAX_URL_BYTES {
        return Err(ParseError::TooLong);
    }
    if !version.starts_with("HTTP/") {
        return Err(ParseError::Bad);
    }

    let method_upper = raw_method.to_ascii_uppercase();
    let method = match method_upper.as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        // Прочие методы — 405, а не 400: сервер существует, но метод не поддерживается.
        "POST" | "PUT" | "DELETE" | "PATCH" | "OPTIONS" | "CONNECT" | "TRACE" => {
            return Err(ParseError::MethodNotAllowed)
        }
        _ => return Err(ParseError::Bad),
    };

    let path = target.split(['?', '#']).next().unwrap_or("").to_string();
    if path.is_empty() {
        return Err(ParseError::Bad);
    }

    let mut content_length: Option<u64> = None;
    let mut authorization: Option<String> = None;

    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut iter = line.splitn(2, ':');
        let key = iter.next().map(str::trim).unwrap_or("");
        let value = iter.next().map(str::trim).unwrap_or("");
        match key.to_ascii_lowercase().as_str() {
            "content-length" => {
                let parsed = value.parse::<u64>().map_err(|_| ParseError::Bad)?;
                if parsed > MAX_REQUEST_BYTES as u64 {
                    return Err(ParseError::TooLarge);
                }
                content_length = Some(parsed);
            }
            "authorization" => authorization = Some(value.to_string()),
            _ => {}
        }
    }

    Ok(ParsedRequest {
        method,
        path,
        content_length,
        authorization,
    })
}

/// Пишет ответ HTTP/1.1 с `Connection: close`.
fn write_response(
    stream: &mut TcpStream,
    include_body: bool,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
    extra: Option<(&str, &str)>,
) -> std::io::Result<()> {
    let mut head = String::with_capacity(192);
    head.push_str("HTTP/1.1 ");
    head.push_str(&status.to_string());
    head.push(' ');
    head.push_str(reason);
    head.push_str("\r\n");
    if let Some((k, v)) = extra {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("Content-Type: ");
    head.push_str(content_type);
    head.push_str("\r\n");
    head.push_str("Content-Length: ");
    head.push_str(&body.len().to_string());
    head.push_str("\r\n");
    head.push_str("Connection: close\r\n\r\n");

    stream.write_all(head.as_bytes())?;
    if include_body && !body.is_empty() {
        stream.write_all(body.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::{Entity, EntityKey, EntityKind, Labels};
    use pulse_core::time::Timestamp;

    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct TempFile(std::path::PathBuf);
    impl TempFile {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn temp_token(content: &str) -> TempFile {
        let n = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "pulse-export-test-{}-{n}-{nanos}.token",
            std::process::id()
        ));
        std::fs::write(&path, content).unwrap();
        TempFile(path)
    }

    fn host_snapshot() -> pulse_core::snapshot::Snapshot {
        let mut snap = pulse_core::snapshot::Snapshot::default();
        snap.host = pulse_core::entity::EntityId::new(1, 1);
        snap.entities = vec![Entity {
            id: snap.host,
            key: EntityKey::Host {
                boot_id: "boot-test".into(),
            },
            kind: EntityKind::Host,
            name: "test".into(),
            parent: None,
            labels: Labels::new(),
            logical: None,
            first_seen: Timestamp::default(),
            last_seen: Timestamp::default(),
            alive: true,
        }];
        snap
    }

    fn source_for(snapshot: pulse_core::snapshot::Snapshot) -> SnapshotSource {
        let arc: std::sync::Arc<pulse_core::snapshot::Snapshot> = Arc::new(snapshot);
        Arc::new(move || arc.clone())
    }

    fn http_request(addr: std::net::SocketAddr, request: &str) -> Option<String> {
        let mut stream = TcpStream::connect(addr).ok()?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.write_all(request.as_bytes());
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut out = String::new();
        stream.read_to_string(&mut out).ok()?;
        Some(out)
    }

    fn http_get(addr: std::net::SocketAddr, path: &str) -> Option<String> {
        http_request(
            addr,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
    }

    #[test]
    fn spawn_rejects_disabled_export() {
        let mut cfg = Export::default();
        cfg.enabled = false;
        assert!(spawn(&cfg, source_for(host_snapshot())).is_err());
    }

    #[test]
    fn spawn_rejects_non_loopback_without_token() {
        let mut cfg = Export::default();
        cfg.bind = "0.0.0.0:0".parse().unwrap();
        cfg.token_file = None;
        assert!(spawn(&cfg, source_for(host_snapshot())).is_err());
    }

    #[test]
    fn spawn_rejects_short_token() {
        let token_file = temp_token("0123456789abcde"); // 15 байт
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        cfg.token_file = Some(token_file.path().to_path_buf());
        assert!(spawn(&cfg, source_for(host_snapshot())).is_err());
    }

    #[test]
    fn loopback_without_token_serves_metrics() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        cfg.token_file = None;

        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let resp = http_get(addr, "/metrics").unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert!(resp.contains("pulse_agent_ticks_total"));
        assert!(resp.contains("# EOF"));

        handle.shutdown();
    }

    #[test]
    fn bearer_token_required_and_accepted() {
        let token = "0123456789abcdef0123456789abcdef";
        let token_file = temp_token(token);
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        cfg.token_file = Some(token_file.path().to_path_buf());

        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let unauth = http_request(
            addr,
            "GET /metrics HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        assert!(unauth.starts_with("HTTP/1.1 401"));
        assert!(unauth.contains("WWW-Authenticate: Bearer"));
        assert!(!unauth.contains("pulse_agent_ticks_total"));

        let wrong = http_request(
            addr,
            "GET /metrics HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer wrongtoken01234567\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        assert!(wrong.starts_with("HTTP/1.1 401"));

        let ok = http_request(
            addr,
            &format!(
                "GET /metrics HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
            ),
        )
        .unwrap();
        assert!(ok.starts_with("HTTP/1.1 200"));
        assert!(ok.contains("pulse_agent_ticks_total"));

        let stats = handle.stats();
        assert!(stats.requests >= 3);
        assert!(stats.rejected >= 2);

        handle.shutdown();
    }

    #[test]
    fn non_get_methods_rejected_with_405() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let resp = http_request(
            addr,
            "POST /metrics HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        assert!(resp.starts_with("HTTP/1.1 405"));
        handle.shutdown();
    }

    #[test]
    fn long_url_rejected_with_414() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let long = "a".repeat(9000);
        let resp = http_request(
            addr,
            &format!("GET /{long} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
        )
        .unwrap();
        assert!(resp.starts_with("HTTP/1.1 414"));
        handle.shutdown();
    }

    #[test]
    fn large_body_declared_rejected_with_413() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let resp = http_request(
            addr,
            "GET /metrics HTTP/1.1\r\nHost: x\r\nContent-Length: 9000\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
        assert!(resp.starts_with("HTTP/1.1 413"));
        handle.shutdown();
    }

    #[test]
    fn unknown_path_returns_404() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let resp = http_get(addr, "/nope").unwrap();
        assert!(resp.starts_with("HTTP/1.1 404"));
        handle.shutdown();
    }

    #[test]
    fn healthz_does_not_require_token() {
        let token = "0123456789abcdef0123456789abcdef";
        let token_file = temp_token(token);
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        cfg.token_file = Some(token_file.path().to_path_buf());

        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let resp = http_get(addr, "/healthz").unwrap();
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert!(resp.contains("ok"));
        handle.shutdown();
    }

    #[test]
    fn rate_limit_rejects_second_burst_request() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        cfg.rate_limit_per_minute = 1;

        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        let first = http_get(addr, "/metrics").unwrap();
        assert!(first.starts_with("HTTP/1.1 200"));

        let second = http_get(addr, "/metrics").unwrap();
        assert!(second.starts_with("HTTP/1.1 429"));

        let stats = handle.stats();
        assert!(stats.rejected >= 1);
        handle.shutdown();
    }
    /// Медленный клиент не должен занимать однопоточный сервер дольше бюджета:
    /// иначе одно соединение блокирует scrape для всех остальных.
    #[test]
    fn slow_client_cannot_block_the_server_indefinitely() {
        let mut cfg = Export::default();
        cfg.bind = "127.0.0.1:0".parse().unwrap();
        let handle = spawn(&cfg, source_for(host_snapshot())).unwrap();
        let addr = handle.local_addr();

        // Соединение, которое присылает заголовок по байту и никогда не
        // заканчивает его пустой строкой.
        let slow = std::thread::spawn(move || {
            let Ok(mut stream) = TcpStream::connect(addr) else {
                return;
            };
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            for _ in 0..64 {
                if stream.write_all(b"X").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        });

        // Даём медленному клиенту занять поток, затем проверяем, что сервер
        // всё равно обслуживает нормальный запрос за разумное время.
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        let resp = http_get(addr, "/healthz");
        let elapsed = started.elapsed();

        assert!(
            resp.is_some_and(|r| r.starts_with("HTTP/1.1 200")),
            "нормальный клиент обязан получить ответ"
        );
        assert!(
            elapsed < REQUEST_BUDGET + Duration::from_secs(5),
            "ожидание {elapsed:?} превысило бюджет запроса"
        );

        let _ = slow.join();
        handle.shutdown();
    }
}
