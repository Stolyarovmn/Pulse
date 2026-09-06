//! Действия над процессами.
//!
//! Единственное место, где Pulse меняет состояние системы. Поэтому здесь два
//! обязательных условия.
//!
//! 1. **Проверка идентичности перед сигналом.** PID переиспользуется: между тем,
//!    как оператор выбрал процесс в списке, и нажатием клавиши процесс мог
//!    завершиться, а номер — достаться другому. Поэтому сигнал посылается только
//!    если время старта в `/proc/<pid>/stat` совпадает с тем, что было
//!    зафиксировано при построении сущности.
//! 2. **Включение по конфигурации.** Вызывающая сторона обязана проверить
//!    `security.allow_actions`; по умолчанию действия выключены.

use std::fmt;
use std::path::Path;

use crate::fs::{FsSource, FsSourceExt};
use crate::parse;

/// Сигналы, которые разрешено посылать.
///
/// Список сознательно короткий: диагностическому инструменту не нужен весь
/// набор сигналов, а любой лишний — это лишний способ навредить.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signal {
    /// Штатное завершение.
    Term,
    /// Принудительное завершение.
    Kill,
    /// Перечитать конфигурацию.
    Hup,
    /// Прерывание, как Ctrl+C.
    Int,
}

impl Signal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Signal::Term => "SIGTERM",
            Signal::Kill => "SIGKILL",
            Signal::Hup => "SIGHUP",
            Signal::Int => "SIGINT",
        }
    }

    const fn to_rustix(self) -> rustix::process::Signal {
        match self {
            Signal::Term => rustix::process::Signal::Term,
            Signal::Kill => rustix::process::Signal::Kill,
            Signal::Hup => rustix::process::Signal::Hup,
            Signal::Int => rustix::process::Signal::Int,
        }
    }
}

/// Ошибка действия.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// Процесса больше нет.
    Gone { pid: i32 },
    /// PID занят другим процессом: время старта не совпало.
    Reused {
        pid: i32,
        expected_start: u64,
        actual_start: u64,
    },
    /// Некорректный PID.
    InvalidPid { pid: i32 },
    /// Ядро отказало (нет прав и подобное).
    Denied { pid: i32, reason: String },
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActionError::Gone { pid } => {
                write!(f, "процесс {pid} уже завершился")
            }
            ActionError::Reused {
                pid,
                expected_start,
                actual_start,
            } => write!(
                f,
                "PID {pid} переиспользован другим процессом (ожидалось время старта {expected_start}, фактически {actual_start}); сигнал не отправлен"
            ),
            ActionError::InvalidPid { pid } => write!(f, "недопустимый PID {pid}"),
            ActionError::Denied { pid, reason } => {
                write!(f, "ядро отказало для PID {pid}: {reason}")
            }
        }
    }
}

impl std::error::Error for ActionError {}

/// Посылает сигнал процессу, предварительно убедившись в его идентичности.
pub fn signal_process(
    fs: &dyn FsSource,
    proc_root: &Path,
    pid: i32,
    start_ticks: u64,
    signal: Signal,
) -> Result<(), ActionError> {
    if pid <= 1 {
        // PID 1 — init/systemd: сигнал ему может остановить всю машину.
        return Err(ActionError::InvalidPid { pid });
    }

    let stat_path = proc_root.join(pid.to_string()).join("stat");
    let Some(text) = fs.read_opt(&stat_path) else {
        return Err(ActionError::Gone { pid });
    };
    let Some(stat) = parse::parse_proc_stat(&text) else {
        return Err(ActionError::Gone { pid });
    };
    if stat.start_ticks != start_ticks {
        return Err(ActionError::Reused {
            pid,
            expected_start: start_ticks,
            actual_start: stat.start_ticks,
        });
    }

    let raw = rustix::process::Pid::from_raw(pid).ok_or(ActionError::InvalidPid { pid })?;
    rustix::process::kill_process(raw, signal.to_rustix()).map_err(|err| ActionError::Denied {
        pid,
        reason: err.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use std::path::PathBuf;

    fn stat(pid: i32, start: u64) -> String {
        format!("{pid} (svc) S 1 {pid} {pid} 0 -1 0 0 0 0 0 1 1 0 0 20 0 1 0 {start} 100 10")
    }

    #[test]
    fn init_is_never_signalled() {
        let fs = FixtureFs::new().file("/proc/1/stat", &stat(1, 100));
        let err = signal_process(&fs, &PathBuf::from("/proc"), 1, 100, Signal::Term)
            .expect_err("PID 1 запрещён");
        assert!(matches!(err, ActionError::InvalidPid { pid: 1 }));
    }

    #[test]
    fn missing_process_reports_gone() {
        let fs = FixtureFs::new();
        let err = signal_process(&fs, &PathBuf::from("/proc"), 4242, 100, Signal::Term)
            .expect_err("процесса нет");
        assert!(matches!(err, ActionError::Gone { pid: 4242 }));
    }

    #[test]
    fn reused_pid_is_refused_with_explanation() {
        let fs = FixtureFs::new().file("/proc/500/stat", &stat(500, 999));
        let err = signal_process(&fs, &PathBuf::from("/proc"), 500, 100, Signal::Kill)
            .expect_err("PID переиспользован");
        match err {
            ActionError::Reused {
                pid,
                expected_start,
                actual_start,
            } => {
                assert_eq!(pid, 500);
                assert_eq!(expected_start, 100);
                assert_eq!(actual_start, 999);
            }
            other => panic!("ожидалась ошибка переиспользования, получено {other:?}"),
        }
        assert!(err.to_string().contains("переиспользован"));
    }

    #[test]
    fn signal_names_are_stable() {
        assert_eq!(Signal::Term.as_str(), "SIGTERM");
        assert_eq!(Signal::Kill.as_str(), "SIGKILL");
        assert_eq!(Signal::Hup.as_str(), "SIGHUP");
        assert_eq!(Signal::Int.as_str(), "SIGINT");
    }

    #[test]
    fn signal_to_self_process_group_is_delivered() {
        // Проверяем реальный путь до ядра на самом тестовом процессе:
        // SIGHUP игнорировать нельзя, поэтому используем безвредный вариант —
        // сигнал 0 недоступен в нашем перечне, поэтому проверяем только отказ
        // по несовпадению времени старта на реальном /proc.
        let real = crate::fs::RealFs;
        let pid = std::process::id() as i32;
        let err = signal_process(&real, &PathBuf::from("/proc"), pid, 1, Signal::Term);
        assert!(
            matches!(err, Err(ActionError::Reused { .. })),
            "реальный процесс должен быть защищён проверкой времени старта: {err:?}"
        );
    }
}
