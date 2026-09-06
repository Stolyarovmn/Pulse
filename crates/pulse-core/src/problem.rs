//! Проблемы: результат детерминированных правил, а не догадок.
//!
//! Правило обязано предъявить доказательства: какие величины и какие пороги
//! привели к выводу. UI показывает доказательства, а не только вердикт.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::entity::EntityId;
use crate::time::Timestamp;

/// Уровень серьёзности. Порядок важен: используется для сортировки списка проблем.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Crit,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Warn => "WARN",
            Severity::Crit => "CRIT",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Одно доказательство: измеренная величина и порог, который она пересекла.
#[derive(Clone, PartialEq, Debug)]
pub struct Evidence {
    /// Короткая подпись: `IO PSI full avg10`.
    pub label: String,
    /// Отформатированное значение: `31%`, `43 ms`.
    pub value: String,
    /// Порог, если он применим: `WARN > 10%`.
    pub threshold: Option<String>,
}

impl Evidence {
    #[must_use]
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Evidence {
            label: label.into(),
            value: value.into(),
            threshold: None,
        }
    }

    #[must_use]
    pub fn with_threshold(mut self, threshold: impl Into<String>) -> Self {
        self.threshold = Some(threshold.into());
        self
    }
}

/// Идентификатор проблемы: правило + сущность. Стабилен между тактами,
/// поэтому проблема «продолжается», а не создаётся заново каждую секунду.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProblemId {
    pub rule: RuleId,
    pub entity: EntityId,
}

/// Идентификатор правила. Строка статическая, сравнение по указателю не используется.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RuleId(pub &'static str);

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Открытая проблема.
#[derive(Clone, PartialEq, Debug)]
pub struct Problem {
    pub id: ProblemId,
    pub severity: Severity,
    /// Имя сущности на момент обнаружения — читаемо и после её исчезновения.
    pub entity_name: String,
    /// Короткий заголовок: `IO latency 43 ms`.
    pub title: String,
    /// Одна строка пояснения, что именно измерено.
    pub summary: String,
    pub evidence: Vec<Evidence>,
    pub since: Timestamp,
    pub last_seen: Timestamp,
    /// Сколько последовательных тактов условие выполнялось.
    pub streak: u32,
}

impl Problem {
    #[must_use]
    pub fn rule(&self) -> RuleId {
        self.id.rule
    }

    /// Порядок сортировки для экрана Problems: сначала критичные, затем более старые.
    #[must_use]
    pub fn sort_key(&self) -> (u8, u64) {
        let sev = match self.severity {
            Severity::Crit => 0,
            Severity::Warn => 1,
            Severity::Info => 2,
        };
        (sev, self.since.as_millis())
    }
}

/// Состояние гистерезиса для одного правила и сущности.
///
/// Асимметричные пороги плюс требование нескольких последовательных попаданий —
/// триггер Шмитта. Без этого проблема «мигает» на границе порога.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Hysteresis {
    /// Сколько подряд тактов условие входа выполнено.
    pub enter_streak: u32,
    /// Сколько подряд тактов условие выхода выполнено.
    pub clear_streak: u32,
    pub open: bool,
}

impl Hysteresis {
    /// Обновляет состояние. `enter` — сработало условие входа,
    /// `clear` — сработало условие снятия.
    ///
    /// Возвращает `true`, если проблема должна считаться открытой после этого такта.
    pub fn update(&mut self, enter: bool, clear: bool, enter_after: u32, clear_after: u32) -> bool {
        if enter {
            self.enter_streak = self.enter_streak.saturating_add(1);
            self.clear_streak = 0;
        } else if clear {
            self.clear_streak = self.clear_streak.saturating_add(1);
            self.enter_streak = 0;
        } else {
            // Зона гистерезиса: ни вход, ни выход. Состояние сохраняется.
            self.enter_streak = 0;
            self.clear_streak = 0;
        }

        if !self.open && self.enter_streak >= enter_after.max(1) {
            self.open = true;
        } else if self.open && self.clear_streak >= clear_after.max(1) {
            self.open = false;
        }
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_sorts_critical_first() {
        let mut list = vec![Severity::Info, Severity::Crit, Severity::Warn];
        list.sort_unstable();
        assert_eq!(list, vec![Severity::Info, Severity::Warn, Severity::Crit]);
    }

    #[test]
    fn hysteresis_requires_consecutive_hits_to_open() {
        let mut h = Hysteresis::default();
        assert!(!h.update(true, false, 3, 3));
        assert!(!h.update(true, false, 3, 3));
        assert!(h.update(true, false, 3, 3));
    }

    #[test]
    fn hysteresis_requires_consecutive_clears_to_close() {
        let mut h = Hysteresis::default();
        for _ in 0..3 {
            h.update(true, false, 3, 3);
        }
        assert!(h.open);
        assert!(h.update(false, true, 3, 3));
        assert!(h.update(false, true, 3, 3));
        assert!(!h.update(false, true, 3, 3));
    }

    #[test]
    fn oscillating_signal_does_not_flap() {
        // Сигнал колеблется вокруг порога: вход/выход через такт.
        let mut h = Hysteresis::default();
        let mut transitions = 0;
        let mut prev = false;
        for i in 0..40 {
            let open = h.update(i % 2 == 0, i % 2 == 1, 3, 3);
            if open != prev {
                transitions += 1;
                prev = open;
            }
        }
        assert_eq!(
            transitions, 0,
            "гистерезис обязан подавлять колебание через такт"
        );
    }

    #[test]
    fn dead_zone_preserves_state() {
        let mut h = Hysteresis::default();
        for _ in 0..3 {
            h.update(true, false, 3, 3);
        }
        // Значение в зоне гистерезиса: ни вход, ни снятие.
        for _ in 0..10 {
            assert!(h.update(false, false, 3, 3));
        }
    }
}
