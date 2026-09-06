//! Детали одного процесса: пользователь, исполняемый файл, дескрипторы, порты.
//!
//! Читаются **по требованию**, а не каждый такт. Причина арифметическая: чтобы
//! узнать открытые файлы, нужно перечислить `/proc/<pid>/fd` и разыменовать
//! каждую ссылку. На хосте с двумя тысячами процессов это десятки тысяч
//! syscall на такт — ровно та стоимость, из-за которой агент сам становится
//! проблемой. Оператор же смотрит детали одного процесса, и только когда
//! спустился до него по цепочке расследования.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pulse_core::details::{OpenFile, Port, ProcessDetails, ProcessDetailsSource};
use pulse_core::redact::sanitize_display;

use crate::fs::{FsSource, FsSourceExt};

/// Чтение деталей процесса из `/proc`.
#[derive(Debug)]
pub struct ProcDetails {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
    /// Сколько дескрипторов показывать. Полный список у сервера с тысячей
    /// соединений бесполезен оператору и дорог для чтения.
    file_limit: usize,
}

impl ProcDetails {
    #[must_use]
    pub fn new(fs: Arc<dyn FsSource>, proc_root: PathBuf) -> Self {
        ProcDetails {
            fs,
            proc_root,
            file_limit: 12,
        }
    }
}

impl ProcessDetailsSource for ProcDetails {
    fn details(&self, pid: i32) -> ProcessDetails {
        let root = self.proc_root.join(pid.to_string());
        let mut out = ProcessDetails::default();

        if let Some(status) = self.fs.read_opt(&root.join("status")) {
            out.uid = parse_uid(&status);
        }
        out.user = out.uid.and_then(|uid| resolve_user(self.fs.as_ref(), uid));
        // Путь исполняемого файла и рабочий каталог задаёт сам процесс: имя
        // файла может содержать управляющие последовательности терминала.
        // Санитизация здесь, в источнике, а не в отрисовке: иначе каждый новый
        // потребитель деталей пришлось бы защищать заново.
        out.exe = self
            .fs
            .read_link(&root.join("exe"))
            .ok()
            .map(|path| sanitize_display(&path.to_string_lossy()));
        out.cwd = self
            .fs
            .read_link(&root.join("cwd"))
            .ok()
            .map(|path| sanitize_display(&path.to_string_lossy()));

        let (files, socket_inodes) = read_descriptors(self.fs.as_ref(), &root, self.file_limit);
        out.fd_total = files.total;
        out.files = files.files;
        out.restricted = files.restricted;

        if !socket_inodes.is_empty() {
            out.ports = listening_ports(self.fs.as_ref(), &self.proc_root, &socket_inodes);
        }
        out
    }
}

/// Результат обхода `/proc/<pid>/fd`.
struct Descriptors {
    total: usize,
    files: Vec<OpenFile>,
    /// Ядро отказало в доступе, а не вернуло пустой каталог.
    restricted: bool,
}

/// Читает дескрипторы: файлы для показа и inode сокетов для сопоставления.
fn read_descriptors(fs: &dyn FsSource, root: &Path, limit: usize) -> (Descriptors, Vec<u64>) {
    let mut files: Vec<OpenFile> = Vec::new();
    let mut sockets: Vec<u64> = Vec::new();
    let mut total = 0_usize;

    let entries = match fs.read_dir(&root.join("fd")) {
        Ok(entries) => entries,
        Err(error) => {
            // Отказ в правах и отсутствие процесса — разные факты. Первый
            // означает «смотри из-под root», второй «процесс уже умер», и
            // оператор обязан видеть, какой именно.
            let restricted = matches!(error.kind(), io::ErrorKind::PermissionDenied);
            return (
                Descriptors {
                    total,
                    files,
                    restricted,
                },
                sockets,
            );
        }
    };
    let mut numbers: Vec<u32> = entries
        .iter()
        .filter_map(|name| name.to_string_lossy().parse::<u32>().ok())
        .collect();
    numbers.sort_unstable();

    for fd in numbers {
        total += 1;
        let Ok(target) = fs.read_link(&root.join("fd").join(fd.to_string())) else {
            // Дескриптор закрылся между обходом и чтением: гонка, не ошибка.
            continue;
        };
        let target = sanitize_display(&target.to_string_lossy());
        if let Some(inode) = target
            .strip_prefix("socket:[")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|digits| digits.parse::<u64>().ok())
        {
            sockets.push(inode);
            continue;
        }
        if files.len() < limit && target.starts_with('/') {
            files.push(OpenFile { fd, target });
        }
    }
    (
        Descriptors {
            total,
            files,
            restricted: false,
        },
        sockets,
    )
}

/// Предел чтения таблиц сокетов ядра.
///
/// Хватает примерно на тридцать тысяч соединений: больше на одном хосте
/// означает, что задача не в диагностике одного процесса.
const NET_TABLE_CAP: usize = 4 * 1024 * 1024;

/// Слушающие сокеты, чьи inode принадлежат процессу.
fn listening_ports(fs: &dyn FsSource, proc_root: &Path, inodes: &[u64]) -> Vec<Port> {
    const SOURCES: [(&str, &str); 4] = [
        ("tcp", "net/tcp"),
        ("tcp6", "net/tcp6"),
        ("udp", "net/udp"),
        ("udp6", "net/udp6"),
    ];
    let mut out: Vec<Port> = Vec::new();
    for (protocol, relative) in SOURCES {
        // Явный лимит вместо `DEFAULT_CAP`: строка сокета занимает около 150
        // байт, поэтому 64 КиБ кончаются на четырёх сотнях соединений. На
        // сервере их бывают тысячи, и обрезка выбросила бы слушающий сокет -
        // экран показал бы «портов нет» там, где процесс слушает 443.
        let Ok(text) = fs.read_string_capped(&proc_root.join(relative), NET_TABLE_CAP) else {
            continue;
        };
        for socket in parse_net_sockets(&text, protocol) {
            if !inodes.contains(&socket.inode) {
                continue;
            }
            if out
                .iter()
                .any(|existing| existing.port == socket.port && existing.protocol == protocol)
            {
                continue;
            }
            out.push(Port {
                protocol,
                address: socket.address,
                port: socket.port,
            });
        }
    }
    out.sort_by_key(|port| (port.port, port.protocol));
    out
}

/// Слушающий сокет из `/proc/net/*`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetSocket {
    pub address: String,
    pub port: u16,
    pub inode: u64,
}

/// Разбирает `/proc/net/tcp`-подобный файл, оставляя только слушающие сокеты.
///
/// Состояние `0A` — `TCP_LISTEN`. UDP не имеет состояний, и там признаком
/// служит нулевой удалённый адрес: сокет ничего не подключал.
#[must_use]
pub fn parse_net_sockets(text: &str, protocol: &str) -> Vec<NetSocket> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // sl local_address rem_address st ... uid timeout inode
        let (Some(local), Some(remote), Some(state), Some(inode)) =
            (fields.get(1), fields.get(2), fields.get(3), fields.get(9))
        else {
            continue;
        };
        let listening = if protocol.starts_with("tcp") {
            *state == "0A"
        } else {
            remote.ends_with(":0000") || remote.ends_with(":0")
        };
        if !listening {
            continue;
        }
        let Some((address, port)) = parse_hex_endpoint(local) else {
            continue;
        };
        let Ok(inode) = inode.parse::<u64>() else {
            continue;
        };
        out.push(NetSocket {
            address,
            port,
            inode,
        });
    }
    out
}

/// Разбирает `0100007F:1F90` в `127.0.0.1` и `8080`.
fn parse_hex_endpoint(raw: &str) -> Option<(String, u16)> {
    let (address, port) = raw.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let address = match address.len() {
        8 => {
            let octets = u32::from_str_radix(address, 16).ok()?.to_le_bytes();
            format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
        }
        32 => {
            // IPv6 печатается как есть, кроме двух частых случаев: слушающий
            // сокет на всех адресах и loopback.
            if address.chars().all(|c| c == '0') {
                "[::]".to_string()
            } else if address == "00000000000000000000000001000000" {
                "[::1]".to_string()
            } else {
                "[ipv6]".to_string()
            }
        }
        _ => return None,
    };
    Some((address, port))
}

/// `Uid:` из `/proc/<pid>/status`.
fn parse_uid(status: &str) -> Option<u32> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Имя пользователя по uid из `/etc/passwd`.
///
/// Без NSS: библиотечный `getpwuid` в статическом musl-бинарнике не работает,
/// а сетевые директории требовали бы блокирующих запросов из интерфейса.
fn resolve_user(fs: &dyn FsSource, uid: u32) -> Option<String> {
    let passwd = fs.read_opt(Path::new("/etc/passwd"))?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _ = fields.next();
        let entry_uid: u32 = fields.next()?.parse().ok()?;
        if entry_uid == uid {
            return Some(sanitize_display(name));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;

    const STATUS: &str = "Name:\tnginx\nUid:\t33\t33\t33\t33\nGid:\t33\t33\t33\t33\n";
    const PASSWD: &str =
        "root:x:0:0:root:/root:/bin/bash\nwww-data:x:33:33:www-data:/var/www:/usr/sbin/nologin\n";
    // sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000    33        0 555001
   1: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000    33        0 555002
   2: 0100007F:E4C2 0100007F:1F90 01 00000000:00000000 00:00000000 00000000    33        0 555003
";

    fn fixture() -> FixtureFs {
        FixtureFs::new()
            .file("/proc/42/status", STATUS)
            .file("/etc/passwd", PASSWD)
            .link("/proc/42/exe", "/usr/sbin/nginx")
            .link("/proc/42/cwd", "/var/www")
            .link("/proc/42/fd/0", "/dev/null")
            .link("/proc/42/fd/1", "/var/log/nginx/access.log")
            .link("/proc/42/fd/2", "/var/log/nginx/error.log")
            .link("/proc/42/fd/3", "socket:[555001]")
            .link("/proc/42/fd/4", "socket:[555002]")
            .link("/proc/42/fd/5", "socket:[555003]")
            .link("/proc/42/fd/6", "pipe:[999]")
            .file("/proc/net/tcp", TCP)
    }

    #[test]
    fn details_answer_user_exe_and_cwd() {
        let source = ProcDetails::new(Arc::new(fixture()), PathBuf::from("/proc"));
        let details = source.details(42);
        assert_eq!(details.uid, Some(33));
        assert_eq!(details.user.as_deref(), Some("www-data"));
        assert_eq!(details.exe.as_deref(), Some("/usr/sbin/nginx"));
        assert_eq!(details.cwd.as_deref(), Some("/var/www"));
    }

    #[test]
    fn files_are_regular_paths_and_count_is_total() {
        let source = ProcDetails::new(Arc::new(fixture()), PathBuf::from("/proc"));
        let details = source.details(42);
        assert_eq!(details.fd_total, 7, "считаются все дескрипторы");
        let targets: Vec<&str> = details.files.iter().map(|f| f.target.as_str()).collect();
        assert_eq!(
            targets,
            vec![
                "/dev/null",
                "/var/log/nginx/access.log",
                "/var/log/nginx/error.log"
            ],
            "сокеты и каналы не являются файлами"
        );
        assert!(details.files.iter().all(OpenFile::is_regular));
    }

    #[test]
    fn only_listening_sockets_of_this_process_become_ports() {
        let source = ProcDetails::new(Arc::new(fixture()), PathBuf::from("/proc"));
        let details = source.details(42);
        let ports: Vec<(u16, &str)> = details
            .ports
            .iter()
            .map(|port| (port.port, port.address.as_str()))
            .collect();
        assert_eq!(
            ports,
            vec![(80, "0.0.0.0"), (8080, "127.0.0.1")],
            "установленное соединение портом не является"
        );
    }

    #[test]
    fn listening_socket_is_found_behind_thousands_of_connections() {
        // Нагруженный сервер: таблица сокетов заведомо больше 64 КиБ, а
        // слушающий сокет лежит в её конце. Обрезка по умолчанию потеряла бы
        // именно его - экран сообщил бы, что процесс ничего не слушает.
        let mut table = String::from(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        );
        for index in 0..3_000 {
            table.push_str(&format!(
                "{index:5}: 0100007F:{:04X} 0100007F:1F90 01 00000000:00000000 00:00000000 00000000    33        0 {}\n",
                0xC000 + (index % 0x3000),
                600_000 + index
            ));
        }
        table.push_str(
            "9999: 00000000:01BB 00000000:0000 0A 00000000:00000000 00:00000000 00000000    33        0 555777\n",
        );
        assert!(
            table.len() > 64 * 1024,
            "фикстура обязана превышать лимит по умолчанию: {}",
            table.len()
        );

        let fixture = FixtureFs::new()
            .file("/proc/50/status", STATUS)
            .link("/proc/50/fd/3", "socket:[555777]")
            .file("/proc/net/tcp", &table);
        let source = ProcDetails::new(Arc::new(fixture), PathBuf::from("/proc"));
        let details = source.details(50);

        assert_eq!(
            details.ports,
            vec![Port {
                protocol: "tcp",
                address: "0.0.0.0".into(),
                port: 443,
            }],
            "слушающий порт обязан находиться и в большой таблице"
        );
    }

    #[test]
    fn foreign_sockets_are_not_attributed() {
        // Процесс без сокетов не получает чужие слушающие порты.
        let fixture = FixtureFs::new()
            .file("/proc/7/status", STATUS)
            .link("/proc/7/fd/0", "/dev/null")
            .file("/proc/net/tcp", TCP);
        let source = ProcDetails::new(Arc::new(fixture), PathBuf::from("/proc"));
        assert!(source.details(7).ports.is_empty());
    }

    #[test]
    fn permission_denied_is_reported_not_shown_as_empty() {
        // Чужой процесс: uid читается, дескрипторы - нет. Пустой список без
        // пометки читался бы как «процесс не держит ни файлов, ни портов».
        let fixture = FixtureFs::new()
            .file("/proc/11/status", STATUS)
            .file("/etc/passwd", PASSWD)
            .denied("/proc/11/fd");
        let source = ProcDetails::new(Arc::new(fixture), PathBuf::from("/proc"));
        let details = source.details(11);

        assert!(details.restricted, "отказ в правах обязан быть виден");
        assert!(details.files.is_empty());
        assert!(details.ports.is_empty());
        assert_eq!(details.user.as_deref(), Some("www-data"));
        assert!(
            !details.is_empty(),
            "детали с отказом не пусты: экран обязан объяснить причину"
        );
    }

    #[test]
    fn dead_process_is_not_reported_as_restricted() {
        let source = ProcDetails::new(Arc::new(FixtureFs::new()), PathBuf::from("/proc"));
        let details = source.details(4243);
        assert!(
            !details.restricted,
            "отсутствие процесса не является отказом в правах"
        );
    }

    #[test]
    fn vanished_process_yields_empty_details_not_panic() {
        let source = ProcDetails::new(Arc::new(FixtureFs::new()), PathBuf::from("/proc"));
        let details = source.details(4242);
        assert_eq!(details, ProcessDetails::default());
    }

    #[test]
    fn hostile_paths_cannot_inject_terminal_escapes() {
        // Локальный процесс контролирует имя файла и может назвать его так,
        // чтобы переписать заголовок окна или сбросить состояние терминала.
        let evil = "/tmp/\u{1b}]0;pwned\u{7}payload";
        let fixture = FixtureFs::new()
            .file("/proc/9/status", STATUS)
            .link("/proc/9/exe", evil)
            .link("/proc/9/cwd", "/tmp/\u{1b}[2Jcleared")
            .link("/proc/9/fd/0", evil);
        let source = ProcDetails::new(Arc::new(fixture), PathBuf::from("/proc"));
        let details = source.details(9);

        let exe = details.exe.expect("exe");
        assert!(
            !exe.contains('\u{1b}') && !exe.contains('\u{7}'),
            "escape-последовательность обязана быть обезврежена: {exe:?}"
        );
        let cwd = details.cwd.expect("cwd");
        assert!(!cwd.contains('\u{1b}'), "cwd тоже: {cwd:?}");
        assert!(
            details
                .files
                .iter()
                .all(|file| !file.target.contains('\u{1b}')),
            "и цели дескрипторов: {:?}",
            details.files
        );
    }

    #[test]
    fn hex_endpoint_decoding_matches_kernel_format() {
        assert_eq!(
            parse_hex_endpoint("0100007F:1F90"),
            Some(("127.0.0.1".to_string(), 8080))
        );
        assert_eq!(
            parse_hex_endpoint("00000000:0050"),
            Some(("0.0.0.0".to_string(), 80))
        );
        assert_eq!(
            parse_hex_endpoint("00000000000000000000000000000000:0016"),
            Some(("[::]".to_string(), 22))
        );
        assert_eq!(parse_hex_endpoint("garbage"), None);
    }
}
