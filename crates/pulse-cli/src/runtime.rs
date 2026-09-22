//! Конвейер агента: collect → graph → history → analyzer → snapshot.
//!
//! Единственный поток изменяет граф и состояние правил. Потребители получают
//! неизменяемый `Arc<Snapshot>` через `ArcSwap`, поэтому медленный HTTP-клиент
//! или перерисовка TUI никогда не блокируют `/proc`-сбор.

use std::cmp::Ordering;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use pulse_collect::{boot_id, build_collectors, hostname, FsSource};
use pulse_core::metric::ids;
use pulse_core::snapshot::{AgentStats, Snapshot, SnapshotSource};
use pulse_core::time::Timestamp;
use pulse_core::{CollectCtx, Config, EntityGraph};
use pulse_engine::Analyzer;
use pulse_store::History;

/// Число последних тактов для приближённого p95 длительности.
const LATENCY_WINDOW: usize = 120;

/// Запущенный локальный агент.
pub struct AgentRuntime {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    source: SnapshotSource,
    history: Arc<RwLock<History>>,
}

impl std::fmt::Debug for AgentRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRuntime")
            .field("running", &self.worker.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentRuntime {
    /// Запускает поток сбора на заданном источнике файловой системы.
    ///
    /// Публичный, потому что детали процесса для пайпа обязаны читаться тем
    /// же источником, что и коллекторы: иначе в демо-режиме кадр показывал
    /// бы сценарий, а пайп — реальный хост.
    pub fn start_with_fs(config: &Config, fs: Arc<dyn FsSource>) -> io::Result<Self> {
        config
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

        let boot = boot_id(fs.as_ref(), &config.general.proc_root);
        let host = hostname(fs.as_ref(), &config.general.proc_root);
        let graph = EntityGraph::new(&boot, &host, Timestamp::now());
        let initial = Arc::new(Snapshot::build(
            &graph,
            Default::default(),
            Vec::new(),
            Vec::new(),
            AgentStats::default(),
            &host,
            &boot,
        ));
        let published = Arc::new(ArcSwap::from(initial));
        let published_for_source = Arc::clone(&published);
        let source: SnapshotSource = Arc::new(move || published_for_source.load_full());
        let history = Arc::new(RwLock::new(History::new(&config.store)));
        let stop = Arc::new(AtomicBool::new(false));

        let worker_config = config.clone();
        let worker_history = Arc::clone(&history);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("pulse-collect".to_string())
            .spawn(move || {
                collect_loop(
                    worker_config,
                    fs,
                    graph,
                    host,
                    boot,
                    worker_history,
                    published,
                    worker_stop,
                );
            })?;

        Ok(AgentRuntime {
            stop,
            worker: Some(worker),
            source,
            history,
        })
    }

    /// Источник текущего неизменяемого снимка.
    #[must_use]
    pub fn source(&self) -> SnapshotSource {
        Arc::clone(&self.source)
    }

    /// Общая история для timeline и A/B diff.
    #[must_use]
    pub fn history(&self) -> Arc<RwLock<History>> {
        Arc::clone(&self.history)
    }

    /// Ждёт указанного номера такта. Используется одноразовыми командами,
    /// которым нужен не пустой стартовый снимок.
    pub fn wait_for_tick(&self, minimum: u64, timeout: Duration) -> io::Result<Arc<Snapshot>> {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = (self.source)();
            if snapshot.tick.0 >= minimum {
                return Ok(snapshot);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("за {timeout:?} агент не собрал такт #{minimum}"),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Останавливает поток немедленно, не дожидаясь конца интервала.
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.stop.store(true, AtomicOrdering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if worker.join().is_err() {
                tracing::error!("поток сбора завершился паникой");
            }
        }
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// Диагностика хранилища за такт.
///
/// Именованные поля вместо кортежа: счётчиков memory manager стало десять, и
/// позиционный доступ к ним уже приводил к перепутанным значениям.
#[derive(Copy, Clone, Debug)]
struct StoreStats {
    series: usize,
    samples: u64,
    bytes: u64,
    events: u64,
    evicted_buckets: u64,
    evicted_hot_ticks: u64,
    evicted_events: u64,
    eviction_no_progress: u64,
    eviction_iterations: u64,
    peak_bytes: u64,
}

/// Профиль фаз такта, включаемый `PULSE_PROFILE=1`.
///
/// Единственная метрика такта — суммарная длительность, поэтому «куда ушли
/// 391 мс на большом узле» по коду можно только предполагать. Предположение
/// вместо замера уже давало ложные выводы, а здесь речь о выборе, что
/// оптимизировать. Шов выключен по умолчанию и ничего не стоит: при
/// отключённом профиле `note` не пишет ничего.
#[derive(Debug)]
struct TickProfile {
    phases: Option<Vec<(&'static str, f64)>>,
}

impl TickProfile {
    fn start() -> Self {
        let enabled = std::env::var_os("PULSE_PROFILE").is_some_and(|value| value != "0");
        TickProfile {
            phases: enabled.then(Vec::new),
        }
    }

    fn note(&mut self, name: &'static str, since: Instant) {
        if let Some(phases) = self.phases.as_mut() {
            phases.push((name, since.elapsed().as_secs_f64() * 1_000.0));
        }
    }

    /// Печатает в stderr: профиль нужен на живом сервере, где TUI не запущен,
    /// а `tracing` по умолчанию молчит.
    fn report(&self, total_ms: f64, series: usize) {
        let Some(phases) = self.phases.as_ref() else {
            return;
        };
        let mut line = format!("tick_profile total={total_ms:.1}ms series={series}");
        for (name, ms) in phases {
            line.push_str(&format!(" {name}={ms:.1}"));
        }
        eprintln!("{line}");
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_loop(
    config: Config,
    fs: Arc<dyn FsSource>,
    mut graph: EntityGraph,
    hostname: String,
    boot_id: String,
    history: Arc<RwLock<History>>,
    published: Arc<ArcSwap<Snapshot>>,
    stop: Arc<AtomicBool>,
) {
    let interval = Duration::from_millis(config.general.interval_ms);
    let mut collectors = build_collectors(&config, fs);
    let mut analyzer = Analyzer::new(config.rules.clone());
    let mut ticks_total = 0_u64;
    let mut ticks_skipped = 0_u64;
    let mut collector_errors = 0_u64;
    let mut collector_truncations = 0_u64;
    let mut latency = LatencyWindow::default();

    while !stop.load(AtomicOrdering::Acquire) {
        let started = Instant::now();
        let now = Timestamp::now();
        graph.begin_tick(now);

        let mut profile = TickProfile::start();
        for collector in &mut collectors {
            let name = collector.name();
            let result;
            let noted;
            let phase = Instant::now();
            {
                let mut ctx = CollectCtx::new(&mut graph, config.interval_secs());
                result = collector.collect(&mut ctx);
                noted = u64::from(ctx.errors());
            }
            profile.note(name, phase);
            collector_errors = collector_errors.saturating_add(noted);
            if let Err(error) = result {
                // Усечение по бюджету — неполнота, а не отказ подсистемы.
                // Событие в timeline остаётся в обоих случаях: оператор обязан
                // видеть, что часть хоста не наблюдалась.
                if matches!(error, pulse_core::CollectError::Truncated { .. }) {
                    collector_truncations = collector_truncations.saturating_add(1);
                } else {
                    collector_errors = collector_errors.saturating_add(1);
                }
                graph.push_collector_error(name, &error.to_string());
                tracing::debug!(collector = name, error = %error, "неполный или отказавший сбор");
            }
            if stop.load(AtomicOrdering::Acquire) {
                return;
            }
        }

        if stop.load(AtomicOrdering::Acquire) {
            return;
        }

        let phase = Instant::now();
        let mut batch = graph.end_tick();
        // Окно свежести — два интервала: одно наблюдение на такт плюс запас
        // на дрожание расписания. Наблюдение старше этого окна перестаёт
        // считаться текущим, и правило видит «не измерено», а не последнее
        // удачное значение исчезнувшего источника.
        let stale_after_ms = interval.as_millis().saturating_mul(2);
        let stale_after_ms = u64::try_from(stale_after_ms).unwrap_or(u64::MAX);
        let mut latest =
            with_history_read(&history, History::latest).with_freshness(now, stale_after_ms);
        for sample in &batch.samples {
            latest.set(sample.series, sample.value);
        }
        latest.retain_entities(&|id| graph.get(id).is_some());
        profile.note("end_tick+latest", phase);

        let phase = Instant::now();
        let problems = with_history_read(&history, |stored| {
            analyzer.evaluate(&graph, &latest, stored, now)
        });
        batch.events.extend(analyzer.take_events());
        profile.note("analyze", phase);

        ticks_total = ticks_total.saturating_add(1);

        // Шов замедления фазы хранения существует только в тестовой сборке:
        // доказать, что метрика длительности покрывает запись в историю,
        // можно лишь задержкой внутри этой фазы.
        #[cfg(test)]
        delay_after_analysis();

        let phase = Instant::now();
        let (events, store_stats) = with_history_write(&history, |stored| {
            stored.ingest(&batch);
            let events = stored.recent_events(200).into_iter().cloned().collect();
            let stats = StoreStats {
                series: stored.series_count(),
                samples: stored.samples_stored(),
                bytes: stored.approx_bytes(),
                events: stored.events_total(),
                evicted_buckets: stored.evicted_buckets(),
                evicted_hot_ticks: stored.evicted_hot_ticks(),
                evicted_events: stored.evicted_events(),
                eviction_no_progress: stored.eviction_no_progress(),
                eviction_iterations: stored.eviction_iterations(),
                peak_bytes: stored.peak_bytes(),
            };
            (events, stats)
        });
        profile.note("ingest", phase);

        if stop.load(AtomicOrdering::Acquire) {
            return;
        }

        let agent = AgentStats {
            // Длительность такта дописывается ниже: до построения снимка она
            // ещё неизвестна, а заполнять её неполным значением - ровно тот
            // дефект, из-за которого self-observability занижала стоимость
            // агента (PULSE-078).
            tick_duration_ms: 0.0,
            tick_duration_p95_ms: 0.0,
            ticks_total,
            ticks_skipped,
            collector_errors,
            collector_truncations,
            series_live: store_stats.series,
            samples_stored: store_stats.samples,
            store_bytes: store_stats.bytes,
            events_total: store_stats.events,
            history_evicted_buckets: store_stats.evicted_buckets,
            history_evicted_hot_ticks: store_stats.evicted_hot_ticks,
            history_evicted_events: store_stats.evicted_events,
            history_eviction_no_progress: store_stats.eviction_no_progress,
            history_eviction_iterations: store_stats.eviction_iterations,
            history_peak_bytes: store_stats.peak_bytes,
            rss_bytes: latest
                .get(graph.host(), ids::AGENT_RSS)
                .unwrap_or_default()
                .max(0.0) as u64,
            cpu_seconds: latest
                .get(graph.host(), ids::AGENT_CPU_SECONDS)
                .unwrap_or_default(),
            ..AgentStats::default()
        };
        let phase = Instant::now();
        let mut snapshot =
            Snapshot::build(&graph, latest, problems, events, agent, &hostname, &boot_id);
        profile.note("snapshot", phase);

        // Такт закончен: измерена вся работа - сбор, анализ, запись в историю
        // и построение снимка. Раньше замер останавливался перед записью, и
        // самая дорогая фаза (вытеснение истории) в стоимость агента не
        // попадала, а `ticks_skipped` не видел перерасхода интервала.
        let full = started.elapsed();
        let full_ms = full.as_secs_f64() * 1_000.0;
        latency.push(full_ms);
        if full > interval {
            ticks_skipped = ticks_skipped.saturating_add(1);
        }
        snapshot.agent.tick_duration_ms = full_ms;
        snapshot.agent.tick_duration_p95_ms = latency.p95();
        snapshot.agent.ticks_skipped = ticks_skipped;
        published.store(Arc::new(snapshot));
        profile.report(full_ms, store_stats.series);

        let remaining = interval.saturating_sub(started.elapsed());
        if !remaining.is_zero() && !stop.load(AtomicOrdering::Acquire) {
            thread::park_timeout(remaining);
            if stop.load(AtomicOrdering::Acquire) {
                break;
            }
        }
    }
}

/// После poisoning продолжаем с сохранёнными данными: потеря TUI-потока не
/// должна уничтожать локальную историю агента.
fn with_history_read<T>(history: &RwLock<History>, f: impl FnOnce(&History) -> T) -> T {
    match history.read() {
        Ok(guard) => f(&guard),
        Err(poisoned) => f(&poisoned.into_inner()),
    }
}

fn with_history_write<T>(history: &RwLock<History>, f: impl FnOnce(&mut History) -> T) -> T {
    match history.write() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => f(&mut poisoned.into_inner()),
    }
}

/// Кольцо последних длительностей без heap-аллокации на каждом такте.
#[derive(Debug)]
struct LatencyWindow {
    values: [f64; LATENCY_WINDOW],
    len: usize,
    cursor: usize,
}

impl Default for LatencyWindow {
    fn default() -> Self {
        LatencyWindow {
            values: [0.0; LATENCY_WINDOW],
            len: 0,
            cursor: 0,
        }
    }
}

impl LatencyWindow {
    fn push(&mut self, value: f64) {
        if let Some(slot) = self.values.get_mut(self.cursor) {
            *slot = value;
        }
        self.cursor = (self.cursor + 1) % LATENCY_WINDOW;
        self.len = self.len.saturating_add(1).min(LATENCY_WINDOW);
    }

    fn p95(&self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let mut sorted = self.values;
        let index = ((self.len - 1) * 95) / 100;
        // Частичная сортировка: нужен только элемент нужного ранга, полная
        // сортировка 120 значений на каждом такте — лишняя работа.
        let Some(window) = sorted.get_mut(..self.len) else {
            return 0.0;
        };
        window.select_nth_unstable_by(index, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        window.get(index).copied().unwrap_or(0.0)
    }
}

/// Задержка фазы хранения, включаемая только тестом.
///
/// Полноту измерения такта нельзя доказать чтением кода: нужен участок
/// конвейера с известной длительностью **после** анализа. В обычной сборке
/// шов отсутствует полностью.
#[cfg(test)]
static POST_ANALYSIS_DELAY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
fn delay_after_analysis() {
    let ms = POST_ANALYSIS_DELAY_MS.load(AtomicOrdering::Acquire);
    if ms > 0 {
        thread::sleep(Duration::from_millis(ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::{SeriesKey, TickId};

    #[test]
    fn latency_window_returns_nearest_rank_without_allocating() {
        let mut window = LatencyWindow::default();
        for value in 1..=100 {
            window.push(f64::from(value));
        }
        assert_eq!(window.p95(), 95.0);
    }

    #[test]
    fn latency_window_wraps_at_fixed_capacity() {
        let mut window = LatencyWindow::default();
        for value in 0..(LATENCY_WINDOW * 3) {
            window.push(value as f64);
        }
        assert_eq!(window.len, LATENCY_WINDOW);
        assert!(window.p95() >= (LATENCY_WINDOW * 2) as f64);
    }

    #[test]
    fn initial_tick_is_zero() {
        assert_eq!(TickId::FIRST.0, 0);
    }

    #[test]
    fn series_key_is_small_enough_for_hot_path() {
        assert!(std::mem::size_of::<SeriesKey>() <= 16);
    }

    /// PULSE-078: стоимость такта обязана включать весь конвейер.
    ///
    /// Длительность фиксировалась до записи в историю, поэтому запись,
    /// вытеснение и построение снимка в стоимость агента не попадали:
    /// `pulse scorecard` показывал агента дешевле, чем он есть, а
    /// `ticks_skipped` не замечал перерасхода интервала. Тест задерживает
    /// фазу хранения на известную величину и требует увидеть её в метрике.
    #[test]
    fn tick_duration_covers_storage_phase() {
        const DELAY_MS: u64 = 250;
        // Минимум, разрешённый валидацией: такт с задержкой обязан выйти за
        // интервал, иначе перерасход нечем показать.
        const INTERVAL_MS: u64 = 100;
        POST_ANALYSIS_DELAY_MS.store(DELAY_MS, AtomicOrdering::Release);
        let mut config = Config::default();
        config.general.interval_ms = INTERVAL_MS;
        let fs = Arc::new(pulse_collect::DemoFs::new(Duration::from_millis(
            INTERVAL_MS,
        )));
        let runtime = AgentRuntime::start_with_fs(&config, fs).expect("агент обязан стартовать");
        let snapshot = runtime.wait_for_tick(2, Duration::from_secs(10));
        let outcome = snapshot.map(|snapshot| snapshot.agent);
        runtime.shutdown();
        POST_ANALYSIS_DELAY_MS.store(0, AtomicOrdering::Release);

        let agent = outcome.expect("такт обязан быть собран");
        assert!(
            agent.tick_duration_ms >= DELAY_MS as f64,
            "фаза хранения обязана входить в длительность такта: {} мс при задержке {DELAY_MS} мс",
            agent.tick_duration_ms
        );
        assert!(
            agent.ticks_skipped >= 1,
            "такт длиннее интервала обязан считаться перерасходом, пропущено: {}",
            agent.ticks_skipped
        );
    }
}
