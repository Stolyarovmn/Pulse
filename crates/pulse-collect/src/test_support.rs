//! Test-only инструменты для проверки Linux I/O-контрактов.
//!
//! [`FixtureFs`] моделирует содержимое дерева, но не отвечает на два вопроса
//! аудита: сколько работы реально сделал коллектор и что произошло, если один
//! путь меняется между двумя чтениями. [`CountingFs`] измеряет первое,
//! [`ScriptedFs`] детерминированно воспроизводит второе без sleeps и гонок.
//! Модуль существует только под `#[cfg(test)]` и не входит в production path.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::fs::{FsSource, FsUsage};

/// Жёсткий предел журнала test-support.
///
/// Тест, проверяющий bounded work, сам не имеет права создавать unbounded
/// структуру. Счётчики продолжают расти после достижения лимита, журнал — нет.
const MAX_CALL_LOG: usize = 100_000;

/// Число вызовов каждого метода [`FsSource`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FsCallCounts {
    pub read: u64,
    pub read_dir: u64,
    pub read_link: u64,
    pub inode: u64,
    pub statfs: u64,
    /// Сколько записей каталогов фактически посещено.
    pub dir_entries: u64,
}

/// Операция в bounded-журнале вызовов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsOperation {
    Read,
    ReadDir,
    ReadLink,
    Inode,
    StatFs,
    DirEntry,
}

/// Один вызов источника файловой системы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsCall {
    pub operation: FsOperation,
    pub path: String,
    /// Ограничение чтения; только для [`FsOperation::Read`].
    pub cap: Option<usize>,
}

#[derive(Debug, Default)]
struct CountingState {
    counts: FsCallCounts,
    log: Vec<FsCall>,
}

/// Wrapper, считающий фактическую работу [`FsSource`].
#[derive(Debug, Clone)]
pub(crate) struct CountingFs {
    inner: Arc<dyn FsSource>,
    state: Arc<Mutex<CountingState>>,
}

impl CountingFs {
    #[must_use]
    pub(crate) fn new(inner: Arc<dyn FsSource>) -> Self {
        Self {
            inner,
            state: Arc::new(Mutex::new(CountingState::default())),
        }
    }

    /// Снимок счётчиков без их сброса.
    #[must_use]
    pub(crate) fn counts(&self) -> FsCallCounts {
        self.with_state(|state| state.counts)
    }

    /// Копия bounded-журнала.
    #[must_use]
    pub(crate) fn calls(&self) -> Vec<FsCall> {
        self.with_state(|state| state.log.clone())
    }

    /// Сбрасывает счётчики и журнал одной атомарной операцией.
    pub(crate) fn reset(&self) {
        self.with_state_mut(|state| *state = CountingState::default());
    }

    fn note(&self, operation: FsOperation, path: &Path, cap: Option<usize>) {
        self.with_state_mut(|state| {
            let count = match operation {
                FsOperation::Read => &mut state.counts.read,
                FsOperation::ReadDir => &mut state.counts.read_dir,
                FsOperation::ReadLink => &mut state.counts.read_link,
                FsOperation::Inode => &mut state.counts.inode,
                FsOperation::StatFs => &mut state.counts.statfs,
                FsOperation::DirEntry => &mut state.counts.dir_entries,
            };
            *count = count.saturating_add(1);
            // Журнал не пишет каждую запись каталога: на большом каталоге он
            // сам стал бы стоимостью, а для диагностики достаточно счётчика.
            if state.log.len() < MAX_CALL_LOG && operation != FsOperation::DirEntry {
                state.log.push(FsCall {
                    operation,
                    path: normalize(path),
                    cap,
                });
            }
        });
    }

    fn with_state<T>(&self, f: impl FnOnce(&CountingState) -> T) -> T {
        match self.state.lock() {
            Ok(state) => f(&state),
            Err(poisoned) => f(&poisoned.into_inner()),
        }
    }

    fn with_state_mut<T>(&self, f: impl FnOnce(&mut CountingState) -> T) -> T {
        match self.state.lock() {
            Ok(mut state) => f(&mut state),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }
}

impl FsSource for CountingFs {
    fn read(&self, path: &Path, cap: usize) -> io::Result<Vec<u8>> {
        self.note(FsOperation::Read, path, Some(cap));
        self.inner.read(path, cap)
    }

    fn scan_dir(&self, path: &Path, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
        self.note(FsOperation::ReadDir, path, None);
        // Считаются и посещённые записи: «сколько раз открыли каталог» не
        // отвечает на вопрос «сколько работы сделано», а бюджет измеряется
        // именно работой.
        self.inner.scan_dir(path, &mut |name| {
            self.note(FsOperation::DirEntry, path, None);
            visit(name)
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.note(FsOperation::ReadLink, path, None);
        self.inner.read_link(path)
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.note(FsOperation::Inode, path, None);
        self.inner.inode(path)
    }

    fn statfs(&self, path: &Path) -> io::Result<FsUsage> {
        self.note(FsOperation::StatFs, path, None);
        self.inner.statfs(path)
    }
}

/// Результат scripted `read`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScriptedRead {
    Bytes(Vec<u8>),
    NotFound,
    PermissionDenied,
    Io(io::ErrorKind),
}

impl ScriptedRead {
    #[must_use]
    pub(crate) fn text(value: impl Into<String>) -> Self {
        Self::Bytes(value.into().into_bytes())
    }
}

/// Результат scripted `read_link`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScriptedLink {
    Path(PathBuf),
    NotFound,
    PermissionDenied,
    Io(io::ErrorKind),
}

/// Результат scripted `read_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScriptedDir {
    Entries(Vec<OsString>),
    NotFound,
    PermissionDenied,
    Io(io::ErrorKind),
}

/// Последовательность результатов с детерминированным исчерпанием.
///
/// После последнего элемента повторяется **последний**. Поведение выбрано
/// вместо fallback в base: гонка должна оставаться в достигнутом состоянии,
/// а не внезапно возвращаться к исходной фикстуре после лишнего чтения.
#[derive(Debug)]
struct Sequence<T> {
    pending: VecDeque<T>,
    last: Option<T>,
}

impl<T: Clone> Sequence<T> {
    fn new(values: impl IntoIterator<Item = T>) -> Self {
        Self {
            pending: values.into_iter().collect(),
            last: None,
        }
    }

    fn next(&mut self) -> Option<T> {
        if let Some(value) = self.pending.pop_front() {
            self.last = Some(value.clone());
            return Some(value);
        }
        self.last.clone()
    }
}

#[derive(Debug, Default)]
struct Scripts {
    reads: BTreeMap<String, Sequence<ScriptedRead>>,
    links: BTreeMap<String, Sequence<ScriptedLink>>,
    dirs: BTreeMap<String, Sequence<ScriptedDir>>,
}

/// Источник с программируемыми результатами отдельных путей.
#[derive(Debug)]
pub(crate) struct ScriptedFs {
    base: Arc<dyn FsSource>,
    scripts: Mutex<Scripts>,
}

impl ScriptedFs {
    #[must_use]
    pub(crate) fn new(base: Arc<dyn FsSource>) -> Self {
        Self {
            base,
            scripts: Mutex::new(Scripts::default()),
        }
    }

    #[must_use]
    pub(crate) fn read_sequence(
        self,
        path: &str,
        values: impl IntoIterator<Item = ScriptedRead>,
    ) -> Self {
        self.with_scripts(|scripts| {
            let _ = scripts
                .reads
                .insert(normalize(Path::new(path)), Sequence::new(values));
        });
        self
    }

    #[must_use]
    pub(crate) fn link_sequence(
        self,
        path: &str,
        values: impl IntoIterator<Item = ScriptedLink>,
    ) -> Self {
        self.with_scripts(|scripts| {
            let _ = scripts
                .links
                .insert(normalize(Path::new(path)), Sequence::new(values));
        });
        self
    }

    #[must_use]
    pub(crate) fn dir_sequence(
        self,
        path: &str,
        values: impl IntoIterator<Item = ScriptedDir>,
    ) -> Self {
        self.with_scripts(|scripts| {
            let _ = scripts
                .dirs
                .insert(normalize(Path::new(path)), Sequence::new(values));
        });
        self
    }

    fn with_scripts<T>(&self, f: impl FnOnce(&mut Scripts) -> T) -> T {
        match self.scripts.lock() {
            Ok(mut scripts) => f(&mut scripts),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }
}

impl FsSource for ScriptedFs {
    fn read(&self, path: &Path, cap: usize) -> io::Result<Vec<u8>> {
        let scripted = self.with_scripts(|scripts| {
            scripts
                .reads
                .get_mut(&normalize(path))
                .and_then(Sequence::next)
        });
        match scripted {
            Some(ScriptedRead::Bytes(mut bytes)) => {
                bytes.truncate(cap);
                Ok(bytes)
            }
            Some(ScriptedRead::NotFound) => Err(error(io::ErrorKind::NotFound, path)),
            Some(ScriptedRead::PermissionDenied) => {
                Err(error(io::ErrorKind::PermissionDenied, path))
            }
            Some(ScriptedRead::Io(kind)) => Err(error(kind, path)),
            None => self.base.read(path, cap),
        }
    }

    fn scan_dir(&self, path: &Path, visit: &mut dyn FnMut(&OsStr) -> bool) -> io::Result<()> {
        let scripted = self.with_scripts(|scripts| {
            scripts
                .dirs
                .get_mut(&normalize(path))
                .and_then(Sequence::next)
        });
        match scripted {
            Some(ScriptedDir::Entries(entries)) => {
                for entry in entries {
                    if !visit(&entry) {
                        break;
                    }
                }
                Ok(())
            }
            Some(ScriptedDir::NotFound) => Err(error(io::ErrorKind::NotFound, path)),
            Some(ScriptedDir::PermissionDenied) => {
                Err(error(io::ErrorKind::PermissionDenied, path))
            }
            Some(ScriptedDir::Io(kind)) => Err(error(kind, path)),
            None => self.base.scan_dir(path, visit),
        }
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        let scripted = self.with_scripts(|scripts| {
            scripts
                .links
                .get_mut(&normalize(path))
                .and_then(Sequence::next)
        });
        match scripted {
            Some(ScriptedLink::Path(target)) => Ok(target),
            Some(ScriptedLink::NotFound) => Err(error(io::ErrorKind::NotFound, path)),
            Some(ScriptedLink::PermissionDenied) => {
                Err(error(io::ErrorKind::PermissionDenied, path))
            }
            Some(ScriptedLink::Io(kind)) => Err(error(kind, path)),
            None => self.base.read_link(path),
        }
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.base.inode(path)
    }

    fn statfs(&self, path: &Path) -> io::Result<FsUsage> {
        self.base.statfs(path)
    }
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn error(kind: io::ErrorKind, path: &Path) -> io::Error {
    io::Error::new(kind, format!("scripted {}: {kind:?}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FixtureFs, FsSourceExt};

    #[test]
    fn counting_fs_counts_resets_and_bounds_log() {
        let base: Arc<dyn FsSource> = Arc::new(
            FixtureFs::new()
                .file("/proc/a", "abc")
                .link("/proc/link", "/target")
                .inode("/proc", 7)
                .mount("/", 100, 40, 30),
        );
        let fs = CountingFs::new(base);

        assert_eq!(fs.read(Path::new("/proc/a"), 2).expect("read"), b"ab");
        assert_eq!(
            fs.read_link(Path::new("/proc/link")).expect("read_link"),
            PathBuf::from("/target")
        );
        let (entries, _) = fs
            .read_dir_capped(Path::new("/proc"), 16)
            .expect("scan_dir");
        assert_eq!(fs.inode(Path::new("/proc")).expect("inode"), 7);
        assert_eq!(fs.statfs(Path::new("/")).expect("statfs").total, 100);

        assert_eq!(
            fs.counts(),
            FsCallCounts {
                read: 1,
                read_dir: 1,
                read_link: 1,
                inode: 1,
                statfs: 1,
                // Записи каталога считаются отдельно: бюджет измеряется
                // работой, а не числом открытий каталога.
                dir_entries: entries.len() as u64,
            }
        );
        let calls = fs.calls();
        assert_eq!(calls.len(), 5, "записи каталога в журнал не пишутся");
        assert!(calls.iter().any(|call| {
            call.operation == FsOperation::Read && call.path == "/proc/a" && call.cap == Some(2)
        }));

        fs.reset();
        assert_eq!(fs.counts(), FsCallCounts::default());
        assert!(fs.calls().is_empty());
    }

    #[test]
    fn scripted_fs_sequences_and_repeats_last_result() {
        let base: Arc<dyn FsSource> = Arc::new(
            FixtureFs::new()
                .file("/proc/123/stat", "base")
                .link("/proc/123/exe", "/base")
                .file("/proc/123/status", "status"),
        );
        let fs = ScriptedFs::new(base)
            .read_sequence(
                "/proc/123/stat",
                [
                    ScriptedRead::text("old"),
                    ScriptedRead::text("new"),
                    ScriptedRead::PermissionDenied,
                ],
            )
            .link_sequence(
                "/proc/123/exe",
                [
                    ScriptedLink::Path(PathBuf::from("/old")),
                    ScriptedLink::NotFound,
                ],
            )
            .dir_sequence(
                "/proc",
                [
                    ScriptedDir::Entries(vec![OsString::from("123")]),
                    ScriptedDir::Io(io::ErrorKind::Interrupted),
                ],
            );

        assert_eq!(
            fs.read(Path::new("/proc/123/stat"), 64).expect("old"),
            b"old"
        );
        assert_eq!(
            fs.read(Path::new("/proc/123/stat"), 64).expect("new"),
            b"new"
        );
        assert_eq!(
            fs.read(Path::new("/proc/123/stat"), 64)
                .expect_err("permission")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        // После исчерпания повторяется последний scripted результат.
        assert_eq!(
            fs.read(Path::new("/proc/123/stat"), 64)
                .expect_err("repeat last")
                .kind(),
            io::ErrorKind::PermissionDenied
        );

        assert_eq!(
            fs.read_link(Path::new("/proc/123/exe")).expect("old link"),
            PathBuf::from("/old")
        );
        assert_eq!(
            fs.read_link(Path::new("/proc/123/exe"))
                .expect_err("link not found")
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            fs.read_dir_capped(Path::new("/proc"), 16)
                .expect("first dir")
                .0,
            vec![OsString::from("123")]
        );
        assert_eq!(
            fs.read_dir_capped(Path::new("/proc"), 16)
                .expect_err("second dir")
                .kind(),
            io::ErrorKind::Interrupted
        );

        // Нескриптованный путь идёт в base.
        assert_eq!(
            fs.read(Path::new("/proc/123/status"), 64)
                .expect("base fallback"),
            b"status"
        );
    }

    #[test]
    fn scripted_read_obeys_call_cap() {
        let fs = ScriptedFs::new(Arc::new(FixtureFs::new()))
            .read_sequence("/proc/value", [ScriptedRead::text("abcdefgh")]);
        assert_eq!(
            fs.read(Path::new("/proc/value"), 3).expect("capped"),
            b"abc"
        );
    }

    /// Каждый вид отказа обязан быть воспроизводим: коллекторы должны
    /// отличать «файла нет» от «нет прав» и от прерванного чтения, а тест
    /// без этих вариантов не смог бы такой разбор проверить.
    #[test]
    fn scripted_fs_reproduces_every_failure_kind() {
        let fs = ScriptedFs::new(Arc::new(FixtureFs::new()))
            .read_sequence(
                "/proc/gone",
                [
                    ScriptedRead::NotFound,
                    ScriptedRead::Io(io::ErrorKind::Interrupted),
                ],
            )
            .link_sequence(
                "/proc/denied",
                [
                    ScriptedLink::PermissionDenied,
                    ScriptedLink::Io(io::ErrorKind::InvalidData),
                ],
            )
            .dir_sequence(
                "/proc/dir",
                [ScriptedDir::NotFound, ScriptedDir::PermissionDenied],
            );

        let kinds = [
            fs.read(Path::new("/proc/gone"), 8)
                .expect_err("gone")
                .kind(),
            fs.read(Path::new("/proc/gone"), 8).expect_err("io").kind(),
        ];
        assert_eq!(kinds, [io::ErrorKind::NotFound, io::ErrorKind::Interrupted]);

        let link_kinds = [
            fs.read_link(Path::new("/proc/denied"))
                .expect_err("denied")
                .kind(),
            fs.read_link(Path::new("/proc/denied"))
                .expect_err("invalid")
                .kind(),
        ];
        assert_eq!(
            link_kinds,
            [io::ErrorKind::PermissionDenied, io::ErrorKind::InvalidData]
        );

        let dir_kinds = [
            fs.read_dir_capped(Path::new("/proc/dir"), 8)
                .expect_err("dir gone")
                .kind(),
            fs.read_dir_capped(Path::new("/proc/dir"), 8)
                .expect_err("dir denied")
                .kind(),
        ];
        assert_eq!(
            dir_kinds,
            [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied]
        );
    }
}
