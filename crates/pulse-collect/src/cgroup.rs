//! Коллектор cgroup v2 и владельцев ресурсов.
//!
//! Здесь строится та часть графа, которая отличает Pulse от «ещё одного top»:
//! из пути cgroup выводится **владелец** ресурса — systemd-unit, контейнер,
//! pod — и телеметрия привязывается не только к анонимной группе, но и к тому,
//! что оператор реально знает по имени.
//!
//! Ключ сущности cgroup — inode каталога, а не путь: `systemctl restart`
//! пересоздаёт каталог (новый inode → новая сущность → событие), а
//! переименование сохраняет inode (та же сущность → событие смены имени).
//!
//! Container/pod распознаются только по пути, без обращения к Docker или CRI.
//! Компромисс осознанный: доступ к сокету демона требует прав и создаёт
//! зависимость от рантайма, а путь cgroup даёт identity в 95% случаев.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pulse_core::entity::{EntityId, EntityKey, EntityKind, EntitySpec, Runtime};
use pulse_core::graph::{CollectCtx, CollectError, Collector};
use pulse_core::metric::ids;
use pulse_core::relation::RelationKind;

use crate::fs::{FsSource, FsSourceExt};
use crate::parse;

/// Максимальная глубина обхода дерева cgroup.
const MAX_DEPTH: usize = 8;

/// Сколько дисков одной cgroup попадает в связи и метку `disks`.
const MAX_LINKED_DISKS: usize = 8;

/// Предыдущие значения счётчиков cgroup для производных величин.
#[derive(Clone, Copy, Debug, Default)]
struct CgroupPrev {
    usage_usec: f64,
    nr_periods: f64,
    nr_throttled: f64,
    /// Накопленные байты io.stat: нужны, чтобы посчитать настоящую скорость.
    io_rbytes: f64,
    io_wbytes: f64,
}

/// Способ сопоставить `major:minor` из cgroup `io.stat` с Disk-сущностью.
///
/// Определяется один раз на первом такте и не пересматривается: проверять
/// платформенные возможности на каждом обновлении — лишний syscall в hot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiskResolver {
    /// Обычный Linux с populated `/sys/dev/block`.
    SysDevBlock,
    /// WSL2/контейнер без sysfs-ссылок: метки Disk из `/proc/diskstats`.
    GraphLabels,
}

/// Владелец ресурса, выведенный из пути cgroup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// systemd-unit: `nginx.service`, `session-3.scope`.
    Unit(String),
    /// Контейнер: рантайм и полный идентификатор.
    Container { runtime: Runtime, id: String },
}

/// Разбирает последний сегмент пути cgroup.
///
/// Порядок проверок важен: `docker-<id>.scope` — тоже `.scope`, но это
/// контейнер, а не unit, поэтому контейнерные шаблоны проверяются первыми.
#[must_use]
pub fn owner_from_segment(segment: &str) -> Option<Owner> {
    let container_patterns: [(&str, Runtime); 5] = [
        ("docker-", Runtime::Docker),
        ("cri-containerd-", Runtime::Containerd),
        ("containerd-", Runtime::Containerd),
        ("crio-", Runtime::CriO),
        ("libpod-", Runtime::Podman),
    ];

    for (prefix, runtime) in container_patterns {
        if let Some(rest) = segment.strip_prefix(prefix) {
            let id = rest.trim_end_matches(".scope").trim_end_matches(".slice");
            if id.len() >= 8 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(Owner::Container {
                    runtime,
                    id: id.to_string(),
                });
            }
        }
    }

    // Драйвер cgroupfs у containerd/Docker: сегмент — просто идентификатор.
    if segment.len() >= 32 && segment.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(Owner::Container {
            runtime: Runtime::Unknown,
            id: segment.to_string(),
        });
    }

    for suffix in [".service", ".socket", ".mount", ".timer", ".scope"] {
        if segment.ends_with(suffix) {
            return Some(Owner::Unit(segment.to_string()));
        }
    }
    None
}

/// Извлекает uid pod из пути kubepods (`.../pod3f2b.../...`).
#[must_use]
pub fn pod_uid_from_path(path: &str) -> Option<String> {
    if !path.contains("kubepods") {
        return None;
    }
    for segment in path.split('/') {
        let cleaned = segment
            .trim_end_matches(".slice")
            .trim_end_matches(".scope");
        if let Some(uid) = cleaned.strip_prefix("pod") {
            if uid.len() >= 8 {
                return Some(uid.replace('_', "-"));
            }
        }
    }
    None
}

/// Коллектор cgroup.
#[derive(Debug)]
pub struct CgroupCollector {
    fs: Arc<dyn FsSource>,
    cgroup_root: PathBuf,
    sys_root: PathBuf,
    max_cgroups: usize,
    previous: HashMap<u64, CgroupPrev>,
    /// Режим платформы, определённый на первом такте.
    disk_resolver: Option<DiskResolver>,
    /// Стабильное на время загрузки ядра соответствие major:minor → disk.
    disk_by_device: HashMap<String, String>,
}

/// Причина неполного обхода. `None` из `walk` означает полный результат.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraversalLimit {
    Depth,
    Groups,
}

impl TraversalLimit {
    fn into_error(self, collector: &CgroupCollector) -> CollectError {
        match self {
            Self::Depth => CollectError::Truncated {
                source_name: "cgroup",
                budget: "max_depth",
                limit: MAX_DEPTH,
            },
            Self::Groups => CollectError::Truncated {
                source_name: "cgroup",
                budget: "max_cgroups",
                limit: collector.max_cgroups,
            },
        }
    }
}

impl CgroupCollector {
    #[must_use]
    pub fn new(
        fs: Arc<dyn FsSource>,
        cgroup_root: PathBuf,
        sys_root: PathBuf,
        max_cgroups: usize,
    ) -> Self {
        CgroupCollector {
            fs,
            cgroup_root,
            sys_root,
            max_cgroups: max_cgroups.max(1),
            previous: HashMap::new(),
            disk_resolver: None,
            disk_by_device: HashMap::new(),
        }
    }

    fn read(&self, dir: &Path, file: &str) -> Option<String> {
        self.fs.read_opt(&dir.join(file))
    }

    /// Фиксирует способ резолва дисков на первом такте.
    fn disk_resolver(&mut self) -> DiskResolver {
        if let Some(mode) = self.disk_resolver {
            return mode;
        }
        let has_links = self
            .fs
            .read_dir(&self.sys_root.join("dev/block"))
            .is_ok_and(|entries| !entries.is_empty());
        let mode = if has_links {
            DiskResolver::SysDevBlock
        } else {
            DiskResolver::GraphLabels
        };
        tracing::debug!(?mode, "выбран способ сопоставления cgroup с дисками");
        self.disk_resolver = Some(mode);
        mode
    }

    /// Имя целого диска для `major:minor`, с кэшем на время загрузки ядра.
    fn whole_disk_for(&mut self, ctx: &CollectCtx<'_>, device: &str) -> Option<String> {
        if let Some(name) = self.disk_by_device.get(device) {
            return Some(name.clone());
        }

        let mode = self.disk_resolver();
        let from_sysfs = || -> Option<String> {
            let target = self
                .fs
                .read_link(&self.sys_root.join("dev/block").join(device))
                .ok()?;
            let last = target.file_name()?.to_string_lossy().into_owned();
            let parent = target
                .parent()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            match parent {
                // `.../block/sda/sda1` — раздел: владелец на уровень выше.
                Some(name) if name != "block" && !name.is_empty() => Some(name),
                _ => Some(last),
            }
        };
        let from_graph = || {
            ctx.entities_of_kind(EntityKind::Disk)
                .find(|entity| entity.labels.get("device") == Some(device))
                .map(|entity| entity.name.clone())
        };

        // Режим не переопределяется. Fallback для конкретного hotplug-устройства
        // допустим: он не проверяет платформу, а использует уже готовый граф.
        let name = match mode {
            DiskResolver::SysDevBlock => from_sysfs().or_else(from_graph),
            DiskResolver::GraphLabels => from_graph(),
        }?;
        let _ = self.disk_by_device.insert(device.to_string(), name.clone());
        Some(name)
    }

    /// Диски, которыми реально пользуется cgroup, по данным `io.stat`.
    fn disks_of(&mut self, ctx: &CollectCtx<'_>, io_stat: Option<&str>) -> Vec<String> {
        let Some(text) = io_stat else {
            return Vec::new();
        };
        let mut names: Vec<String> = parse::io_stat_devices(text)
            .iter()
            .filter_map(|device| self.whole_disk_for(ctx, device))
            .collect();
        names.sort_unstable();
        names.dedup();
        // Метка ограничена по длине, а десятки дисков у одной cgroup — редкость.
        names.truncate(MAX_LINKED_DISKS);
        names
    }

    /// Рекурсивный обход. Возвращает число обработанных групп.
    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        ctx: &mut CollectCtx<'_>,
        dir: &Path,
        rel_path: &str,
        parent: EntityId,
        depth: usize,
        processed: &mut usize,
        seen: &mut HashSet<u64>,
    ) -> Option<TraversalLimit> {
        if depth > MAX_DEPTH {
            return Some(TraversalLimit::Depth);
        }
        if *processed >= self.max_cgroups {
            return Some(TraversalLimit::Groups);
        }

        let Ok(inode) = self.fs.inode(dir) else {
            return None;
        };
        *processed += 1;
        let _ = seen.insert(inode);

        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".to_string());
        let display_path = if rel_path.is_empty() { "/" } else { rel_path };

        // `io.stat` читается до создания сущности: список дисков нужен как метка,
        // а метки задаются при upsert. Текст переиспользуется коллектором метрик,
        // второго чтения файла не происходит.
        let io_stat = self.read(dir, "io.stat");
        let disks = self.disks_of(ctx, io_stat.as_deref());

        let mut spec = EntitySpec::new(EntityKey::Cgroup { cgroup_id: inode }, name.clone())
            .parent(parent)
            .label("path", display_path);
        if !disks.is_empty() {
            // Метка нужна не для красоты: связи не переживают такт в истории, а
            // метки сохраняются в `EntityRecord`, и по ним A/B diff восстановит
            // «этот сервис лежит на этом диске» уже после смерти сущности.
            spec = spec.label("disks", disks.join(","));
        }
        let cgroup = ctx.upsert(spec);
        ctx.relate(parent, RelationKind::ParentOf, cgroup);

        for disk in &disks {
            if let Some(entity) = ctx
                .entity_by_key(&EntityKey::Disk {
                    name: disk.clone().into(),
                })
                .map(|e| e.id)
            {
                ctx.relate(cgroup, RelationKind::BackedBy, entity);
            }
        }

        let derived = self.collect_metrics(ctx, dir, cgroup, inode, io_stat.as_deref());
        let owner = self.attach_owner(ctx, &name, display_path, cgroup, derived, &disks);

        // Дети: подкаталоги. Владелец не меняет иерархию — родителем остаётся cgroup.
        let _ = owner;
        if let Ok(entries) = self.fs.read_dir(dir) {
            for entry in entries {
                let child_name = entry.to_string_lossy().into_owned();
                if child_name.starts_with('.')
                    || child_name.contains('.')
                        && !child_name.ends_with(".slice")
                        && !child_name.ends_with(".service")
                        && !child_name.ends_with(".scope")
                        && !child_name.ends_with(".socket")
                        && !child_name.ends_with(".mount")
                {
                    // Файлы контроллеров (`cpu.stat`, `memory.max`) — не каталоги.
                    continue;
                }
                let child_dir = dir.join(&child_name);
                if self.fs.inode(&child_dir).is_err() {
                    continue;
                }
                let child_rel = format!("{}/{}", rel_path.trim_end_matches('/'), child_name);
                if let Some(limit) = self.walk(
                    ctx,
                    &child_dir,
                    &child_rel,
                    cgroup,
                    depth + 1,
                    processed,
                    seen,
                ) {
                    // Бюджет глобальный для одного обхода: после его исчерпания
                    // нельзя продолжать проверять соседей и платить inode-read
                    // за каждый из них. Один признак поднимается до `collect`.
                    return Some(limit);
                }
            }
        }
        None
    }

    /// Собирает метрики одной cgroup. Возвращает производные величины,
    /// которые дублируются на сущность-владельца.
    fn collect_metrics(
        &mut self,
        ctx: &mut CollectCtx<'_>,
        dir: &Path,
        cgroup: EntityId,
        inode: u64,
        io_stat: Option<&str>,
    ) -> DerivedCgroup {
        let interval = ctx.interval_secs().max(0.001);
        let mut derived = DerivedCgroup::default();
        // Один снимок предыдущих значений на весь такт: и CPU, и io считают
        // производные от него, а обновление происходит один раз в конце.
        let prev = self.previous.get(&inode).copied();
        let mut cpu_totals: Option<(f64, f64, f64)> = None;
        let mut io_totals: Option<(f64, f64)> = None;

        if let Some(text) = self.read(dir, "cpu.stat") {
            let usage = parse::field(&text, "usage_usec").unwrap_or(0.0);
            let user = parse::field(&text, "user_usec").unwrap_or(0.0);
            let system = parse::field(&text, "system_usec").unwrap_or(0.0);
            let periods = parse::field(&text, "nr_periods").unwrap_or(0.0);
            let throttled = parse::field(&text, "nr_throttled").unwrap_or(0.0);
            let throttled_usec = parse::field(&text, "throttled_usec").unwrap_or(0.0);

            ctx.sample(cgroup, ids::CG_CPU_USAGE_USEC, usage);
            ctx.sample(cgroup, ids::CG_CPU_USER_USEC, user);
            ctx.sample(cgroup, ids::CG_CPU_SYSTEM_USEC, system);
            ctx.sample(cgroup, ids::CG_CPU_NR_PERIODS, periods);
            ctx.sample(cgroup, ids::CG_CPU_NR_THROTTLED, throttled);
            ctx.sample(cgroup, ids::CG_CPU_THROTTLED_USEC, throttled_usec);

            if let Some(previous) = prev {
                let d_usage = usage - previous.usage_usec;
                if d_usage >= 0.0 {
                    let cores = d_usage / 1_000_000.0 / interval;
                    ctx.sample(cgroup, ids::CG_CPU_CORES, cores);
                    derived.cores = Some(cores);
                }
                let d_periods = periods - previous.nr_periods;
                let d_throttled = throttled - previous.nr_throttled;
                if d_periods > 0.0 && d_throttled >= 0.0 {
                    let ratio = (d_throttled / d_periods).clamp(0.0, 1.0);
                    ctx.sample(cgroup, ids::CG_CPU_THROTTLE_RATIO, ratio);
                    derived.throttle_ratio = Some(ratio);
                }
            }
            cpu_totals = Some((usage, periods, throttled));
        }

        let limit_cores = self
            .read(dir, "cpu.max")
            .map(|text| parse::parse_cpu_max(&text))
            .unwrap_or(0.0);
        ctx.sample(cgroup, ids::CG_CPU_LIMIT_CORES, limit_cores);
        derived.limit_cores = Some(limit_cores);

        let current = self
            .read(dir, "memory.current")
            .map(|t| parse::parse_limit(&t));
        if let Some(current) = current {
            ctx.sample(cgroup, ids::CG_MEM_CURRENT, current);
            derived.memory_current = Some(current);
            let limit = self
                .read(dir, "memory.max")
                .map(|t| parse::parse_limit(&t))
                .unwrap_or(0.0);
            ctx.sample(cgroup, ids::CG_MEM_LIMIT, limit);
            if limit > 0.0 {
                let util = (current / limit).clamp(0.0, 1.0);
                ctx.sample(cgroup, ids::CG_MEM_UTIL, util);
                derived.memory_util = Some(util);
            }
        }

        if let Some(text) = self.read(dir, "memory.stat") {
            if let Some(anon) = parse::field(&text, "anon") {
                ctx.sample(cgroup, ids::CG_MEM_ANON, anon);
            }
            if let Some(file) = parse::field(&text, "file") {
                ctx.sample(cgroup, ids::CG_MEM_FILE, file);
            }
        }

        if let Some(text) = self.read(dir, "memory.swap.current") {
            ctx.sample(cgroup, ids::CG_MEM_SWAP_CURRENT, parse::parse_limit(&text));
        }

        if let Some(text) = self.read(dir, "memory.events") {
            for (key, metric) in [
                ("high", ids::CG_MEM_EVENTS_HIGH),
                ("max", ids::CG_MEM_EVENTS_MAX),
                ("oom", ids::CG_MEM_EVENTS_OOM),
                ("oom_kill", ids::CG_MEM_EVENTS_OOM_KILL),
            ] {
                if let Some(value) = parse::field(&text, key) {
                    ctx.sample(cgroup, metric, value);
                }
            }
        }

        if let Some(text) = io_stat {
            let io = parse::parse_io_stat(text);
            ctx.sample(cgroup, ids::CG_IO_READ_BYTES, io.rbytes);
            ctx.sample(cgroup, ids::CG_IO_WRITE_BYTES, io.wbytes);
            ctx.sample(cgroup, ids::CG_IO_READ_OPS, io.rios);
            ctx.sample(cgroup, ids::CG_IO_WRITE_OPS, io.wios);

            // Скорость считается как delta/interval, а не выводится из
            // размерности значения (раздел 154). Без предыдущего такта скорость
            // неизвестна, и метрика не публикуется: ноль означал бы «нагрузки
            // нет», чего мы на первом такте не знаем.
            if let Some(previous) = prev {
                let read = (io.rbytes - previous.io_rbytes) / interval;
                let write = (io.wbytes - previous.io_wbytes) / interval;
                // Отрицательная дельта - перезапуск счётчика ядром: значение
                // такого такта пропускается, а не публикуется как всплеск.
                if read >= 0.0 {
                    ctx.sample(cgroup, ids::CG_IO_READ_THROUGHPUT, read);
                }
                if write >= 0.0 {
                    ctx.sample(cgroup, ids::CG_IO_WRITE_THROUGHPUT, write);
                }
            }
            io_totals = Some((io.rbytes, io.wbytes));
        }

        for (file, avg_metric, total_metric, use_full) in [
            (
                "cpu.pressure",
                ids::CG_PSI_CPU_SOME_AVG10,
                ids::CG_PSI_CPU_SOME_TOTAL,
                false,
            ),
            (
                "memory.pressure",
                ids::CG_PSI_MEM_FULL_AVG10,
                ids::CG_PSI_MEM_FULL_TOTAL,
                true,
            ),
            (
                "io.pressure",
                ids::CG_PSI_IO_FULL_AVG10,
                ids::CG_PSI_IO_FULL_TOTAL,
                true,
            ),
        ] {
            let Some(text) = self.read(dir, file) else {
                continue;
            };
            let pressure = parse::parse_pressure(&text);
            let (avg, total) = if use_full {
                (pressure.full_avg10, pressure.full_total_secs)
            } else {
                (pressure.some_avg10, pressure.some_total_secs)
            };
            ctx.sample(cgroup, avg_metric, avg);
            ctx.sample(cgroup, total_metric, total);
            match file {
                "cpu.pressure" => derived.psi_cpu = Some(avg),
                "memory.pressure" => derived.psi_memory = Some(avg),
                "io.pressure" => derived.psi_io = Some(avg),
                _ => {}
            }
        }

        if let Some(text) = self.read(dir, "pids.current") {
            let current = parse::parse_limit(&text);
            ctx.sample(cgroup, ids::CG_PIDS_CURRENT, current);
            derived.pids_current = Some(current);
            let limit = self
                .read(dir, "pids.max")
                .map(|t| parse::parse_limit(&t))
                .unwrap_or(0.0);
            ctx.sample(cgroup, ids::CG_PIDS_MAX, limit);
            if limit > 0.0 {
                ctx.sample(cgroup, ids::CG_PIDS_UTIL, (current / limit).clamp(0.0, 1.0));
            }
        }

        // Одно обновление снимка на такт. Значения нечитаемых файлов
        // переносятся из предыдущего снимка: иначе пропуск одного чтения
        // обнулял бы базу и следующий такт показал бы всю историю как всплеск.
        let (usage_usec, nr_periods, nr_throttled) = cpu_totals.unwrap_or_else(|| {
            prev.map_or((0.0, 0.0, 0.0), |p| {
                (p.usage_usec, p.nr_periods, p.nr_throttled)
            })
        });
        let (io_rbytes, io_wbytes) =
            io_totals.unwrap_or_else(|| prev.map_or((0.0, 0.0), |p| (p.io_rbytes, p.io_wbytes)));
        let _ = self.previous.insert(
            inode,
            CgroupPrev {
                usage_usec,
                nr_periods,
                nr_throttled,
                io_rbytes,
                io_wbytes,
            },
        );

        derived
    }

    /// Создаёт сущность владельца и дублирует на неё производные метрики.
    ///
    /// Дублирование — сознательный компромисс: UI и правила получают владельца
    /// без обхода графа на каждый кадр, ценой нескольких лишних серий на
    /// владельца (десятки, не тысячи).
    fn attach_owner(
        &self,
        ctx: &mut CollectCtx<'_>,
        segment: &str,
        rel_path: &str,
        cgroup: EntityId,
        derived: DerivedCgroup,
        disks: &[String],
    ) -> Option<EntityId> {
        let owner_kind = owner_from_segment(segment)?;

        let owner = match &owner_kind {
            Owner::Unit(name) => {
                // Имя unit не уникально глобально: `dbus.socket` и
                // `session.scope` одновременно существуют в system- и
                // user-manager. Полный cgroup-путь стабилен между рестартами и
                // не склеивает разные экземпляры в одну сущность.
                let identity = rel_path.trim_start_matches('/').to_string();
                let spec = EntitySpec::new(
                    EntityKey::Unit {
                        name: identity.clone().into(),
                    },
                    name.clone(),
                )
                .parent(cgroup)
                .logical(identity)
                .label("path", rel_path);
                let spec = if disks.is_empty() {
                    spec
                } else {
                    // Владелец наследует зависимость от диска: оператор ищет
                    // «nginx.service», а не inode технической cgroup.
                    spec.label("disks", disks.join(","))
                };
                ctx.upsert(spec)
            }
            Owner::Container { runtime, id } => {
                let short: String = id.chars().take(12).collect();
                ctx.upsert(
                    EntitySpec::new(
                        EntityKey::Container {
                            runtime: *runtime,
                            id: id.clone().into(),
                        },
                        short,
                    )
                    .parent(cgroup)
                    .logical(id.clone())
                    .label("runtime", runtime.as_str())
                    .label("path", rel_path),
                )
            }
        };

        ctx.relate(cgroup, RelationKind::OwnedBy, owner);
        ctx.relate(owner, RelationKind::BackedBy, cgroup);

        // Pod: контейнер внутри kubepods принадлежит pod.
        if matches!(owner_kind, Owner::Container { .. }) {
            if let Some(uid) = pod_uid_from_path(rel_path) {
                let pod = ctx.upsert(
                    EntitySpec::new(
                        EntityKey::Pod {
                            namespace: "unknown".into(),
                            name: uid.clone().into(),
                        },
                        uid.clone(),
                    )
                    .parent(cgroup)
                    .logical(uid),
                );
                ctx.relate(owner, RelationKind::OwnedBy, pod);
            }
        }

        derived.publish(ctx, owner);
        Some(owner)
    }
}

/// Производные метрики cgroup, дублируемые на владельца.
#[derive(Clone, Copy, Debug, Default)]
struct DerivedCgroup {
    cores: Option<f64>,
    limit_cores: Option<f64>,
    throttle_ratio: Option<f64>,
    memory_current: Option<f64>,
    memory_util: Option<f64>,
    psi_cpu: Option<f64>,
    psi_memory: Option<f64>,
    psi_io: Option<f64>,
    pids_current: Option<f64>,
}

impl DerivedCgroup {
    fn publish(&self, ctx: &mut CollectCtx<'_>, entity: EntityId) {
        let pairs = [
            (self.cores, ids::CG_CPU_CORES),
            (self.limit_cores, ids::CG_CPU_LIMIT_CORES),
            (self.throttle_ratio, ids::CG_CPU_THROTTLE_RATIO),
            (self.memory_current, ids::CG_MEM_CURRENT),
            (self.memory_util, ids::CG_MEM_UTIL),
            (self.psi_cpu, ids::CG_PSI_CPU_SOME_AVG10),
            (self.psi_memory, ids::CG_PSI_MEM_FULL_AVG10),
            (self.psi_io, ids::CG_PSI_IO_FULL_AVG10),
            (self.pids_current, ids::CG_PIDS_CURRENT),
        ];
        for (value, metric) in pairs {
            if let Some(value) = value {
                ctx.sample(entity, metric, value);
            }
        }
    }
}

impl Collector for CgroupCollector {
    fn name(&self) -> &'static str {
        "cgroup"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        let root = self.cgroup_root.clone();
        if self.fs.inode(&root).is_err() {
            return Err(CollectError::Unavailable("cgroup v2 root"));
        }
        let host = ctx.host();
        // Если обход окажется неполным, непосещённые сущности нельзя объявлять
        // исчезнувшими: мы не наблюдали их отсутствие. Снимок старых id
        // снимается до обхода и используется только при усечении.
        let mut previously_known: Vec<EntityId> = Vec::new();
        for kind in [
            EntityKind::Cgroup,
            EntityKind::Unit,
            EntityKind::Container,
            EntityKind::Pod,
        ] {
            previously_known.extend(ctx.entities_of_kind(kind).map(|entity| entity.id));
        }
        let mut processed = 0usize;
        // Каталог cgroup исчезает вместе с unit или контейнером, а его inode
        // ядро переиспользует. Без чистки карта прошлых значений росла бы весь
        // срок жизни агента на хосте с churn контейнеров.
        let mut seen: HashSet<u64> = HashSet::with_capacity(self.previous.len().max(16));
        let truncated = self.walk(ctx, &root, "", host, 0, &mut processed, &mut seen);
        if let Some(limit) = truncated {
            // Непосещённое при неполном обходе — UNKNOWN, не «удалено». Touch
            // не подставляет старые метрики как свежие; он только не даёт
            // породить ложные Deleted/Reparented и потерять историю сущности.
            for id in previously_known {
                ctx.touch(id);
            }
            // Baseline счётчиков непосещённых cgroup тоже сохраняется: удалить
            // его сейчас означало бы ложный скачок rate при следующем полном
            // обходе.
            return Err(limit.into_error(self));
        }
        self.previous.retain(|inode, _| seen.contains(inode));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use pulse_core::entity::EntityKind;
    use pulse_core::sample::SeriesKey;
    use pulse_core::time::Timestamp;
    use pulse_core::{EntityGraph, TickBatch};

    /// Дерево: корень, system.slice/nginx.service, docker-контейнер в kubepods.
    fn tree(usage: u64, periods: u64, throttled: u64) -> FixtureFs {
        FixtureFs::new()
            .inode("/sys/fs/cgroup", 1)
            .file("/sys/fs/cgroup/cpu.stat", "usage_usec 1000\nnr_periods 0\nnr_throttled 0\n")
            .inode("/sys/fs/cgroup/system.slice", 2)
            .file(
                "/sys/fs/cgroup/system.slice/cpu.stat",
                "usage_usec 500\nnr_periods 0\nnr_throttled 0\n",
            )
            .inode("/sys/fs/cgroup/system.slice/nginx.service", 3)
            .file(
                "/sys/fs/cgroup/system.slice/nginx.service/cpu.stat",
                &format!(
                    "usage_usec {usage}\nuser_usec 100\nsystem_usec 50\nnr_periods {periods}\nnr_throttled {throttled}\nthrottled_usec 900\n"
                ),
            )
            .file("/sys/fs/cgroup/system.slice/nginx.service/cpu.max", "800000 100000\n")
            .file("/sys/fs/cgroup/system.slice/nginx.service/memory.current", "104857600\n")
            .file("/sys/fs/cgroup/system.slice/nginx.service/memory.max", "209715200\n")
            .file("/sys/fs/cgroup/system.slice/nginx.service/memory.stat", "anon 52428800\nfile 52428800\n")
            .file(
                "/sys/fs/cgroup/system.slice/nginx.service/memory.events",
                "low 0\nhigh 2\nmax 1\noom 0\noom_kill 0\n",
            )
            .file(
                "/sys/fs/cgroup/system.slice/nginx.service/io.stat",
                "8:0 rbytes=1000 wbytes=2000 rios=10 wios=20\n",
            )
            .file(
                "/sys/fs/cgroup/system.slice/nginx.service/io.pressure",
                "some avg10=20.00 avg60=10.00 avg300=1.00 total=100\nfull avg10=8.00 avg60=4.00 avg300=1.00 total=50\n",
            )
            .file("/sys/fs/cgroup/system.slice/nginx.service/pids.current", "42\n")
            .file("/sys/fs/cgroup/system.slice/nginx.service/pids.max", "100\n")
            .inode("/sys/fs/cgroup/kubepods.slice", 4)
            .inode("/sys/fs/cgroup/kubepods.slice/pod3f2b8c1d9e4a.slice", 5)
            .inode(
                "/sys/fs/cgroup/kubepods.slice/pod3f2b8c1d9e4a.slice/docker-abcdef0123456789abcdef0123456789.scope",
                6,
            )
            .file(
                "/sys/fs/cgroup/kubepods.slice/pod3f2b8c1d9e4a.slice/docker-abcdef0123456789abcdef0123456789.scope/cpu.stat",
                "usage_usec 2000\nnr_periods 10\nnr_throttled 3\n",
            )
            .file(
                "/sys/fs/cgroup/kubepods.slice/pod3f2b8c1d9e4a.slice/docker-abcdef0123456789abcdef0123456789.scope/memory.current",
                "52428800\n",
            )
    }

    fn run(collector: &mut CgroupCollector, graph: &mut EntityGraph, at: u64) -> TickBatch {
        graph.begin_tick(Timestamp::from_millis(at));
        {
            let mut ctx = CollectCtx::new(graph, 1.0);
            collector.collect(&mut ctx).expect("сбор cgroup");
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

    #[test]
    fn systemd_unit_is_recognised_from_path() {
        assert_eq!(
            owner_from_segment("nginx.service"),
            Some(Owner::Unit("nginx.service".to_string()))
        );
        assert_eq!(
            owner_from_segment("session-3.scope"),
            Some(Owner::Unit("session-3.scope".to_string()))
        );
        assert_eq!(owner_from_segment("system.slice"), None);
    }

    #[test]
    fn container_scopes_are_recognised_before_units() {
        let docker = owner_from_segment("docker-abcdef0123456789abcdef0123456789.scope");
        assert_eq!(
            docker,
            Some(Owner::Container {
                runtime: Runtime::Docker,
                id: "abcdef0123456789abcdef0123456789".to_string()
            })
        );
        let cri = owner_from_segment("cri-containerd-0123456789abcdef.scope");
        assert!(matches!(
            cri,
            Some(Owner::Container {
                runtime: Runtime::Containerd,
                ..
            })
        ));
        let crio = owner_from_segment("crio-fedcba9876543210.scope");
        assert!(matches!(
            crio,
            Some(Owner::Container {
                runtime: Runtime::CriO,
                ..
            })
        ));
    }

    #[test]
    fn cgroupfs_driver_bare_id_is_container() {
        let bare = owner_from_segment("abcdef0123456789abcdef0123456789abcdef01");
        assert!(matches!(bare, Some(Owner::Container { .. })));
    }

    #[test]
    fn pod_uid_is_extracted_from_kubepods_path() {
        assert_eq!(
            pod_uid_from_path("/kubepods.slice/pod3f2b8c1d9e4a.slice/docker-x.scope"),
            Some("3f2b8c1d9e4a".to_string())
        );
        assert!(pod_uid_from_path("/system.slice/nginx.service").is_none());
    }

    #[test]
    fn unit_entity_gets_duplicated_derived_metrics() {
        let fs = Arc::new(tree(1_000_000, 100, 10));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);

        // Второй такт с ростом счётчиков даёт производные.
        let fs2 = Arc::new(tree(3_000_000, 200, 40));
        collector.fs = fs2;
        let batch = run(&mut collector, &mut graph, 3_000);

        let unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .map(|e| e.id)
            .expect("unit должен быть создан");

        let cores = value(&batch, SeriesKey::new(unit, ids::CG_CPU_CORES)).expect("ядра на unit");
        assert!((cores - 2.0).abs() < 1e-9, "cores = {cores}");
        let throttle =
            value(&batch, SeriesKey::new(unit, ids::CG_CPU_THROTTLE_RATIO)).expect("throttle");
        assert!((throttle - 0.3).abs() < 1e-9, "throttle = {throttle}");
        let mem_util = value(&batch, SeriesKey::new(unit, ids::CG_MEM_UTIL)).expect("память");
        assert!((mem_util - 0.5).abs() < 1e-9);
        let limit = value(&batch, SeriesKey::new(unit, ids::CG_CPU_LIMIT_CORES)).expect("лимит");
        assert!((limit - 8.0).abs() < 1e-9);
    }

    #[test]
    fn container_and_pod_are_linked() {
        let fs = Arc::new(tree(1_000, 10, 1));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);

        let container = graph
            .entities_of_kind(EntityKind::Container)
            .next()
            .map(|e| e.id)
            .expect("контейнер должен быть создан");
        let pod = graph
            .entities_of_kind(EntityKind::Pod)
            .next()
            .map(|e| e.id)
            .expect("pod должен быть создан");
        assert!(
            graph
                .relations_of(container)
                .any(|r| r.kind == RelationKind::OwnedBy && r.to == pod),
            "контейнер обязан принадлежать pod"
        );
    }

    #[test]
    fn unit_identity_survives_cgroup_rename_but_restart_creates_new_cgroup() {
        let fs = Arc::new(tree(1_000, 0, 0));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let first_unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .map(|e| e.id);

        // Перезапуск unit: тот же путь, новый inode каталога.
        let mut renamed = tree(1_000, 0, 0);
        renamed.remove("/sys/fs/cgroup/system.slice/nginx.service");
        let fs2 = Arc::new(
            renamed
                .inode("/sys/fs/cgroup/system.slice/nginx.service", 99)
                .file(
                    "/sys/fs/cgroup/system.slice/nginx.service/cpu.stat",
                    "usage_usec 10\nnr_periods 0\nnr_throttled 0\n",
                ),
        );
        collector.fs = fs2;
        let _ = run(&mut collector, &mut graph, 3_000);

        let second_unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .map(|e| e.id);
        assert_eq!(
            first_unit, second_unit,
            "логическая сущность unit обязана сохраниться"
        );
        let cgroup_inodes: Vec<u64> = graph
            .entities_of_kind(EntityKind::Cgroup)
            .filter_map(|e| match &e.key {
                EntityKey::Cgroup { cgroup_id } => Some(*cgroup_id),
                _ => None,
            })
            .collect();
        assert!(
            cgroup_inodes.contains(&99),
            "новый inode должен дать новую cgroup-сущность: {cgroup_inodes:?}"
        );
    }
    #[test]
    fn cgroup_is_linked_to_the_disk_it_actually_uses() {
        // io.stat ссылается на раздел 8:1; сущностью графа является целый диск.
        let fs = Arc::new(
            tree(1_000, 0, 0)
                .file(
                    "/sys/fs/cgroup/system.slice/nginx.service/io.stat",
                    "8:1 rbytes=4096 wbytes=8192 rios=2 wios=3\n253:9 rbytes=0 wbytes=0\n",
                )
                .link(
                    "/sys/dev/block/8:1",
                    "../../devices/pci0000:00/block/sda/sda1",
                )
                .link("/sys/dev/block/253:9", "../../devices/virtual/block/dm-9"),
        );
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));

        // Диск обязан существовать до обхода cgroup: в конвейере disk-коллектор
        // идёт раньше (см. build_collectors).
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let disk = graph
            .upsert(EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор cgroup");
        }
        let _ = graph.end_tick();

        let unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .expect("unit создан");
        assert_eq!(
            unit.labels.get("disks"),
            Some("sda"),
            "раздел обязан подняться до целого диска, dm-9 без трафика не попадает"
        );

        let cgroup = graph
            .entities_of_kind(EntityKind::Cgroup)
            .find(|e| e.name == "nginx.service")
            .expect("cgroup создан");
        assert!(
            graph
                .relations_of(cgroup.id)
                .any(|r| r.kind == RelationKind::BackedBy && r.to == disk),
            "должно появиться ребро cgroup -> disk"
        );
    }

    #[test]
    fn cgroup_without_io_traffic_has_no_disk_dependency() {
        let fs = Arc::new(tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/nginx.service/io.stat",
            "8:0 rbytes=0 wbytes=0 rios=0 wios=0\n",
        ));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);

        let unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .expect("unit создан");
        assert_eq!(
            unit.labels.get("disks"),
            None,
            "без трафика зависимость выдумывать нельзя"
        );
    }

    #[test]
    fn vanished_cgroups_are_dropped_from_previous_values() {
        // Рестарт unit/контейнера пересоздаёт каталог с новым inode. Карта
        // прошлых значений обязана сжиматься до реально существующих групп.
        let mut collector = CgroupCollector::new(
            Arc::new(tree(1_000, 10, 1)),
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let first_len = collector.previous.len();
        assert!(first_len >= 4, "обход должен увидеть дерево: {first_len}");

        // Остались только корень и system.slice; nginx.service и kubepods удалены.
        collector.fs = Arc::new(
            FixtureFs::new()
                .inode("/sys/fs/cgroup", 1)
                .file(
                    "/sys/fs/cgroup/cpu.stat",
                    "usage_usec 5000\nnr_periods 0\nnr_throttled 0\n",
                )
                .inode("/sys/fs/cgroup/system.slice", 2)
                .file(
                    "/sys/fs/cgroup/system.slice/cpu.stat",
                    "usage_usec 2000\nnr_periods 0\nnr_throttled 0\n",
                ),
        );
        let _ = run(&mut collector, &mut graph, 3_000);

        assert_eq!(
            collector.previous.len(),
            2,
            "мёртвые группы обязаны выпасть: {:?}",
            collector.previous.keys().collect::<Vec<_>>()
        );
        assert!(!collector.previous.contains_key(&3), "inode nginx.service");
    }

    #[test]
    fn equal_unit_basenames_in_different_managers_do_not_reparent() {
        let fs = Arc::new(
            FixtureFs::new()
                .inode("/sys/fs/cgroup", 1)
                .inode("/sys/fs/cgroup/user.slice", 2)
                .inode("/sys/fs/cgroup/user.slice/user-1000.slice", 3)
                .inode(
                    "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service",
                    4,
                )
                .inode(
                    "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/dbus.socket",
                    5,
                )
                .inode("/sys/fs/cgroup/user.slice/user-1001.slice", 6)
                .inode(
                    "/sys/fs/cgroup/user.slice/user-1001.slice/user@1001.service",
                    7,
                )
                .inode(
                    "/sys/fs/cgroup/user.slice/user-1001.slice/user@1001.service/dbus.socket",
                    8,
                ),
        );
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let batch = run(&mut collector, &mut graph, 2_000);

        let dbus_units = graph
            .entities_of_kind(EntityKind::Unit)
            .filter(|entity| entity.name == "dbus.socket")
            .count();
        assert_eq!(dbus_units, 2, "unit разных user-manager нельзя склеивать");
        assert!(
            batch
                .events
                .iter()
                .all(|event| event.kind != pulse_core::EventKind::Reparented),
            "первый обход не должен порождать ложный reparent: {:?}",
            batch.events
        );
    }

    #[test]
    fn cgroup_limit_stops_the_walk_and_reports_truncation() {
        let fs = Arc::new(tree(1_000, 0, 0));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            2,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let result = {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx)
        };
        let _ = graph.end_tick();

        assert!(matches!(
            result,
            Err(CollectError::Truncated {
                source_name: "cgroup",
                budget: "max_cgroups",
                limit: 2
            })
        ));
        let count = graph.entities_of_kind(EntityKind::Cgroup).count();
        assert!(count <= 2, "обработано {count} групп при лимите 2");
    }

    /// После достижения глобального лимита DFS обязан прекратиться целиком,
    /// а не продолжать inode-проверку каждого оставшегося соседа.
    #[test]
    fn cgroup_limit_stops_issuing_inode_reads() {
        let mut fs = FixtureFs::new().inode("/sys/fs/cgroup", 1);
        for index in 0..100_u64 {
            let path = format!("/sys/fs/cgroup/group-{index}.slice");
            fs = fs.inode(&path, index + 2);
        }
        let counting = crate::test_support::CountingFs::new(Arc::new(fs));
        let mut collector = CgroupCollector::new(
            Arc::new(counting.clone()),
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            2,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let result = {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx)
        };

        assert!(matches!(result, Err(CollectError::Truncated { .. })));
        assert!(
            counting.counts().inode <= 5,
            "после лимита обход обязан остановиться, вызовы: {:?}",
            counting.calls()
        );
    }

    /// Повторный неполный обход не является доказательством удаления.
    /// Без touch старые сущности пережили бы один grace-такт, а на втором
    /// получили ложный `Deleted`; baseline rate тоже был бы забыт.
    #[test]
    fn repeated_truncation_preserves_unvisited_entities_and_baselines() {
        let fs = Arc::new(tree(1_000, 10, 3));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let _ = run(&mut collector, &mut graph, 2_000);
        let entities_before = graph.entities().count();
        let baselines_before = collector.previous.len();
        assert!(
            entities_before > 3,
            "фикстура обязана содержать несколько cgroup"
        );
        assert!(
            baselines_before > 2,
            "фикстура обязана создать baseline счётчиков"
        );

        collector.max_cgroups = 2;
        for at in [3_000, 4_000] {
            graph.begin_tick(Timestamp::from_millis(at));
            let result = {
                let mut ctx = CollectCtx::new(&mut graph, 1.0);
                collector.collect(&mut ctx)
            };
            assert!(matches!(result, Err(CollectError::Truncated { .. })));
            let batch = graph.end_tick();
            assert!(
                batch
                    .events
                    .iter()
                    .all(|event| event.kind != pulse_core::EventKind::Deleted),
                "неполный обход не доказывает удаление: {:?}",
                batch.events
            );
        }

        assert_eq!(
            graph.entities().count(),
            entities_before,
            "непосещённые сущности обязаны пережить повторное усечение"
        );
        assert_eq!(
            collector.previous.len(),
            baselines_before,
            "baseline непосещённых cgroup нельзя забывать"
        );
    }

    #[test]
    fn depth_limit_reports_truncation() {
        let mut fs = FixtureFs::new();
        let mut path = PathBuf::from("/sys/fs/cgroup");
        fs = fs.inode("/sys/fs/cgroup", 1);
        for depth in 0..=MAX_DEPTH {
            path.push(format!("level-{depth}.slice"));
            fs = fs.inode(
                &path.to_string_lossy(),
                u64::try_from(depth).unwrap_or(0) + 2,
            );
        }
        let mut collector = CgroupCollector::new(
            Arc::new(fs),
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let result = {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx)
        };

        assert!(matches!(
            result,
            Err(CollectError::Truncated {
                source_name: "cgroup",
                budget: "max_depth",
                limit: MAX_DEPTH
            })
        ));
    }

    #[test]
    fn missing_cgroup_root_is_unavailable() {
        let fs = Arc::new(FixtureFs::new());
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            10,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let mut ctx = CollectCtx::new(&mut graph, 1.0);
        assert!(collector.collect(&mut ctx).is_err());
    }

    #[test]
    fn unlimited_cpu_max_yields_zero_limit() {
        let fs = Arc::new(tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/nginx.service/cpu.max",
            "max 100000\n",
        ));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let batch = run(&mut collector, &mut graph, 2_000);
        let unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .map(|e| e.id)
            .expect("unit");
        assert_eq!(
            value(&batch, SeriesKey::new(unit, ids::CG_CPU_LIMIT_CORES)),
            Some(0.0)
        );
    }

    #[test]
    fn io_pressure_uses_full_not_some() {
        let fs = Arc::new(tree(1_000, 0, 0));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let batch = run(&mut collector, &mut graph, 2_000);
        let unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|e| e.name == "nginx.service")
            .map(|e| e.id)
            .expect("unit");
        let io = value(&batch, SeriesKey::new(unit, ids::CG_PSI_IO_FULL_AVG10)).expect("io psi");
        assert!(
            (io - 0.08).abs() < 1e-9,
            "должно быть full=8%, получено {io}"
        );
    }
    #[test]
    fn graph_label_fallback_links_disks_without_sys_dev_block() {
        // WSL2: cgroup v2 и io.stat есть, но /sys/dev/block может быть пуст.
        let fs = Arc::new(tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/nginx.service/io.stat",
            "8:0 rbytes=4096 wbytes=0 rios=1 wios=0\n",
        ));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let disk = graph.upsert(
            EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda")
                .parent(host)
                .label("device", "8:0"),
        );
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор cgroup");
        }
        let _ = graph.end_tick();

        assert_eq!(collector.disk_resolver, Some(DiskResolver::GraphLabels));
        assert_eq!(
            collector.disk_by_device.get("8:0").map(String::as_str),
            Some("sda"),
            "fallback обязан кэшировать то же major:minor из diskstats"
        );
        let cgroup = graph
            .entities_of_kind(EntityKind::Cgroup)
            .find(|entity| entity.name == "nginx.service")
            .expect("cgroup создан");
        assert!(
            graph
                .relations_of(cgroup.id)
                .any(|relation| relation.kind == RelationKind::BackedBy && relation.to == disk),
            "fallback обязан создать то же ребро, что sysfs-путь"
        );
    }

    /// Известное ограничение: на хосте без `/sys/dev/block` раздел в `io.stat`
    /// не поднимается до целого диска. Сущности разделов не существует (их
    /// отсеивает disk-коллектор), а угадывать диск по имени раздела нельзя:
    /// у `nvme0n1p1` и `sda1` правила разные. Зависимость не строится молча —
    /// это фиксируется тестом, а не остаётся сюрпризом.
    #[test]
    fn partition_without_sysfs_link_yields_no_dependency() {
        let fs = Arc::new(tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/nginx.service/io.stat",
            "8:1 rbytes=4096 wbytes=0 rios=1 wios=0\n",
        ));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        // Диск существует только как целое устройство 8:0.
        let disk = graph.upsert(
            EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda")
                .parent(host)
                .label("device", "8:0"),
        );
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор cgroup");
        }
        let _ = graph.end_tick();

        let unit = graph
            .entities_of_kind(EntityKind::Unit)
            .find(|entity| entity.name == "nginx.service")
            .expect("unit создан");
        assert_eq!(
            unit.labels.get("disks"),
            None,
            "выдуманной связи быть не должно"
        );

        let cgroup = graph
            .entities_of_kind(EntityKind::Cgroup)
            .find(|entity| entity.name == "nginx.service")
            .expect("cgroup создан");
        let linked = graph
            .relations_of(cgroup.id)
            .any(|relation| relation.kind == RelationKind::BackedBy && relation.to == disk);
        assert!(!linked, "раздел без sysfs-ссылки не даёт ребро");
    }

    /// Раздел 154: скорость обязана считаться как delta/interval, а не
    /// выводиться из размерности накопительного счётчика.
    #[test]
    fn io_throughput_is_a_real_rate_not_a_counter() {
        let tree_first = tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/io.stat",
            "8:0 rbytes=1000 wbytes=500 rios=1 wios=1\n",
        );
        let mut collector = CgroupCollector::new(
            Arc::new(tree_first),
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(0));

        // Первый такт: предыдущего значения нет, скорость неизвестна.
        let batch = run(&mut collector, &mut graph, 1_000);
        let key_read = |graph: &EntityGraph| {
            graph
                .entities_of_kind(EntityKind::Cgroup)
                .find(|e| e.name == "system.slice")
                .map(|e| e.id)
        };
        let cgroup = key_read(&graph).expect("cgroup создан");
        let rate_first = value(&batch, SeriesKey::new(cgroup, ids::CG_IO_READ_THROUGHPUT));
        assert_eq!(
            rate_first, None,
            "на первом такте скорость неизвестна и не публикуется"
        );

        // Второй такт: +2000 байт при интервале сбора 1 с (его задаёт
        // тестовый помощник) - ровно 2000 байт/с. Сырой счётчик дал бы 3000,
        // поэтому значение доказывает именно расчёт delta/interval.
        collector.fs = Arc::new(tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/io.stat",
            "8:0 rbytes=3000 wbytes=500 rios=2 wios=1\n",
        ));
        let batch = run(&mut collector, &mut graph, 3_000);
        let cgroup = key_read(&graph).expect("cgroup жив");
        let rate = value(&batch, SeriesKey::new(cgroup, ids::CG_IO_READ_THROUGHPUT))
            .expect("скорость посчитана");
        assert!(
            (rate - 2_000.0).abs() < 1e-6,
            "ожидалось 2000 байт/с, получено {rate}"
        );

        // Накопительный счётчик при этом остаётся накопительным.
        let total = value(&batch, SeriesKey::new(cgroup, ids::CG_IO_READ_BYTES))
            .expect("счётчик публикуется");
        assert!((total - 3_000.0).abs() < 1e-6, "счётчик сырой: {total}");
    }

    /// Сброс счётчика (перезапуск контейнера, пересоздание cgroup) не имеет
    /// права превратиться в отрицательную скорость: панель хоста в инциденте
    /// показывала `-301 MiB/s`, и это ровно тот класс лжи, который здесь
    /// запрещён.
    #[test]
    fn counter_reset_never_produces_negative_io_rate() {
        let mut collector = CgroupCollector::new(
            Arc::new(tree(1_000, 0, 0).file(
                "/sys/fs/cgroup/system.slice/io.stat",
                "8:0 rbytes=900000 wbytes=800000 rios=9 wios=8\n",
            )),
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(0));
        let _ = run(&mut collector, &mut graph, 1_000);

        // Счётчик уехал вниз: устройство пересоздано.
        collector.fs = Arc::new(tree(1_000, 0, 0).file(
            "/sys/fs/cgroup/system.slice/io.stat",
            "8:0 rbytes=100 wbytes=50 rios=1 wios=1\n",
        ));
        let batch = run(&mut collector, &mut graph, 3_000);
        let cgroup = graph
            .entities_of_kind(EntityKind::Cgroup)
            .find(|e| e.name == "system.slice")
            .map(|e| e.id)
            .expect("cgroup жив");

        for metric in [ids::CG_IO_READ_THROUGHPUT, ids::CG_IO_WRITE_THROUGHPUT] {
            let sampled = value(&batch, SeriesKey::new(cgroup, metric));
            assert!(
                sampled.is_none(),
                "при сбросе счётчика скорость публиковать нельзя, получено {sampled:?}"
            );
        }
        // И ни одно значение такта не является отрицательным.
        assert!(
            batch.samples.iter().all(|sample| sample.value >= 0.0),
            "отрицательных значений быть не может"
        );
    }

    #[test]
    fn disk_resolver_mode_is_detected_once_per_process() {
        let fs = Arc::new(tree(1_000, 0, 0));
        let mut collector = CgroupCollector::new(
            fs,
            PathBuf::from("/sys/fs/cgroup"),
            PathBuf::from("/sys"),
            100,
        );
        assert_eq!(collector.disk_resolver(), DiskResolver::GraphLabels);

        // Даже если sysfs появился позже, режим не переопределяется каждый
        // такт. Новый hotplug-диск всё равно разрешается fallback-ом по графу.
        collector.fs =
            Arc::new(tree(1_000, 0, 0).link("/sys/dev/block/8:0", "../../devices/pci/block/sda"));
        assert_eq!(collector.disk_resolver(), DiskResolver::GraphLabels);
    }
}
