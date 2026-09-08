//! RED-репродьюсеры аудита для `pulse-core`.
//!
//! Каждый тест описывает **желаемый** контракт и потому падает на текущей
//! реализации. Они помечены `#[ignore]`, поэтому обязательный прогон
//! `cargo test --workspace` остаётся зелёным, а `scripts/audit-red.sh`
//! доказывает, что дефект действительно воспроизводится. Когда remediation-PR
//! исправит дефект, `#[ignore]` снимается и тест уходит в обычный регресс.
//!
//! Запрещено превращать эти тесты в фиксацию текущего поведения: утверждения
//! описывают контракт, а не наблюдаемый дефект.

// Тест обязан падать и указывать строку, поэтому `expect`/`panic` уместны.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use pulse_core::config::Config;
use pulse_core::entity::Labels;

/// RED-01 / PULSE-007, PULSE-043: `Labels::set` обязан усекать по границе
/// символа.
///
/// `String::truncate` требует границу UTF-8, а значение метки приходит из
/// недоверенного источника (`cmdline`, путь cgroup). Значит локальный процесс
/// может вызвать панику, подобрав длину аргумента.
#[test]
#[ignore = "audit RED: PULSE-007/043 — promote when remediation lands"]
fn audit_red_labels_truncate_on_char_boundary() {
    // «ы» занимает два байта: 255 символов = 510 байт. Следующий трёхбайтовый
    // символ занимает байты 510..513, поэтому лимит 512 попадает внутрь него.
    let mut value = "ы".repeat(255);
    value.push_str(&"—".repeat(8));
    assert!(value.len() > Labels::MAX_VALUE_LEN);
    assert!(!value.is_char_boundary(Labels::MAX_VALUE_LEN));

    let mut labels = Labels::new();
    labels.set("cmdline", value);

    let stored = labels.get("cmdline").expect("метка сохранена");
    assert!(
        stored.len() <= Labels::MAX_VALUE_LEN,
        "значение обязано быть усечено до лимита: {} байт",
        stored.len()
    );
}

/// RED-12 / PULSE-087: значение метки в доверенном графе обязано быть
/// безопасным для терминала.
///
/// `SECURITY.md` заявляет, что после границы доверия в состоянии лежат уже
/// очищенные строки, но `Labels::set` не вызывает `sanitize_display`.
#[test]
#[ignore = "audit RED: PULSE-087 — promote when remediation lands"]
fn audit_red_labels_are_terminal_safe() {
    let hostile = "safe\u{1b}]8;;https://evil\u{7}LINK\u{1b}]8;;\u{7}\u{202e}tail";
    let mut labels = Labels::new();
    labels.set("exe", hostile);

    let stored = labels.get("exe").expect("метка сохранена");
    assert!(
        !stored.contains('\u{1b}'),
        "метка не имеет права содержать ESC: {stored:?}"
    );
    assert!(
        !stored.contains('\u{7}'),
        "метка не имеет права содержать BEL: {stored:?}"
    );
    assert!(
        !stored.contains('\u{202e}'),
        "метка не имеет права содержать bidi override: {stored:?}"
    );
}

/// RED-08 / PULSE-023: конфигурация с `crit < warn` обязана отклоняться.
///
/// Проверка гистерезиса ловит только `warn <= clear`, поэтому пороги
/// `clear < crit < warn` проходят валидацию: правило никогда не сможет
/// сообщить о критическом уровне, потому что вход в проблему строже него.
#[test]
#[ignore = "audit RED: PULSE-023 — promote when remediation lands"]
fn audit_red_reject_crit_below_warn() {
    let mut config = Config::default();
    config.rules.memory_util_clear = 0.10;
    config.rules.memory_util_crit = 0.20;
    config.rules.memory_util_warn = 0.90;

    let result = config.validate();
    assert!(
        result.is_err(),
        "порог crit ниже warn обязан быть отклонён, получено: {result:?}"
    );
}

/// RED-09 / PULSE-081: нулевые операционные лимиты обязаны отклоняться.
///
/// Сейчас конфигурация валидна, но делает сервис нерабочим: лимит запросов
/// в ноль не выдаёт ни одного токена, нулевой бюджет серий отдаёт пустой
/// `/metrics`, нулевой лимит серий истории отвергает каждую новую серию.
#[test]
#[ignore = "audit RED: PULSE-081 — promote when remediation lands"]
fn audit_red_reject_zero_operational_limits() {
    // Отдельная функция вместо массива кортежей с типом-функцией: сложный
    // тип в тесте лишь мешает читать сценарий.
    fn accepted(name: &'static str, apply: impl FnOnce(&mut Config)) -> Option<&'static str> {
        let mut config = Config::default();
        apply(&mut config);
        config.validate().is_ok().then_some(name)
    }

    let accepted: Vec<&str> = [
        accepted("export.rate_limit_per_minute", |config: &mut Config| {
            config.export.rate_limit_per_minute = 0;
        }),
        accepted("export.max_series", |config: &mut Config| {
            config.export.max_series = 0;
        }),
        accepted("store.max_series", |config: &mut Config| {
            config.store.max_series = 0;
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    assert!(
        accepted.is_empty(),
        "нулевые лимиты обязаны отклоняться, приняты: {accepted:?}"
    );
}
