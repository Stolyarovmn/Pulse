//! Коллектор блочных устройств.
//!
//! Диски — не просто «ещё одни метрики»: в графе сущностей они узел, к которому
//! привязывается диагноз «задержка выросла», а через владельца cgroup —
//! кандидат в ресурсную зависимость при ранжировании доказательств.
//!
//! Производные величины считаются ровно так, как их считает `iostat`:
//! `await = Δ(время ожидания) / Δ(операции)`, `util = Δ(время занятости) / интервал`,
//! `queue = Δ(взвешенное время) / интервал`. Все три требуют базы предыдущего
//! такта, поэтому на первом такте не публикуются.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use pulse_core::entity::{EntityKey, EntitySpec};
use pulse_core::graph::{CollectCtx, CollectError, Collector};
use pulse_core::metric::ids;
use pulse_core::relation::RelationKind;

use crate::fs::{FsSource, FsSourceExt};
use crate::parse::{self, DiskStat};

/// Размер сектора в байтах: ядро всегда отдаёт diskstats в 512-байтных секторах,
/// независимо от физического размера сектора устройства.
const SECTOR_BYTES: f64 = 512.0;

/// Коллектор дисков.
#[derive(Debug)]
pub struct DiskCollector {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
    sys_root: PathBuf,
    previous: HashMap<String, DiskStat>,
    /// Предыдущий агрегат по хосту: `(read, write)` накопленных байт.
    previous_host: Option<(f64, f64)>,
}

impl DiskCollector {
    #[must_use]
    pub fn new(fs: Arc<dyn FsSource>, proc_root: PathBuf, sys_root: PathBuf) -> Self {
        DiskCollector {
            fs,
            proc_root,
            sys_root,
            previous: HashMap::new(),
            previous_host: None,
        }
    }

    /// Является ли устройство «целым диском», а не разделом или виртуальным слоем.
    ///
    /// Основной признак — наличие каталога `/sys/block/<name>`: у разделов там
    /// нет собственной записи. Если sysfs недоступен (контейнер без него),
    /// используется эвристика по имени.
    fn is_whole_disk(&self, name: &str) -> bool {
        for prefix in ["loop", "ram", "zram", "dm-", "md", "sr", "fd"] {
            if name.starts_with(prefix) {
                return false;
            }
        }
        let sys_path = self.sys_root.join("block").join(name);
        if self.fs.read_dir(&sys_path).is_ok() || self.fs.inode(&sys_path).is_ok() {
            return true;
        }
        // Эвристика: раздел заканчивается цифрой у nvme/mmc — через `p`, у sd/vd — просто цифрой.
        if name.contains("nvme") || name.contains("mmcblk") {
            return !name.contains('p');
        }
        !name.chars().last().is_some_and(|c| c.is_ascii_digit())
    }
}

impl Collector for DiskCollector {
    fn name(&self) -> &'static str {
        "disk"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        let path = self.proc_root.join("diskstats");
        let Some(text) = self.fs.read_opt(&path) else {
            return Err(CollectError::Unavailable("diskstats"));
        };

        let host = ctx.host();
        let interval = ctx.interval_secs().max(0.001);
        let mut host_read = 0.0;
        let mut host_write = 0.0;

        for stat in parse::parse_diskstats(&text) {
            if !self.is_whole_disk(&stat.name) {
                continue;
            }

            let entity = ctx.upsert(
                EntitySpec::new(
                    EntityKey::Disk {
                        name: stat.name.clone().into(),
                    },
                    stat.name.clone(),
                )
                .parent(host)
                // Fallback для сред без `/sys/dev/block` (в частности WSL2):
                // cgroup/io.stat ссылается на этот же major:minor.
                .label("device", stat.device.clone()),
            );
            ctx.relate(host, RelationKind::ParentOf, entity);

            let read_bytes = stat.read_sectors * SECTOR_BYTES;
            let write_bytes = stat.write_sectors * SECTOR_BYTES;
            host_read += read_bytes;
            host_write += write_bytes;

            ctx.sample(entity, ids::DISK_READ_BYTES, read_bytes);
            ctx.sample(entity, ids::DISK_WRITE_BYTES, write_bytes);
            ctx.sample(entity, ids::DISK_READ_OPS, stat.read_ops);
            ctx.sample(entity, ids::DISK_WRITE_OPS, stat.write_ops);
            ctx.sample(entity, ids::DISK_READ_WAIT, stat.read_wait_ms);
            ctx.sample(entity, ids::DISK_WRITE_WAIT, stat.write_wait_ms);
            ctx.sample(entity, ids::DISK_IO_TIME, stat.io_time_ms);
            ctx.sample(entity, ids::DISK_INFLIGHT, stat.inflight);

            if let Some(previous) = self.previous.get(&stat.name) {
                let d_read_ops = stat.read_ops - previous.read_ops;
                let d_write_ops = stat.write_ops - previous.write_ops;
                let d_ops = d_read_ops + d_write_ops;
                let d_wait = (stat.read_wait_ms - previous.read_wait_ms)
                    + (stat.write_wait_ms - previous.write_wait_ms);
                let d_io_time = stat.io_time_ms - previous.io_time_ms;
                let d_weighted = stat.weighted_io_time_ms - previous.weighted_io_time_ms;
                let d_read_bytes = read_bytes - previous.read_sectors * SECTOR_BYTES;
                let d_write_bytes = write_bytes - previous.write_sectors * SECTOR_BYTES;

                // Сброс счётчиков (переподключение устройства) — производные пропускаем.
                if d_ops >= 0.0 && d_wait >= 0.0 && d_io_time >= 0.0 {
                    let await_ms = if d_ops > 0.0 { d_wait / d_ops } else { 0.0 };
                    ctx.sample(entity, ids::DISK_AWAIT, await_ms);
                    ctx.sample(
                        entity,
                        ids::DISK_UTIL,
                        (d_io_time / (interval * 1000.0)).clamp(0.0, 1.0),
                    );
                    ctx.sample(
                        entity,
                        ids::DISK_QUEUE,
                        (d_weighted / (interval * 1000.0)).max(0.0),
                    );
                }
                if d_read_bytes >= 0.0 {
                    ctx.sample(entity, ids::DISK_READ_THROUGHPUT, d_read_bytes / interval);
                }
                if d_write_bytes >= 0.0 {
                    ctx.sample(entity, ids::DISK_WRITE_THROUGHPUT, d_write_bytes / interval);
                }
            }

            let _ = self.previous.insert(stat.name.clone(), stat);
        }

        ctx.sample(host, ids::HOST_DISK_READ_BYTES, host_read);
        ctx.sample(host, ids::HOST_DISK_WRITE_BYTES, host_write);

        // Скорость по хосту: тот же контракт, что у сети. Без неё Overview
        // не может показать дисковый поток, не выдавая накопительный счётчик
        // за скорость.
        if let Some((previous_read, previous_write)) = self.previous_host {
            let d_read = host_read - previous_read;
            let d_write = host_write - previous_write;
            if d_read >= 0.0 {
                ctx.sample(host, ids::HOST_DISK_READ_THROUGHPUT, d_read / interval);
            }
            if d_write >= 0.0 {
                ctx.sample(host, ids::HOST_DISK_WRITE_THROUGHPUT, d_write / interval);
            }
        }
        self.previous_host = Some((host_read, host_write));
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

    fn diskstats(io_time: u64, wait: u64, ops: u64, sectors: u64) -> String {
        format!(
            "   8       0 sda {ops} 0 {sectors} {wait} 0 0 0 0 0 {io_time} {io_time} 0 0 0 0\n\
             253       0 dm-0 5 0 10 5 0 0 0 0 0 5 5 0 0 0 0\n\
               7       0 loop0 1 0 2 1 0 0 0 0 0 1 1 0 0 0 0\n\
               8       1 sda1 500 0 1000 250 0 0 0 0 0 100 100 0 0 0 0\n"
        )
    }

    fn value(batch: &TickBatch, key: SeriesKey) -> Option<f64> {
        batch
            .samples
            .iter()
            .find(|s| s.series == key)
            .map(|s| s.value)
    }

    fn disk_entity(graph: &EntityGraph) -> pulse_core::EntityId {
        graph
            .entities_of_kind(EntityKind::Disk)
            .next()
            .map(|e| e.id)
            .unwrap_or(pulse_core::EntityId::NONE)
    }

    #[test]
    fn only_whole_disks_become_entities() {
        let fs = Arc::new(
            FixtureFs::new()
                .file("/proc/diskstats", &diskstats(100, 50, 10, 20))
                .inode("/sys/block/sda", 1),
        );
        let mut collector = DiskCollector::new(fs, PathBuf::from("/proc"), PathBuf::from("/sys"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор дисков");
        }
        let _ = graph.end_tick();

        let names: Vec<String> = graph
            .entities_of_kind(EntityKind::Disk)
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, vec!["sda".to_string()], "получено: {names:?}");
    }

    #[test]
    fn derived_values_match_iostat_formulas() {
        let mut fixture = FixtureFs::new()
            .file("/proc/diskstats", &diskstats(1_000, 2_000, 100, 400))
            .inode("/sys/block/sda", 1);
        let first = Arc::new(
            FixtureFs::new()
                .file("/proc/diskstats", &diskstats(1_000, 2_000, 100, 400))
                .inode("/sys/block/sda", 1),
        );

        let mut collector =
            DiskCollector::new(first, PathBuf::from("/proc"), PathBuf::from("/sys"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let _ = graph.end_tick();

        // Такт 2: +50 операций, +1500 мс ожидания, +300 мс занятости, +100 секторов.
        fixture.set("/proc/diskstats", &diskstats(1_300, 3_500, 150, 500));
        collector.fs = Arc::new(fixture);

        graph.begin_tick(Timestamp::from_millis(3_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 2");
        }
        let batch = graph.end_tick();

        let disk = disk_entity(&graph);
        let await_ms = value(&batch, SeriesKey::new(disk, ids::DISK_AWAIT)).unwrap_or_default();
        let util = value(&batch, SeriesKey::new(disk, ids::DISK_UTIL)).unwrap_or_default();
        let throughput =
            value(&batch, SeriesKey::new(disk, ids::DISK_READ_THROUGHPUT)).unwrap_or_default();

        assert!((await_ms - 30.0).abs() < 1e-9, "await = {await_ms}");
        assert!((util - 0.3).abs() < 1e-9, "util = {util}");
        assert!(
            (throughput - 100.0 * SECTOR_BYTES).abs() < 1e-9,
            "throughput = {throughput}"
        );
    }

    #[test]
    fn zero_operations_give_zero_await_not_division_by_zero() {
        let stats = diskstats(1_000, 2_000, 100, 400);
        let fs = Arc::new(
            FixtureFs::new()
                .file("/proc/diskstats", &stats)
                .inode("/sys/block/sda", 1),
        );
        let mut collector = DiskCollector::new(fs, PathBuf::from("/proc"), PathBuf::from("/sys"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        for tick in 0..2 {
            graph.begin_tick(Timestamp::from_millis(2_000 + tick * 1_000));
            {
                let mut ctx = CollectCtx::new(&mut graph, 1.0);
                collector.collect(&mut ctx).expect("сбор");
            }
            let batch = graph.end_tick();
            if tick == 1 {
                let disk = disk_entity(&graph);
                let await_ms =
                    value(&batch, SeriesKey::new(disk, ids::DISK_AWAIT)).unwrap_or(f64::NAN);
                assert!(await_ms.is_finite(), "await должен быть конечным");
                assert!((await_ms - 0.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn counter_reset_skips_derived_values() {
        let mut fixture = FixtureFs::new()
            .file("/proc/diskstats", &diskstats(5_000, 9_000, 500, 900))
            .inode("/sys/block/sda", 1);
        let first = Arc::new(
            FixtureFs::new()
                .file("/proc/diskstats", &diskstats(5_000, 9_000, 500, 900))
                .inode("/sys/block/sda", 1),
        );
        let mut collector =
            DiskCollector::new(first, PathBuf::from("/proc"), PathBuf::from("/sys"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let _ = graph.end_tick();

        // Устройство переподключено: счётчики обнулились.
        fixture.set("/proc/diskstats", &diskstats(10, 20, 1, 2));
        collector.fs = Arc::new(fixture);
        graph.begin_tick(Timestamp::from_millis(3_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 2");
        }
        let batch = graph.end_tick();
        let disk = disk_entity(&graph);
        assert!(
            value(&batch, SeriesKey::new(disk, ids::DISK_AWAIT)).is_none(),
            "при сбросе счётчиков производные публиковать нельзя"
        );
    }

    #[test]
    fn missing_diskstats_is_reported_as_unavailable() {
        let fs = Arc::new(FixtureFs::new());
        let mut collector = DiskCollector::new(fs, PathBuf::from("/proc"), PathBuf::from("/sys"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let mut ctx = CollectCtx::new(&mut graph, 1.0);
        assert!(collector.collect(&mut ctx).is_err());
    }

    #[test]
    fn host_aggregates_sum_all_disks() {
        let fs = Arc::new(
            FixtureFs::new()
                .file("/proc/diskstats", &diskstats(100, 50, 10, 20))
                .inode("/sys/block/sda", 1),
        );
        let mut collector = DiskCollector::new(fs, PathBuf::from("/proc"), PathBuf::from("/sys"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор");
        }
        let batch = graph.end_tick();
        let host = graph.host();
        let read =
            value(&batch, SeriesKey::new(host, ids::HOST_DISK_READ_BYTES)).unwrap_or_default();
        assert!((read - 20.0 * SECTOR_BYTES).abs() < 1e-9, "read = {read}");
    }
}
