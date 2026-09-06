//! Образцы телеметрии и ключи серий.

use crate::entity::EntityId;
use crate::metric::MetricId;

/// Ключ серии: сущность + метрика. 12 байт, `Copy`, дешёвый хеш.
///
/// Cardinality контролируется тем, что метрика — это `u16` из статического реестра,
/// а сущность — идентификатор из арены, а не набор строковых меток.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SeriesKey {
    pub entity: EntityId,
    pub metric: MetricId,
}

impl SeriesKey {
    #[must_use]
    pub const fn new(entity: EntityId, metric: MetricId) -> Self {
        SeriesKey { entity, metric }
    }
}

/// Один образец за такт.
///
/// Counter-метрики передаются сырыми накопленными значениями; скорость считается
/// на чтении. Так downsampling остаётся корректным, а перезапуск источника
/// (сброс счётчика) виден в данных, а не маскируется усреднением.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Sample {
    pub series: SeriesKey,
    pub value: f64,
}

impl Sample {
    #[must_use]
    pub const fn new(entity: EntityId, metric: MetricId, value: f64) -> Self {
        Sample {
            series: SeriesKey::new(entity, metric),
            value,
        }
    }

    #[must_use]
    pub const fn entity(&self) -> EntityId {
        self.series.entity
    }

    #[must_use]
    pub const fn metric(&self) -> MetricId {
        self.series.metric
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_key_is_small() {
        assert!(std::mem::size_of::<SeriesKey>() <= 16);
    }

    #[test]
    fn same_entity_and_metric_yield_equal_keys() {
        let e = EntityId::new(4, 1);
        let m = MetricId(7);
        assert_eq!(SeriesKey::new(e, m), SeriesKey::new(e, m));
        assert_ne!(SeriesKey::new(e, m), SeriesKey::new(EntityId::new(4, 2), m));
    }
}
