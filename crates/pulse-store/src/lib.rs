//! `pulse-store` — локальная история наблюдений.
//!
//! Три независимых журнала, а не один «поток метрик»:
//!
//! * телеметрия — горячее кольцо сырых значений плюс тёплый слой агрегатов;
//! * сущности — записи жизненного цикла, переживающие смерть сущности;
//! * события — не агрегируются никогда.
//!
//! Такое разделение и делает возможным семантический A/B diff: чтобы сказать
//! «процесс исчез, контейнер перезапустился, задержка диска выросла», нужны все
//! три класса данных, а не только числовые серии.
//!
//! Counter-серии хранятся сырыми (накопленными). Скорость считается на чтении
//! в [`History::rate`]: так окно усреднения выбирает потребитель, а обработка
//! сброса счётчика живёт в одном месте, а не в каждом коллекторе.

// В тестах unwrap/expect/panic допустимы: тест обязан упасть и указать строку.
// В рабочем коде запрет остаётся в силе (см. lints workspace).
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::field_reassign_with_default
    )
)]

pub mod aggregate;
pub mod entities;
pub mod events;
pub mod hot;
pub mod warm;

use pulse_core::config::Store as StoreConfig;
use pulse_core::entity::{EntityId, EntityKind, EntityRecord};
use pulse_core::event::Event;
use pulse_core::metric::{describe, retains_long_window, MetricKind};
use pulse_core::sample::SeriesKey;
use pulse_core::snapshot::LatestValues;
use pulse_core::time::Timestamp;
use pulse_core::TickBatch;

pub use aggregate::Aggregate;
pub use entities::Entities;
pub use events::Events;
pub use hot::Hot;
pub use warm::Warm;

/// Статистика значений серии в окне.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowStats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    /// Выборочное стандартное отклонение, алгоритм Уэлфорда.
    ///
    /// При `approximate = true` считается по средним бакетов, а не по сырым
    /// значениям, и систематически занижен: внутрибакетный разброс из
    /// агрегата невосстановим.
    pub stddev: f64,
    pub first: f64,
    pub last: f64,
    /// Статистика восстановлена из агрегатов тёплого слоя, а не из сырых
    /// точек. Потребитель обязан не делить на [`WindowStats::stddev`]:
    /// заниженный знаменатель раздувает z-оценку.
    pub approximate: bool,
}

/// Локальная история.
#[derive(Debug)]
pub struct History {
    hot: Hot,
    warm: Warm,
    entities: Entities,
    events: Events,
    warm_bucket_ticks: usize,
    warm_buckets: usize,
    current_seq: u64,
    /// Потолок памяти истории; `0` — без проверки.
    max_bytes: u64,
    /// Диагностика memory manager.
    eviction_iterations: u64,
    eviction_no_progress: u64,
    evicted_buckets: u64,
    evicted_hot_ticks: u64,
    evicted_events: u64,
    evicted_entity_records: u64,
    peak_bytes: u64,
    ceiling_warned: bool,
}

impl History {
    #[must_use]
    pub fn new(cfg: &StoreConfig) -> Self {
        History {
            hot: Hot::new(cfg.hot_ticks, cfg.max_series),
            warm: Warm::new(cfg.warm_bucket_ticks, cfg.warm_buckets),
            entities: Entities::new(cfg.max_entity_records),
            events: Events::new(cfg.max_events),
            warm_bucket_ticks: cfg.warm_bucket_ticks.max(1),
            warm_buckets: cfg.warm_buckets.max(1),
            current_seq: 0,
            max_bytes: cfg.max_bytes,
            eviction_iterations: 0,
            eviction_no_progress: 0,
            evicted_buckets: 0,
            evicted_hot_ticks: 0,
            evicted_events: 0,
            evicted_entity_records: 0,
            peak_bytes: 0,
            ceiling_warned: false,
        }
    }

    /// Принимает результат такта сбора.
    ///
    /// Порядок важен: сначала записи сущностей (по ним определяется вид сущности
    /// и, значит, нужен ли тёплый слой), затем образцы и события.
    pub fn ingest(&mut self, batch: &TickBatch) {
        let seq = self.hot.begin_tick(batch.at);
        self.current_seq = seq;
        self.warm.note_bucket_start(seq, batch.at);

        for record in &batch.records {
            self.entities.upsert(record.clone());
        }
        self.entities.touch(&batch.alive, batch.at);

        for sample in &batch.samples {
            self.hot.push(sample.series, seq, sample.value);
            // Тёплый слой — только для метрик длинного окна и только для
            // владельцев. Иначе память уходит на серии, которые никто не
            // запрашивает за пределами горячего кольца: на типовом хосте это
            // тысячи серий против сотен нужных.
            if retains_long_window(sample.series.metric)
                && self.entities.kind_of(sample.series.entity) != Some(EntityKind::Process)
            {
                self.warm.push(sample.series, seq, sample.value, batch.at);
            }
        }

        for event in &batch.events {
            self.events.push(event.clone());
        }

        self.hot.prune();
        let oldest_bucket = self
            .warm
            .bucket_of(seq)
            .saturating_sub(u64::try_from(self.warm_buckets).unwrap_or(1));
        self.warm.prune(oldest_bucket);
        self.entities.prune(self.horizon());
        self.enforce_memory_ceiling(seq);
    }

    /// Держит историю в пределах потолка памяти, отдавая самые старые данные.
    ///
    /// Порядок: warm buckets → hot ticks → raw events. Каждый шаг обязан
    /// уменьшить байты или число удерживаемых элементов; иначе цикл немедленно
    /// завершается. Это load-bearing guard против production-инцидента, где
    /// минимальные hot/warm уже не уменьшались, но журнал событий продолжал
    /// удерживать память выше ceiling.
    fn enforce_memory_ceiling(&mut self, seq: u64) {
        let mut current = self.approx_bytes();
        self.peak_bytes = self.peak_bytes.max(current);
        if self.max_bytes == 0 || current <= self.max_bytes {
            return;
        }

        let before_all = current;
        let current_bucket = self.warm.bucket_of(seq);
        let mut depth = u64::try_from(self.warm_buckets).unwrap_or(1);
        let should_warn = !self.ceiling_warned;

        while current > self.max_bytes && depth > 0 && self.warm.approx_bytes() > 0 {
            self.eviction_iterations = self.eviction_iterations.saturating_add(1);
            let before = current;
            let oldest = if depth > 1 {
                depth /= 2;
                current_bucket.saturating_sub(depth)
            } else {
                depth = 0;
                current_bucket.saturating_add(1)
            };
            self.warm.prune(oldest);
            current = self.approx_bytes();
            if current >= before {
                break;
            }
            self.evicted_buckets = self.evicted_buckets.saturating_add(1);
        }

        while current > self.max_bytes && self.hot.capacity() > 2 {
            self.eviction_iterations = self.eviction_iterations.saturating_add(1);
            let before = current;
            let next = (self.hot.capacity() / 2).max(2);
            let removed = self.hot.shrink_capacity(next);
            current = self.approx_bytes();
            if removed == 0 || current >= before {
                break;
            }
            self.evicted_hot_ticks = self
                .evicted_hot_ticks
                .saturating_add(u64::try_from(removed).unwrap_or(u64::MAX));
        }

        if current > self.max_bytes && !self.events.is_empty() {
            self.eviction_iterations = self.eviction_iterations.saturating_add(1);
            let requested = current.saturating_sub(self.max_bytes);
            let (count, _) = self.events.evict_bytes(requested);
            self.evicted_events = self.evicted_events.saturating_add(count);
            current = self.approx_bytes();
        }

        if current > self.max_bytes {
            // Последний рубеж: мёртвые записи журнала сущностей. Живые не
            // отдаются — без них снимок не сможет назвать свои же сущности.
            self.eviction_iterations = self.eviction_iterations.saturating_add(1);
            let requested = current.saturating_sub(self.max_bytes);
            let (count, _) = self.entities.evict_dead_bytes(requested);
            self.evicted_entity_records = self.evicted_entity_records.saturating_add(count);
            current = self.approx_bytes();
        }

        if current >= before_all {
            // Ничего больше не вытесняется: минимальное живое состояние может
            // быть больше абсурдно малого бюджета, но повторять цикл нельзя.
            self.eviction_no_progress = self.eviction_no_progress.saturating_add(1);
        }

        if should_warn {
            self.ceiling_warned = true;
            tracing::warn!(
                max_bytes = self.max_bytes,
                current_bytes = current,
                evicted_buckets = self.evicted_buckets,
                evicted_hot_ticks = self.evicted_hot_ticks,
                evicted_events = self.evicted_events,
                evicted_entity_records = self.evicted_entity_records,
                no_progress = self.eviction_no_progress,
                "история достигла потолка памяти: retention сокращён"
            );
        }
    }

    /// Сколько раз потолок памяти сокращал глубину тёплого слоя.
    #[must_use]
    pub const fn evicted_buckets(&self) -> u64 {
        self.evicted_buckets
    }

    /// Сколько тактов горячего окна вытеснено потолком памяти.
    #[must_use]
    pub const fn evicted_hot_ticks(&self) -> u64 {
        self.evicted_hot_ticks
    }

    /// Число сырых событий, фактически удерживаемых сейчас.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Сколько сырых событий вытеснено memory ceiling.
    #[must_use]
    pub const fn evicted_events(&self) -> u64 {
        self.evicted_events
    }

    /// Сколько мёртвых записей журнала сущностей вытеснено memory ceiling.
    #[must_use]
    pub const fn evicted_entity_records(&self) -> u64 {
        self.evicted_entity_records
    }

    /// Число bounded eviction-шагов.
    #[must_use]
    pub const fn eviction_iterations(&self) -> u64 {
        self.eviction_iterations
    }

    /// Сколько раз минимальное живое состояние не удалось уменьшить.
    #[must_use]
    pub const fn eviction_no_progress(&self) -> u64 {
        self.eviction_no_progress
    }

    /// Максимальная оценка памяти, наблюдённая до eviction.
    #[must_use]
    pub const fn peak_bytes(&self) -> u64 {
        self.peak_bytes
    }

    /// Число emitted ceiling diagnostics: transition логируется ровно один раз.
    #[must_use]
    pub fn ceiling_warnings(&self) -> u64 {
        u64::from(self.ceiling_warned)
    }

    /// Последние значения всех серий с их моментами наблюдения.
    ///
    /// Окно свежести задаёт вызывающий (`with_freshness`), потому что оно
    /// зависит от интервала сбора, а история о нём не решает. Сама история
    /// обязана сообщить факт: когда значение измерили.
    #[must_use]
    pub fn latest(&self) -> LatestValues {
        let mut latest = LatestValues::new();
        for (key, value, at) in self.hot.latest_pairs() {
            latest.set_observed(key, value, at);
        }
        latest
    }

    /// Сырые точки серии в интервале.
    #[must_use]
    pub fn series(&self, key: SeriesKey, from: Timestamp, to: Timestamp) -> Vec<(Timestamp, f64)> {
        let points = self.hot.series_points(key, from, to);
        if !points.is_empty() {
            return points;
        }
        // Запрос за пределами горячего окна — отдаём средние по бакетам.
        self.warm
            .buckets_in(key, from, to)
            .into_iter()
            .filter_map(|(at, aggregate)| aggregate.mean().map(|mean| (at, mean)))
            .collect()
    }

    /// Скорость counter-серии в интервале, единиц в секунду.
    ///
    /// Сброс счётчика обрабатывается суммированием только положительных
    /// приращений: перезапуск процесса не должен давать отрицательную скорость
    /// и не должен маскировать реальный рост после перезапуска.
    #[must_use]
    pub fn rate(&self, key: SeriesKey, from: Timestamp, to: Timestamp) -> Option<f64> {
        if describe(key.metric).map(|d| d.kind) != Some(MetricKind::Counter) {
            return None;
        }
        let points = self.series(key, from, to);
        if points.len() < 2 {
            return None;
        }
        let mut growth = 0.0;
        let mut previous: Option<f64> = None;
        for (_, value) in &points {
            if let Some(prev) = previous {
                let delta = value - prev;
                if delta > 0.0 {
                    growth += delta;
                }
            }
            previous = Some(*value);
        }
        let first_at = points.first().map(|(t, _)| *t)?;
        let last_at = points.last().map(|(t, _)| *t)?;
        let span_ms = last_at.as_millis().saturating_sub(first_at.as_millis());
        if span_ms == 0 {
            return None;
        }
        Some(growth / (span_ms as f64 / 1000.0))
    }

    /// Сырые точки серии в горячем окне.
    ///
    /// Нужны отрисовке metric lanes: спецификация запрещает выдумывать
    /// историю, поэтому TUI обязан отличать «нет точек» от «есть точки».
    /// Тёплый слой здесь не используется: он хранит агрегаты бакетов, а не
    /// сырые значения, и подмена одного другим исказила бы форму дорожки.
    #[must_use]
    pub fn series_points(
        &self,
        key: SeriesKey,
        from: Timestamp,
        to: Timestamp,
    ) -> Vec<(Timestamp, f64)> {
        self.hot.series_points(key, from, to)
    }

    /// Статистика значений серии в окне.
    #[must_use]
    pub fn window(&self, key: SeriesKey, from: Timestamp, to: Timestamp) -> Option<WindowStats> {
        let points = self.hot.series_points(key, from, to);
        if !points.is_empty() {
            return Some(welford(points.iter().map(|(_, v)| *v)));
        }

        // Вне горячего окна сырых точек уже нет, есть только агрегаты бакетов.
        let buckets = self.warm.buckets_in(key, from, to);
        if buckets.is_empty() {
            return None;
        }
        let mut stats = welford(buckets.iter().filter_map(|(_, a)| a.mean()));
        let merged = buckets
            .iter()
            .fold(Aggregate::empty(), |acc, (_, a)| acc.merge(a));
        stats.count = usize::try_from(merged.count).unwrap_or(stats.count);
        stats.min = merged.min;
        stats.max = merged.max;
        stats.first = merged.first;
        stats.last = merged.last;
        if merged.count > 0 {
            stats.mean = merged.sum / f64::from(merged.count);
        }
        // Разброс средних по бакетам систематически ниже разброса сырых
        // значений: усреднение делит дисперсию на число наблюдений, а суммы
        // квадратов внутри бакета не хранятся и восстановлению не подлежат.
        // Врать точностью нельзя: помечаем статистику приближённой, и
        // потребитель переключается на относительное изменение.
        stats.approximate = true;
        Some(stats)
    }

    /// Значение серии на момент `at` (последнее наблюдение не позже `at`).
    ///
    /// Лимит давности — пять тактов: иначе на графике «зависшее» значение
    /// мёртвой серии выглядело бы как живое измерение.
    #[must_use]
    pub fn value_at(&self, key: SeriesKey, at: Timestamp) -> Option<f64> {
        let staleness = self.hot.tick_interval_ms().saturating_mul(5);
        if let Some((ts, value)) = self.hot.last_before(key, at) {
            if at.as_millis().saturating_sub(ts.as_millis()) <= staleness {
                return Some(value);
            }
        }
        // Тёплый бакет покрывает несколько тактов, поэтому его последнее
        // наблюдение может быть ПОЗЖЕ запрошенного момента. Отдать такое
        // значение — показать оператору будущее как настоящее.
        let bucket_span = self
            .hot
            .tick_interval_ms()
            .saturating_mul(u64::try_from(self.warm_bucket_ticks).unwrap_or(1));
        self.warm
            .buckets_in(key, at.saturating_sub_millis(bucket_span * 2), at)
            .last()
            .filter(|(_, aggregate)| aggregate.last_at <= at.as_millis())
            .map(|(_, aggregate)| aggregate.last)
    }

    /// Сущности, живые в момент `at`.
    #[must_use]
    pub fn entities_at(&self, at: Timestamp) -> Vec<&EntityRecord> {
        self.entities.at(at)
    }

    #[must_use]
    pub fn entity_record(&self, id: EntityId) -> Option<&EntityRecord> {
        self.entities.get(id)
    }

    #[must_use]
    pub fn events_between(&self, from: Timestamp, to: Timestamp) -> Vec<&Event> {
        self.events.between(from, to)
    }

    #[must_use]
    pub fn recent_events(&self, limit: usize) -> Vec<&Event> {
        self.events.recent(limit)
    }

    /// Начало горячего окна.
    #[must_use]
    pub fn oldest(&self) -> Timestamp {
        self.hot.oldest().unwrap_or(Timestamp::ZERO)
    }

    /// Время последнего такта.
    #[must_use]
    pub fn newest(&self) -> Timestamp {
        self.hot.newest().unwrap_or(Timestamp::ZERO)
    }

    /// Горизонт истории: раньше этого момента данные не хранятся.
    #[must_use]
    pub fn horizon(&self) -> Timestamp {
        let span = self
            .hot
            .tick_interval_ms()
            .saturating_mul(u64::try_from(self.warm_buckets * self.warm_bucket_ticks).unwrap_or(1));
        self.newest().saturating_sub_millis(span)
    }

    /// Ось времени горячего кольца.
    #[must_use]
    pub fn ticks(&self) -> Vec<Timestamp> {
        self.hot.ticks()
    }

    #[must_use]
    pub fn series_count(&self) -> usize {
        self.hot.series_count()
    }

    #[must_use]
    pub fn dropped_series(&self) -> u64 {
        self.hot.dropped_series()
    }

    #[must_use]
    pub fn samples_stored(&self) -> u64 {
        self.hot.samples_stored()
    }

    #[must_use]
    pub fn events_total(&self) -> u64 {
        self.events.total()
    }

    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Оценка занятой памяти. Используется самометриками и scorecard.
    #[must_use]
    pub fn approx_bytes(&self) -> u64 {
        self.hot
            .approx_bytes()
            .saturating_add(self.warm.approx_bytes())
            .saturating_add(self.entities.approx_bytes())
            .saturating_add(self.events.approx_bytes())
    }

    /// Число серий в тёплом слое — нужно тестам и диагностике памяти.
    #[must_use]
    pub fn warm_series_count(&self) -> usize {
        self.warm.series_count()
    }

    /// Есть ли серия в тёплом слое.
    #[must_use]
    pub fn warm_has(&self, key: SeriesKey) -> bool {
        !self
            .warm
            .buckets_in(key, Timestamp::ZERO, Timestamp::MAX)
            .is_empty()
    }
}

/// Одно проходное вычисление среднего и дисперсии по алгоритму Уэлфорда.
///
/// Наивная сумма квадратов теряет точность на рядах с большим смещением
/// (например, счётчик байт около 1e9 с колебаниями в единицы), а именно это
/// значение попадает в z-score алгоритма diff.
fn welford(values: impl Iterator<Item = f64>) -> WindowStats {
    let mut count = 0usize;
    let mut mean = 0.0f64;
    let mut m2 = 0.0f64;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut first = 0.0f64;
    let mut last = 0.0f64;

    for value in values {
        count += 1;
        if count == 1 {
            first = value;
        }
        last = value;
        min = min.min(value);
        max = max.max(value);
        let delta = value - mean;
        mean += delta / count as f64;
        m2 += delta * (value - mean);
    }

    let stddev = if count > 1 {
        (m2 / (count as f64 - 1.0)).max(0.0).sqrt()
    } else {
        0.0
    };

    WindowStats {
        count,
        min: if count == 0 { 0.0 } else { min },
        max: if count == 0 { 0.0 } else { max },
        mean,
        stddev,
        first,
        last,
        approximate: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::{EntityKey, EntitySpec};
    use pulse_core::metric::ids;
    use pulse_core::EntityGraph;

    fn config() -> StoreConfig {
        StoreConfig {
            hot_ticks: 60,
            warm_buckets: 12,
            warm_bucket_ticks: 10,
            max_events: 64,
            max_entity_records: 256,
            max_series: 1_000,
            // Тесты проверяют логику ретеншна, а не потолок памяти.
            max_bytes: 0,
        }
    }

    /// Синтетический источник тактов: граф + счётчик времени.
    struct Feeder {
        graph: EntityGraph,
        now: u64,
    }

    impl Feeder {
        fn new() -> Self {
            Feeder {
                graph: EntityGraph::new("boot", "host", Timestamp::from_millis(1_000)),
                now: 1_000,
            }
        }

        fn tick(&mut self, mut body: impl FnMut(&mut EntityGraph)) -> TickBatch {
            self.now += 1_000;
            self.graph.begin_tick(Timestamp::from_millis(self.now));
            body(&mut self.graph);
            self.graph.end_tick()
        }
    }

    fn host_key(graph: &EntityGraph) -> SeriesKey {
        SeriesKey::new(graph.host(), ids::HOST_CPU_UTIL)
    }

    #[test]
    fn latest_returns_last_observed_value() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        for i in 1..=3 {
            let batch = feeder.tick(|g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, f64::from(i) / 10.0);
            });
            history.ingest(&batch);
        }
        let key = host_key(&feeder.graph);
        let latest = history.latest();
        assert_eq!(latest.get(key.entity, key.metric), Some(0.3));
    }

    #[test]
    fn hot_window_is_bounded() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        for _ in 0..1_000 {
            let batch = feeder.tick(|g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, 0.5);
            });
            history.ingest(&batch);
        }
        assert_eq!(history.ticks().len(), 60);
    }

    #[test]
    fn rate_of_monotonic_counter_is_exact() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        // Счётчик растёт на 1000 единиц за такт (1 с).
        for i in 1..=10u64 {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_SECONDS, (i * 1_000) as f64);
            });
            history.ingest(&batch);
        }
        let key = SeriesKey::new(feeder.graph.host(), ids::HOST_CPU_SECONDS);
        let rate = history
            .rate(key, history.oldest(), history.newest())
            .unwrap_or_default();
        assert!((rate - 1_000.0).abs() < 1e-6, "скорость {rate}");
    }

    #[test]
    fn rate_survives_counter_reset() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        let values = [1_000.0, 2_000.0, 3_000.0, 10.0, 1_010.0];
        for value in values {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_SECONDS, value);
            });
            history.ingest(&batch);
        }
        let key = SeriesKey::new(feeder.graph.host(), ids::HOST_CPU_SECONDS);
        let rate = history
            .rate(key, history.oldest(), history.newest())
            .unwrap_or_default();
        assert!(rate > 0.0, "скорость должна быть положительной: {rate}");
        assert!(rate < 10_000.0, "скорость не должна взрываться: {rate}");
    }

    #[test]
    fn rate_is_none_for_gauge() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        for _ in 0..3 {
            let batch = feeder.tick(|g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, 0.5);
            });
            history.ingest(&batch);
        }
        let key = host_key(&feeder.graph);
        assert!(history
            .rate(key, history.oldest(), history.newest())
            .is_none());
    }

    #[test]
    fn window_stddev_matches_reference() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        let values = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        for value in values {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, value);
            });
            history.ingest(&batch);
        }
        let stats = history
            .window(host_key(&feeder.graph), history.oldest(), history.newest())
            .expect("окно должно существовать");
        // Выборочное стандартное отклонение этого ряда: sqrt(32/7).
        let expected = (32.0f64 / 7.0).sqrt();
        assert_eq!(stats.count, 8);
        assert!((stats.mean - 5.0).abs() < 1e-12);
        assert!(
            (stats.stddev - expected).abs() < 1e-9,
            "stddev {} != {}",
            stats.stddev,
            expected
        );
    }

    #[test]
    fn window_stddev_is_stable_with_large_offset() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        // Ряд со смещением 1e9: наивная сумма квадратов здесь теряет точность.
        let values = [
            1e9 + 2.0,
            1e9 + 4.0,
            1e9 + 4.0,
            1e9 + 4.0,
            1e9 + 5.0,
            1e9 + 5.0,
            1e9 + 7.0,
            1e9 + 9.0,
        ];
        for value in values {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, value);
            });
            history.ingest(&batch);
        }
        let stats = history
            .window(host_key(&feeder.graph), history.oldest(), history.newest())
            .expect("окно должно существовать");
        let expected = (32.0f64 / 7.0).sqrt();
        assert!(
            (stats.stddev - expected).abs() < 1e-6,
            "stddev {} != {}",
            stats.stddev,
            expected
        );
    }

    #[test]
    fn entities_at_reflects_lifecycle() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();

        let first = feeder.tick(|g| {
            let host = g.host();
            let _ = g.upsert(
                EntitySpec::new(
                    EntityKey::Process {
                        pid: 100,
                        start_ticks: 1,
                    },
                    "worker",
                )
                .parent(host),
            );
        });
        let a = first.at;
        history.ingest(&first);

        // Процесс исчезает: два такта без наблюдения (учитывая фору графа).
        for _ in 0..3 {
            let batch = feeder.tick(|_| {});
            history.ingest(&batch);
        }
        let b = history.newest();

        let at_a = history.entities_at(a);
        let at_b = history.entities_at(b);
        assert!(at_a.iter().any(|r| r.name == "worker"), "в A процесс жив");
        assert!(
            !at_b.iter().any(|r| r.name == "worker"),
            "в B процесса быть не должно"
        );
    }

    #[test]
    fn process_series_are_absent_from_warm_tier() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        let mut process_key: Option<SeriesKey> = None;

        for _ in 0..5 {
            let batch = feeder.tick(|g| {
                let host = g.host();
                let pid = g.upsert(
                    EntitySpec::new(
                        EntityKey::Process {
                            pid: 200,
                            start_ticks: 7,
                        },
                        "svc",
                    )
                    .parent(host),
                );
                g.sample(pid, ids::PROC_CPU_CORES, 1.0);
                g.sample(host, ids::HOST_CPU_UTIL, 0.4);
            });
            if process_key.is_none() {
                if let Some(sample) = batch
                    .samples
                    .iter()
                    .find(|s| s.metric() == ids::PROC_CPU_CORES)
                {
                    process_key = Some(sample.series);
                }
            }
            history.ingest(&batch);
        }

        let host_series = SeriesKey::new(feeder.graph.host(), ids::HOST_CPU_UTIL);
        assert!(
            history.warm_has(host_series),
            "владелец обязан быть в тёплом слое"
        );
        let process_series = process_key.expect("серия процесса должна существовать");
        assert!(
            !history.warm_has(process_series),
            "серии процессов не должны попадать в тёплый слой"
        );
    }

    #[test]
    fn series_limit_is_enforced() {
        let mut cfg = config();
        cfg.max_series = 3;
        let mut history = History::new(&cfg);
        let mut feeder = Feeder::new();
        let batch = feeder.tick(|g| {
            let host = g.host();
            for (index, metric) in [
                ids::HOST_CPU_UTIL,
                ids::HOST_MEM_UTIL,
                ids::HOST_LOAD1,
                ids::HOST_LOAD5,
                ids::HOST_LOAD15,
            ]
            .into_iter()
            .enumerate()
            {
                g.sample(host, metric, index as f64);
            }
        });
        history.ingest(&batch);
        assert_eq!(history.series_count(), 3);
        assert_eq!(history.dropped_series(), 2);
    }

    #[test]
    fn events_between_includes_boundaries() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        let batch = feeder.tick(|g| {
            let host = g.host();
            let _ = g.upsert(
                EntitySpec::new(EntityKey::Cgroup { cgroup_id: 5 }, "svc.slice").parent(host),
            );
        });
        let at = batch.at;
        history.ingest(&batch);
        let found = history.events_between(at, at);
        assert!(
            !found.is_empty(),
            "событие создания должно попасть в интервал"
        );
    }

    #[test]
    fn value_at_carries_last_observation_but_not_forever() {
        let mut history = History::new(&config());
        let mut feeder = Feeder::new();
        for _ in 0..3 {
            let batch = feeder.tick(|g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, 0.7);
            });
            history.ingest(&batch);
        }
        let key = host_key(&feeder.graph);
        let last_tick = history.newest();
        assert_eq!(history.value_at(key, last_tick), Some(0.7));
        assert_eq!(
            history.value_at(key, last_tick.saturating_add_millis(2_000)),
            Some(0.7),
            "в пределах давности значение переносится"
        );
        assert!(
            history
                .value_at(key, last_tick.saturating_add_millis(600_000))
                .is_none(),
            "спустя минуты значение считать живым нельзя"
        );
    }

    #[test]
    fn approx_bytes_grows_with_data() {
        let mut history = History::new(&config());
        let empty = history.approx_bytes();
        let mut feeder = Feeder::new();
        for _ in 0..10 {
            let batch = feeder.tick(|g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, 0.5);
                g.sample(host, ids::HOST_MEM_UTIL, 0.6);
            });
            history.ingest(&batch);
        }
        assert!(history.approx_bytes() > empty);
    }

    /// Конфигурация, в которой тёплый слой реально обслуживает чтение.
    fn warm_config() -> StoreConfig {
        StoreConfig {
            hot_ticks: 10,
            warm_buckets: 40,
            warm_bucket_ticks: 4,
            max_events: 64,
            max_entity_records: 256,
            max_series: 1_000,
            // Тесты проверяют логику ретеншна, а не потолок памяти.
            max_bytes: 0,
        }
    }

    /// Детерминированный шум с постоянным средним: пила 0.4/0.6.
    fn saw(index: usize) -> f64 {
        if index % 2 == 0 {
            0.4
        } else {
            0.6
        }
    }

    #[test]
    fn warm_window_is_marked_approximate_and_hot_window_is_not() {
        let mut history = History::new(&warm_config());
        let mut feeder = Feeder::new();
        let mut first_at = None;
        for index in 0..40 {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, saw(index));
            });
            if first_at.is_none() {
                first_at = Some(batch.at);
            }
            history.ingest(&batch);
        }
        let key = host_key(&feeder.graph);
        let from = first_at.expect("первый такт");
        // Окно целиком за горячими 10 тактами: обслуживает тёплый слой.
        let to = from.saturating_add_millis(9_000);
        assert!(
            history.hot.series_points(key, from, to).is_empty(),
            "окно должно выйти за горячее кольцо"
        );
        let warm = history.window(key, from, to).expect("тёплая статистика");
        assert!(
            warm.approximate,
            "статистика из агрегатов обязана быть помечена приближённой"
        );
        // Пила 0.4/0.6 с бакетом из 4 тактов даёт постоянные средние бакетов,
        // поэтому stddev по ним близок к нулю: сырой разброс невосстановим.
        assert!(
            warm.stddev < 0.02,
            "оценка не должна притворяться сырой точностью: {}",
            warm.stddev
        );

        let hot = history
            .window(
                key,
                history.newest().saturating_sub_millis(5_000),
                history.newest(),
            )
            .expect("горячая статистика");
        assert!(!hot.approximate, "сырые точки — точная статистика");
        assert!(hot.stddev > 0.05, "сырой разброс пилы: {}", hot.stddev);
    }
    #[test]
    fn memory_ceiling_trades_history_depth_for_a_hard_limit() {
        // Один и тот же вход прогоняется дважды: с потолком и без. Абсолютные
        // байты зависят от накладных расходов структур, поэтому проверяется
        // отношение, а не подогнанная константа.
        let feed = |cfg: &StoreConfig| -> (u64, u64, u64, bool) {
            let mut history = History::new(cfg);
            let mut feeder = Feeder::new();
            for index in 0..200 {
                let batch = feeder.tick(move |g| {
                    let host = g.host();
                    g.sample(host, ids::HOST_CPU_UTIL, saw(index));
                    g.sample(host, ids::HOST_MEM_UTIL, saw(index + 1));
                });
                history.ingest(&batch);
            }
            let key = host_key(&feeder.graph);
            let fresh = history.latest().get(key.entity, key.metric).is_some();
            (
                history.approx_bytes(),
                history.evicted_buckets(),
                history.evicted_hot_ticks(),
                fresh,
            )
        };

        let mut unlimited = warm_config();
        unlimited.hot_ticks = 10;
        unlimited.max_bytes = 0;
        let (bytes_unlimited, evicted_unlimited, hot_unlimited, _) = feed(&unlimited);

        let mut capped = unlimited.clone();
        capped.max_bytes = 1_024;
        let (bytes_capped, evicted_capped, hot_capped, fresh_capped) = feed(&capped);

        assert_eq!(
            evicted_unlimited, 0,
            "без потолка вытеснения быть не должно"
        );
        assert_eq!(hot_unlimited, 0, "без потолка hot не сокращается");
        assert!(
            evicted_capped > 0 || hot_capped > 0,
            "потолок обязан быть наблюдаемым через счётчики вытеснений"
        );
        assert!(
            hot_capped > 0,
            "если warm уже сокращён, потолок обязан сократить и hot"
        );
        assert!(
            bytes_capped < bytes_unlimited,
            "с потолком история должна занимать меньше: {bytes_capped} против {bytes_unlimited}"
        );
        assert!(
            fresh_capped,
            "потолок режет глубину истории, а не текущие значения"
        );
    }

    #[test]
    fn sustained_ceiling_churn_makes_progress_and_stays_bounded() {
        use pulse_core::event::EventKind;
        use pulse_core::metric::MetricId;
        use pulse_core::sample::Sample;
        use pulse_core::time::TickId;
        use std::time::{Duration, Instant};

        let mut cfg = config();
        cfg.hot_ticks = 128;
        cfg.warm_buckets = 64;
        cfg.max_events = 8_192;
        cfg.max_series = 4_096;
        cfg.max_bytes = 1024 * 1024;
        let mut history = History::new(&cfg);
        let started = Instant::now();
        let mut peak = 0_u64;

        // 100 тактов × (2k samples + 1k raw churn events) = 300k входных
        // элементов. Это воспроизводит production-класс: ceiling достигнут,
        // но ingest обязан продолжать делать progress, а не крутить eviction.
        for tick in 0..100_u64 {
            let at = Timestamp::from_millis(1_000 + tick * 1_000);
            let samples = (0..2_000_u32)
                .map(|index| Sample::new(EntityId::new(index, 1), MetricId(190), tick as f64))
                .collect();
            let events = (0..1_000_u32)
                .map(|index| {
                    Event::new(
                        at,
                        EventKind::MetadataChanged,
                        format!("kworker/{index}:0-events"),
                    )
                    .detail("routine metadata churn")
                })
                .collect();
            history.ingest(&TickBatch {
                tick: TickId(tick + 1),
                at,
                samples,
                events,
                records: Vec::new(),
                alive: Vec::new(),
            });
            peak = peak.max(history.approx_bytes());
        }

        let elapsed = started.elapsed();
        eprintln!(
            "ceiling reproduction: elapsed={elapsed:?} bytes={} peak_before_eviction={} \
             hot_evicted={} events_total={} events_retained={} events_evicted={} \
             eviction_iterations={} no_progress={}",
            history.approx_bytes(),
            history.peak_bytes(),
            history.evicted_hot_ticks(),
            history.events_total(),
            history.event_count(),
            history.evicted_events(),
            history.eviction_iterations(),
            history.eviction_no_progress(),
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "sustained ceiling workload перестал делать progress: {elapsed:?}"
        );
        assert!(history.evicted_hot_ticks() > 0, "ceiling обязан сработать");
        assert!(
            history.approx_bytes() <= cfg.max_bytes,
            "hard bound нарушен: {} > {} (events={})",
            history.approx_bytes(),
            cfg.max_bytes,
            history.events_total(),
        );
        assert!(
            history.evicted_events() > 0,
            "event journal обязан участвовать в hard bound"
        );
        assert!(
            history.event_count() < cfg.max_events,
            "численный event cap недостаточен для memory ceiling"
        );
        assert_eq!(
            history.eviction_no_progress(),
            0,
            "реалистичный 1 MiB budget обязан допускать progress"
        );
        assert!(
            history.eviction_iterations() <= 210,
            "не более двух bounded eviction шагов на steady-state tick: {}",
            history.eviction_iterations()
        );
    }

    #[test]
    fn ceiling_with_only_hot_data_is_bounded() {
        use pulse_core::metric::MetricId;
        use pulse_core::sample::Sample;
        use pulse_core::time::TickId;

        let mut cfg = config();
        cfg.hot_ticks = 256;
        cfg.max_series = 2_000;
        cfg.max_events = 16;
        cfg.max_bytes = 1024 * 1024;
        let mut history = History::new(&cfg);
        for tick in 0..100_u64 {
            let at = Timestamp::from_millis(1_000 + tick * 1_000);
            let samples = (0..1_000_u32)
                .map(|index| Sample::new(EntityId::new(index, 1), MetricId(190), tick as f64))
                .collect();
            history.ingest(&TickBatch {
                tick: TickId(tick + 1),
                at,
                samples,
                events: Vec::new(),
                records: Vec::new(),
                alive: Vec::new(),
            });
        }
        assert!(history.evicted_hot_ticks() > 0);
        assert_eq!(history.evicted_events(), 0);
        assert!(history.approx_bytes() <= cfg.max_bytes);
        assert!(
            history.eviction_no_progress() <= 2,
            "no-progress exits must stay bounded: {}",
            history.eviction_no_progress()
        );
    }

    #[test]
    fn impossible_ceiling_exits_without_retry_loop() {
        use pulse_core::metric::MetricId;
        use pulse_core::sample::Sample;
        use pulse_core::time::TickId;

        let mut cfg = config();
        cfg.hot_ticks = 2;
        cfg.max_series = 4;
        cfg.max_events = 16;
        cfg.max_bytes = 1; // меньше минимального живого состояния
        let mut history = History::new(&cfg);
        for tick in 0..100_u64 {
            history.ingest(&TickBatch {
                tick: TickId(tick + 1),
                at: Timestamp::from_millis(1_000 + tick * 1_000),
                samples: vec![Sample::new(EntityId::new(1, 1), MetricId(190), tick as f64)],
                events: Vec::new(),
                records: Vec::new(),
                alive: Vec::new(),
            });
        }
        assert!(
            history.approx_bytes() > cfg.max_bytes,
            "невозможный budget честно превышен"
        );
        assert!(
            history.eviction_iterations() <= 300,
            "не более трёх bounded шагов на такт: {}",
            history.eviction_iterations()
        );
        assert!(
            (99..=100).contains(&history.eviction_no_progress()),
            "после достижения ceiling — один bounded exit на такт: {}",
            history.eviction_no_progress()
        );
        assert_eq!(history.ceiling_warnings(), 1, "warning только на переходе");
    }

    /// Журнал сущностей — последний рубеж потолка. Без него «жёсткий предел»
    /// был бы ложью: 32k мёртвых записей держат мегабайты, которых не касались
    /// ни warm, ни hot, ни события.
    #[test]
    fn ceiling_evicts_dead_entity_records_last() {
        use pulse_core::entity::{EntityKey, EntityRecord, Labels};
        use pulse_core::time::TickId;

        let mut cfg = config();
        cfg.hot_ticks = 4;
        cfg.max_series = 8;
        cfg.max_events = 16;
        cfg.max_entity_records = 20_000;
        cfg.max_bytes = 256 * 1024;
        let mut history = History::new(&cfg);

        for tick in 0..40_u64 {
            let at = Timestamp::from_millis(1_000 + tick * 1_000);
            // Каждый такт умирает пятьсот процессов: типичный контейнерный хост.
            let records = (0..500_u32)
                .map(|index| EntityRecord {
                    id: EntityId::new(u32::try_from(tick).unwrap_or(0) * 500 + index, 1),
                    key: EntityKey::Process {
                        pid: i32::try_from(index).unwrap_or(0),
                        start_ticks: tick,
                    },
                    kind: EntityKind::Process,
                    name: format!("short-lived-{tick}-{index}"),
                    parent: None,
                    labels: Labels::default(),
                    first_seen: at,
                    last_seen: at,
                    alive: false,
                })
                .collect();
            history.ingest(&TickBatch {
                tick: TickId(tick + 1),
                at,
                samples: Vec::new(),
                events: Vec::new(),
                records,
                alive: Vec::new(),
            });
        }

        assert!(
            history.evicted_entity_records() > 0,
            "мёртвые записи обязаны участвовать в потолке"
        );
        assert!(
            history.approx_bytes() <= cfg.max_bytes,
            "жёсткий предел обязан соблюдаться: {} > {}",
            history.approx_bytes(),
            cfg.max_bytes
        );
        assert_eq!(
            history.eviction_no_progress(),
            0,
            "при наличии мёртвых записей прогресс обязан быть"
        );
    }

    #[test]
    fn zero_ceiling_disables_the_check() {
        let mut cfg = warm_config();
        cfg.max_bytes = 0;
        let mut history = History::new(&cfg);
        let mut feeder = Feeder::new();

        for index in 0..60 {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, saw(index));
            });
            history.ingest(&batch);
        }
        assert_eq!(history.evicted_buckets(), 0);
        assert_eq!(history.evicted_hot_ticks(), 0);
    }
    #[test]
    fn ceiling_empties_warm_before_shrinking_hot() {
        let mut cfg = warm_config();
        cfg.hot_ticks = 10;
        cfg.max_bytes = 0;
        let mut history = History::new(&cfg);
        let mut feeder = Feeder::new();
        for index in 0..100 {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, saw(index));
                g.sample(host, ids::HOST_MEM_UTIL, saw(index + 1));
            });
            history.ingest(&batch);
        }
        assert!(history.warm.approx_bytes() > 0, "warm обязан наполниться");

        let hot_capacity = history.hot.capacity();
        let baseline = history
            .hot
            .approx_bytes()
            .saturating_add(history.entities.approx_bytes())
            .saturating_add(history.events.approx_bytes());
        history.max_bytes = baseline;
        history.enforce_memory_ceiling(history.current_seq);

        assert_eq!(
            history.warm.approx_bytes(),
            0,
            "при бюджете только на базис warm обязан освободиться полностью"
        );
        assert_eq!(
            history.hot.capacity(),
            hot_capacity,
            "если базис помещается, hot сокращать нельзя"
        );
        assert_eq!(history.evicted_hot_ticks(), 0);
        assert!(history.evicted_buckets() > 0);
        assert!(history.approx_bytes() <= baseline);
    }

    #[test]
    fn warm_value_at_never_returns_observation_from_the_future() {
        let mut history = History::new(&warm_config());
        let mut feeder = Feeder::new();
        let mut times = Vec::new();
        for index in 0..40 {
            let batch = feeder.tick(move |g| {
                let host = g.host();
                g.sample(host, ids::HOST_CPU_UTIL, saw(index));
            });
            times.push(batch.at);
            history.ingest(&batch);
        }
        let key = host_key(&feeder.graph);
        // Момент внутри бакета, но не на его конце: значения следующих тактов
        // того же бакета относятся к будущему и не могут быть ответом.
        let inside = times.get(4).copied().expect("пятый такт");
        assert_eq!(
            history.value_at(key, inside),
            None,
            "агрегат бакета содержит наблюдения позже запрошенного момента"
        );
        // Конец бакета: наблюдение не в будущем, ответ допустим.
        let boundary = times.get(7).copied().expect("восьмой такт");
        assert_eq!(history.value_at(key, boundary), Some(saw(7)));
    }

    #[test]
    fn welford_handles_empty_and_single_value() {
        let empty = welford(std::iter::empty());
        assert_eq!(empty.count, 0);
        assert_eq!(empty.stddev, 0.0);
        let single = welford(std::iter::once(5.0));
        assert_eq!(single.count, 1);
        assert_eq!(single.stddev, 0.0);
        assert!((single.mean - 5.0).abs() < 1e-12);
    }
}
