//! Общая визуальная грамматика спецификации v0.7.
//!
//! Здесь живут правила, которые обязаны выглядеть одинаково на всех экранах:
//! заголовок раздела с линией (раздел 134), усечение длинных значений
//! (раздел 96), сжатие breadcrumb (раздел 97), маркер выбранной строки
//! (раздел 136) и пресеты шапки и футера (разделы 105, 106).
//!
//! Смысл выноса: если каждый экран решает это сам, грамматика расходится, и
//! спецификация превращается в набор пожеланий.

use crate::layout::{Compression, FooterPreset, HeaderPreset};
use crate::theme::Capability;

/// Маркер выбранной строки (раздел 136).
///
/// Ровно один вид маркера на весь продукт, и только у выбранной строки.
/// Фокус панели показывается акцентом рамки, а не вторым маркером.
pub const SELECTED: &str = "> ";
/// Отступ невыбранной строки: та же ширина, иначе таблица дёргается.
pub const UNSELECTED: &str = "  ";

/// Маркер строки по признаку выбора.
#[must_use]
pub const fn row_marker(selected: bool) -> &'static str {
    if selected {
        SELECTED
    } else {
        UNSELECTED
    }
}

/// Символ горизонтальной линии.
#[must_use]
pub const fn rule_char(capability: Capability) -> char {
    if matches!(capability, Capability::Ascii) {
        '-'
    } else {
        '─'
    }
}

/// Заголовок раздела (раздел 134).
///
/// Фиксирован первый вариант: `SECTION TITLE ─────`. При недостатке ширины
/// линия не рисуется вовсе - смешивать два стиля внутри одного экрана нельзя.
#[must_use]
pub fn section_title(title: &str, width: u16, rules: bool, capability: Capability) -> String {
    let title = title.trim();
    if !rules {
        return title.to_string();
    }
    let width = usize::from(width);
    // Заголовок, пробел и хотя бы четыре символа линии, иначе линия бессмысленна.
    let min_tail = 4;
    if title.chars().count() + 1 + min_tail > width {
        return title.to_string();
    }
    let tail = width - title.chars().count() - 1;
    let mut line = String::with_capacity(width);
    line.push_str(title);
    line.push(' ');
    for _ in 0..tail {
        line.push(rule_char(capability));
    }
    line
}

/// Горизонтальный разделитель на всю ширину.
#[must_use]
pub fn rule(width: u16, capability: Capability) -> String {
    let ch = rule_char(capability);
    std::iter::repeat_n(ch, usize::from(width)).collect()
}

/// Усечение значения с многоточием (раздел 96).
///
/// Обрезка идёт по символам, а не по байтам: имена юнитов бывают не-ASCII, и
/// байтовая обрезка ломала бы UTF-8. Многоточие в ASCII-режиме - три точки.
#[must_use]
pub fn truncate(value: &str, width: usize, capability: Capability) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_string();
    }
    let ellipsis: &str = if matches!(capability, Capability::Ascii) {
        "..."
    } else {
        "…"
    };
    let ellipsis_len = ellipsis.chars().count();
    if width <= ellipsis_len {
        return value.chars().take(width).collect();
    }
    let keep = width - ellipsis_len;
    let mut out: String = value.chars().take(keep).collect();
    out.push_str(ellipsis);
    out
}

/// Сжатие breadcrumb (раздел 97).
///
/// Полный путь -> `первый > … > предпоследний > текущий` -> `… > текущий`.
/// Текущий элемент не выбрасывается никогда: он отвечает на вопрос «где я».
#[must_use]
pub fn breadcrumb(parts: &[String], width: usize, capability: Capability) -> String {
    if parts.is_empty() {
        return String::new();
    }
    let sep = " > ";
    let gap: &str = if matches!(capability, Capability::Ascii) {
        "..."
    } else {
        "…"
    };

    let full = parts.join(sep);
    if full.chars().count() <= width {
        return full;
    }

    let Some(last) = parts.last() else {
        return String::new();
    };
    if let (Some(first), Some(previous)) = (
        parts.first(),
        parts.len().checked_sub(2).and_then(|i| parts.get(i)),
    ) {
        let medium = format!("{first}{sep}{gap}{sep}{previous}{sep}{last}");
        if medium.chars().count() <= width {
            return medium;
        }
    }

    let narrow = format!("{gap}{sep}{last}");
    if narrow.chars().count() <= width {
        return narrow;
    }
    truncate(last, width, capability)
}

/// Подпись состояния «живём сейчас» или «смотрим историю» (раздел 111).
///
/// Никогда не скрывается: оператор обязан знать, реальны ли числа на экране.
#[must_use]
pub fn live_label(paused: bool, preset: HeaderPreset) -> &'static str {
    match (paused, preset) {
        (false, HeaderPreset::Wide | HeaderPreset::Medium) => "LIVE",
        (false, _) => "LIVE",
        (true, HeaderPreset::Wide) => "HISTORY",
        (true, _) => "HIST",
    }
}

/// Отметка режима наблюдения (раздел 157).
///
/// `●` здесь означает «поток данных живой», а не состояние сущности. Символ
/// `○` для этой роли запрещён: в алфавите состояния он уже значит «норма», и
/// оператор читал бы режим как диагноз.
#[must_use]
pub const fn live_mark(capability: Capability) -> &'static str {
    if matches!(capability, Capability::Ascii) {
        "*"
    } else {
        "●"
    }
}

/// Footer — hotkey legend, не Tab bar (v0.9 §180, §193).
///
/// Иконка ставится перед подписью, а не вместо неё: клавиша и слово остаются
/// единственным гарантированным способом прочитать строку, а глиф лишь
/// ускоряет узнавание. В узком пресете иконок нет — там каждая колонка уже
/// потрачена на сокращённые подписи.
#[must_use]
pub fn footer_line(
    preset: FooterPreset,
    width: u16,
    capability: Capability,
    icons: pulse_core::config::IconSet,
) -> String {
    use crate::icons::Icon;

    let mut parts: Vec<(&'static str, Option<Icon>)> = match preset {
        FooterPreset::Wide => vec![
            ("[1]Overview", Some(Icon::Overview)),
            ("[2]Problems", Some(Icon::ProblemsScreen)),
            ("[3]Entities", Some(Icon::Entities)),
            ("[4]Timeline", Some(Icon::Timeline)),
            ("[/]Search", Some(Icon::Search)),
            ("[:]Commands", Some(Icon::Commands)),
            ("[?]Help", Some(Icon::Help)),
        ],
        FooterPreset::Medium => vec![
            ("[1]Overview", Some(Icon::Overview)),
            ("[2]Problems", Some(Icon::ProblemsScreen)),
            ("[3]Entities", Some(Icon::Entities)),
            ("[4]Timeline", Some(Icon::Timeline)),
            ("[/]Search", Some(Icon::Search)),
            ("[?]Help", Some(Icon::Help)),
        ],
        FooterPreset::Narrow => vec![
            ("[1]O", None),
            ("[2]P", None),
            ("[3]E", None),
            ("[4]T", None),
            ("[/]", None),
            ("[?]", None),
        ],
        FooterPreset::Tiny => vec![("[?]Help", Some(Icon::Help))],
    };
    let set = if matches!(capability, Capability::Ascii) {
        pulse_core::config::IconSet::Off
    } else {
        icons
    };
    let width = usize::from(width);
    let draw = |parts: &[(&'static str, Option<Icon>)]| {
        parts
            .iter()
            .map(|(label, icon)| match icon {
                Some(icon) => format!("{}{label}", crate::icons::prefix(*icon, set)),
                None => (*label).to_string(),
            })
            .collect::<Vec<String>>()
            .join("  ")
    };
    while parts.len() > 1 && draw(&parts).chars().count() > width {
        // Сначала уступает Commands/Search, top-level map и Help остаются.
        let remove = parts
            .iter()
            .position(|(label, _)| *label == "[:]Commands")
            .or_else(|| parts.iter().position(|(label, _)| *label == "[/]Search"))
            .unwrap_or(parts.len().saturating_sub(2));
        parts.remove(remove);
    }
    let line = draw(&parts);
    if line.chars().count() > width {
        truncate(&line, width, Capability::Ascii)
    } else {
        line
    }
}

/// Выбор варианта подписи по уровню сжатия (раздел 93).
#[must_use]
pub fn label_for(
    compression: Compression,
    full: &'static str,
    short: &'static str,
) -> &'static str {
    match compression {
        Compression::Full => full,
        _ => short,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Раздел 134: заголовок с линией на всю ширину.
    #[test]
    fn section_title_fills_width_with_rule() {
        let line = section_title("WHAT NEEDS ATTENTION", 40, true, Capability::TrueColor);
        assert_eq!(line.chars().count(), 40);
        assert!(line.starts_with("WHAT NEEDS ATTENTION "));
        assert!(line.ends_with('─'));
    }

    /// Раздел 134: при узкой ширине - только заголовок, без линии.
    #[test]
    fn section_title_drops_rule_when_narrow() {
        let line = section_title("WHAT NEEDS ATTENTION", 40, false, Capability::TrueColor);
        assert_eq!(line, "WHAT NEEDS ATTENTION");
        // Линия не влезает даже при разрешённых правилах.
        let tight = section_title("WHAT NEEDS ATTENTION", 22, true, Capability::TrueColor);
        assert_eq!(tight, "WHAT NEEDS ATTENTION");
    }

    #[test]
    fn section_title_is_ascii_safe() {
        let line = section_title("STATE", 20, true, Capability::Ascii);
        assert!(line.is_ascii(), "в ASCII-режиме линия обязана быть ASCII");
        assert_eq!(line.chars().count(), 20);
    }

    /// Раздел 96: усечение по символам, а не по байтам.
    #[test]
    fn truncate_keeps_utf8_and_adds_ellipsis() {
        let value = "checkout-api-production-worker.service";
        let short = truncate(value, 18, Capability::TrueColor);
        assert_eq!(short.chars().count(), 18);
        assert!(short.ends_with('…'));
        assert_eq!(short, "checkout-api-prod…");

        let cyrillic = truncate("сервис-очень-длинное-имя", 10, Capability::TrueColor);
        assert_eq!(cyrillic.chars().count(), 10);
        assert!(cyrillic.ends_with('…'));
    }

    #[test]
    fn truncate_is_noop_when_it_fits() {
        assert_eq!(truncate("pulse", 10, Capability::TrueColor), "pulse");
        assert_eq!(truncate("pulse", 5, Capability::TrueColor), "pulse");
    }

    #[test]
    fn truncate_ascii_uses_three_dots() {
        let short = truncate("checkout-api-worker", 12, Capability::Ascii);
        assert!(short.is_ascii());
        assert!(short.ends_with("..."));
        assert_eq!(short.chars().count(), 12);
    }

    /// Раздел 97: три уровня сжатия пути.
    #[test]
    fn breadcrumb_compresses_in_three_steps() {
        let parts: Vec<String> = [
            "asuspc",
            "system.slice",
            "checkout.service",
            "container",
            "pod",
            "python3/18421",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

        let full = breadcrumb(&parts, 120, Capability::TrueColor);
        assert_eq!(
            full,
            "asuspc > system.slice > checkout.service > container > pod > python3/18421"
        );

        let medium = breadcrumb(&parts, 45, Capability::TrueColor);
        assert_eq!(medium, "asuspc > … > pod > python3/18421");

        let narrow = breadcrumb(&parts, 20, Capability::TrueColor);
        assert_eq!(narrow, "… > python3/18421");
    }

    /// Раздел 97: текущий элемент не выбрасывается никогда.
    #[test]
    fn breadcrumb_always_keeps_current_entity() {
        let parts: Vec<String> = ["host", "a", "b", "python3/18421"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let tiny = breadcrumb(&parts, 8, Capability::TrueColor);
        assert!(
            tiny.contains("py") || tiny.contains('…'),
            "в самом узком виде остаётся текущая сущность: {tiny}"
        );
        assert!(tiny.chars().count() <= 8);
    }

    #[test]
    fn breadcrumb_empty_path_is_empty() {
        assert_eq!(breadcrumb(&[], 40, Capability::TrueColor), "");
    }

    /// Раздел 137: футер не переносится на две строки при 80 колонках.
    #[test]
    fn footer_fits_recommended_terminal() {
        for preset in [
            FooterPreset::Wide,
            FooterPreset::Medium,
            FooterPreset::Narrow,
            FooterPreset::Tiny,
        ] {
            for width in [50_u16, 60, 80, 120, 160] {
                // Иконки занимают колонки, поэтому лестница сжатия обязана
                // выдерживать любой набор, а не только выключенный.
                for icons in [
                    pulse_core::config::IconSet::Off,
                    pulse_core::config::IconSet::Nerd,
                    pulse_core::config::IconSet::Unicode,
                ] {
                    let line = footer_line(preset, width, Capability::TrueColor, icons);
                    assert!(
                        line.chars().count() <= usize::from(width),
                        "футер {preset:?} с набором {icons:?} не влез в {width}: {line:?}"
                    );
                }
            }
        }
    }

    /// Футер всегда сохраняет хотя бы одну подсказку: пустая строка подсказок
    /// оставляет оператора без выхода из программы.
    #[test]
    fn footer_never_becomes_empty() {
        let line = footer_line(
            FooterPreset::Wide,
            12,
            Capability::TrueColor,
            pulse_core::config::IconSet::Nerd,
        );
        assert!(!line.is_empty());
    }

    /// Раздел 111: признак LIVE/HISTORY виден при любом пресете.
    #[test]
    fn live_label_is_always_present() {
        for preset in [
            HeaderPreset::Wide,
            HeaderPreset::Medium,
            HeaderPreset::Narrow,
            HeaderPreset::Tiny,
        ] {
            assert!(!live_label(false, preset).is_empty());
            assert!(!live_label(true, preset).is_empty());
            assert_ne!(
                live_label(false, preset),
                live_label(true, preset),
                "режим наблюдения обязан отличаться визуально"
            );
        }
    }

    /// Раздел 136: маркер только у выбранной строки, ширина одинаковая.
    #[test]
    fn row_marker_keeps_column_alignment() {
        assert_eq!(row_marker(true), "> ");
        assert_eq!(row_marker(false), "  ");
        assert_eq!(row_marker(true).len(), row_marker(false).len());
    }

    #[test]
    fn label_for_shrinks_with_compression() {
        assert_eq!(
            label_for(Compression::Full, "MEMORY PRESSURE", "MEM"),
            "MEMORY PRESSURE"
        );
        assert_eq!(
            label_for(Compression::Terse, "MEMORY PRESSURE", "MEM"),
            "MEM"
        );
    }

    #[test]
    fn rule_matches_requested_width() {
        assert_eq!(rule(10, Capability::TrueColor).chars().count(), 10);
        assert!(rule(10, Capability::Ascii).is_ascii());
    }
}
