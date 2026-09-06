//! Санитизация и сокрытие секретов.
//!
//! Две независимые задачи безопасности:
//!
//! 1. **Санитизация для терминала.** Имена процессов, контейнеров и cgroup полностью
//!    контролируются потенциально враждебным локальным процессом: `comm` пишется
//!    самим процессом, `cmdline` тоже. Без обработки процесс с именем
//!    `\x1b[2J\x1b[?1049h` перерисует чужой терминал, а bidi-override подменит
//!    видимый порядок символов. Поэтому любая строка из ядра проходит
//!    [`sanitize_display`] до попадания в граф.
//!
//! 2. **Сокрытие секретов.** Командная строка регулярно содержит пароли и токены
//!    (`--password=...`, `mysql://user:pass@host`). Pulse показывает командные строки
//!    в TUI и может отдавать их наружу, поэтому по умолчанию действует политика
//!    [`RedactMode::Secrets`].

use serde::{Deserialize, Serialize};

/// Максимальная длина отображаемой строки после санитизации.
pub const MAX_DISPLAY_LEN: usize = 256;

/// Максимальная длина командной строки, которую вообще обрабатываем.
/// `/proc/<pid>/cmdline` не ограничен по размеру: злонамеренный процесс может
/// подставить мегабайты аргументов и заставить агент тратить CPU.
pub const MAX_CMDLINE_BYTES: usize = 16 * 1024;

/// Символ-заместитель для вырезанных управляющих последовательностей.
const REPLACEMENT: char = '\u{fffd}';

/// Строка, которой заменяется скрытое значение.
pub const REDACTED: &str = "<redacted>";

/// Политика сокрытия секретов в командных строках.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactMode {
    /// Скрывать значения аргументов, похожих на секреты. Значение по умолчанию.
    #[default]
    Secrets,
    /// Показывать только исполняемый файл, аргументы заменять их числом.
    Aggressive,
    /// Ничего не скрывать. Требует явного включения в конфигурации.
    Off,
}

/// Результат сокрытия: строка и число скрытых фрагментов (для самометрики).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redacted {
    pub text: String,
    pub hidden: u32,
}

/// Ключи аргументов, значение которых считается секретом.
///
/// Второе поле — можно ли считать секретом ключ, который лишь *заканчивается* этим
/// словом без разделителя (`PGPASSWORD`). Для коротких и многозначных слов
/// (`pass`, `auth`, `conn`) такое сопоставление даёт ложные срабатывания
/// (`bypass`, `author`), поэтому для них требуется точное совпадение
/// или разделитель: `--db.pass`, `--pg-auth`.
const SECRET_KEYS: &[(&str, bool)] = &[
    ("password", true),
    ("passwd", true),
    ("secret", true),
    ("token", true),
    ("apikey", true),
    ("api-key", true),
    ("api_key", true),
    ("accesskey", true),
    ("access-key", true),
    ("access_key", true),
    ("secretkey", true),
    ("secret-key", true),
    ("secret_key", true),
    ("credential", true),
    ("credentials", true),
    ("privatekey", true),
    ("private-key", true),
    ("private_key", true),
    ("sessionid", true),
    ("authorization", true),
    ("pass", false),
    ("pwd", false),
    ("auth", false),
    ("cookie", false),
    ("bearer", false),
    ("session", false),
    ("otp", false),
    ("dsn", false),
    ("conn", false),
    ("connectionstring", false),
    ("connection-string", false),
    ("sasl-password", false),
    ("keyfile-password", false),
];

/// Убирает из строки всё, что может управлять терминалом или подменять его вывод.
///
/// Удаляются: ANSI/DEC escape-последовательности целиком, символы C0/C1,
/// zero-width и bidi-override символы. Длина ограничена [`MAX_DISPLAY_LEN`].
#[must_use]
pub fn sanitize_display(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(MAX_DISPLAY_LEN));
    let mut chars = input.chars().peekable();
    let mut truncated = false;

    while let Some(c) = chars.next() {
        if out.chars().count() >= MAX_DISPLAY_LEN {
            truncated = true;
            break;
        }
        match c {
            // ESC: съедаем последовательность целиком, чтобы не оставить мусор вида "[2J".
            '\u{1b}' => {
                skip_escape_sequence(&mut chars);
                out.push(REPLACEMENT);
            }
            // C0 и DEL.
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push(REPLACEMENT),
            // C1.
            c if (0x80..0xa0).contains(&(c as u32)) => out.push(REPLACEMENT),
            // Zero-width, bidi-override, невидимые разделители.
            '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}' => out.push(REPLACEMENT),
            c => out.push(c),
        }
    }

    if truncated {
        out.push('…');
    }
    out
}

/// Пропускает тело escape-последовательности после уже прочитанного ESC.
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.peek().copied() {
        // CSI: ESC [ ... final byte 0x40..0x7e
        Some('[') => {
            let _ = chars.next();
            while let Some(&c) = chars.peek() {
                let _ = chars.next();
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        // OSC: ESC ] ... BEL или ST
        Some(']') => {
            let _ = chars.next();
            while let Some(&c) = chars.peek() {
                let _ = chars.next();
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' {
                    // ESC \  — терминатор строки.
                    if chars.peek() == Some(&'\\') {
                        let _ = chars.next();
                    }
                    break;
                }
            }
        }
        // Двухсимвольные последовательности: ESC ( B, ESC = и подобные.
        Some(_) => {
            let _ = chars.next();
        }
        None => {}
    }
}

/// Скрывает секреты в командной строке, представленной массивом аргументов.
#[must_use]
pub fn redact_argv(argv: &[String], mode: RedactMode) -> Redacted {
    if argv.is_empty() {
        return Redacted {
            text: String::new(),
            hidden: 0,
        };
    }

    if mode == RedactMode::Aggressive {
        let exe = argv.first().map(String::as_str).unwrap_or_default();
        let args = argv.len().saturating_sub(1);
        let hidden = u32::try_from(args).unwrap_or(u32::MAX);
        let text = if args == 0 {
            sanitize_display(exe)
        } else {
            format!("{} <{} args hidden>", sanitize_display(exe), args)
        };
        return Redacted { text, hidden };
    }

    let mut hidden = 0u32;
    let mut parts: Vec<String> = Vec::with_capacity(argv.len());
    let mut hide_next_value = false;

    for (index, raw) in argv.iter().enumerate() {
        let arg = sanitize_display(raw);

        if mode == RedactMode::Off {
            parts.push(arg);
            continue;
        }

        // Значение отдельного `--password value`.
        if hide_next_value {
            hide_next_value = false;
            hidden = hidden.saturating_add(1);
            parts.push(REDACTED.to_string());
            continue;
        }

        // `--password=value` и `PASSWORD=value`.
        if let Some(eq) = arg.find('=') {
            let (key, value) = arg.split_at(eq);
            if is_secret_key(key) && value.len() > 1 {
                hidden = hidden.saturating_add(1);
                parts.push(format!("{key}={REDACTED}"));
                continue;
            }
        } else if index > 0 && is_secret_key(&arg) {
            // Флаг без значения: скрываем следующий аргумент.
            hide_next_value = true;
            parts.push(arg);
            continue;
        }

        // URL с userinfo: postgres://user:secret@host/db
        if let Some(masked) = mask_url_userinfo(&arg) {
            hidden = hidden.saturating_add(1);
            parts.push(masked);
            continue;
        }

        parts.push(arg);
    }

    Redacted {
        text: parts.join(" "),
        hidden,
    }
}

/// Разбирает сырое содержимое `/proc/<pid>/cmdline` (NUL-разделённое) в аргументы.
///
/// Ограничивает объём обрабатываемых данных [`MAX_CMDLINE_BYTES`] и число аргументов.
#[must_use]
pub fn parse_cmdline(raw: &[u8]) -> Vec<String> {
    const MAX_ARGS: usize = 64;
    let slice = if raw.len() > MAX_CMDLINE_BYTES {
        raw.get(..MAX_CMDLINE_BYTES).unwrap_or(raw)
    } else {
        raw
    };
    slice
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .take(MAX_ARGS)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect()
}

fn is_secret_key(key: &str) -> bool {
    let trimmed = key.trim_start_matches('-').to_ascii_lowercase();
    if trimmed.is_empty() {
        return false;
    }
    SECRET_KEYS.iter().any(|(candidate, suffix_ok)| {
        if trimmed == *candidate {
            return true;
        }
        // Сегментное совпадение: db.password, --pg-password, PG_PASSWORD.
        for separator in ['.', '-', '_'] {
            if trimmed.ends_with(&format!("{separator}{candidate}")) {
                return true;
            }
        }
        // Слитное совпадение допускается только для однозначных слов: PGPASSWORD.
        *suffix_ok && trimmed.ends_with(*candidate)
    })
}

/// Заменяет `user:password@` в URL на `user:<redacted>@`.
fn mask_url_userinfo(arg: &str) -> Option<String> {
    let scheme_end = arg.find("://")?;
    let rest_start = scheme_end + 3;
    let rest = arg.get(rest_start..)?;
    let at = rest.find('@')?;
    let userinfo = rest.get(..at)?;
    let colon = userinfo.find(':')?;
    let user = userinfo.get(..colon)?;
    let scheme = arg.get(..scheme_end)?;
    let tail = rest.get(at..)?;
    Some(format!("{scheme}://{user}:{REDACTED}{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn strips_csi_sequences_entirely() {
        let evil = "\u{1b}[2J\u{1b}[?1049hnginx";
        let clean = sanitize_display(evil);
        assert!(!clean.contains('\u{1b}'));
        assert!(!clean.contains("2J"));
        assert!(!clean.contains("1049h"));
        assert!(clean.ends_with("nginx"));
    }

    #[test]
    fn strips_osc_title_injection() {
        let evil = "\u{1b}]0;pwned\u{7}postgres";
        let clean = sanitize_display(evil);
        assert!(!clean.contains("pwned"));
        assert!(clean.ends_with("postgres"));
    }

    #[test]
    fn strips_control_and_bidi_characters() {
        let evil = "web\u{202e}gnp.exe\u{0}\r\n\u{200b}";
        let clean = sanitize_display(evil);
        assert!(!clean.contains('\u{202e}'));
        assert!(!clean.contains('\u{200b}'));
        assert!(!clean.contains('\r'));
        assert!(!clean.contains('\n'));
        assert!(clean.starts_with("web"));
    }

    #[test]
    fn display_length_is_bounded() {
        let long = "a".repeat(MAX_DISPLAY_LEN * 4);
        let clean = sanitize_display(&long);
        assert!(clean.chars().count() <= MAX_DISPLAY_LEN + 1);
    }

    #[test]
    fn sanitize_keeps_useful_unicode() {
        assert_eq!(sanitize_display("postgres: писатель"), "postgres: писатель");
    }

    #[test]
    fn redacts_inline_secret_values() {
        let r = redact_argv(
            &argv(&["mysqld", "--password=hunter2", "--port=3306"]),
            RedactMode::Secrets,
        );
        assert!(!r.text.contains("hunter2"));
        assert!(r.text.contains("--password=<redacted>"));
        assert!(r.text.contains("--port=3306"));
        assert_eq!(r.hidden, 1);
    }

    #[test]
    fn redacts_separated_secret_values() {
        let r = redact_argv(
            &argv(&["app", "--token", "abc123xyz", "--verbose"]),
            RedactMode::Secrets,
        );
        assert!(!r.text.contains("abc123xyz"));
        assert!(r.text.contains("--token <redacted>"));
        assert!(r.text.contains("--verbose"));
        assert_eq!(r.hidden, 1);
    }

    #[test]
    fn redacts_url_userinfo() {
        let r = redact_argv(
            &argv(&["worker", "postgres://admin:s3cr3t@db:5432/app"]),
            RedactMode::Secrets,
        );
        assert!(!r.text.contains("s3cr3t"));
        assert!(r.text.contains("postgres://admin:<redacted>@db:5432/app"));
        assert_eq!(r.hidden, 1);
    }

    #[test]
    fn redacts_nested_key_names() {
        let r = redact_argv(
            &argv(&["svc", "--db.password=zzz", "PGPASSWORD=yyy"]),
            RedactMode::Secrets,
        );
        assert!(!r.text.contains("zzz"));
        assert!(!r.text.contains("yyy"));
        assert_eq!(r.hidden, 2);
    }

    #[test]
    fn aggressive_mode_hides_all_arguments() {
        let r = redact_argv(
            &argv(&["/usr/bin/app", "--safe", "--also-safe"]),
            RedactMode::Aggressive,
        );
        assert_eq!(r.text, "/usr/bin/app <2 args hidden>");
        assert_eq!(r.hidden, 2);
    }

    #[test]
    fn off_mode_still_sanitizes_terminal_escapes() {
        let r = redact_argv(
            &argv(&["app", "--password=p", "\u{1b}[31mred"]),
            RedactMode::Off,
        );
        assert!(
            r.text.contains("--password=p"),
            "секреты не скрыты: {}",
            r.text
        );
        assert!(!r.text.contains('\u{1b}'), "escape не удалён: {:?}", r.text);
    }

    #[test]
    fn cmdline_parsing_is_bounded() {
        let mut raw = Vec::new();
        for i in 0..1000 {
            raw.extend_from_slice(format!("arg{i}").as_bytes());
            raw.push(0);
        }
        let args = parse_cmdline(&raw);
        assert!(args.len() <= 64);
    }

    #[test]
    fn cmdline_parsing_handles_invalid_utf8() {
        let raw = b"/bin/app\0--name\0\xff\xfe\0";
        let args = parse_cmdline(raw);
        assert_eq!(args.len(), 3);
        assert_eq!(args.first().map(String::as_str), Some("/bin/app"));
    }

    #[test]
    fn huge_cmdline_is_truncated_before_processing() {
        let raw = vec![b'x'; MAX_CMDLINE_BYTES * 4];
        let args = parse_cmdline(&raw);
        let total: usize = args.iter().map(String::len).sum();
        assert!(total <= MAX_CMDLINE_BYTES);
    }
}
