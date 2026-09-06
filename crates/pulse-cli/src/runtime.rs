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
use pulse_collect::{boot_id, build_collectors, hostname, FsSource, RealFs};
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
    /// Запускает поток сбора с реальными procfs/sysfs/cgroupfs.
    pub fn start(config: &Config) -> io::Result<Self> {
        config
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

        let fs: Arc<dyn FsSource> = Arc::new(RealFs);
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
    let mut latency = LatencyWindow::default();

    while !stop.load(AtomicOrdering::Acquire) {
        let started = Instant::now();
        let now = Timestamp::now();
        graph.begin_tick(now);

        for collector in &mut collectors {
            let name = collector.name();
            let result;
            let noted;
            {
                let mut ctx = CollectCtx::new(&mut graph, config.interval_secs());
                result = collector.collect(&mut ctx);
                noted = u64::from(ctx.errors());
            }
            collector_errors = collector_errors.saturating_add(noted);
            if let Err(error) = result {
                collector_errors = collector_errors.saturating_add(1);
                graph.push_collector_error(name, &error.to_string());
                tracing::debug!(collector = name, error = %error, "ошибка коллектора");
            }
            if stop.load(AtomicOrdering::Acquire) {
                return;
            }
        }

        if stop.load(AtomicOrdering::Acquire) {
            return;
        }

        let mut batch = graph.end_tick();
        let mut latest = with_history_read(&history, History::latest);
        for sample in &batch.samples {
            latest.set(sample.series, sample.value);
        }
        latest.retain_entities(&|id| graph.get(id).is_some());

        let problems = with_history_read(&history, |stored| {
            analyzer.evaluate(&graph, &latest, stored, now)
        });
        batch.events.extend(analyzer.take_events());

        ticks_total = ticks_total.saturating_add(1);
        let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        latency.push(elapsed_ms);
        if started.elapsed() > interval {
            ticks_skipped = ticks_skipped.saturating_add(1);
        }

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

        if stop.load(AtomicOrdering::Acquire) {
            return;
        }

        let agent = AgentStats {
            tick_duration_ms: elapsed_ms,
            tick_duration_p95_ms: latency.p95(),
            ticks_total,
            ticks_skipped,
            collector_errors,
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
        let snapshot =
            Snapshot::build(&graph, latest, problems, events, agent, &hostname, &boot_id);
        published.store(Arc::new(snapshot));

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
}
