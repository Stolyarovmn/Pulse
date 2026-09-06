//! `pulse-collect` — коллекторы Linux.
//!
//! Все чтения идут через [`FsSource`], поэтому коллекторы тестируются на
//! синтетическом дереве `/proc` и `/sys` ([`FixtureFs`]) без боевого ядра, а
//! ограничение размера чтения задаётся в одном месте.
//!
//! Область: cgroup v2, procfs, sysfs. Сознательно вне области на этом этапе —
//! обращения к Docker/CRI/Kubernetes API и eBPF: идентичность контейнеров и
//! pod выводится из пути cgroup, чего достаточно для большинства хостов и не
//! требует ни прав на сокет демона, ни привилегий загрузки BPF-программ.

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

pub mod actions;
pub mod agent;
pub mod cgroup;
pub mod details;
pub mod disk;
pub mod filesystem;
pub mod fs;
pub mod host;
pub mod net;
pub mod parse;
pub mod process;

use std::path::Path;
use std::sync::Arc;

use pulse_core::graph::Collector;
use pulse_core::Config;

pub use agent::SelfCollector;
pub use cgroup::CgroupCollector;
pub use disk::DiskCollector;
pub use fs::{FixtureFs, FsSource, FsSourceExt, RealFs, DEFAULT_CAP};
pub use host::HostCollector;
pub use net::NetCollector;
pub use process::ProcessCollector;

/// Идентификатор загрузки системы.
///
/// Входит в ключ хоста: после перезагрузки те же PID и inode относятся уже к
/// другой «эпохе», и склеивать историю через перезагрузку нельзя.
#[must_use]
pub fn boot_id(fs: &dyn FsSource, proc_root: &Path) -> String {
    fs.read_opt(&proc_root.join("sys/kernel/random/boot_id"))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Имя хоста для заголовка TUI и меток.
#[must_use]
pub fn hostname(fs: &dyn FsSource, proc_root: &Path) -> String {
    fs.read_opt(&proc_root.join("sys/kernel/hostname"))
        .map(|text| pulse_core::sanitize_display(text.trim()))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Число тактов планировщика в секунде (`_SC_CLK_TCK`).
///
/// Значение нужно для перевода `utime`/`stime` в секунды. Хардкодить 100 нельзя:
/// ядро можно собрать с другим `CONFIG_HZ`, и тогда все проценты CPU окажутся
/// неверными в разы.
#[must_use]
pub fn clock_ticks_per_second() -> u64 {
    rustix::param::clock_ticks_per_second()
}

/// Собирает набор коллекторов согласно конфигурации.
///
/// Порядок важен: cgroup идёт до процессов, потому что процесс связывается с
/// уже существующей сущностью cgroup, а не создаёт её сам.
#[must_use]
pub fn build_collectors(cfg: &Config, fs: Arc<dyn FsSource>) -> Vec<Box<dyn Collector>> {
    let ticks = clock_ticks_per_second();
    let mut collectors: Vec<Box<dyn Collector>> = Vec::with_capacity(6);

    collectors.push(Box::new(HostCollector::new(
        Arc::clone(&fs),
        cfg.general.proc_root.clone(),
    )));
    collectors.push(Box::new(DiskCollector::new(
        Arc::clone(&fs),
        cfg.general.proc_root.clone(),
        cfg.general.sys_root.clone(),
    )));
    collectors.push(Box::new(NetCollector::new(
        Arc::clone(&fs),
        cfg.general.proc_root.clone(),
    )));
    collectors.push(Box::new(filesystem::FilesystemCollector::new(
        Arc::clone(&fs),
        cfg.general.proc_root.clone(),
    )));
    collectors.push(Box::new(CgroupCollector::new(
        Arc::clone(&fs),
        cfg.general.cgroup_root.clone(),
        cfg.general.sys_root.clone(),
        cfg.general.max_cgroups,
    )));
    if cfg.general.collect_processes {
        collectors.push(Box::new(ProcessCollector::new(
            Arc::clone(&fs),
            cfg.general.proc_root.clone(),
            cfg.general.cgroup_root.clone(),
            cfg.security.clone(),
            cfg.general.max_processes,
            ticks,
        )));
    }
    collectors.push(Box::new(SelfCollector::new(
        fs,
        cfg.general.proc_root.clone(),
        ticks,
    )));

    collectors
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn boot_id_and_hostname_have_fallbacks() {
        let empty = FixtureFs::new();
        assert_eq!(boot_id(&empty, &PathBuf::from("/proc")), "unknown");
        assert_eq!(hostname(&empty, &PathBuf::from("/proc")), "localhost");
    }

    #[test]
    fn hostname_is_sanitized() {
        let fs = FixtureFs::new().file("/proc/sys/kernel/hostname", "worker\u{1b}[2J-03\n");
        let name = hostname(&fs, &PathBuf::from("/proc"));
        assert!(!name.contains('\u{1b}'));
        assert!(name.contains("worker"));
    }

    #[test]
    fn boot_id_is_trimmed() {
        let fs = FixtureFs::new().file("/proc/sys/kernel/random/boot_id", "8f1c-42\n");
        assert_eq!(boot_id(&fs, &PathBuf::from("/proc")), "8f1c-42");
    }

    #[test]
    fn clock_ticks_is_plausible() {
        let ticks = clock_ticks_per_second();
        assert!(
            (50..=10_000).contains(&ticks),
            "неправдоподобное значение CLK_TCK: {ticks}"
        );
    }

    #[test]
    fn collector_set_follows_configuration() {
        let fs: Arc<dyn FsSource> = Arc::new(FixtureFs::new());
        let mut cfg = Config::default();
        let with_processes = build_collectors(&cfg, Arc::clone(&fs));
        let names: Vec<&str> = with_processes.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            vec![
                "host",
                "disk",
                "net",
                "filesystem",
                "cgroup",
                "process",
                "agent"
            ]
        );

        cfg.general.collect_processes = false;
        let without = build_collectors(&cfg, fs);
        let names: Vec<&str> = without.iter().map(|c| c.name()).collect();
        assert!(!names.contains(&"process"));
        assert!(
            names.contains(&"filesystem"),
            "заполненность ФС не зависит от сбора процессов"
        );
    }

    #[test]
    fn cgroup_collector_runs_before_processes() {
        // Инвариант: процесс связывается с уже созданной сущностью cgroup.
        let fs: Arc<dyn FsSource> = Arc::new(FixtureFs::new());
        let cfg = Config::default();
        let collectors = build_collectors(&cfg, fs);
        let cgroup_index = collectors.iter().position(|c| c.name() == "cgroup");
        let process_index = collectors.iter().position(|c| c.name() == "process");
        assert!(cgroup_index < process_index);
    }
}
