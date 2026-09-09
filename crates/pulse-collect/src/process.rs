//! Коллектор процессов.
//!
//! Два свойства, которых нет у наивной реализации `top`.
//!
//! **Идентичность.** Процесс опознаётся парой `(pid, start_ticks)`. PID
//! переиспользуется ядром, поэтому без времени старта «тот же процесс» и
//! «новый процесс с тем же номером» неразличимы, а история и diff показывали бы
//! склеенные графики двух разных программ.
//!
//! **Защита от гонки (TOCTOU).** Один такт читает несколько файлов одного PID.
//! Между чтениями процесс может завершиться, а его номер — достаться другому
//! процессу; тогда `stat` относился бы к одному, а `io`/`cmdline` — к другому.
//! Поэтому `stat` перечитывается в конце: если время старта изменилось, все
//! данные этого PID за такт отбрасываются.
//!
//! Файл `environ` не читается никогда: он почти всегда содержит секреты, а
//! диагностической ценности не даёт.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use pulse_core::config::Security;
use pulse_core::entity::{EntityId, EntityKey, EntitySpec, Labels};
use pulse_core::graph::{CollectCtx, CollectError, Collector};
use pulse_core::metric::ids;
use pulse_core::redact::{parse_cmdline, redact_argv};
use pulse_core::relation::RelationKind;

use crate::fs::{FsSource, FsSourceExt, DEFAULT_CAP};
use crate::parse::{self, ProcStat};

/// Максимальный размер `/proc/<pid>/cmdline`, который вообще читается.
const CMDLINE_CAP: usize = 16 * 1024;

/// Ключ процесса для хранения предыдущих значений счётчиков.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct ProcKey {
    pid: i32,
    start_ticks: u64,
}

/// Состояние процесса между тактами.
///
/// Кроме базы счётчиков здесь живут данные, неизменные в течение жизни
/// процесса: командная строка, путь исполняемого файла, uid и лимит
/// дескрипторов. Перечитывать их каждый такт — три лишних обращения к ядру на
/// каждый процесс, а измениться они могут только вместе с идентичностью
/// `(pid, start_ticks)`, то есть вместе с ключом этой записи. Исключение —
/// `setuid` и `prctl` на живом процессе: такие изменения будут видны только
/// после его перезапуска, и это осознанный компромисс ради стоимости такта.
#[derive(Clone, Debug, Default)]
struct ProcPrev {
    cpu_ticks: u64,
    /// Метки, читаемые один раз за жизнь процесса.
    static_labels: Labels,
    /// Лимит открытых файлов из `/proc/<pid>/limits`.
    fd_limit: Option<f64>,
}

/// Коллектор процессов.
#[derive(Debug)]
pub struct ProcessCollector {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
    cgroup_root: PathBuf,
    security: Security,
    max_processes: usize,
    clock_ticks: f64,
    previous: HashMap<ProcKey, ProcPrev>,
    /// Сколько раз данные PID были отброшены из-за гонки. Для диагностики.
    toctou_drops: u64,
    /// Скрыто потенциальных секретов за такт (складывается в самометрику).
    redactions: u64,
}

impl ProcessCollector {
    #[must_use]
    pub fn new(
        fs: Arc<dyn FsSource>,
        proc_root: PathBuf,
        cgroup_root: PathBuf,
        security: Security,
        max_processes: usize,
        clock_ticks: u64,
    ) -> Self {
        ProcessCollector {
            fs,
            proc_root,
            cgroup_root,
            security,
            max_processes: max_processes.max(1),
            clock_ticks: (clock_ticks.max(1)) as f64,
            previous: HashMap::new(),
            toctou_drops: 0,
            redactions: 0,
        }
    }

    /// Число отброшенных из-за гонки PID.
    #[must_use]
    pub fn toctou_drops(&self) -> u64 {
        self.toctou_drops
    }

    /// Сколько фрагментов было скрыто политикой redaction.
    #[must_use]
    pub fn redactions(&self) -> u64 {
        self.redactions
    }

    fn read(&self, pid: &str, file: &str) -> Option<String> {
        self.fs.read_opt(&self.proc_root.join(pid).join(file))
    }

    fn read_stat(&self, pid: &str) -> Option<ProcStat> {
        let text = self.read(pid, "stat")?;
        parse::parse_proc_stat(&text)
    }

    /// Сущность cgroup процесса, если она уже есть в графе.
    ///
    /// Связь строится через inode каталога: путь из `/proc/<pid>/cgroup`
    /// превращается в inode и ищется среди уже созданных сущностей.
    fn cgroup_entity(&self, ctx: &CollectCtx<'_>, pid: &str) -> Option<EntityId> {
        let text = self.read(pid, "cgroup")?;
        let rel = parse::parse_proc_cgroup(&text)?;
        let trimmed = rel.trim_start_matches('/');
        let dir = if trimmed.is_empty() {
            self.cgroup_root.clone()
        } else {
            self.cgroup_root.join(trimmed)
        };
        let inode = self.fs.inode(&dir).ok()?;
        ctx.entity_by_key(&EntityKey::Cgroup { cgroup_id: inode })
            .map(|e| e.id)
    }

    /// Метки, читаемые один раз за жизнь процесса: uid, командная строка, файл.
    ///
    /// Вызывается только для процессов, которых нет в [`Self::previous`].
    fn static_labels_for(&mut self, pid: &str, status: Option<&str>) -> Labels {
        let mut labels = Labels::new();

        if let Some(status) = status {
            if let Some(uid_line) = status.lines().find(|l| l.starts_with("Uid:")) {
                if let Some(uid) = uid_line.split_whitespace().nth(1) {
                    if uid.chars().all(|c| c.is_ascii_digit()) {
                        labels.set("uid", uid);
                    }
                }
            }
        }

        if self.security.read_cmdline {
            if let Ok(raw) = self
                .fs
                .read(&self.proc_root.join(pid).join("cmdline"), CMDLINE_CAP)
            {
                let argv = parse_cmdline(&raw);
                if !argv.is_empty() {
                    let redacted = redact_argv(
                        &argv,
                        self.security.redact,
                        self.security.redact_high_entropy,
                    );
                    self.redactions = self.redactions.saturating_add(u64::from(redacted.hidden));
                    labels.set("cmdline", redacted.text);
                }
            }
        }

        if let Ok(target) = self.fs.read_link(&self.proc_root.join(pid).join("exe")) {
            labels.set("exe", target.to_string_lossy().into_owned());
        }

        labels
    }

    /// Лимит открытых файлов. Читается один раз за жизнь процесса.
    fn fd_limit_for(&self, pid: &str) -> Option<f64> {
        let text = self.read(pid, "limits")?;
        parse::parse_limit_row(&text, "Max open files")
    }

    fn state_code(state: char) -> f64 {
        match state {
            'R' => 0.0,
            'S' => 1.0,
            'D' => 2.0,
            'T' | 't' => 3.0,
            'Z' => 4.0,
            _ => 5.0,
        }
    }
}

impl Collector for ProcessCollector {
    fn name(&self) -> &'static str {
        "process"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        // Бюджет применяется к самому обходу, а не после него: на хосте с
        // десятками тысяч процессов материализация всего `/proc` стоила
        // памяти и syscall прежде, чем `max_processes` вообще смотрелся.
        // `Arc` клонируется, чтобы замыкание не держало `&self`.
        let fs = Arc::clone(&self.fs);
        let mut pids: Vec<String> = Vec::with_capacity(self.max_processes.min(1_024));
        let budget = self.max_processes;
        let scanned = fs.scan_dir(&self.proc_root, &mut |name| {
            let text = name.to_string_lossy();
            // Не-числовые записи (`self`, `meminfo`, `net`) бюджет не тратят:
            // иначе лимит съедался бы служебными именами.
            if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
                pids.push(text.into_owned());
            }
            pids.len() < budget
        });
        if scanned.is_err() {
            return Err(CollectError::Unavailable("procfs"));
        }

        let host = ctx.host();
        let interval = ctx.interval_secs().max(0.001);
        let mut seen: Vec<ProcKey> = Vec::new();

        for pid_str in pids {
            // Фильтр и бюджет уже применены при обходе.

            let Some(stat) = self.read_stat(&pid_str) else {
                continue;
            };
            let key = ProcKey {
                pid: stat.pid,
                start_ticks: stat.start_ticks,
            };
            // Уже известный процесс: статические данные берём из кэша и
            // экономим три обращения к ядру на каждом такте.
            let cached = self.previous.get(&key).cloned();

            let status = self.read(&pid_str, "status");
            let statm = self.read(&pid_str, "statm");
            let io = self.read(&pid_str, "io");
            let oom_score = self.read(&pid_str, "oom_score");
            // Счёт без материализации: список дескрипторов сервера с тысячами
            // соединений нужен как число, а вектор имён — только цена.
            let fd_count = self
                .fs
                .count_dir(&self.proc_root.join(&pid_str).join("fd"))
                .map(|count| count as f64)
                .ok();
            let cgroup = self.cgroup_entity(ctx, &pid_str);

            let (static_labels, fd_limit) = match &cached {
                Some(previous) => (previous.static_labels.clone(), previous.fd_limit),
                None => (
                    self.static_labels_for(&pid_str, status.as_deref()),
                    self.fd_limit_for(&pid_str),
                ),
            };
            let mut labels = static_labels.clone();
            // Состояние динамическое: берётся из уже прочитанного `stat`.
            labels.set("state", stat.state.to_string());
            // Родитель нужен интерфейсу, чтобы найти корень дерева процессов
            // сервиса: главный процесс — тот, чей родитель вне этого сервиса.
            // Ни минимальный PID, ни время старта этого не дают: воркеры
            // стартуют в тот же тик, а PID переиспользуется.
            labels.set("ppid", stat.ppid.to_string());

            // Проверка гонки: время старта обязано совпадать после всех чтений.
            match self.read_stat(&pid_str) {
                Some(again) if again.start_ticks == stat.start_ticks => {}
                _ => {
                    self.toctou_drops = self.toctou_drops.saturating_add(1);
                    tracing::debug!(
                        pid = pid_str.as_str(),
                        "данные PID отброшены: процесс сменился между чтениями"
                    );
                    continue;
                }
            }

            // Бюджет уже применён при обходе каталога, а счёт обработанных
            // процессов равен длине `seen`.
            seen.push(key);

            let parent = cgroup.unwrap_or(host);
            let mut spec = EntitySpec::new(
                EntityKey::Process {
                    pid: stat.pid,
                    start_ticks: stat.start_ticks,
                },
                stat.comm.clone(),
            )
            .parent(parent);
            spec.labels = labels;
            let entity = ctx.upsert(spec);

            if let Some(cgroup) = cgroup {
                ctx.relate(entity, RelationKind::RunsIn, cgroup);
            }

            // CPU: накопленное время и потребление в ядрах за такт.
            let cpu_ticks = stat.utime_ticks.saturating_add(stat.stime_ticks);
            ctx.sample(
                entity,
                ids::PROC_CPU_SECONDS,
                cpu_ticks as f64 / self.clock_ticks,
            );
            ctx.sample(
                entity,
                ids::PROC_CPU_USER_SECONDS,
                stat.utime_ticks as f64 / self.clock_ticks,
            );
            ctx.sample(
                entity,
                ids::PROC_CPU_SYSTEM_SECONDS,
                stat.stime_ticks as f64 / self.clock_ticks,
            );
            if let Some(previous) = &cached {
                let delta = cpu_ticks.saturating_sub(previous.cpu_ticks) as f64;
                let cores = delta / self.clock_ticks / interval;
                ctx.sample(entity, ids::PROC_CPU_CORES, cores);
            }
            let _ = self.previous.insert(
                key,
                ProcPrev {
                    cpu_ticks,
                    static_labels,
                    fd_limit,
                },
            );

            // Память: VmRSS точнее, чем rss_pages, но доступен не всегда.
            let page_size = 4096.0;
            let rss = status
                .as_deref()
                .and_then(|text| parse::field(text, "VmRSS"))
                .unwrap_or(stat.rss_pages as f64 * page_size);
            ctx.sample(entity, ids::PROC_RSS, rss);
            ctx.sample(entity, ids::PROC_VMS, stat.vsize_bytes as f64);
            if let Some(text) = statm.as_deref() {
                if let Some(shared) = text
                    .split_whitespace()
                    .nth(2)
                    .and_then(|v| v.parse::<f64>().ok())
                {
                    ctx.sample(entity, ids::PROC_SHARED, shared * page_size);
                }
            }

            ctx.sample(entity, ids::PROC_MINOR_FAULTS, stat.minor_faults as f64);
            ctx.sample(entity, ids::PROC_MAJOR_FAULTS, stat.major_faults as f64);
            ctx.sample(entity, ids::PROC_THREADS, stat.num_threads.max(0) as f64);
            ctx.sample(entity, ids::PROC_NICE, stat.nice as f64);
            ctx.sample(entity, ids::PROC_STATE_CODE, Self::state_code(stat.state));

            if let Some(text) = io.as_deref() {
                for (key_name, metric) in [
                    ("read_bytes", ids::PROC_IO_READ_BYTES),
                    ("write_bytes", ids::PROC_IO_WRITE_BYTES),
                    ("syscr", ids::PROC_IO_READ_SYSCALLS),
                    ("syscw", ids::PROC_IO_WRITE_SYSCALLS),
                ] {
                    if let Some(value) = parse::field(text, key_name) {
                        ctx.sample(entity, metric, value);
                    }
                }
            }

            if let Some(text) = status.as_deref() {
                for (key_name, metric) in [
                    ("voluntary_ctxt_switches", ids::PROC_CTX_VOLUNTARY),
                    ("nonvoluntary_ctxt_switches", ids::PROC_CTX_INVOLUNTARY),
                ] {
                    if let Some(value) = parse::field(text, key_name) {
                        ctx.sample(entity, metric, value);
                    }
                }
            }

            if let Some(count) = fd_count {
                ctx.sample(entity, ids::PROC_FD_COUNT, count);
                if let Some(limit) = fd_limit {
                    ctx.sample(entity, ids::PROC_FD_LIMIT, limit);
                    if limit > 0.0 {
                        ctx.sample(entity, ids::PROC_FD_UTIL, (count / limit).clamp(0.0, 1.0));
                    }
                }
            }

            if let Some(value) = oom_score
                .as_deref()
                .and_then(|t| t.trim().parse::<f64>().ok())
            {
                ctx.sample(entity, ids::PROC_OOM_SCORE, value);
            }
        }

        // Забываем базу счётчиков исчезнувших процессов, иначе таблица растёт
        // вместе с общим числом когда-либо запущенных программ.
        self.previous.retain(|key, _| seen.contains(key));
        let _ = DEFAULT_CAP;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use pulse_core::entity::EntityKind;
    use pulse_core::redact::RedactMode;
    use pulse_core::sample::SeriesKey;
    use pulse_core::time::Timestamp;
    use pulse_core::{EntityGraph, TickBatch};

    fn security() -> Security {
        Security {
            redact: RedactMode::Secrets,
            allow_actions: false,
            read_cmdline: true,
            redact_high_entropy: false,
        }
    }

    fn stat_line(pid: i32, comm: &str, cpu: u64, start: u64) -> String {
        format!(
            "{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194560 100 0 200 0 {cpu} {cpu} 0 0 20 0 4 0 {start} 123456789 4096 18446744073709551615"
        )
    }

    fn tree(cpu: u64) -> FixtureFs {
        FixtureFs::new()
            .inode("/sys/fs/cgroup", 1)
            .inode("/sys/fs/cgroup/system.slice/nginx.service", 77)
            .file("/proc/100/stat", &stat_line(100, "nginx", cpu, 5_000))
            .file(
                "/proc/100/status",
                "Name:\tnginx\nUid:\t33\t33\t33\t33\nVmRSS:\t   204800 kB\nThreads:\t4\nvoluntary_ctxt_switches:\t10\nnonvoluntary_ctxt_switches:\t2\n",
            )
            .file("/proc/100/statm", "30000 5000 1000 100 0 2000 0\n")
            .file(
                "/proc/100/io",
                "rchar: 1000\nwchar: 2000\nsyscr: 30\nsyscw: 40\nread_bytes: 4096\nwrite_bytes: 8192\n",
            )
            .file("/proc/100/oom_score", "17\n")
            .file(
                "/proc/100/limits",
                "Limit                     Soft Limit           Hard Limit           Units\nMax open files            1024                 4096                 files\n",
            )
            .file("/proc/100/cgroup", "0::/system.slice/nginx.service\n")
            .file("/proc/100/cmdline", "nginx\0-g\0daemon off;\0")
            .link("/proc/100/exe", "/usr/sbin/nginx")
            .inode("/proc/100/fd/0", 10)
            .inode("/proc/100/fd/1", 11)
            .inode("/proc/100/fd/2", 12)
    }

    fn collector(fs: Arc<dyn FsSource>) -> ProcessCollector {
        ProcessCollector::new(
            fs,
            PathBuf::from("/proc"),
            PathBuf::from("/sys/fs/cgroup"),
            security(),
            100,
            100,
        )
    }

    fn run(collector: &mut ProcessCollector, graph: &mut EntityGraph, at: u64) -> TickBatch {
        graph.begin_tick(Timestamp::from_millis(at));
        {
            let mut ctx = CollectCtx::new(graph, 1.0);
            collector.collect(&mut ctx).expect("сбор процессов");
        }
        graph.end_tick()
    }

    fn value(batch: &TickBatch, key: SeriesKey) -> Option<f64> {
        batch
            .samples
            .iter()
            .find(|s| s.series == key)
            .map(|s| s.value)
    }

    fn process_id(graph: &EntityGraph) -> EntityId {
        graph
            .entities_of_kind(EntityKind::Process)
            .next()
            .map(|e| e.id)
            .unwrap_or(EntityId::NONE)
    }

    #[test]
    fn process_entity_and_metrics_are_collected() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let batch = run(&mut collector, &mut graph, 2_000);
        let pid = process_id(&graph);
        assert_ne!(pid, EntityId::NONE, "процесс должен появиться в графе");

        let rss = value(&batch, SeriesKey::new(pid, ids::PROC_RSS)).unwrap_or_default();
        assert!((rss - 204_800.0 * 1024.0).abs() < 1.0, "rss = {rss}");
        assert_eq!(
            value(&batch, SeriesKey::new(pid, ids::PROC_THREADS)),
            Some(4.0)
        );
        assert_eq!(
            value(&batch, SeriesKey::new(pid, ids::PROC_FD_COUNT)),
            Some(3.0)
        );
        let fd_util = value(&batch, SeriesKey::new(pid, ids::PROC_FD_UTIL)).unwrap_or_default();
        assert!((fd_util - 3.0 / 1024.0).abs() < 1e-12);
        assert_eq!(
            value(&batch, SeriesKey::new(pid, ids::PROC_IO_READ_BYTES)),
            Some(4096.0)
        );
        assert_eq!(
            value(&batch, SeriesKey::new(pid, ids::PROC_OOM_SCORE)),
            Some(17.0)
        );
    }

    #[test]
    fn cpu_cores_use_delta_between_ticks() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let first = run(&mut collector, &mut graph, 2_000);
        let pid = process_id(&graph);
        assert!(
            value(&first, SeriesKey::new(pid, ids::PROC_CPU_CORES)).is_none(),
            "первый такт без базы"
        );

        // utime и stime по 150 -> суммарно +100 тиков за такт при 100 Гц = 1 ядро.
        collector.fs = Arc::new(tree(150));
        let second = run(&mut collector, &mut graph, 3_000);
        let cores = value(&second, SeriesKey::new(pid, ids::PROC_CPU_CORES)).unwrap_or_default();
        assert!((cores - 1.0).abs() < 1e-9, "cores = {cores}");
    }

    #[test]
    fn process_is_linked_to_its_cgroup() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        // Сущность cgroup создаём заранее, как это делает CgroupCollector.
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let cgroup = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 77 }, "nginx.service").parent(host),
        );
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор");
        }
        let _ = graph.end_tick();

        let pid = process_id(&graph);
        let entity = graph.get(pid).expect("процесс");
        assert_eq!(entity.parent, Some(cgroup), "родитель — cgroup процесса");
        assert!(
            graph
                .relations_of(pid)
                .any(|r| r.kind == RelationKind::RunsIn && r.to == cgroup),
            "должна быть связь RunsIn"
        );
    }

    #[test]
    fn pid_reuse_creates_new_entity() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let first = process_id(&graph);

        // Тот же PID, другое время старта: это другой процесс.
        let reused = tree(100).file("/proc/100/stat", &stat_line(100, "python", 10, 9_999));
        collector.fs = Arc::new(reused);
        let _ = run(&mut collector, &mut graph, 3_000);
        let ids_now: Vec<EntityId> = graph
            .entities_of_kind(EntityKind::Process)
            .map(|e| e.id)
            .collect();
        assert!(
            !ids_now.contains(&first) || ids_now.len() > 1,
            "должна появиться новая сущность процесса"
        );
        let names: Vec<String> = graph
            .entities_of_kind(EntityKind::Process)
            .map(|e| e.name.clone())
            .collect();
        assert!(names.contains(&"python".to_string()), "имена: {names:?}");
    }

    #[test]
    fn toctou_race_drops_pid_data() {
        // Второе чтение stat даёт другое время старта — данные должны быть отброшены.
        let fs = FixtureFs::new()
            .file("/proc/100/stat", &stat_line(100, "nginx", 100, 5_000))
            .vanishing("/proc/100/stat", 1);
        let mut collector = collector(Arc::new(fs));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let batch = run(&mut collector, &mut graph, 2_000);
        assert_eq!(
            graph.entities_of_kind(EntityKind::Process).count(),
            0,
            "процесс не должен попасть в граф"
        );
        assert_eq!(collector.toctou_drops(), 1);
        assert!(batch.samples.is_empty());
    }

    #[test]
    fn secrets_in_cmdline_are_redacted() {
        let fs = tree(100).bytes(
            "/proc/100/cmdline",
            b"/usr/bin/app\0--password=hunter2\0--port=80\0",
        );
        let mut collector = collector(Arc::new(fs));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let pid = process_id(&graph);
        let entity = graph.get(pid).expect("процесс");
        let cmdline = entity.labels.get("cmdline").unwrap_or_default();
        assert!(!cmdline.contains("hunter2"), "секрет в метке: {cmdline}");
        assert!(cmdline.contains("<redacted>"));
        assert!(cmdline.contains("--port=80"));
        assert!(collector.redactions() >= 1);
    }

    #[test]
    fn high_entropy_redaction_reaches_process_labels() {
        let token = "AbCdEf0123456789GhIjKlMnOpQrStUv";
        let mut raw = b"/usr/bin/app\0".to_vec();
        raw.extend_from_slice(token.as_bytes());
        raw.push(0);
        let fs = tree(100).bytes("/proc/100/cmdline", &raw);
        let mut security = security();
        security.redact_high_entropy = true;
        let mut collector = ProcessCollector::new(
            Arc::new(fs),
            PathBuf::from("/proc"),
            PathBuf::from("/sys/fs/cgroup"),
            security,
            4096,
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let pid = process_id(&graph);
        let entity = graph.get(pid).expect("процесс");
        let cmdline = entity.labels.get("cmdline").unwrap_or_default();

        assert!(
            !cmdline.contains(token),
            "opt-in эвристика обязана скрыть токен в метке: {cmdline}"
        );
        assert!(cmdline.contains("<redacted>"));
        assert!(collector.redactions() >= 1);
    }

    /// PULSE-080: бюджет обязан ограничивать работу, а не только результат.
    ///
    /// Раньше `read_dir` материализовал весь `/proc`, и только потом цикл
    /// останавливался по `max_processes`. На хосте с десятками тысяч процессов
    /// агент платил за весь каталог памятью и обходом прежде, чем решить, что
    /// столько ему не нужно.
    #[test]
    fn proc_walk_work_is_bounded_by_max_processes() {
        const TOTAL: usize = 20_000;
        const BUDGET: usize = 64;

        let mut fs = FixtureFs::new();
        for pid in 1..=TOTAL {
            fs = fs.file(
                &format!("/proc/{pid}/stat"),
                &stat_line(pid as i32, "worker", 10, 5_000),
            );
        }
        let counting = crate::test_support::CountingFs::new(Arc::new(fs));
        let mut collector = ProcessCollector::new(
            Arc::new(counting.clone()),
            PathBuf::from("/proc"),
            PathBuf::from("/sys/fs/cgroup"),
            security(),
            BUDGET,
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);

        let counts = counting.counts();
        assert!(
            counts.dir_entries <= (BUDGET as u64) + 8,
            "обход обязан прекратиться на бюджете, посещено записей: {}",
            counts.dir_entries
        );
        assert_eq!(
            graph.entities_of_kind(EntityKind::Process).count(),
            BUDGET,
            "в графе обязано быть ровно столько процессов, сколько разрешил бюджет"
        );
    }
    #[test]
    fn cmdline_can_be_disabled_entirely() {
        let mut security = security();
        security.read_cmdline = false;
        let mut collector = ProcessCollector::new(
            Arc::new(tree(100)),
            PathBuf::from("/proc"),
            PathBuf::from("/sys/fs/cgroup"),
            security,
            100,
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let pid = process_id(&graph);
        let entity = graph.get(pid).expect("процесс");
        assert!(entity.labels.get("cmdline").is_none());
    }

    #[test]
    fn ansi_escape_in_comm_never_reaches_entity_name() {
        let fs = tree(100).file(
            "/proc/100/stat",
            &stat_line(100, "\u{1b}[2Jevil", 100, 5_000),
        );
        let mut collector = collector(Arc::new(fs));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let pid = process_id(&graph);
        let entity = graph.get(pid).expect("процесс");
        assert!(!entity.name.contains('\u{1b}'), "имя: {:?}", entity.name);
        assert!(entity.name.contains("evil"));
    }

    #[test]
    fn non_utf8_and_missing_files_are_tolerated() {
        let fs = FixtureFs::new()
            .file("/proc/100/stat", &stat_line(100, "svc", 100, 5_000))
            .bytes("/proc/100/cmdline", &[0xff, 0xfe, 0x00])
            .bytes("/proc/100/status", &[0xff, 0xff]);
        let mut collector = collector(Arc::new(fs));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let batch = run(&mut collector, &mut graph, 2_000);
        assert_eq!(graph.entities_of_kind(EntityKind::Process).count(), 1);
        assert!(batch.samples.iter().all(|s| s.value.is_finite()));
    }

    #[test]
    fn process_limit_is_respected() {
        let mut fs = FixtureFs::new();
        for pid in 100..200 {
            fs = fs.file(
                &format!("/proc/{pid}/stat"),
                &stat_line(pid, "svc", 10, 1_000),
            );
        }
        let mut collector = ProcessCollector::new(
            Arc::new(fs),
            PathBuf::from("/proc"),
            PathBuf::from("/sys/fs/cgroup"),
            security(),
            10,
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        assert_eq!(graph.entities_of_kind(EntityKind::Process).count(), 10);
    }

    #[test]
    fn dead_process_state_is_forgotten() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        assert_eq!(collector.previous.len(), 1);
        collector.fs = Arc::new(FixtureFs::new().inode("/proc", 1));
        let _ = run(&mut collector, &mut graph, 3_000);
        assert!(
            collector.previous.is_empty(),
            "база счётчиков мёртвых процессов должна освобождаться"
        );
    }

    #[test]
    fn static_labels_and_fd_limit_are_read_once_per_process_lifetime() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);

        // Второй такт: файлы, которые обязаны читаться один раз, исчезли.
        // Если бы коллектор перечитывал их каждый такт, метки и лимит пропали бы.
        let mut second = tree(200);
        second.remove("/proc/100/cmdline");
        second.remove("/proc/100/limits");
        collector.fs = Arc::new(second);
        let batch = run(&mut collector, &mut graph, 3_000);

        let process = graph
            .entities_of_kind(EntityKind::Process)
            .next()
            .expect("процесс в графе");
        assert!(
            process
                .labels
                .get("cmdline")
                .is_some_and(|c| c.contains("nginx")),
            "командная строка обязана браться из кэша: {:?}",
            process.labels.get("cmdline")
        );
        assert_eq!(process.labels.get("exe"), Some("/usr/sbin/nginx"));
        assert_eq!(
            value(&batch, SeriesKey::new(process.id, ids::PROC_FD_LIMIT)),
            Some(1_024.0),
            "лимит дескрипторов обязан браться из кэша"
        );
        // Динамическая метка остаётся динамической.
        assert_eq!(process.labels.get("state"), Some("S"));
    }

    #[test]
    fn exec_changes_identity_and_forces_static_reread() {
        let mut collector = collector(Arc::new(tree(100)));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);

        // Тот же PID, другое время старта и другая командная строка.
        let replaced = FixtureFs::new()
            .inode("/sys/fs/cgroup", 1)
            .file("/proc/100/stat", &stat_line(100, "python3", 10, 9_999))
            .file("/proc/100/cmdline", "python3\0worker.py\0")
            .file("/proc/100/cgroup", "0::/\n");
        collector.fs = Arc::new(replaced);
        let _ = run(&mut collector, &mut graph, 3_000);

        let fresh = graph
            .entities_of_kind(EntityKind::Process)
            .find(|e| e.name == "python3")
            .expect("новая идентичность должна появиться");
        assert!(
            fresh
                .labels
                .get("cmdline")
                .is_some_and(|c| c.contains("worker.py")),
            "смена идентичности обязана перечитать статические метки: {:?}",
            fresh.labels.get("cmdline")
        );
    }

    #[test]
    fn state_code_mapping_is_stable() {
        assert_eq!(ProcessCollector::state_code('R'), 0.0);
        assert_eq!(ProcessCollector::state_code('S'), 1.0);
        assert_eq!(ProcessCollector::state_code('D'), 2.0);
        assert_eq!(ProcessCollector::state_code('Z'), 4.0);
        assert_eq!(ProcessCollector::state_code('X'), 5.0);
    }
}
