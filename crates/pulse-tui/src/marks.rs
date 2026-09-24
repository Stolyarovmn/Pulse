//! Отметки событий: форма — вид события, цвет — серьёзность.
//!
//! Раньше у Overview и Timeline было по своему словарю, и почти любое событие
//! рисовалось `!`: перезапуск, OOM и открытие проблемы на оси выглядели
//! одинаково. Здесь словарь один, и форма читается без подписи:
//!
//! | событие            | знак | ASCII |
//! |--------------------|------|-------|
//! | начало наблюдения  | `●`  | `*`   |
//! | проблема открыта   | `◇ ◆ ▲` по серьёзности | `< * ^` |
//! | проблема закрыта   | `✓`  | `v`   |
//! | OOM kill           | `▼`  | `V`   |
//! | перезапуск         | `↻`  | `r`   |
//! | появился / исчез   | `+ −`| `+ -` |
//! | метаданные, родитель | `~` | `~`   |
//! | ошибка сбора       | `!`  | `!`   |
//!
//! Проблемы используют символы алфавита состояния — треугольник значит
//! критично и на фигуре, и на оси. Остальные формы с алфавитом не
//! пересекаются, поэтому событие не спутать с состоянием.

use ratatui::style::Style;

use pulse_core::problem::Severity;
use pulse_core::EventKind;

use crate::state::StateClass;
use crate::theme::{Capability, Theme};

/// Знак события.
#[must_use]
pub fn event_mark(kind: EventKind, severity: Severity, capability: Capability) -> char {
    let ascii = matches!(capability, Capability::Ascii);
    let pick = |unicode: char, plain: char| if ascii { plain } else { unicode };
    match kind {
        EventKind::ObservationStarted => pick('●', '*'),
        EventKind::ProblemOpened => StateClass::from_severity(severity).symbol(capability),
        EventKind::ProblemClosed => pick('✓', 'v'),
        EventKind::OomKill => pick('▼', 'V'),
        EventKind::Restarted => pick('↻', 'r'),
        EventKind::Created => '+',
        EventKind::Deleted => pick('−', '-'),
        EventKind::Reparented | EventKind::MetadataChanged => '~',
        EventKind::CollectorError => '!',
    }
}

/// Цвет знака: тревога — цветом серьёзности, спокойное — приглушённо.
#[must_use]
pub fn event_style(kind: EventKind, severity: Severity, theme: &Theme) -> Style {
    match kind {
        EventKind::ObservationStarted => theme.strong(),
        EventKind::ProblemOpened => StateClass::from_severity(severity).style(theme),
        EventKind::ProblemClosed => theme.nominal(),
        EventKind::OomKill | EventKind::CollectorError => theme.severity(Severity::Crit),
        EventKind::Restarted => theme.severity(Severity::Warn),
        EventKind::Created
        | EventKind::Deleted
        | EventKind::Reparented
        | EventKind::MetadataChanged => theme.dim(),
    }
}

/// Насколько событие важнее соседа в той же колонке оси.
///
/// В одну колонку попадают несколько событий, и остаться обязано самое
/// тревожное: OOM не может спрятаться за «появился процесс».
#[must_use]
pub const fn event_rank(kind: EventKind, severity: Severity) -> u8 {
    match kind {
        EventKind::OomKill | EventKind::CollectorError => 9,
        EventKind::ProblemOpened => match severity {
            Severity::Crit => 8,
            Severity::Warn => 7,
            Severity::Info => 6,
        },
        EventKind::Restarted => 5,
        EventKind::ProblemClosed => 4,
        EventKind::ObservationStarted => 3,
        EventKind::Deleted | EventKind::Created => 2,
        EventKind::Reparented | EventKind::MetadataChanged => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [EventKind; 10] = [
        EventKind::ObservationStarted,
        EventKind::Created,
        EventKind::Deleted,
        EventKind::Restarted,
        EventKind::Reparented,
        EventKind::MetadataChanged,
        EventKind::OomKill,
        EventKind::ProblemOpened,
        EventKind::ProblemClosed,
        EventKind::CollectorError,
    ];

    /// Разные по смыслу события различимы формой: раньше перезапуск, OOM и
    /// открытие проблемы были одним `!`. Ровно одинаковы только пары,
    /// которые и задуманы одним знаком.
    #[test]
    fn kinds_are_told_apart_by_shape() {
        let mark = |kind| event_mark(kind, Severity::Warn, Capability::TrueColor);
        assert_ne!(mark(EventKind::Restarted), mark(EventKind::OomKill));
        assert_ne!(mark(EventKind::Restarted), mark(EventKind::ProblemOpened));
        assert_ne!(mark(EventKind::OomKill), mark(EventKind::ProblemOpened));
        assert_ne!(mark(EventKind::Created), mark(EventKind::Deleted));
        assert_ne!(
            mark(EventKind::ProblemOpened),
            mark(EventKind::ProblemClosed)
        );
        // Серьёзность открытой проблемы видна формой алфавита состояния.
        assert_eq!(
            event_mark(
                EventKind::ProblemOpened,
                Severity::Crit,
                Capability::TrueColor
            ),
            '▲'
        );
    }

    /// ASCII-режим не пропускает ни одного знака вне ASCII.
    #[test]
    fn ascii_marks_are_ascii() {
        for kind in KINDS {
            for severity in [Severity::Info, Severity::Warn, Severity::Crit] {
                assert!(
                    event_mark(kind, severity, Capability::Ascii).is_ascii(),
                    "{kind:?}"
                );
            }
        }
    }
}
