//! Тёплый слой: агрегаты по бакетам для сущностей-владельцев.
//!
//! Почему не для всех серий. На типичном хосте процессов тысячи, а владельцев
//! (host, cgroup, unit, container, pod, диски, интерфейсы) — десятки. Ведение
//! тёплого слоя по процессам увеличило бы память на порядок, не добавив
//! диагностической ценности: расследование «что изменилось час назад» ведётся
//! по владельцам, а мгновенный процесс уже виден в горячем кольце.

use std::collections::{HashMap, VecDeque};

use pulse_core::sample::SeriesKey;
use pulse_core::time::Timestamp;

use crate::aggregate::Aggregate;

/// Бакет: номер и агрегат.
type Buckets = VecDeque<(u64, Aggregate)>;

/// Тёплый слой.
#[derive(Debug)]
pub struct Warm {
    series: HashMap<SeriesKey, Buckets>,
    retained_buckets: usize,
    /// Ширина бакета в тактах.
    bucket_ticks: u64,
    /// Глубина в бакетах.
    depth: usize,
    /// Время начала каждого бакета, чтобы отвечать на запросы по времени.
    bucket_start: HashMap<u64, Timestamp>,
}

impl Warm {
    #[must_use]
    pub fn new(bucket_ticks: usize, depth: usize) -> Self {
        Warm {
            series: HashMap::new(),
            retained_buckets: 0,
            bucket_ticks: u64::try_from(bucket_ticks.max(1)).unwrap_or(1),
            depth: depth.max(1),
            bucket_start: HashMap::new(),
        }
    }

    /// Номер бакета для такта.
    #[must_use]
    pub fn bucket_of(&self, seq: u64) -> u64 {
        seq / self.bucket_ticks
    }

    /// Запоминает время начала бакета (первое наблюдение в нём).
    pub fn note_bucket_start(&mut self, seq: u64, at: Timestamp) {
        let bucket = self.bucket_of(seq);
        let _ = self.bucket_start.entry(bucket).or_insert(at);
    }

    /// Добавляет наблюдение в тёплый слой.
    pub fn push(&mut self, key: SeriesKey, seq: u64, value: f64, at: Timestamp) {
        if !value.is_finite() {
            return;
        }
        let bucket = self.bucket_of(seq);
        let point = Aggregate::point(value, at.as_millis());
        let buckets = self
            .series
            .entry(key)
            .or_insert_with(|| VecDeque::with_capacity(4));

        match buckets.back_mut() {
            Some((existing, aggregate)) if *existing == bucket => {
                *aggregate = aggregate.merge(&point);
            }
            _ => {
                buckets.push_back((bucket, point));
                self.retained_buckets = self.retained_buckets.saturating_add(1);
                while buckets.len() > self.depth {
                    if buckets.pop_front().is_some() {
                        self.retained_buckets = self.retained_buckets.saturating_sub(1);
                    }
                }
            }
        }
    }

    /// Удаляет серии, чьи бакеты полностью вышли за глубину слоя.
    pub fn prune(&mut self, oldest_bucket: u64) {
        let retained_buckets = &mut self.retained_buckets;
        self.series.retain(|_, buckets| {
            while buckets.front().is_some_and(|(b, _)| *b < oldest_bucket) {
                if buckets.pop_front().is_some() {
                    *retained_buckets = retained_buckets.saturating_sub(1);
                }
            }
            !buckets.is_empty()
        });
        self.bucket_start
            .retain(|bucket, _| *bucket >= oldest_bucket);
    }

    /// Агрегаты серии в интервале `[from, to]` вместе со временем начала бакета.
    #[must_use]
    pub fn buckets_in(
        &self,
        key: SeriesKey,
        from: Timestamp,
        to: Timestamp,
    ) -> Vec<(Timestamp, Aggregate)> {
        let Some(buckets) = self.series.get(&key) else {
            return Vec::new();
        };
        buckets
            .iter()
            .filter_map(|(bucket, aggregate)| {
                let at = self.bucket_start.get(bucket).copied()?;
                at.within(from, to).then_some((at, *aggregate))
            })
            .collect()
    }

    /// Момент начала самого раннего сохранённого бакета.
    ///
    /// Нужен, чтобы история могла честно назвать свою глубину: без этого
    /// `oldest` описывал только горячее кольцо и занижал доступный интервал
    /// в десятки раз.
    #[must_use]
    pub fn oldest(&self) -> Option<Timestamp> {
        let earliest = self
            .series
            .values()
            .filter_map(|buckets| buckets.front().map(|(bucket, _)| *bucket))
            .min()?;
        self.bucket_start.get(&earliest).copied()
    }

    #[must_use]
    pub fn series_count(&self) -> usize {
        self.series.len()
    }

    #[must_use]
    pub fn approx_bytes(&self) -> u64 {
        let per_bucket = std::mem::size_of::<(u64, Aggregate)>();
        let per_series = std::mem::size_of::<SeriesKey>() + std::mem::size_of::<Buckets>() + 48;
        let starts = self.bucket_start.len()
            * (std::mem::size_of::<u64>() + std::mem::size_of::<Timestamp>() + 32);
        u64::try_from(self.retained_buckets * per_bucket + self.series.len() * per_series + starts)
            .unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::EntityId;
    use pulse_core::metric::MetricId;

    fn key() -> SeriesKey {
        SeriesKey::new(EntityId::new(1, 1), MetricId(1))
    }

    #[test]
    fn observations_in_one_bucket_are_merged() {
        let mut warm = Warm::new(10, 5);
        for seq in 0..10u64 {
            let at = Timestamp::from_millis(1_000 + seq * 1_000);
            warm.note_bucket_start(seq, at);
            warm.push(key(), seq, f64::from(u32::try_from(seq).unwrap_or(0)), at);
        }
        let buckets = warm.buckets_in(key(), Timestamp::ZERO, Timestamp::MAX);
        assert_eq!(buckets.len(), 1, "10 тактов при ширине 10 — один бакет");
        let aggregate = buckets.first().map(|(_, a)| *a).unwrap_or_default();
        assert_eq!(aggregate.count, 10);
        assert!((aggregate.max - 9.0).abs() < 1e-12);
        assert!((aggregate.min - 0.0).abs() < 1e-12);
    }

    #[test]
    fn depth_is_bounded() {
        let mut warm = Warm::new(2, 3);
        for seq in 0..100u64 {
            let at = Timestamp::from_millis(seq * 1_000);
            warm.note_bucket_start(seq, at);
            warm.push(key(), seq, 1.0, at);
        }
        let buckets = warm.buckets_in(key(), Timestamp::ZERO, Timestamp::MAX);
        assert!(buckets.len() <= 3, "бакетов {} > 3", buckets.len());
    }

    #[test]
    fn prune_removes_dead_series() {
        let mut warm = Warm::new(2, 10);
        let at = Timestamp::from_millis(1_000);
        warm.note_bucket_start(0, at);
        warm.push(key(), 0, 1.0, at);
        assert_eq!(warm.series_count(), 1);
        warm.prune(100);
        assert_eq!(warm.series_count(), 0);
    }
}
