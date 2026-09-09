//! Коллектор хоста: CPU, память, PSI, процессы, дескрипторы, сеть, OOM.
//!
//! Производные величины (доли CPU) считаются как разность счётчиков между
//! тактами. На первом такте базы нет, поэтому производные не публикуются:
//! показать «CPU 100%» из накопленного с загрузки системы значения — хуже,
//! чем не показать ничего.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pulse_core::graph::{CollectCtx, CollectError, Collector};
use pulse_core::metric::ids;

use crate::fs::{FsSource, FsSourceExt};
use crate::parse;

/// Бюджет обхода `/proc` для счёта процессов и потоков.
///
/// Хост-коллектор не имеет настройки: `max_processes` ограничивает подробный
/// сбор по процессам, а здесь считаются только числа. Потолок нужен, чтобы
/// один такт не превращался в обход каталога неизвестного размера; при его
/// достижении метрики не публикуются, а не занижаются.
const PROC_SCAN_CAP: usize = 65_536;

/// Коллектор метрик уровня хоста.
#[derive(Debug)]
pub struct HostCollector {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
    previous_cpu: Option<parse::CpuTimes>,
}

impl HostCollector {
    #[must_use]
    pub fn new(fs: Arc<dyn FsSource>, proc_root: PathBuf) -> Self {
        HostCollector {
            fs,
            proc_root,
            previous_cpu: None,
        }
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.proc_root.join(rest)
    }

    fn read(&self, rest: &str) -> Option<String> {
        self.fs.read_opt(&self.path(rest))
    }

    /// CPU: сырой счётчик и доли по режимам из разности с прошлым тактом.
    ///
    /// Возвращает `false`, если источник не прочитан: `/proc/stat` — не
    /// опциональная подсистема, а основа наблюдения за хостом, и его потеря
    /// обязана быть заявлена наружу, а не выглядеть удачным тактом.
    fn collect_cpu(&mut self, ctx: &mut CollectCtx<'_>) -> bool {
        let Some(text) = self.read("stat") else {
            return false;
        };
        let host = ctx.host();

        if let Some(times) = parse::parse_cpu_times(&text) {
            ctx.sample(host, ids::HOST_CPU_SECONDS, times.busy());

            if let Some(previous) = self.previous_cpu {
                let total = times.total() - previous.total();
                if total > 0.0 {
                    let busy = (times.busy() - previous.busy()).max(0.0);
                    let user = (times.user + times.nice - previous.user - previous.nice).max(0.0);
                    let system = (times.system - previous.system).max(0.0);
                    let iowait = (times.iowait - previous.iowait).max(0.0);
                    let steal = (times.steal - previous.steal).max(0.0);
                    ctx.sample(host, ids::HOST_CPU_UTIL, (busy / total).clamp(0.0, 1.0));
                    ctx.sample(host, ids::HOST_CPU_USER, (user / total).clamp(0.0, 1.0));
                    ctx.sample(host, ids::HOST_CPU_SYSTEM, (system / total).clamp(0.0, 1.0));
                    ctx.sample(host, ids::HOST_CPU_IOWAIT, (iowait / total).clamp(0.0, 1.0));
                    ctx.sample(host, ids::HOST_CPU_STEAL, (steal / total).clamp(0.0, 1.0));
                }
            }
            self.previous_cpu = Some(times);
        }

        // Число логических CPU — по строкам `cpuN`.
        let cpus = text
            .lines()
            .filter(|line| {
                line.starts_with("cpu")
                    && line
                        .get(3..4)
                        .is_some_and(|c| c.chars().all(char::is_numeric))
            })
            .count();
        if cpus > 0 {
            ctx.sample(host, ids::HOST_CPU_COUNT, cpus as f64);
        }

        for (key, metric) in [
            ("procs_running", ids::HOST_PROCS_RUNNING),
            ("procs_blocked", ids::HOST_PROCS_BLOCKED),
        ] {
            if let Some(value) = parse::field(&text, key) {
                ctx.sample(host, metric, value);
            }
        }
        for (key, metric) in [
            ("ctxt", ids::HOST_CTX_SWITCHES),
            ("processes", ids::HOST_FORKS),
        ] {
            if let Some(value) = parse::field(&text, key) {
                ctx.sample(host, metric, value);
            }
        }
        true
    }

    /// Память хоста. `false` — `/proc/meminfo` недоступен: это второй
    /// критический источник, без которого хост не наблюдается.
    fn collect_memory(&self, ctx: &mut CollectCtx<'_>) -> bool {
        let Some(text) = self.read("meminfo") else {
            return false;
        };
        let host = ctx.host();
        let total = parse::field(&text, "MemTotal").unwrap_or(0.0);
        let available = parse::field(&text, "MemAvailable").unwrap_or(0.0);
        let free = parse::field(&text, "MemFree").unwrap_or(0.0);
        let cached = parse::field(&text, "Cached").unwrap_or(0.0);
        let buffers = parse::field(&text, "Buffers").unwrap_or(0.0);
        let swap_total = parse::field(&text, "SwapTotal").unwrap_or(0.0);
        let swap_free = parse::field(&text, "SwapFree").unwrap_or(0.0);

        ctx.sample(host, ids::HOST_MEM_TOTAL, total);
        ctx.sample(host, ids::HOST_MEM_AVAILABLE, available);
        ctx.sample(host, ids::HOST_MEM_FREE, free);
        ctx.sample(host, ids::HOST_MEM_CACHED, cached);
        ctx.sample(host, ids::HOST_MEM_BUFFERS, buffers);
        ctx.sample(host, ids::HOST_SWAP_TOTAL, swap_total);
        ctx.sample(host, ids::HOST_SWAP_USED, (swap_total - swap_free).max(0.0));

        if total > 0.0 {
            // «Занято» считается от available, а не от free: страничный кеш
            // отдаётся под нагрузку и занятой памятью не является.
            let used = (total - available).max(0.0);
            ctx.sample(host, ids::HOST_MEM_USED, used);
            ctx.sample(host, ids::HOST_MEM_UTIL, (used / total).clamp(0.0, 1.0));
        }
        true
    }

    fn collect_pressure(&self, ctx: &mut CollectCtx<'_>) {
        let host = ctx.host();
        let sources = [
            (
                "pressure/cpu",
                [
                    ids::HOST_PSI_CPU_SOME_AVG10,
                    ids::HOST_PSI_CPU_SOME_TOTAL,
                    ids::HOST_PSI_CPU_SOME_AVG10,
                    ids::HOST_PSI_CPU_SOME_TOTAL,
                ],
            ),
            (
                "pressure/memory",
                [
                    ids::HOST_PSI_MEM_SOME_AVG10,
                    ids::HOST_PSI_MEM_FULL_AVG10,
                    ids::HOST_PSI_MEM_FULL_AVG10,
                    ids::HOST_PSI_MEM_FULL_TOTAL,
                ],
            ),
            (
                "pressure/io",
                [
                    ids::HOST_PSI_IO_SOME_AVG10,
                    ids::HOST_PSI_IO_FULL_AVG10,
                    ids::HOST_PSI_IO_FULL_AVG10,
                    ids::HOST_PSI_IO_FULL_TOTAL,
                ],
            ),
        ];

        for (file, metrics) in sources {
            // Отсутствие PSI — норма: ядро может быть собрано без CONFIG_PSI.
            let Some(text) = self.read(file) else {
                continue;
            };
            let pressure = parse::parse_pressure(&text);
            match file {
                "pressure/cpu" => {
                    ctx.sample(host, ids::HOST_PSI_CPU_SOME_AVG10, pressure.some_avg10);
                    ctx.sample(host, ids::HOST_PSI_CPU_SOME_TOTAL, pressure.some_total_secs);
                }
                "pressure/memory" => {
                    ctx.sample(host, ids::HOST_PSI_MEM_SOME_AVG10, pressure.some_avg10);
                    ctx.sample(host, ids::HOST_PSI_MEM_FULL_AVG10, pressure.full_avg10);
                    ctx.sample(host, ids::HOST_PSI_MEM_FULL_TOTAL, pressure.full_total_secs);
                }
                "pressure/io" => {
                    ctx.sample(host, ids::HOST_PSI_IO_SOME_AVG10, pressure.some_avg10);
                    ctx.sample(host, ids::HOST_PSI_IO_FULL_AVG10, pressure.full_avg10);
                    ctx.sample(host, ids::HOST_PSI_IO_FULL_TOTAL, pressure.full_total_secs);
                }
                _ => {
                    let _ = metrics;
                }
            }
        }
    }

    fn collect_misc(&self, ctx: &mut CollectCtx<'_>) {
        let host = ctx.host();

        if let Some(text) = self.read("loadavg") {
            if let Some((one, five, fifteen)) = parse::parse_loadavg(&text) {
                ctx.sample(host, ids::HOST_LOAD1, one);
                ctx.sample(host, ids::HOST_LOAD5, five);
                ctx.sample(host, ids::HOST_LOAD15, fifteen);
            }
        }

        if let Some(text) = self.read("uptime") {
            if let Some(value) = text
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
            {
                ctx.sample(host, ids::HOST_UPTIME, value);
            }
        }

        if let Some(text) = self.read("sys/fs/file-nr") {
            if let Some((used, max)) = parse::parse_file_nr(&text) {
                ctx.sample(host, ids::HOST_FD_OPEN, used);
                ctx.sample(host, ids::HOST_FD_MAX, max);
                if max > 0.0 {
                    ctx.sample(host, ids::HOST_FD_UTIL, (used / max).clamp(0.0, 1.0));
                }
            }
        }

        if let Some(text) = self.read("net/sockstat") {
            if let Some(value) = parse::parse_sockstat(&text, "TCP", "inuse") {
                ctx.sample(host, ids::HOST_TCP_INUSE, value);
            }
        }

        if let Some(text) = self.read("net/snmp") {
            if let Some(value) = parse::parse_snmp_field(&text, "Tcp", "RetransSegs") {
                ctx.sample(host, ids::HOST_TCP_RETRANS, value);
            }
        }

        if let Some(text) = self.read("vmstat") {
            if let Some(value) = parse::field(&text, "oom_kill") {
                ctx.sample(host, ids::HOST_OOM_KILLS, value);
            }
        }
    }

    /// Число процессов и потоков: считается обходом каталога `/proc`.
    ///
    /// Два прохода вместо одного: читать `stat` внутри обхода нельзя, потому
    /// что источник вправе держать замок на время обхода. Сначала имена под
    /// бюджетом, затем чтения.
    fn collect_process_counts(&self, ctx: &mut CollectCtx<'_>) {
        let Ok((names, truncated)) = self
            .fs
            .read_dir_capped(Path::new(&self.proc_root), PROC_SCAN_CAP)
        else {
            return;
        };
        let mut processes = 0usize;
        let mut threads = 0f64;
        for name in &names {
            let name = name.to_string_lossy();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            processes += 1;
            if let Some(text) = self.fs.read_opt(&self.path(&format!("{name}/stat"))) {
                if let Some(stat) = parse::parse_proc_stat(&text) {
                    threads += stat.num_threads as f64;
                }
            }
        }
        let host = ctx.host();
        // Усечённый обход даёт заниженные числа, а заниженное число процессов
        // хуже отсутствующего: по нему строят вывод «нагрузка упала». Поэтому
        // при усечении метрики не публикуются вовсе.
        if truncated {
            tracing::debug!(
                cap = PROC_SCAN_CAP,
                "обход /proc усечён: счёт процессов и потоков не публикуется"
            );
            return;
        }
        ctx.sample(host, ids::HOST_PROCS_TOTAL, processes as f64);
        if threads > 0.0 {
            ctx.sample(host, ids::HOST_THREADS_TOTAL, threads);
        }
    }
}

impl Collector for HostCollector {
    fn name(&self) -> &'static str {
        "host"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        // Разделение обязательное. `/proc/stat` и `/proc/meminfo` — основа
        // наблюдения за хостом: без них CPU и память вообще не измеряются,
        // и такт нельзя считать удачным. Давление, файловые дескрипторы и
        // счётчики процессов опциональны — подсистема может быть выключена
        // в ядре, и это не отказ сбора.
        //
        // Раньше все методы молча выходили, а `collect` всегда возвращал
        // `Ok(())`: агент мог ослепнуть, не увеличив ни одного счётчика
        // ошибок и не породив ни одного события.
        let cpu = self.collect_cpu(ctx);
        let memory = self.collect_memory(ctx);
        self.collect_pressure(ctx);
        self.collect_misc(ctx);
        self.collect_process_counts(ctx);

        match (cpu, memory) {
            (true, true) => Ok(()),
            (false, false) => Err(CollectError::Unavailable(
                "не читаются ни /proc/stat, ни /proc/meminfo",
            )),
            (false, true) => Err(CollectError::Unavailable(
                "не читается /proc/stat: метрики CPU хоста не собраны",
            )),
            (true, false) => Err(CollectError::Unavailable(
                "не читается /proc/meminfo: метрики памяти хоста не собраны",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use pulse_core::sample::SeriesKey;
    use pulse_core::time::Timestamp;
    use pulse_core::EntityGraph;

    fn fixture() -> FixtureFs {
        FixtureFs::new()
            .file(
                "/proc/stat",
                "cpu  1000 100 500 8000 200 0 0 0 0 0\n\
                 cpu0 500 50 250 4000 100 0 0 0\n\
                 cpu1 500 50 250 4000 100 0 0 0\n\
                 ctxt 123456\n\
                 processes 4321\n\
                 procs_running 3\n\
                 procs_blocked 1\n",
            )
            .file(
                "/proc/meminfo",
                "MemTotal:       8000000 kB\n\
                 MemFree:        1000000 kB\n\
                 MemAvailable:   2000000 kB\n\
                 Buffers:         100000 kB\n\
                 Cached:         3000000 kB\n\
                 SwapTotal:      2000000 kB\n\
                 SwapFree:       1500000 kB\n",
            )
            .file(
                "/proc/pressure/cpu",
                "some avg10=25.00 avg60=10.00 avg300=1.00 total=5000000\n",
            )
            .file(
                "/proc/pressure/io",
                "some avg10=40.00 avg60=20.00 avg300=2.00 total=9000000\n\
                 full avg10=15.00 avg60=5.00 avg300=1.00 total=4000000\n",
            )
            .file("/proc/loadavg", "1.25 1.32 1.18 2/1234 5678\n")
            .file("/proc/uptime", "123456.78 987654.32\n")
            .file("/proc/sys/fs/file-nr", "1216 0 65536\n")
            .file("/proc/net/sockstat", "TCP: inuse 42 orphan 0 tw 1\n")
            .file(
                "/proc/net/snmp",
                "Tcp: RtoAlgorithm RtoMin RetransSegs\nTcp: 1 200 777\n",
            )
            .file("/proc/vmstat", "nr_free_pages 1000\noom_kill 3\n")
            .file(
                "/proc/1/stat",
                "1 (systemd) S 0 1 1 0 -1 0 10 0 20 0 5 5 0 0 20 0 2 0 100 1000 500",
            )
            .file(
                "/proc/2/stat",
                "2 (nginx) S 1 2 2 0 -1 0 10 0 20 0 7 3 0 0 20 0 4 0 200 2000 800",
            )
    }

    /// Прогоняет коллектор указанное число тактов и возвращает последний пакет.
    fn run_ticks(
        collector: &mut HostCollector,
        ticks: usize,
    ) -> (EntityGraph, pulse_core::TickBatch) {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let mut batch = None;
        for tick in 0..ticks {
            graph.begin_tick(Timestamp::from_millis(2_000 + (tick as u64) * 1_000));
            {
                let mut ctx = CollectCtx::new(&mut graph, 1.0);
                collector
                    .collect(&mut ctx)
                    .expect("коллектор не должен падать");
            }
            batch = Some(graph.end_tick());
        }
        (graph, batch.expect("хотя бы один такт"))
    }

    fn value(batch: &pulse_core::TickBatch, key: SeriesKey) -> Option<f64> {
        batch
            .samples
            .iter()
            .find(|s| s.series == key)
            .map(|s| s.value)
    }

    #[test]
    fn first_tick_has_no_cpu_ratios_but_has_counters() {
        let fs = Arc::new(fixture());
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        assert!(
            value(&batch, SeriesKey::new(host, ids::HOST_CPU_UTIL)).is_none(),
            "на первом такте нет базы для разности"
        );
        assert!(value(&batch, SeriesKey::new(host, ids::HOST_CPU_SECONDS)).is_some());
    }

    #[test]
    fn cpu_ratio_is_computed_from_delta() {
        let mut fs = fixture();
        // Второй такт: busy +100 (user), total +200.
        let first = Arc::new(fixture());
        let mut collector = HostCollector::new(first, PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let _ = graph.end_tick();

        fs.set(
            "/proc/stat",
            "cpu  1100 100 500 8100 200 0 0 0 0 0\n\
             cpu0 500 50 250 4000 100 0 0 0\n\
             cpu1 500 50 250 4000 100 0 0 0\n\
             ctxt 124456\n\
             processes 4400\n\
             procs_running 5\n\
             procs_blocked 0\n",
        );
        let mut collector2 = HostCollector::new(Arc::new(fs), PathBuf::from("/proc"));
        // Переносим базу первого такта.
        collector2.previous_cpu = collector.previous_cpu;

        graph.begin_tick(Timestamp::from_millis(3_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector2.collect(&mut ctx).expect("такт 2");
        }
        let batch = graph.end_tick();
        let host = graph.host();
        let util = value(&batch, SeriesKey::new(host, ids::HOST_CPU_UTIL))
            .expect("доля CPU должна появиться");
        // busy: +100, total: +200 -> 0.5
        assert!((util - 0.5).abs() < 1e-9, "util = {util}");
        let user = value(&batch, SeriesKey::new(host, ids::HOST_CPU_USER)).unwrap_or_default();
        assert!((user - 0.5).abs() < 1e-9);
    }

    #[test]
    fn memory_used_is_based_on_available() {
        let fs = Arc::new(fixture());
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        let total = value(&batch, SeriesKey::new(host, ids::HOST_MEM_TOTAL)).unwrap_or_default();
        let used = value(&batch, SeriesKey::new(host, ids::HOST_MEM_USED)).unwrap_or_default();
        let util = value(&batch, SeriesKey::new(host, ids::HOST_MEM_UTIL)).unwrap_or_default();
        assert!((total - 8_000_000.0 * 1024.0).abs() < 1.0);
        assert!((used - 6_000_000.0 * 1024.0).abs() < 1.0);
        assert!((util - 0.75).abs() < 1e-9, "util = {util}");
    }

    #[test]
    fn pressure_is_collected_as_ratio() {
        let fs = Arc::new(fixture());
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        let cpu =
            value(&batch, SeriesKey::new(host, ids::HOST_PSI_CPU_SOME_AVG10)).expect("PSI cpu");
        let io_full =
            value(&batch, SeriesKey::new(host, ids::HOST_PSI_IO_FULL_AVG10)).expect("PSI io full");
        assert!((cpu - 0.25).abs() < 1e-9);
        assert!((io_full - 0.15).abs() < 1e-9);
    }

    #[test]
    fn missing_pressure_file_is_not_an_error() {
        // Ядро без CONFIG_PSI: файлов нет вообще.
        let fs = Arc::new(
            FixtureFs::new()
                .file("/proc/stat", "cpu  1 1 1 1 1 0 0 0\n")
                .file("/proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 500 kB\n"),
        );
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        assert!(value(&batch, SeriesKey::new(host, ids::HOST_PSI_CPU_SOME_AVG10)).is_none());
        assert!(value(&batch, SeriesKey::new(host, ids::HOST_MEM_TOTAL)).is_some());
    }

    #[test]
    fn garbage_in_proc_does_not_panic() {
        let fs = Arc::new(
            FixtureFs::new()
                .file("/proc/stat", "cpu  abc def\nctxt xyz\n")
                .file("/proc/meminfo", "MemTotal: not-a-number\n")
                .file("/proc/pressure/cpu", "\0\0\0garbage")
                .file("/proc/loadavg", "")
                .file("/proc/sys/fs/file-nr", "1")
                .file("/proc/vmstat", "oom_kill"),
        );
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (_, batch) = run_ticks(&mut collector, 2);
        // Инвариант — не паника и отсутствие мусорных значений в образцах.
        assert!(
            batch.samples.iter().all(|s| s.value.is_finite()),
            "все опубликованные значения должны быть конечными"
        );
    }

    #[test]
    fn fd_utilization_and_counts_are_collected() {
        let fs = Arc::new(fixture());
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        let util = value(&batch, SeriesKey::new(host, ids::HOST_FD_UTIL)).unwrap_or_default();
        assert!((util - 1216.0 / 65536.0).abs() < 1e-12);
        let procs = value(&batch, SeriesKey::new(host, ids::HOST_PROCS_TOTAL)).unwrap_or_default();
        assert!((procs - 2.0).abs() < 1e-12, "процессов {procs}");
        let threads =
            value(&batch, SeriesKey::new(host, ids::HOST_THREADS_TOTAL)).unwrap_or_default();
        assert!((threads - 6.0).abs() < 1e-12, "потоков {threads}");
    }

    #[test]
    fn cpu_count_is_derived_from_per_cpu_lines() {
        let fs = Arc::new(fixture());
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        let cpus = value(&batch, SeriesKey::new(host, ids::HOST_CPU_COUNT)).unwrap_or_default();
        assert!((cpus - 2.0).abs() < 1e-12);
    }

    #[test]
    fn tcp_and_oom_counters_are_collected() {
        let fs = Arc::new(fixture());
        let mut collector = HostCollector::new(fs, PathBuf::from("/proc"));
        let (graph, batch) = run_ticks(&mut collector, 1);
        let host = graph.host();
        assert_eq!(
            value(&batch, SeriesKey::new(host, ids::HOST_TCP_INUSE)),
            Some(42.0)
        );
        assert_eq!(
            value(&batch, SeriesKey::new(host, ids::HOST_TCP_RETRANS)),
            Some(777.0)
        );
        assert_eq!(
            value(&batch, SeriesKey::new(host, ids::HOST_OOM_KILLS)),
            Some(3.0)
        );
    }
}
