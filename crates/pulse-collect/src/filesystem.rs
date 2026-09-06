//! Заполненность файловых систем.
//!
//! Отдельный коллектор, а не часть `disk`: устройство и файловая система — это
//! разные сущности. Диск может быть занят на 10% по IO и при этом иметь
//! файловую систему, заполненную на 100%; ровно это и приводит к отказам.
//!
//! Главная задача — отфильтровать шум. На хосте с Docker в `/proc/mounts`
//! сотни записей: overlay каждого контейнера, `tmpfs` каждого namespace,
//! служебные `proc`/`sysfs`/`cgroup`. Показывать их оператору бессмысленно, и
//! именно поэтому здесь публикуются только значимые точки монтирования.

use std::path::PathBuf;
use std::sync::Arc;

use pulse_core::metric::ids;
use pulse_core::{CollectCtx, CollectError, Collector};

use crate::fs::{FsSource, FsSourceExt};

/// Коллектор заполненности файловых систем.
#[derive(Debug)]
pub struct FilesystemCollector {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
}

impl FilesystemCollector {
    #[must_use]
    pub fn new(fs: Arc<dyn FsSource>, proc_root: PathBuf) -> Self {
        FilesystemCollector { fs, proc_root }
    }
}

/// Точка монтирования, отобранная как значимая.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub fstype: String,
}

/// Является ли тип файловой системы носителем пользовательских данных.
///
/// Список разрешающий, а не запрещающий: новый служебный тип в будущем ядре
/// по умолчанию окажется отфильтрован, а не засорит экран. Цена — новый
/// настоящий тип ФС придётся добавить явно.
#[must_use]
pub fn is_real_fstype(fstype: &str) -> bool {
    const REAL: &[&str] = &[
        "ext2",
        "ext3",
        "ext4",
        "xfs",
        "btrfs",
        "zfs",
        "f2fs",
        "jfs",
        "reiserfs",
        "nilfs2",
        "ntfs",
        "ntfs3",
        "vfat",
        "exfat",
        "msdos",
        "iso9660",
        "udf",
        "ufs",
        "hfsplus",
        "apfs",
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "glusterfs",
        "cephfs",
        "lustre",
        "9p",
        "virtiofs",
    ];
    REAL.contains(&fstype)
}

/// Служебная ли это точка монтирования по своему пути.
///
/// Даже настоящая ФС под `/var/lib/docker/...` или `/snap/...` является
/// внутренностью рантайма: таких точек сотни, и оператор их не обслуживает.
#[must_use]
pub fn is_service_target(target: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/snap",
        "/var/lib/docker",
        "/var/lib/containerd",
        "/var/lib/kubelet",
        "/var/lib/rancher",
        "/var/snap",
        "/tmp/.mount",
        // Внутренности WSL: смонтированы как настоящие ФС (9p, ext4), но
        // обслуживает их не оператор хоста.
        "/mnt/wsl",
        "/mnt/wslg",
        "/usr/lib/wsl",
        "/usr/lib/modules",
    ];
    PREFIXES
        .iter()
        .any(|prefix| target == *prefix || target.starts_with(&format!("{prefix}/")))
}

/// Отбирает значимые точки монтирования из содержимого `/proc/mounts`.
#[must_use]
pub fn significant_mounts(text: &str) -> Vec<Mount> {
    let mut out: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(source) = fields.next() else {
            continue;
        };
        let Some(target) = fields.next() else {
            continue;
        };
        let Some(fstype) = fields.next() else {
            continue;
        };
        if !is_real_fstype(fstype) || is_service_target(target) {
            continue;
        }
        // Один и тот же источник, смонтированный дважды (bind mount), — это
        // одна файловая система: считать её дважды значит соврать в сумме.
        if out.iter().any(|mount| mount.source == source) {
            continue;
        }
        out.push(Mount {
            source: unescape_mount(source),
            target: unescape_mount(target),
            fstype: fstype.to_string(),
        });
    }
    out
}

/// Раскрывает восьмеричные escape-последовательности `/proc/mounts`.
///
/// Ядро кодирует пробел как `\040`. Без раскрытия путь с пробелом не совпадёт
/// ни с чем и точка монтирования потеряется.
fn unescape_mount(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        if digits.len() == 3 && digits.chars().all(|d| ('0'..='7').contains(&d)) {
            if let Ok(code) = u8::from_str_radix(&digits, 8) {
                for _ in 0..3 {
                    let _ = chars.next();
                }
                out.push(char::from(code));
                continue;
            }
        }
        out.push(c);
    }
    out
}

impl Collector for FilesystemCollector {
    fn name(&self) -> &'static str {
        "filesystem"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        let path = self.proc_root.join("mounts");
        // Лимит выше стандартного: на хосте с Docker `/proc/mounts` содержит
        // сотни overlay-записей, и обрезка списка потеряла бы настоящие ФС.
        let text = self
            .fs
            .read_string_capped(&path, 256 * 1024)
            .map_err(CollectError::from)?;

        let host = ctx.host();
        let mounts = significant_mounts(&text);
        let mut worst: Option<f64> = None;

        for mount in &mounts {
            let Ok(usage) = self.fs.statfs(std::path::Path::new(&mount.target)) else {
                // Точка могла исчезнуть между чтением списка и запросом: это
                // гонка с ядром, а не ошибка сбора.
                continue;
            };
            // Формула `df`: резерв root не выдаётся за занятое место, иначе
            // оператор видит 6% там, где `df` показывает 2%.
            let Some(util) = usage.utilization() else {
                continue;
            };
            worst = Some(worst.map_or(util, |current: f64| current.max(util)));

            if mount.target == "/" {
                ctx.sample(host, ids::HOST_FS_ROOT_UTIL, util);
                ctx.sample(host, ids::HOST_FS_ROOT_TOTAL, usage.total as f64);
                ctx.sample(host, ids::HOST_FS_ROOT_FREE, usage.available as f64);
            }
        }

        if let Some(worst) = worst {
            ctx.sample(host, ids::HOST_FS_WORST_UTIL, worst);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use pulse_core::sample::SeriesKey;
    use pulse_core::time::Timestamp;
    use pulse_core::{EntityGraph, TickBatch};

    /// Реальный `/proc/mounts` хоста с Docker: настоящие ФС утонули в служебных.
    const MOUNTS: &str = "\
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
udev /dev devtmpfs rw,nosuid,relatime 0 0
tmpfs /run tmpfs rw,nosuid,nodev 0 0
/dev/sda2 / ext4 rw,relatime 0 0
/dev/sda1 /boot/efi vfat rw,relatime 0 0
/dev/sdb1 /mnt/data xfs rw,relatime 0 0
overlay /var/lib/docker/overlay2/aaaa/merged overlay rw,relatime 0 0
overlay /var/lib/docker/overlay2/bbbb/merged overlay rw,relatime 0 0
tmpfs /var/lib/kubelet/pods/xxx/volumes/kubernetes.io~projected/kube-api-access tmpfs rw 0 0
/dev/loop3 /snap/core22/1122 squashfs ro,nodev,relatime 0 0
cgroup2 /sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec,relatime 0 0
";

    fn value(batch: &TickBatch, key: SeriesKey) -> Option<f64> {
        batch
            .samples
            .iter()
            .find(|sample| sample.series == key)
            .map(|sample| sample.value)
    }

    #[test]
    fn docker_and_service_mounts_are_filtered_out() {
        let mounts = significant_mounts(MOUNTS);
        let targets: Vec<&str> = mounts.iter().map(|m| m.target.as_str()).collect();
        assert_eq!(
            targets,
            vec!["/", "/boot/efi", "/mnt/data"],
            "оператор обслуживает эти точки, а не внутренности рантайма"
        );
    }

    #[test]
    fn hundreds_of_overlays_do_not_leak() {
        let mut text = String::from("/dev/sda2 / ext4 rw 0 0\n");
        for index in 0..400 {
            text.push_str(&format!(
                "overlay /var/lib/docker/overlay2/{index}/merged overlay rw 0 0\n"
            ));
        }
        let mounts = significant_mounts(&text);
        assert_eq!(mounts.len(), 1, "получено: {mounts:?}");
    }

    #[test]
    fn runtime_internals_are_filtered_even_when_fstype_is_real() {
        // Живой WSL: `/mnt/c` и `/usr/lib/wsl/drivers` смонтированы как 9p и
        // заполнены на 94%, что делало их «худшей ФС» вместо настоящей.
        let text = "\
/dev/sde / ext4 rw 0 0
drvfs /mnt/c 9p rw 0 0
drivers /usr/lib/wsl/drivers 9p rw 0 0
none /usr/lib/modules/6.6.87 overlay rw 0 0
/dev/sdd /mnt/wslg/distro ext4 rw 0 0
/dev/sdb1 /mnt/data xfs rw 0 0
";
        let targets: Vec<String> = significant_mounts(text)
            .into_iter()
            .map(|mount| mount.target)
            .collect();
        // `/mnt/c` остаётся: это настоящая смонтированная файловая система, и
        // скрывать её заполненность на 94% значило бы врать. Отфильтрованы
        // только внутренности рантайма, которые оператор не обслуживает.
        assert_eq!(
            targets,
            vec!["/", "/mnt/c", "/mnt/data"],
            "получено: {targets:?}"
        );
    }

    #[test]
    fn bind_mount_of_the_same_source_is_counted_once() {
        let text = "/dev/sda2 / ext4 rw 0 0\n/dev/sda2 /var/lib/mysql ext4 rw 0 0\n";
        assert_eq!(significant_mounts(text).len(), 1);
    }

    #[test]
    fn escaped_spaces_are_decoded() {
        let text = "/dev/sdc1 /mnt/my\\040disk ext4 rw 0 0\n";
        let mounts = significant_mounts(text);
        assert_eq!(
            mounts.first().map(|m| m.target.as_str()),
            Some("/mnt/my disk")
        );
    }

    #[test]
    fn root_utilization_is_published_and_worst_is_the_fullest() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let fixture = FixtureFs::new()
            .file("/proc/mounts", MOUNTS)
            // Корень: 1007 GiB, занято 14, доступно 943 — ровно как на живом
            // хосте, где `df` показывает 2%, а наивная формула давала 6%.
            .mount("/", 1007 * GIB, 993 * GIB, 943 * GIB)
            // Загрузочный раздел почти пуст.
            .mount(
                "/boot/efi",
                512 * 1024 * 1024,
                506 * 1024 * 1024,
                506 * 1024 * 1024,
            )
            // Данные заполнены на 91% — это и есть худшая ФС.
            .mount("/mnt/data", 1000 * GIB, 90 * GIB, 90 * GIB);
        let mut collector = FilesystemCollector::new(Arc::new(fixture), PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(0));
        graph.begin_tick(Timestamp::from_millis(1_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор файловых систем");
        }
        let batch = graph.end_tick();
        let host = graph.host();

        let root = value(&batch, SeriesKey::new(host, ids::HOST_FS_ROOT_UTIL)).expect("корень");
        // used = 1007 - 993 = 14; 14 / (14 + 943) = 1.46% — это и печатает `df`.
        assert!(
            (root - 0.0146).abs() < 0.002,
            "заполненность обязана совпадать с df: {root}"
        );
        let total = value(&batch, SeriesKey::new(host, ids::HOST_FS_ROOT_TOTAL)).expect("размер");
        assert!((total - 1007.0 * 1024.0 * 1024.0 * 1024.0).abs() < 1.0);
        let worst = value(&batch, SeriesKey::new(host, ids::HOST_FS_WORST_UTIL)).expect("худшая");
        assert!((worst - 0.91).abs() < 0.01, "worst = {worst}");
    }

    /// Резерв root не является занятым местом. Ровно это расхождение давало
    /// `/ 6%` в интерфейсе против `2%` в `df` на живом хосте.
    #[test]
    fn reserved_root_blocks_are_not_counted_as_used() {
        use crate::fs::FsUsage;
        let usage = FsUsage {
            total: 1000,
            free: 100,
            available: 50,
        };
        let util = usage.utilization().expect("заполненность");
        // used = 900, знаменатель = 950.
        assert!((util - 0.947).abs() < 0.001, "util = {util}");
        // Наивная формула `1 - available/total` дала бы 0.95 — почти то же,
        // но на почти пустой ФС расхождение кратное, что и ловит тест ниже.
        let empty = FsUsage {
            total: 1_000_000,
            free: 990_000,
            available: 940_000,
        };
        let empty_util = empty.utilization().expect("заполненность");
        assert!((empty_util - 0.0105).abs() < 0.002, "util = {empty_util}");
        let naive = 1.0 - (940_000.0 / 1_000_000.0);
        assert!(
            naive > empty_util * 4.0,
            "наивная формула завышает в разы: {naive} против {empty_util}"
        );
    }

    #[test]
    fn missing_mount_is_a_race_not_an_error() {
        // Список есть, а statfs не отвечает: точка исчезла между чтениями.
        let fixture = FixtureFs::new().file("/proc/mounts", "/dev/sda2 / ext4 rw 0 0\n");
        let mut collector = FilesystemCollector::new(Arc::new(fixture), PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(0));
        graph.begin_tick(Timestamp::from_millis(1_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector
                .collect(&mut ctx)
                .expect("гонка не является ошибкой");
        }
        let batch = graph.end_tick();
        let host = graph.host();
        assert!(value(&batch, SeriesKey::new(host, ids::HOST_FS_ROOT_UTIL)).is_none());
    }
}
