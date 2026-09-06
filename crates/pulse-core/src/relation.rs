//! Связи между сущностями. Именно они превращают набор таблиц в граф.

use std::fmt;

use crate::entity::EntityId;
use crate::time::Timestamp;

/// Тип связи. Направление всегда `from -> to`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RelationKind {
    /// Родитель в иерархии процессов или cgroup.
    ParentOf,
    /// Процесс исполняется внутри cgroup/контейнера.
    RunsIn,
    /// Ресурс принадлежит владельцу (cgroup -> unit, container -> pod).
    OwnedBy,
    /// Сущность входит в состав другой (процесс -> unit).
    MemberOf,
    /// Сущность обслуживается конкретным устройством (cgroup -> disk).
    BackedBy,
    /// Сущность слушает адрес (заготовка для socket-графа).
    ListensOn,
    /// Сетевое соединение (заготовка для eBPF-этапа).
    ConnectsTo,
    /// Размещение на узле.
    ScheduledOn,
}

impl RelationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RelationKind::ParentOf => "parent_of",
            RelationKind::RunsIn => "runs_in",
            RelationKind::OwnedBy => "owned_by",
            RelationKind::MemberOf => "member_of",
            RelationKind::BackedBy => "backed_by",
            RelationKind::ListensOn => "listens_on",
            RelationKind::ConnectsTo => "connects_to",
            RelationKind::ScheduledOn => "scheduled_on",
        }
    }

    /// Считается ли связь «ресурсной зависимостью» для скоринга diff-доказательств.
    ///
    /// Формула ранжирования отдельно учитывает прямую ресурсную зависимость,
    /// чтобы «postgres обслуживается этим диском» не растворялось в общей
    /// графовой близости.
    #[must_use]
    pub const fn is_resource_dependency(self) -> bool {
        matches!(
            self,
            RelationKind::BackedBy | RelationKind::ConnectsTo | RelationKind::RunsIn
        )
    }
}

impl fmt::Display for RelationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Связь с временем жизни. `until = None` означает «активна сейчас».
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Relation {
    pub from: EntityId,
    pub kind: RelationKind,
    pub to: EntityId,
    pub since: Timestamp,
    pub until: Option<Timestamp>,
}

impl Relation {
    #[must_use]
    pub const fn new(from: EntityId, kind: RelationKind, to: EntityId, since: Timestamp) -> Self {
        Relation {
            from,
            kind,
            to,
            since,
            until: None,
        }
    }

    /// Была ли связь активна в момент `at`.
    #[must_use]
    pub fn active_at(&self, at: Timestamp) -> bool {
        at >= self.since && self.until.is_none_or(|until| at <= until)
    }

    /// Ключ связи без времени: используется для дедупликации внутри такта.
    #[must_use]
    pub const fn triple(&self) -> (EntityId, RelationKind, EntityId) {
        (self.from, self.kind, self.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_relation_is_active_forever() {
        let r = Relation::new(
            EntityId::new(1, 1),
            RelationKind::RunsIn,
            EntityId::new(2, 1),
            Timestamp::from_millis(100),
        );
        assert!(!r.active_at(Timestamp::from_millis(99)));
        assert!(r.active_at(Timestamp::from_millis(100)));
        assert!(r.active_at(Timestamp::from_millis(10_000)));
    }

    #[test]
    fn closed_relation_respects_end() {
        let mut r = Relation::new(
            EntityId::new(1, 1),
            RelationKind::RunsIn,
            EntityId::new(2, 1),
            Timestamp::from_millis(100),
        );
        r.until = Some(Timestamp::from_millis(200));
        assert!(r.active_at(Timestamp::from_millis(200)));
        assert!(!r.active_at(Timestamp::from_millis(201)));
    }

    #[test]
    fn resource_dependencies_are_explicit() {
        assert!(RelationKind::BackedBy.is_resource_dependency());
        assert!(!RelationKind::ParentOf.is_resource_dependency());
    }
}
