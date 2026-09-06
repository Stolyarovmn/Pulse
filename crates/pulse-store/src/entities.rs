//! Журнал сущностей — основа A/B diff.
//!
//! Запись живёт и после смерти сущности: именно она позволяет ответить на
//! вопрос «что исчезло между A и B». Удаляется по горизонту истории или по
//! жёсткому лимиту числа записей.

use std::collections::HashMap;

use pulse_core::entity::{EntityId, EntityKind, EntityRecord};
use pulse_core::time::Timestamp;

/// Журнал записей о сущностях.
#[derive(Debug, Default)]
pub struct Entities {
    records: HashMap<EntityId, EntityRecord>,
    max_records: usize,
}

impl Entities {
    #[must_use]
    pub fn new(max_records: usize) -> Self {
        Entities {
            records: HashMap::new(),
            max_records: max_records.max(16),
        }
    }

    /// Обновляет или создаёт запись.
    pub fn upsert(&mut self, record: EntityRecord) {
        match self.records.get_mut(&record.id) {
            Some(existing) => {
                // `first_seen` не переписываем: он определяет начало жизни.
                let first_seen = existing.first_seen.min(record.first_seen);
                *existing = record;
                existing.first_seen = first_seen;
            }
            None => {
                let _ = self.records.insert(record.id, record);
            }
        }
    }

    /// Продлевает время жизни живых сущностей.
    pub fn touch(&mut self, ids: &[EntityId], at: Timestamp) {
        for id in ids {
            if let Some(record) = self.records.get_mut(id) {
                record.last_seen = at;
                record.alive = true;
            }
        }
    }

    /// Удаляет записи, чей `last_seen` старше горизонта, и соблюдает лимит.
    pub fn prune(&mut self, horizon: Timestamp) {
        self.records
            .retain(|_, record| record.alive || record.last_seen >= horizon);

        if self.records.len() > self.max_records {
            // Вытесняем самые давно не наблюдавшиеся записи.
            let mut by_age: Vec<(EntityId, Timestamp)> = self
                .records
                .iter()
                .map(|(id, record)| (*id, record.last_seen))
                .collect();
            by_age.sort_unstable_by_key(|(_, last_seen)| *last_seen);
            let excess = self.records.len().saturating_sub(self.max_records);
            for (id, _) in by_age.into_iter().take(excess) {
                let _ = self.records.remove(&id);
            }
        }
    }

    /// Освобождает журнал жизненного цикла до требуемого бюджета байт.
    ///
    /// Последний рубеж memory ceiling: живые записи не удаляются никогда —
    /// без них снимок перестанет объяснять, что за сущности он показывает.
    /// Отдаются только мёртвые, начиная с самых давно не наблюдавшихся.
    /// Возвращает `(число записей, байты)`.
    pub fn evict_dead_bytes(&mut self, requested: u64) -> (u64, u64) {
        if requested == 0 {
            return (0, 0);
        }
        let per_record = 256_u64;
        let mut dead: Vec<(EntityId, Timestamp)> = self
            .records
            .iter()
            .filter(|(_, record)| !record.alive)
            .map(|(id, record)| (*id, record.last_seen))
            .collect();
        dead.sort_unstable_by_key(|(_, last_seen)| *last_seen);

        let mut count = 0_u64;
        let mut bytes = 0_u64;
        for (id, _) in dead {
            if bytes >= requested {
                break;
            }
            if self.records.remove(&id).is_some() {
                count = count.saturating_add(1);
                bytes = bytes.saturating_add(per_record);
            }
        }
        (count, bytes)
    }

    /// Сущности, живые в момент `at`.
    #[must_use]
    pub fn at(&self, at: Timestamp) -> Vec<&EntityRecord> {
        let mut found: Vec<&EntityRecord> =
            self.records.values().filter(|r| r.alive_at(at)).collect();
        // Устойчивый порядок: результат diff не должен зависеть от обхода таблицы.
        found.sort_unstable_by(|a, b| {
            a.kind
                .rank()
                .cmp(&b.kind.rank())
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.as_u64().cmp(&b.id.as_u64()))
        });
        found
    }

    #[must_use]
    pub fn get(&self, id: EntityId) -> Option<&EntityRecord> {
        self.records.get(&id)
    }

    #[must_use]
    pub fn kind_of(&self, id: EntityId) -> Option<EntityKind> {
        self.records.get(&id).map(|r| r.kind)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &EntityRecord> + '_ {
        self.records.values()
    }

    #[must_use]
    pub fn approx_bytes(&self) -> u64 {
        // Запись содержит ключ, имя и метки; 256 байт — грубая, но честная оценка.
        u64::try_from(self.records.len() * 256).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::{EntityKey, Labels};

    fn record(id: u32, first: u64, last: u64, alive: bool) -> EntityRecord {
        EntityRecord {
            id: EntityId::new(id, 1),
            key: EntityKey::Process {
                pid: i32::try_from(id).unwrap_or(1),
                start_ticks: 1,
            },
            kind: EntityKind::Process,
            name: format!("p{id}"),
            parent: None,
            labels: Labels::new(),
            first_seen: Timestamp::from_millis(first),
            last_seen: Timestamp::from_millis(last),
            alive,
        }
    }

    #[test]
    fn at_returns_only_entities_alive_at_that_moment() {
        let mut entities = Entities::new(100);
        entities.upsert(record(1, 1_000, 5_000, false));
        entities.upsert(record(2, 4_000, 9_000, true));

        let early = entities.at(Timestamp::from_millis(2_000));
        assert_eq!(early.len(), 1);
        assert_eq!(early.first().map(|r| r.name.as_str()), Some("p1"));

        let late = entities.at(Timestamp::from_millis(8_000));
        assert_eq!(late.len(), 1);
        assert_eq!(late.first().map(|r| r.name.as_str()), Some("p2"));

        let both = entities.at(Timestamp::from_millis(4_500));
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn upsert_preserves_first_seen() {
        let mut entities = Entities::new(100);
        entities.upsert(record(1, 1_000, 2_000, true));
        entities.upsert(record(1, 5_000, 6_000, true));
        let found = entities.get(EntityId::new(1, 1)).map(|r| r.first_seen);
        assert_eq!(found, Some(Timestamp::from_millis(1_000)));
    }

    #[test]
    fn dead_records_survive_until_horizon() {
        let mut entities = Entities::new(100);
        entities.upsert(record(1, 1_000, 2_000, false));
        entities.prune(Timestamp::from_millis(1_500));
        assert_eq!(entities.len(), 1, "запись ещё в горизонте истории");
        entities.prune(Timestamp::from_millis(3_000));
        assert_eq!(entities.len(), 0, "запись вышла из горизонта");
    }

    #[test]
    fn live_records_are_never_pruned_by_horizon() {
        let mut entities = Entities::new(100);
        entities.upsert(record(1, 1_000, 2_000, true));
        entities.prune(Timestamp::from_millis(1_000_000));
        assert_eq!(entities.len(), 1);
    }

    #[test]
    fn record_limit_evicts_oldest() {
        let mut entities = Entities::new(16);
        for i in 0..40u32 {
            entities.upsert(record(i, u64::from(i) * 10, u64::from(i) * 10 + 5, false));
        }
        entities.prune(Timestamp::ZERO);
        assert!(entities.len() <= 16, "записей {} > 16", entities.len());
        // Должны остаться самые свежие.
        assert!(entities.get(EntityId::new(39, 1)).is_some());
    }

    #[test]
    fn touch_extends_lifetime() {
        let mut entities = Entities::new(100);
        entities.upsert(record(1, 1_000, 2_000, true));
        entities.touch(&[EntityId::new(1, 1)], Timestamp::from_millis(9_000));
        assert!(entities
            .at(Timestamp::from_millis(8_000))
            .iter()
            .any(|r| r.name == "p1"));
    }
}
