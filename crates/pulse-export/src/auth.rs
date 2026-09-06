//! Bearer-токены: загрузка, парсинг, сравнение без раннего выхода.
//!
//! Единственный источник правды для аутентификации экспорта. Сервер обязан
//! использовать именно эти функции: две независимые реализации разошлись бы по
//! поведению, и исправление в одной не попало бы в другую.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

/// Минимальная длина токена после отбрасывания краевых пробелов.
pub const MIN_TOKEN_BYTES: usize = 16;

/// Читает файл с токеном.
///
/// Обрезаются только краевые пробельные символы: перевод строки в конце файла —
/// норма для редакторов. Внутренний пробел или перевод строки — отказ старта, а
/// не «тихая» починка: такой токен нельзя передать в HTTP-заголовке, и агент
/// иначе поднялся бы с токеном, который никакой клиент предъявить не может, а
/// все запросы молча получали бы 401.
pub fn load_token(path: &Path) -> io::Result<Vec<u8>> {
    let raw = fs::read(path)?;
    let start = raw
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(raw.len());
    let end = raw
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|p| p + 1)
        .unwrap_or(raw.len());
    let token = raw.get(start..end).unwrap_or(&[]).to_vec();

    if token.len() < MIN_TOKEN_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("токен короче {MIN_TOKEN_BYTES} байт после обрезки краевых пробелов"),
        ));
    }
    if token.iter().any(|b| b.is_ascii_whitespace()) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "токен не должен содержать пробелов и переводов строки внутри",
        ));
    }
    Ok(token)
}

/// Парсит заголовок `Authorization` в схеме `Bearer`.
///
/// Схема сравнивается без учёта регистра. Токен с внутренним пробелом
/// отвергается: HTTP-заголовок такой токен не переносит однозначно, а
/// «склеивание» частей превратило бы разные строки в один секрет.
#[must_use]
pub fn parse_bearer(header: &str) -> Option<&str> {
    let (scheme, rest) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return None;
    }
    Some(token)
}

/// Сравнение двух срезов в константное время: XOR-накопитель, без раннего выхода.
///
/// Несовпадение длины учитывается отдельным XOR, а не ранним `return`: иначе по
/// времени ответа утекала бы длина токена.
#[must_use]
pub fn constant_time_equal(a: &[u8], b: &[u8]) -> bool {
    let mut diff: u32 = (a.len() as u32) ^ (b.len() as u32);
    let n = a.len().max(b.len());
    let mut i = 0usize;
    while i < n {
        let ba = *a.get(i).unwrap_or(&0);
        let bb = *b.get(i).unwrap_or(&0);
        diff |= u32::from(ba ^ bb);
        i += 1;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Уникальный путь во временном каталоге: тесты идут параллельно.
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pulse-token-{name}-{}", std::process::id()))
    }

    fn write(name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = temp_path(name);
        std::fs::write(&path, content).expect("записать файл токена");
        path
    }

    #[test]
    fn load_token_accepts_valid_token_and_trims_edges() {
        let path = write("valid", b"  \tsecret-token-0123456789 \n");
        let token = load_token(&path).expect("токен должен загрузиться");
        assert_eq!(token, b"secret-token-0123456789");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_token_rejects_empty_and_short() {
        let empty = write("empty", b"   \n");
        assert!(load_token(&empty).is_err());
        let _ = std::fs::remove_file(empty);

        let short = write("short", b"short\n");
        assert!(load_token(&short).is_err());
        let _ = std::fs::remove_file(short);
    }

    #[test]
    fn load_token_rejects_internal_whitespace_instead_of_silently_stripping_it() {
        // Свёрнутый при вставке base64: без отказа агент поднялся бы с токеном,
        // который клиент не может предъявить, и все запросы давали бы 401.
        let path = write("folded", b"secret-token-01\n23456789\n");
        let error = load_token(&path).expect_err("такой токен обязан отклоняться");
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parse_bearer_accepts_scheme_case_insensitively() {
        assert_eq!(parse_bearer("Bearer abcdef"), Some("abcdef"));
        assert_eq!(parse_bearer("bearer abcdef"), Some("abcdef"));
    }

    #[test]
    fn parse_bearer_rejects_other_schemes_and_empty_token() {
        assert!(parse_bearer("Basic abcdef").is_none());
        assert!(parse_bearer("Bearer").is_none());
        assert!(parse_bearer("Bearer   ").is_none());
    }

    #[test]
    fn parse_bearer_rejects_token_with_internal_whitespace() {
        assert!(parse_bearer("Bearer a b c").is_none());
    }

    /// Инвариант согласованности: то, что принимает загрузчик файла, должно
    /// приниматься и парсером заголовка. Иначе сервер стартует, но 401 всегда.
    #[test]
    fn loader_and_header_parser_agree_on_the_same_token() {
        let path = write("agree", b"secret-token-0123456789\n");
        let loaded = load_token(&path).expect("токен файла");
        let header = format!("Bearer {}", String::from_utf8_lossy(&loaded));
        let parsed = parse_bearer(&header).expect("тот же токен в заголовке");
        assert!(constant_time_equal(loaded.as_slice(), parsed.as_bytes()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn constant_time_equal_compares_content_and_length() {
        assert!(constant_time_equal(
            b"abcdef0123456789",
            b"abcdef0123456789"
        ));
        assert!(!constant_time_equal(
            b"abcdef0123456789",
            b"abcdef0123456788"
        ));
        assert!(!constant_time_equal(
            b"abcdef0123456789",
            b"abcdef012345678"
        ));
        assert!(constant_time_equal(b"", b""));
        assert!(!constant_time_equal(b"", b"a"));
    }
}
