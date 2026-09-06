//! События: жизненный цикл сущностей, переходы насыщения, ошибки сбора.
//!
//! События не подвергаются downsampling: теряется именно та информация,
//! ради которой смотрят историю. Ограничивается только retention.

use std::fmt;

use crate::entity::{EntityId, EntityKind};
use crate::problem::{RuleId, Severity};
use crate::time::Timestamp;

/// Вид события.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum EventKind {
    /// Наблюдение началось: базовая инвентаризация хоста (разделы 125, 132).
    ///
    /// Одно событие вместо сотен `Created`. Объекты, существовавшие до
    /// подключения наблюдателя, изменением не являются: PULSE не знает, когда
    /// они возникли, и не имеет права выдавать момент своего запуска за момент
    /// их создания.
    ObservationStarted,
    /// Сущность появилась.
    Created,
    /// Сущность исчезла.
    Deleted,
    /// Логическая сущность жива, но её носитель сменился (перезапуск unit/контейнера).
    Restarted,
    /// Сменился родитель при неизменной идентичности сущности.
    Reparented,
    /// Изменились значимые метаданные (имя, лимиты).
    MetadataChanged,
    /// Ядро убило процесс по нехватке памяти.
    OomKill,
    /// Проблема открылась.
    ProblemOpened,
    /// Проблема закрылась.
    ProblemClosed,
    /// Ошибка коллектора. Хранится, чтобы «пустой график» не выглядел как «всё хорошо».
    CollectorError,
}

impl EventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EventKind::ObservationStarted => "observation_started",
            EventKind::Created => "created",
            EventKind::Deleted => "deleted",
            EventKind::Restarted => "restarted",
            EventKind::Reparented => "reparented",
            EventKind::MetadataChanged => "metadata_changed",
            EventKind::OomKill => "oom_kill",
            EventKind::ProblemOpened => "problem_opened",
            EventKind::ProblemClosed => "problem_closed",
            EventKind::CollectorError => "collector_error",
        }
    }

    /// Знак для таймлайна TUI.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            // Раздел 133: `●` - обычное значимое событие жизненного цикла.
            EventKind::ObservationStarted => "●",
            EventKind::Created => "+",
            EventKind::Deleted => "-",
            EventKind::Restarted => "↻",
            EventKind::Reparented => "→",
            EventKind::MetadataChanged => "~",
            EventKind::OomKill => "!",
            EventKind::ProblemOpened => "▲",
            EventKind::ProblemClosed => "▼",
            EventKind::CollectorError => "?",
        }
    }

    /// Структурные события участвуют в A/B diff графа.
    #[must_use]
    pub const fn is_structural(self) -> bool {
        matches!(
            self,
            EventKind::Created
                | EventKind::Deleted
                | EventKind::Restarted
                | EventKind::Reparented
                | EventKind::MetadataChanged
        )
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Событие в журнале.
#[derive(Clone, PartialEq, Debug)]
pub struct Event {
    pub at: Timestamp,
    pub kind: EventKind,
    pub severity: Severity,
    /// Сущность, к которой относится событие. `None` для системных событий.
    pub entity: Option<EntityId>,
    pub entity_kind: Option<EntityKind>,
    /// Снимок имени: остаётся читаемым после исчезновения сущности.
    pub entity_name: String,
    /// Человекочитаемая деталь. Уже санитизирована.
    pub detail: String,
    /// Правило, если событие породила проблема.
    pub rule: Option<RuleId>,
}

impl Event {
    #[must_use]
    pub fn new(at: Timestamp, kind: EventKind, entity_name: impl Into<String>) -> Self {
        Event {
            at,
            kind,
            severity: Severity::Info,
            entity: None,
            entity_kind: None,
            entity_name: entity_name.into(),
            detail: String::new(),
            rule: None,
        }
    }

    #[must_use]
    pub fn entity(mut self, id: EntityId, kind: EntityKind) -> Self {
        self.entity = Some(id);
        self.entity_kind = Some(kind);
        self
    }

    #[must_use]
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    #[must_use]
    pub fn rule(mut self, rule: RuleId) -> Self {
        self.rule = Some(rule);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_events_are_classified() {
        assert!(EventKind::Restarted.is_structural());
        assert!(!EventKind::ProblemOpened.is_structural());
        assert!(!EventKind::CollectorError.is_structural());
    }

    #[test]
    fn builder_sets_fields() {
        let e = Event::new(Timestamp::from_millis(5), EventKind::OomKill, "payment-api")
            .entity(EntityId::new(3, 1), EntityKind::Container)
            .severity(Severity::Crit)
            .detail("killed pid 18321");
        assert_eq!(e.severity, Severity::Crit);
        assert_eq!(e.entity_kind, Some(EntityKind::Container));
        assert!(e.detail.contains("18321"));
    }
}
