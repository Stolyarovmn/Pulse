//! Горячее кольцо: сырые значения всех серий на последние `capacity` тактов.
//!
//! Выбор представления. Серии живут и умирают вместе с сущностями (процессы
//! появляются и исчезают каждую секунду), поэтому плотный вектор на такт
//! (`VecDeque<Option<f64>>` длиной `capacity` для каждой серии) стоил бы
//! `capacity` слотов даже для серии, прожившей один такт, и требовал бы
//! выравнивания при каждом сдвиге окна. Здесь хранятся разрежённые пары
//! `(номер такта, значение)`: серия занимает ровно столько, сколько наблюдалась,
//! сдвиг окна — это отбрасывание головы дека, а обращение к времени идёт через
//! общую ось. Цена — 16 байт на образец вместо 8.

use std::collections::HashMap;
use std::collections::VecDeque;

use pulse_core::sample::SeriesKey;
use pulse_core::time::Timestamp;

/// Одна серия: пары `(номер такта, значение)`, возрастающие по номеру.
type Points = VecDeque<(u64, f64)>;

/// Горячее кольцо.
#[derive(Debug)]
pub struct Hot {
    /// Ось времени: `axis[i]` — время такта с номером `seq_base + i`.
    axis: VecDeque<Timestamp>,
    /// Номер такта, соответствующий `axis[0]`.
    seq_base: u64,
    /// Номер следующего такта.
    next_seq: u64,
    series: HashMap<SeriesKey, Points>,
    retained_points: usize,
    capacity: usize,
    max_series: usize,
    samples_stored: u64,
    dropped_series: u64,
    warned_about_limit: bool,
}

impl Hot {
    #[must_use]
    pub fn new(capacity: usize, max_series: usize) -> Self {
        let capacity = capacity.max(2);
        Hot {
            axis: VecDeque::with_capacity(capacity),
            seq_base: 0,
            next_seq: 0,
            series: HashMap::new(),
            retained_points: 0,
            capacity,
            max_series,
            samples_stored: 0,
            dropped_series: 0,
            warned_about_limit: false,
        }
    }

    /// Открывает новый такт и сдвигает окно. Возвращает номер такта.
    pub fn begin_tick(&mut self, at: Timestamp) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.axis.push_back(at);
        while self.axis.len() > self.capacity {
            let _ = self.axis.pop_front();
            self.seq_base = self.seq_base.saturating_add(1);
        }
        seq
    }

    /// Записывает значение серии в текущий такт.
    pub fn push(&mut self, key: SeriesKey, seq: u64, value: f64) {
        if !value.is_finite() {
            return;
        }
        if !self.series.contains_key(&key) {
            if self.series.len() >= self.max_series {
                self.dropped_series = self.dropped_series.saturating_add(1);
                if !self.warned_about_limit {
                    self.warned_about_limit = true;
                    tracing::warn!(
                        max_series = self.max_series,
                        "достигнут предел числа серий: новые серии не создаются"
                    );
                }
                return;
            }
            let _ = self.series.insert(key, VecDeque::with_capacity(8));
        }
        if let Some(points) = self.series.get_mut(&key) {
            points.push_back((seq, value));
            self.retained_points = self.retained_points.saturating_add(1);
            self.samples_stored = self.samples_stored.saturating_add(1);
        }
    }

    /// Отбрасывает точки, вышедшие из окна, и пустые серии.
    ///
    /// Удаление пустых серий обязательно: без него ключи процессов, живших
    /// одну секунду, накапливались бы в таблице до исчерпания памяти.
    pub fn prune(&mut self) {
        let base = self.seq_base;
        let retained_points = &mut self.retained_points;
        self.series.retain(|_, points| {
            while points.front().is_some_and(|(seq, _)| *seq < base) {
                let _ = points.pop_front();
                *retained_points = retained_points.saturating_sub(1);
            }
            !points.is_empty()
        });
    }

    /// Сокращает глубину горячего окна и возвращает число вытесненных тактов.
    ///
    /// Используется только как аварийная защита потолка памяти после того, как
    /// тёплый слой уже сокращён. Минимум два такта сохраняется: без предыдущей
    /// точки нельзя считать производные величины текущего кадра.
    pub fn shrink_capacity(&mut self, requested: usize) -> usize {
        let previous_len = self.axis.len();
        self.capacity = requested.max(2).min(self.capacity);
        while self.axis.len() > self.capacity {
            let _ = self.axis.pop_front();
            self.seq_base = self.seq_base.saturating_add(1);
        }
        self.prune();
        previous_len.saturating_sub(self.axis.len())
    }

    /// Текущая настроенная глубина горячего окна.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Время такта по его номеру.
    #[must_use]
    pub fn tick_time(&self, seq: u64) -> Option<Timestamp> {
        let index = seq.checked_sub(self.seq_base)?;
        self.axis.get(usize::try_from(index).ok()?).copied()
    }

    /// Точки серии в интервале `[from, to]`.
    #[must_use]
    pub fn series_points(
        &self,
        key: SeriesKey,
        from: Timestamp,
        to: Timestamp,
    ) -> Vec<(Timestamp, f64)> {
        let Some(points) = self.series.get(&key) else {
            return Vec::new();
        };
        points
            .iter()
            .filter_map(|(seq, value)| {
                let at = self.tick_time(*seq)?;
                at.within(from, to).then_some((at, *value))
            })
            .collect()
    }

    /// Последнее значение серии.
    #[must_use]
    pub fn last(&self, key: SeriesKey) -> Option<f64> {
        self.series
            .get(&key)
            .and_then(|p| p.back())
            .map(|(_, v)| *v)
    }

    /// Последнее значение серии не позже `at`, вместе с его временем.
    #[must_use]
    pub fn last_before(&self, key: SeriesKey, at: Timestamp) -> Option<(Timestamp, f64)> {
        let points = self.series.get(&key)?;
        points.iter().rev().find_map(|(seq, value)| {
            let ts = self.tick_time(*seq)?;
            (ts <= at).then_some((ts, *value))
        })
    }

    /// Все ключи серий.
    pub fn keys(&self) -> impl Iterator<Item = SeriesKey> + '_ {
        self.series.keys().copied()
    }

    /// Последние значения всех серий вместе с моментом наблюдения.
    ///
    /// Момент обязателен: получатель должен отличать «наблюдали только что»
    /// от «последний раз видели много тактов назад», иначе устаревшее
    /// значение выглядит текущим.
    pub fn latest_pairs(&self) -> impl Iterator<Item = (SeriesKey, f64, Timestamp)> + '_ {
        self.series.iter().filter_map(|(key, points)| {
            let (seq, value) = points.back()?;
            let at = self.tick_time(*seq)?;
            Some((*key, *value, at))
        })
    }

    #[must_use]
    pub fn series_count(&self) -> usize {
        self.series.len()
    }

    #[must_use]
    pub fn samples_stored(&self) -> u64 {
        self.samples_stored
    }

    #[must_use]
    pub fn dropped_series(&self) -> u64 {
        self.dropped_series
    }

    #[must_use]
    pub fn oldest(&self) -> Option<Timestamp> {
        self.axis.front().copied()
    }

    #[must_use]
    pub fn newest(&self) -> Option<Timestamp> {
        self.axis.back().copied()
    }

    #[must_use]
    pub fn ticks(&self) -> Vec<Timestamp> {
        self.axis.iter().copied().collect()
    }

    /// Оценка интервала между тактами в миллисекундах.
    #[must_use]
    pub fn tick_interval_ms(&self) -> u64 {
        let len = self.axis.len();
        if len < 2 {
            return 1_000;
        }
        let last = self.axis.back().copied().unwrap_or(Timestamp::ZERO);
        let first = self.axis.front().copied().unwrap_or(Timestamp::ZERO);
        let span = last.as_millis().saturating_sub(first.as_millis());
        let steps = u64::try_from(len - 1).unwrap_or(1).max(1);
        (span / steps).max(1)
    }

    /// Оценка занятой памяти: пары `(u64, f64)` плюс накладные расходы таблицы.
    #[must_use]
    pub fn approx_bytes(&self) -> u64 {
        let per_point = std::mem::size_of::<(u64, f64)>();
        let per_series = std::mem::size_of::<SeriesKey>() + std::mem::size_of::<Points>() + 48;
        let axis = self.axis.len() * std::mem::size_of::<Timestamp>();
        u64::try_from(self.retained_points * per_point + self.series.len() * per_series + axis)
            .unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::EntityId;
    use pulse_core::metric::MetricId;

    fn key(n: u32) -> SeriesKey {
        SeriesKey::new(EntityId::new(n, 1), MetricId(1))
    }

    fn feed(hot: &mut Hot, ticks: usize, series: usize) {
        for t in 0..ticks {
            let at = Timestamp::from_millis(1_000 + (t as u64) * 1_000);
            let seq = hot.begin_tick(at);
            for s in 0..series {
                hot.push(key(s as u32), seq, t as f64);
            }
            hot.prune();
        }
    }

    #[test]
    fn window_does_not_grow_beyond_capacity() {
        let mut hot = Hot::new(50, 1_000);
        feed(&mut hot, 1_000, 3);
        assert_eq!(hot.ticks().len(), 50);
        let points = hot.series_points(key(0), Timestamp::ZERO, Timestamp::MAX);
        assert!(points.len() <= 50, "точек {} > 50", points.len());
    }

    #[test]
    fn empty_series_are_removed() {
        let mut hot = Hot::new(5, 1_000);
        // Серия наблюдалась один такт, затем исчезла.
        let seq = hot.begin_tick(Timestamp::from_millis(1_000));
        hot.push(key(7), seq, 1.0);
        hot.prune();
        assert_eq!(hot.series_count(), 1);
        for t in 1..10 {
            let _ = hot.begin_tick(Timestamp::from_millis(1_000 + t * 1_000));
            hot.prune();
        }
        assert_eq!(
            hot.series_count(),
            0,
            "ключ мёртвой серии должен освободиться"
        );
    }

    #[test]
    fn series_limit_is_enforced_and_counted() {
        let mut hot = Hot::new(10, 4);
        let seq = hot.begin_tick(Timestamp::from_millis(1_000));
        for s in 0..10 {
            hot.push(key(s), seq, 1.0);
        }
        assert_eq!(hot.series_count(), 4);
        assert_eq!(hot.dropped_series(), 6);
    }

    #[test]
    fn non_finite_values_are_rejected() {
        let mut hot = Hot::new(10, 10);
        let seq = hot.begin_tick(Timestamp::from_millis(1_000));
        hot.push(key(0), seq, f64::NAN);
        hot.push(key(0), seq, f64::INFINITY);
        assert_eq!(hot.series_count(), 0);
    }

    #[test]
    fn points_are_filtered_by_interval() {
        let mut hot = Hot::new(100, 100);
        feed(&mut hot, 10, 1);
        let points = hot.series_points(
            key(0),
            Timestamp::from_millis(4_000),
            Timestamp::from_millis(6_000),
        );
        assert_eq!(points.len(), 3);
        assert_eq!(
            points.first().map(|p| p.0),
            Some(Timestamp::from_millis(4_000))
        );
    }

    #[test]
    fn last_before_carries_previous_observation() {
        let mut hot = Hot::new(100, 100);

        feed(&mut hot, 5, 1);
        let found = hot.last_before(key(0), Timestamp::from_millis(3_500));
        assert_eq!(found.map(|(t, _)| t), Some(Timestamp::from_millis(3_000)));
        assert!(hot
            .last_before(key(0), Timestamp::from_millis(500))
            .is_none());
    }
    #[test]
    fn shrink_capacity_evicts_old_ticks_but_keeps_latest_two() {
        let mut hot = Hot::new(10, 100);
        feed(&mut hot, 10, 1);
        let newest = hot.newest();
        let removed = hot.shrink_capacity(2);
        assert_eq!(removed, 8);
        assert_eq!(hot.capacity(), 2);
        assert_eq!(hot.ticks().len(), 2);
        assert_eq!(hot.newest(), newest, "свежая точка не вытесняется");
        assert_eq!(
            hot.series_points(key(0), Timestamp::ZERO, Timestamp::MAX)
                .len(),
            2
        );
    }

    #[test]
    fn shrink_capacity_never_grows_or_drops_below_two() {
        let mut hot = Hot::new(5, 100);
        feed(&mut hot, 5, 1);
        assert_eq!(hot.shrink_capacity(100), 0, "метод не расширяет окно");
        assert_eq!(hot.capacity(), 5);
        let _ = hot.shrink_capacity(0);
        assert_eq!(hot.capacity(), 2);
    }

    #[test]
    fn tick_interval_is_estimated_from_axis() {
        let mut hot = Hot::new(100, 100);
        feed(&mut hot, 5, 1);
        assert_eq!(hot.tick_interval_ms(), 1_000);
    }
}
