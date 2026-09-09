//! Абстракция файловой системы для коллекторов.
//!
//! Существует по двум причинам.
//!
//! 1. **Тестируемость.** Коллекторы читают Linux-специфичные файлы, которых нет
//!    ни на машине разработчика под Windows, ни в CI без нужного ядра.
//!    [`FixtureFs`] даёт детерминированное дерево `/proc` и `/sys` в памяти,
//!    включая случаи «файл исчез между чтениями» и «в файле мусор».
//! 2. **Безопасность.** Все чтения проходят через одну точку с обязательным
//!    ограничением размера: файлы procfs сообщают размер 0, поэтому обычный
//!    `read_to_string` на враждебном или бесконечном файле (`/proc/kcore`,
//!    гигабайтный `cmdline`) читал бы неограниченно долго.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Максимальный объём одного чтения по умолчанию.
pub const DEFAULT_CAP: usize = 64 * 1024;

/// Источник файловых данных.
/// Занятость файловой системы в байтах.
///
/// Три величины, а не две: `free` включает зарезервированные для root блоки,
/// `available` — нет. `df` считает заполненность как `used / (used + available)`
/// где `used = total - free`, и отклонение от этой формулы даёт расхождение в
/// разы на ext4 с пятипроцентным резервом.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FsUsage {
    pub total: u64,
    pub free: u64,
    pub available: u64,
}

impl FsUsage {
    /// Заполненность в долях по формуле `df`.
    #[must_use]
    pub fn utilization(self) -> Option<f64> {
        let used = self.total.saturating_sub(self.free);
        let denominator = used.saturating_add(self.available);
        if denominator == 0 {
            return None;
        }
        Some((used as f64 / denominator as f64).clamp(0.0, 1.0))
    }
}

pub trait FsSource: Send + Sync + std::fmt::Debug {
    /// Читает не более `cap` байт файла.
    fn read(&self, path: &Path, cap: usize) -> io::Result<Vec<u8>>;

    /// Обходит каталог, отдавая имена по одному (без `.` и `..`).
    ///
    /// Потоковый, а не `Vec`, именно из-за бюджетов: обход `/proc` на хосте с
    /// десятками тысяч процессов сначала материализовал весь список, и только
    /// потом коллектор применял `max_processes`. То есть лимит ограничивал
    /// обработку, но не работу и не память - агент платил за весь каталог
    /// прежде, чем решить, что столько ему не нужно.
    ///
    /// `visit` возвращает `false`, чтобы прекратить обход немедленно.
    ///
    /// **Внутри `visit` обращаться к этому же источнику запрещено.**
    /// Реализация вправе держать внутренний замок на время обхода (так делает
    /// демо-источник), поэтому вложенное чтение даёт взаимную блокировку, а не
    /// ошибку. Нужны данные по каждой записи — сначала соберите имена
    /// бюджетом, затем читайте.
    fn scan_dir(&self, path: &Path, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()>;

    /// Цель символической ссылки.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    /// Inode каталога или файла.
    fn inode(&self, path: &Path) -> io::Result<u64>;

    /// Занятость файловой системы точки монтирования.
    fn statfs(&self, path: &Path) -> io::Result<FsUsage>;
}

/// Удобные обёртки над [`FsSource`].
pub trait FsSourceExt: FsSource {
    /// Читает файл как строку с потерей некорректного UTF-8.
    ///
    /// Данные из ядра не гарантированно UTF-8 (имя процесса пишет сам процесс),
    /// поэтому здесь именно `from_utf8_lossy`, а не отказ.
    fn read_string(&self, path: &Path) -> io::Result<String> {
        let bytes = self.read(path, DEFAULT_CAP)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Читает файл как строку с явным лимитом размера.
    ///
    /// Нужен там, где `DEFAULT_CAP` мал: `/proc/mounts` на хосте с Docker
    /// содержит сотни overlay-записей и легко перерастает 64 КиБ, а обрезка
    /// списка монтирований потеряла бы реальные файловые системы.
    fn read_string_capped(&self, path: &Path, cap: usize) -> io::Result<String> {
        let bytes = self.read(path, cap)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Читает файл и возвращает `None` при любой ошибке.
    ///
    /// Отсутствие файла — норма: подсистема может быть выключена в ядре,
    /// процесс мог завершиться между обходом каталога и чтением.
    fn read_opt(&self, path: &Path) -> Option<String> {
        self.read_string(path).ok()
    }

    /// Имена элементов каталога, не более `cap`.
    ///
    /// Возвращает признак усечения: «мы увидели ровно столько, сколько
    /// разрешил бюджет» и «в каталоге ровно столько» — разные факты, и
    /// потребитель обязан их различать.
    fn read_dir_capped(&self, path: &Path, cap: usize) -> io::Result<(Vec<OsString>, bool)> {
        let mut names: Vec<OsString> = Vec::new();
        let mut truncated = false;
        self.scan_dir(path, &mut |name| {
            if names.len() >= cap {
                truncated = true;
                return false;
            }
            names.push(name.to_os_string());
            true
        })?;
        Ok((names, truncated))
    }

    /// Число элементов каталога без их материализации.
    fn count_dir(&self, path: &Path) -> io::Result<usize> {
        let mut count = 0_usize;
        self.scan_dir(path, &mut |_| {
            count += 1;
            true
        })?;
        Ok(count)
    }
}

impl<T: FsSource + ?Sized> FsSourceExt for T {}

/// Реальная файловая система.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

impl FsSource for RealFs {
    fn read(&self, path: &Path, cap: usize) -> io::Result<Vec<u8>> {
        let file = File::open(path)?;
        let mut buf = Vec::with_capacity(4096.min(cap));
        // `take` обязателен: размер файлов procfs равен нулю, а некоторые
        // (например `/proc/kcore`) фактически бесконечны.
        let _ = file.take(cap as u64).read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn scan_dir(&self, path: &Path, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
        // `std::fs::read_dir` — ленивый итератор: имена берутся порциями из
        // `getdents64` по мере обхода, поэтому ранний выход экономит и
        // syscall, и память.
        for entry in std::fs::read_dir(path)? {
            let Ok(entry) = entry else {
                // Каталог мог измениться во время обхода — это не ошибка такта.
                continue;
            };
            if !visit(&entry.file_name()) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::read_link(path)
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        let stat = rustix::fs::stat(path)?;
        Ok(stat.st_ino as u64)
    }

    fn statfs(&self, path: &Path) -> io::Result<FsUsage> {
        let stat = rustix::fs::statvfs(path)?;
        let block = if stat.f_frsize == 0 {
            stat.f_bsize
        } else {
            stat.f_frsize
        };
        Ok(FsUsage {
            total: stat.f_blocks.saturating_mul(block),
            free: stat.f_bfree.saturating_mul(block),
            available: stat.f_bavail.saturating_mul(block),
        })
    }
}

/// Дерево в памяти для тестов.
///
/// Хранит содержимое файлов, цели ссылок и inode. Каталоги выводятся из путей,
/// поэтому объявлять их отдельно не нужно.
#[derive(Debug, Default)]
pub struct FixtureFs {
    files: BTreeMap<String, Vec<u8>>,
    links: BTreeMap<String, String>,
    inodes: BTreeMap<String, u64>,
    /// Точки монтирования и их занятость.
    statfs: BTreeMap<String, FsUsage>,
    /// Пути, которые «исчезают» при первом чтении — модель гонки с ядром.
    vanish_after_read: Mutex<BTreeMap<String, u32>>,
    /// Пути, на которые ядро отвечает отказом в правах.
    ///
    /// Непривилегированный агент не видит `/proc/<pid>/fd` чужого
    /// пользователя, и это поведение обязано быть воспроизводимо в тестах:
    /// иначе «нет прав» и «нет файлов» неотличимы.
    denied: BTreeMap<String, ()>,
}

impl FixtureFs {
    #[must_use]
    pub fn new() -> Self {
        FixtureFs::default()
    }

    #[must_use]
    pub fn file(mut self, path: &str, content: &str) -> Self {
        let _ = self
            .files
            .insert(normalize(path), content.as_bytes().to_vec());
        self
    }

    #[must_use]
    pub fn bytes(mut self, path: &str, content: &[u8]) -> Self {
        let _ = self.files.insert(normalize(path), content.to_vec());
        self
    }

    #[must_use]
    pub fn link(mut self, path: &str, target: &str) -> Self {
        let _ = self.links.insert(normalize(path), target.to_string());
        self
    }

    #[must_use]
    pub fn inode(mut self, path: &str, ino: u64) -> Self {
        let _ = self.inodes.insert(normalize(path), ino);
        self
    }

    /// Объявляет файловую систему точки монтирования.
    ///
    /// `free` — всё свободное место, `available` — доступное обычному
    /// пользователю. Разница (резерв root) обязана участвовать в тесте: именно
    /// из-за неё наивная формула расходилась с `df` в разы.
    #[must_use]
    pub fn mount(mut self, path: &str, total: u64, free: u64, available: u64) -> Self {
        let _ = self.statfs.insert(
            normalize(path),
            FsUsage {
                total,
                free,
                available,
            },
        );
        self
    }

    /// Объявляет путь запретным: чтение вернёт `PermissionDenied`.
    #[must_use]
    pub fn denied(mut self, path: &str) -> Self {
        let _ = self.denied.insert(normalize(path), ());
        self
    }

    /// Файл исчезнет после `reads` успешных чтений.
    #[must_use]
    pub fn vanishing(self, path: &str, reads: u32) -> Self {
        if let Ok(mut map) = self.vanish_after_read.lock() {
            let _ = map.insert(normalize(path), reads);
        }
        self
    }

    /// Заменяет содержимое уже созданного файла (сдвиг такта в тестах).
    pub fn set(&mut self, path: &str, content: &str) {
        let _ = self
            .files
            .insert(normalize(path), content.as_bytes().to_vec());
    }

    pub fn remove(&mut self, path: &str) {
        let key = normalize(path);
        let _ = self.files.remove(&key);
        let _ = self.links.remove(&key);
    }
}

fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn key_of(path: &Path) -> String {
    normalize(&path.to_string_lossy().replace('\\', "/"))
}

impl FsSource for FixtureFs {
    fn statfs(&self, path: &Path) -> io::Result<FsUsage> {
        self.statfs
            .get(&key_of(path))
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, key_of(path)))
    }

    fn read(&self, path: &Path, cap: usize) -> io::Result<Vec<u8>> {
        let key = key_of(path);
        if self.denied.contains_key(&key) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, key));
        }

        if let Ok(mut map) = self.vanish_after_read.lock() {
            if let Some(remaining) = map.get_mut(&key) {
                if *remaining == 0 {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "файл исчез"));
                }
                *remaining -= 1;
            }
        }

        match self.files.get(&key) {
            Some(content) => {
                let end = content.len().min(cap);
                Ok(content.get(..end).unwrap_or_default().to_vec())
            }
            None => Err(io::Error::new(io::ErrorKind::NotFound, key)),
        }
    }

    fn scan_dir(&self, path: &Path, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
        if self.denied.contains_key(&key_of(path)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                key_of(path),
            ));
        }
        let prefix = {
            let base = key_of(path);
            if base == "/" {
                "/".to_string()
            } else {
                format!("{base}/")
            }
        };

        let mut names: Vec<String> = Vec::new();
        let sources = self
            .files
            .keys()
            .chain(self.links.keys())
            .chain(self.inodes.keys());
        for full in sources {
            let Some(rest) = full.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let head = rest.split('/').next().unwrap_or(rest).to_string();
            if !names.contains(&head) {
                names.push(head);
            }
        }
        if names.is_empty() && !self.inodes.contains_key(&key_of(path)) {
            return Err(io::Error::new(io::ErrorKind::NotFound, key_of(path)));
        }
        // Порядок фиксирован: фикстура обязана давать воспроизводимый обход,
        // иначе тест на бюджет зависел бы от порядка ключей карты.
        names.sort();
        for name in names {
            if !visit(OsStr::new(&name)) {
                break;
            }
        }
        Ok(())
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.links
            .get(&key_of(path))
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, key_of(path)))
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.inodes
            .get(&key_of(path))
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, key_of(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_reads_and_caps_content() {
        let fs = FixtureFs::new().file("/proc/uptime", "12345.67 98765.43\n");
        let text = fs.read_string(Path::new("/proc/uptime")).unwrap();
        assert!(text.starts_with("12345.67"));
        let short = fs.read(Path::new("/proc/uptime"), 5).unwrap();
        assert_eq!(short.len(), 5);
    }

    #[test]
    fn missing_file_is_error_not_panic() {
        let fs = FixtureFs::new();
        assert!(fs.read(Path::new("/proc/nope"), 128).is_err());
        assert!(fs.read_opt(Path::new("/proc/nope")).is_none());
    }

    #[test]
    fn vanishing_file_models_race_with_kernel() {
        let fs = FixtureFs::new()
            .file("/proc/7/stat", "7 (x) R 1 0 0")
            .vanishing("/proc/7/stat", 1);
        assert!(fs.read(Path::new("/proc/7/stat"), 128).is_ok());
        assert!(
            fs.read(Path::new("/proc/7/stat"), 128).is_err(),
            "второе чтение обязано провалиться"
        );
    }

    #[test]
    fn scan_dir_lists_immediate_children_only() {
        let fs = FixtureFs::new()
            .file("/proc/1/stat", "x")
            .file("/proc/1/status", "x")
            .file("/proc/2/stat", "x")
            .file("/proc/uptime", "x");
        let (names, truncated) = fs.read_dir_capped(Path::new("/proc"), 16).unwrap();
        let mut names: Vec<String> = names
            .into_iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["1", "2", "uptime"]);
        assert!(!truncated, "каталог меньше бюджета: усечения нет");
    }

    /// Бюджет обязан быть отличим от фактического размера каталога: иначе
    /// «увидели три записи» и «в каталоге три записи» сливаются в одно.
    #[test]
    fn capped_scan_reports_truncation_and_stops_early() {
        let fs = FixtureFs::new()
            .file("/proc/1/stat", "x")
            .file("/proc/2/stat", "x")
            .file("/proc/3/stat", "x");
        let (names, truncated) = fs.read_dir_capped(Path::new("/proc"), 2).unwrap();
        assert_eq!(names.len(), 2, "бюджет обязан ограничить выдачу");
        assert!(truncated, "усечение обязано быть заявлено");

        let mut visited = 0_usize;
        fs.scan_dir(Path::new("/proc"), &mut |_| {
            visited += 1;
            visited < 2
        })
        .unwrap();
        assert_eq!(visited, 2, "ранний выход обязан прекратить обход");
    }

    #[test]
    fn scan_dir_sees_directories_declared_only_by_inode() {
        let fs = FixtureFs::new()
            .inode("/sys/fs/cgroup/system.slice", 42)
            .file("/sys/fs/cgroup/cpu.stat", "usage_usec 1");
        let (names, _) = fs.read_dir_capped(Path::new("/sys/fs/cgroup"), 16).unwrap();
        let names: Vec<String> = names
            .into_iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"system.slice".to_string()));
    }

    #[test]
    fn non_utf8_content_is_read_lossily() {
        let fs = FixtureFs::new().bytes("/proc/9/comm", &[0xff, 0xfe, b'o', b'k']);
        let text = fs.read_string(Path::new("/proc/9/comm")).unwrap();
        assert!(text.ends_with("ok"));
    }

    #[test]
    fn links_are_resolved() {
        let fs = FixtureFs::new().link("/proc/5/exe", "/usr/bin/nginx");
        assert_eq!(
            fs.read_link(Path::new("/proc/5/exe")).unwrap(),
            PathBuf::from("/usr/bin/nginx")
        );
        assert!(fs.read_link(Path::new("/proc/6/exe")).is_err());
    }
}
