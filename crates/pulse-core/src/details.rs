//! Терминальный уровень расследования: детали одного процесса.
//!
//! Цепочка `host → сервис → cgroup → процесс` обязана заканчиваться ответом, а
//! не ещё одной таблицей чисел. Ответ — это пользователь, исполняемый файл,
//! рабочий каталог, открытые файлы и слушающие порты.
//!
//! Здесь только домен: типы и контракт источника. Чтение `/proc` живёт в
//! `pulse-collect`, интерфейс зависит от этого контракта, а не от файловой
//! системы.
//!
//! Детали читаются **по требованию**. Перечислить `/proc/<pid>/fd` и
//! разыменовать каждую ссылку для двух тысяч процессов — десятки тысяч syscall
//! на такт, то есть агент сам становится проблемой. Оператор смотрит один
//! процесс и только когда до него дошёл.

use std::collections::HashMap;

/// Слушающий сокет процесса.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    /// `tcp`, `tcp6`, `udp`, `udp6`.
    pub protocol: &'static str,
    /// Локальный адрес в человекочитаемом виде.
    pub address: String,
    pub port: u16,
}

/// Открытый дескриптор.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenFile {
    pub fd: u32,
    /// Цель ссылки: путь, `socket:[inode]`, `pipe:[inode]`.
    pub target: String,
}

impl OpenFile {
    /// Обычный файл, а не сокет, канал или anon-inode.
    #[must_use]
    pub fn is_regular(&self) -> bool {
        self.target.starts_with('/')
    }
}

/// Детали процесса для терминального уровня расследования.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessDetails {
    pub uid: Option<u32>,
    pub user: Option<String>,
    pub exe: Option<String>,
    pub cwd: Option<String>,
    /// Сколько дескрипторов открыто всего.
    pub fd_total: usize,
    /// Дескрипторы-файлы, отсортированные по номеру, ограниченные источником.
    pub files: Vec<OpenFile>,
    /// Слушающие сокеты, принадлежащие процессу.
    pub ports: Vec<Port>,
    /// Ядро отказало в доступе к дескрипторам процесса.
    ///
    /// Отличать отказ от пустоты обязательно: непривилегированный агент не
    /// видит `/proc/<pid>/fd` чужого пользователя, и молчаливо пустой список
    /// читался бы как «процесс не держит ни файлов, ни портов».
    pub restricted: bool,
}

impl ProcessDetails {
    /// Есть ли хоть один факт: пустые детали не занимают место на экране.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.restricted
            && self.user.is_none()
            && self.exe.is_none()
            && self.cwd.is_none()
            && self.files.is_empty()
            && self.ports.is_empty()
            && self.fd_total == 0
    }
}

/// Источник деталей процесса.
///
/// Трейт, а не свободная функция: интерфейс не должен знать о файловой системе,
/// а тесты обязаны подставлять фикстуру без `/proc`.
pub trait ProcessDetailsSource: Send + Sync + std::fmt::Debug {
    fn details(&self, pid: i32) -> ProcessDetails;
}

/// Кэш деталей: одно чтение на процесс, пока снимок не сменился.
///
/// Без кэша каждая перерисовка кадра означала бы новый обход `/proc/<pid>/fd`,
/// то есть нажатие любой клавиши стоило бы десятки syscall.
#[derive(Clone, Debug, Default)]
pub struct DetailsCache {
    entries: HashMap<i32, ProcessDetails>,
}

impl DetailsCache {
    pub fn get(&mut self, source: &dyn ProcessDetailsSource, pid: i32) -> ProcessDetails {
        if let Some(cached) = self.entries.get(&pid) {
            return cached.clone();
        }
        let details = source.details(pid);
        let _ = self.entries.insert(pid, details.clone());
        details
    }

    /// Сбрасывает кэш: вызывается при смене снимка.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    struct Counting {
        calls: AtomicUsize,
    }

    impl ProcessDetailsSource for Counting {
        fn details(&self, pid: i32) -> ProcessDetails {
            let _ = self.calls.fetch_add(1, Ordering::Relaxed);
            ProcessDetails {
                uid: Some(pid as u32),
                ..ProcessDetails::default()
            }
        }
    }

    #[test]
    fn cache_reads_once_per_pid() {
        let source = Counting::default();
        let mut cache = DetailsCache::default();
        assert_eq!(cache.get(&source, 1).uid, Some(1));
        assert_eq!(cache.get(&source, 1).uid, Some(1));
        let _ = cache.get(&source, 2);
        assert_eq!(
            source.calls.load(Ordering::Relaxed),
            2,
            "повторный запрос того же процесса не читает /proc снова"
        );
    }

    #[test]
    fn cache_clear_forces_reread() {
        let source = Counting::default();
        let mut cache = DetailsCache::default();
        let _ = cache.get(&source, 1);
        cache.clear();
        let _ = cache.get(&source, 1);
        assert_eq!(
            source.calls.load(Ordering::Relaxed),
            2,
            "новый снимок обязан давать новые детали"
        );
    }

    #[test]
    fn empty_details_are_recognised() {
        assert!(ProcessDetails::default().is_empty());
        let details = ProcessDetails {
            user: Some("root".into()),
            ..ProcessDetails::default()
        };
        assert!(!details.is_empty());
    }

    #[test]
    fn regular_file_is_distinguished_from_socket() {
        assert!(OpenFile {
            fd: 1,
            target: "/var/log/app.log".into()
        }
        .is_regular());
        assert!(!OpenFile {
            fd: 2,
            target: "socket:[1234]".into()
        }
        .is_regular());
    }
}
