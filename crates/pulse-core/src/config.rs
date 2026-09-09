//! Конфигурация. Значения по умолчанию безопасны: локальный bind, redaction включена,
//! действия выключены, экспорт метрик процессов выключен.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::redact::RedactMode;

/// Корневая конфигурация агента.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub security: Security,
    pub store: Store,
    pub export: Export,
    pub rules: Rules,
    pub ui: Ui,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    /// Интервал такта сбора в миллисекундах.
    pub interval_ms: u64,
    /// Собирать ли метрики отдельных процессов (дороже всего по CPU и памяти).
    pub collect_processes: bool,
    /// Максимальное число процессов, обрабатываемых за такт. Защита от хоста
    /// с десятками тысяч процессов: берутся самые тяжёлые по CPU/RSS.
    pub max_processes: usize,
    /// Максимальное число cgroup, обходимых за такт.
    pub max_cgroups: usize,
    /// Корень procfs. Меняется в тестах и при работе в контейнере.
    pub proc_root: PathBuf,
    /// Корень sysfs.
    pub sys_root: PathBuf,
    /// Точка монтирования cgroup v2.
    pub cgroup_root: PathBuf,
}

impl Default for General {
    fn default() -> Self {
        General {
            interval_ms: 1_000,
            collect_processes: true,
            max_processes: 4_096,
            max_cgroups: 2_048,
            proc_root: PathBuf::from("/proc"),
            sys_root: PathBuf::from("/sys"),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Security {
    /// Политика сокрытия секретов в командных строках.
    pub redact: RedactMode,
    /// Разрешить ли изменение состояния системы (посылка сигналов).
    /// По умолчанию выключено: инструмент только наблюдает.
    pub allow_actions: bool,
    /// Читать ли командные строки вообще. `false` — показывать только `comm`.
    pub read_cmdline: bool,
    /// Дополнительная эвристика: скрывать значения с высокой энтропией.
    /// Выключена по умолчанию — даёт ложные срабатывания на хешах и UUID.
    pub redact_high_entropy: bool,
}

impl Default for Security {
    fn default() -> Self {
        Security {
            redact: RedactMode::Secrets,
            allow_actions: false,
            read_cmdline: true,
            redact_high_entropy: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Store {
    /// Глубина «горячего» кольца в тактах: сырые значения по всем сериям.
    pub hot_ticks: usize,
    /// Глубина «тёплого» слоя в бакетах для сущностей-владельцев.
    pub warm_buckets: usize,
    /// Размер тёплого бакета в тактах (10 тактов по 1 с = 10-секундное окно).
    pub warm_bucket_ticks: usize,
    /// Максимум событий в журнале.
    pub max_events: usize,
    /// Максимум записей жизненного цикла сущностей.
    pub max_entity_records: usize,
    /// Жёсткий предел числа серий. Защита от неограниченного роста памяти.
    pub max_series: usize,
    /// Потолок памяти истории в байтах.
    ///
    /// Числа серий и глубины кольца недостаточно: сколько именно памяти займёт
    /// история, зависит от числа сущностей на хосте, а его агент не выбирает.
    /// При превышении потолка тёплый слой отдаёт самые старые бакеты, то есть
    /// сокращается глубина истории, а не растёт RSS. `0` отключает проверку.
    pub max_bytes: u64,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            hot_ticks: 300,
            warm_buckets: 360,
            warm_bucket_ticks: 10,
            max_events: 8_192,
            max_entity_records: 32_768,
            max_series: 200_000,
            // 64 MiB: на типовом хосте (порядка 3 500 серий) плато истории
            // около 34 MiB, поэтому потолок оставляет запас на разброс числа
            // сущностей, но не даёт превратиться в сотни мегабайт.
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Export {
    /// Включён ли HTTP-эндпоинт.
    pub enabled: bool,
    /// Адрес прослушивания. По умолчанию только loopback.
    pub bind: SocketAddr,
    /// Файл с bearer-токеном. Обязателен для не-loopback адреса.
    pub token_file: Option<PathBuf>,
    /// Экспортировать ли метрики процессов (высокая cardinality).
    pub processes: ProcessExport,
    /// Максимум серий в одном ответе.
    pub max_series: usize,
    /// Максимум серий на одну метрику.
    pub max_series_per_metric: usize,
    /// Максимум запросов в минуту с одного адреса.
    pub rate_limit_per_minute: u32,
}

impl Default for Export {
    fn default() -> Self {
        Export {
            enabled: true,
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9099),
            token_file: None,
            processes: ProcessExport::default(),
            max_series: 20_000,
            max_series_per_metric: 2_000,
            rate_limit_per_minute: 120,
        }
    }
}

/// Режим экспорта метрик уровня процесса.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProcessExport {
    /// `none` — не экспортировать; `top` — только N самых тяжёлых.
    pub mode: ProcessExportMode,
    pub limit: usize,
}

impl Default for ProcessExport {
    fn default() -> Self {
        ProcessExport {
            mode: ProcessExportMode::None,
            limit: 50,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessExportMode {
    #[default]
    None,
    Top,
}

/// Пороги правил. Значения — доли (0..1) либо натуральные единицы, указанные в имени.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    /// Сколько подряд тактов условие должно выполняться до открытия проблемы.
    pub enter_after_ticks: u32,
    /// Сколько подряд тактов условие снятия должно выполняться до закрытия.
    pub clear_after_ticks: u32,

    pub psi_cpu_warn: f64,
    pub psi_cpu_crit: f64,
    pub psi_cpu_clear: f64,

    pub psi_memory_warn: f64,
    pub psi_memory_crit: f64,
    pub psi_memory_clear: f64,

    pub psi_io_warn: f64,
    pub psi_io_crit: f64,
    pub psi_io_clear: f64,

    pub throttle_warn: f64,
    pub throttle_crit: f64,
    pub throttle_clear: f64,

    pub memory_util_warn: f64,
    pub memory_util_crit: f64,
    pub memory_util_clear: f64,

    pub disk_await_warn_ms: f64,
    pub disk_await_crit_ms: f64,
    pub disk_await_clear_ms: f64,

    pub fd_util_warn: f64,
    pub fd_util_crit: f64,
    pub fd_util_clear: f64,

    pub swap_util_warn: f64,
    pub swap_util_crit: f64,
    pub swap_util_clear: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            enter_after_ticks: 3,
            clear_after_ticks: 15,

            psi_cpu_warn: 0.20,
            psi_cpu_crit: 0.50,
            psi_cpu_clear: 0.10,

            psi_memory_warn: 0.10,
            psi_memory_crit: 0.25,
            psi_memory_clear: 0.05,

            psi_io_warn: 0.10,
            psi_io_crit: 0.30,
            psi_io_clear: 0.05,

            throttle_warn: 0.10,
            throttle_crit: 0.25,
            throttle_clear: 0.05,

            memory_util_warn: 0.85,
            memory_util_crit: 0.95,
            memory_util_clear: 0.80,

            disk_await_warn_ms: 20.0,
            disk_await_crit_ms: 50.0,
            disk_await_clear_ms: 10.0,

            fd_util_warn: 0.80,
            fd_util_crit: 0.95,
            fd_util_clear: 0.70,

            swap_util_warn: 0.25,
            swap_util_crit: 0.60,
            swap_util_clear: 0.15,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    /// Частота перерисовки в миллисекундах.
    pub refresh_ms: u64,
    /// Только ASCII-графика (для терминалов без UTF-8).
    pub ascii: bool,
    /// Показывать метрики агента на экране Overview.
    pub show_self_metrics: bool,
    /// Набор иконок категорий в пайпе расследования.
    ///
    /// `off` — без иконок; `nerd` — глифы Nerd Font (нужен патченный шрифт,
    /// вариант `Mono`); `unicode` — геометрические символы обычного шрифта,
    /// если патченного шрифта нет. По умолчанию `off`: наличие глифа
    /// в шрифте из терминала измерить нельзя (приложение общается
    /// с эмулятором, а не со шрифтом), поэтому выбор остаётся за оператором.
    /// Что установить — в `docs/GRAPH-VIEW.md`.
    pub icons: IconSet,
}

/// Какой набор иконок рисовать.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IconSet {
    #[default]
    Off,
    /// Глифы Nerd Font: приватная область Unicode, нужен патченный шрифт.
    Nerd,
    /// Геометрические символы обычного моноширинного шрифта.
    Unicode,
}

impl<'de> Deserialize<'de> for IconSet {
    /// Принимает и строку, и логическое значение.
    ///
    /// `icons = true` успел попасть в конфигурации до появления наборов,
    /// и ломать чужой файл из-за собственного переименования нельзя:
    /// `true` означает набор обычного Unicode, как и раньше.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Flag(bool),
            Name(String),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Flag(true) => Ok(Self::Unicode),
            Raw::Flag(false) => Ok(Self::Off),
            Raw::Name(name) => match name.trim().to_ascii_lowercase().as_str() {
                "off" | "none" | "false" => Ok(Self::Off),
                "nerd" | "nerdfont" | "nerd_font" => Ok(Self::Nerd),
                "unicode" | "geometric" | "true" => Ok(Self::Unicode),
                other => Err(serde::de::Error::custom(format!(
                    "неизвестный набор иконок {other:?}: допустимы off, nerd, unicode"
                ))),
            },
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            refresh_ms: 500,
            ascii: false,
            show_self_metrics: false,
            icons: IconSet::Off,
        }
    }
}

/// Ошибка валидации конфигурации.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("интервал сбора должен быть в диапазоне 100..=60000 мс, получено {0}")]
    Interval(u64),
    #[error("hot_ticks должен быть в диапазоне 10..=100000, получено {0}")]
    HotTicks(usize),
    #[error(
        "прослушивание не-loopback адреса {0} требует security-токена: задайте export.token_file"
    )]
    RemoteBindWithoutToken(SocketAddr),
    #[error("порог {name}: значение {value} вне допустимого диапазона {min}..={max}")]
    Threshold {
        name: &'static str,
        value: f64,
        min: f64,
        max: f64,
    },
    #[error("порог входа ({enter}) должен быть строго выше порога снятия ({clear}) для {name}")]
    Hysteresis {
        name: &'static str,
        enter: f64,
        clear: f64,
    },
    #[error(
        "критический порог ({crit}) должен быть не ниже порога предупреждения ({warn}) для {name}"
    )]
    Severity {
        name: &'static str,
        warn: f64,
        crit: f64,
    },
    #[error("{name} = 0 делает подсистему неработоспособной: {consequence}")]
    ZeroLimit {
        name: &'static str,
        consequence: &'static str,
    },
}

impl Config {
    /// Проверяет конфигурацию. Отказ в старте лучше молчаливого небезопасного режима.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(100..=60_000).contains(&self.general.interval_ms) {
            return Err(ConfigError::Interval(self.general.interval_ms));
        }
        if !(10..=100_000).contains(&self.store.hot_ticks) {
            return Err(ConfigError::HotTicks(self.store.hot_ticks));
        }

        // Безопасность: открытый наружу порт без аутентификации не допускается.
        if self.export.enabled
            && !self.export.bind.ip().is_loopback()
            && self.export.token_file.is_none()
        {
            return Err(ConfigError::RemoteBindWithoutToken(self.export.bind));
        }

        let r = &self.rules;
        let ratios: &[(&'static str, f64)] = &[
            ("psi_cpu_warn", r.psi_cpu_warn),
            ("psi_cpu_crit", r.psi_cpu_crit),
            ("psi_cpu_clear", r.psi_cpu_clear),
            ("psi_memory_warn", r.psi_memory_warn),
            ("psi_memory_crit", r.psi_memory_crit),
            ("psi_memory_clear", r.psi_memory_clear),
            ("psi_io_warn", r.psi_io_warn),
            ("psi_io_crit", r.psi_io_crit),
            ("psi_io_clear", r.psi_io_clear),
            ("throttle_warn", r.throttle_warn),
            ("throttle_crit", r.throttle_crit),
            ("throttle_clear", r.throttle_clear),
            ("memory_util_warn", r.memory_util_warn),
            ("memory_util_crit", r.memory_util_crit),
            ("memory_util_clear", r.memory_util_clear),
            ("fd_util_warn", r.fd_util_warn),
            ("fd_util_crit", r.fd_util_crit),
            ("fd_util_clear", r.fd_util_clear),
            ("swap_util_warn", r.swap_util_warn),
            ("swap_util_crit", r.swap_util_crit),
            ("swap_util_clear", r.swap_util_clear),
        ];
        for (name, value) in ratios {
            if !value.is_finite() || *value < 0.0 || *value > 1.0 {
                return Err(ConfigError::Threshold {
                    name,
                    value: *value,
                    min: 0.0,
                    max: 1.0,
                });
            }
        }
        for (name, value) in [
            ("disk_await_warn_ms", r.disk_await_warn_ms),
            ("disk_await_crit_ms", r.disk_await_crit_ms),
            ("disk_await_clear_ms", r.disk_await_clear_ms),
        ] {
            if !value.is_finite() || !(0.0..=600_000.0).contains(&value) {
                return Err(ConfigError::Threshold {
                    name,
                    value,
                    min: 0.0,
                    max: 600_000.0,
                });
            }
        }

        // Гистерезис имеет смысл только при warn > clear.
        let pairs: &[(&'static str, f64, f64)] = &[
            ("psi_cpu", r.psi_cpu_warn, r.psi_cpu_clear),
            ("psi_memory", r.psi_memory_warn, r.psi_memory_clear),
            ("psi_io", r.psi_io_warn, r.psi_io_clear),
            ("throttle", r.throttle_warn, r.throttle_clear),
            ("memory_util", r.memory_util_warn, r.memory_util_clear),
            ("disk_await", r.disk_await_warn_ms, r.disk_await_clear_ms),
            ("fd_util", r.fd_util_warn, r.fd_util_clear),
            ("swap_util", r.swap_util_warn, r.swap_util_clear),
        ];
        for (name, enter, clear) in pairs {
            if enter <= clear {
                return Err(ConfigError::Hysteresis {
                    name,
                    enter: *enter,
                    clear: *clear,
                });
            }
        }

        // Критический порог ниже порога предупреждения делает уровень
        // недостижимым: вход в проблему строже самой критики, поэтому
        // правило никогда не сообщит crit.
        let severities: &[(&'static str, f64, f64)] = &[
            ("psi_cpu", r.psi_cpu_warn, r.psi_cpu_crit),
            ("psi_memory", r.psi_memory_warn, r.psi_memory_crit),
            ("psi_io", r.psi_io_warn, r.psi_io_crit),
            ("throttle", r.throttle_warn, r.throttle_crit),
            ("memory_util", r.memory_util_warn, r.memory_util_crit),
            ("disk_await", r.disk_await_warn_ms, r.disk_await_crit_ms),
            ("fd_util", r.fd_util_warn, r.fd_util_crit),
            ("swap_util", r.swap_util_warn, r.swap_util_crit),
        ];
        for (name, warn, crit) in severities {
            if crit < warn {
                return Err(ConfigError::Severity {
                    name,
                    warn: *warn,
                    crit: *crit,
                });
            }
        }

        // Ноль в операционных лимитах молча ломает подсистему. Отказ в
        // старте честнее: иначе сервер поднимается и на каждый запрос
        // отвечает 429, либо отдаёт пустой `/metrics`, либо история
        // отвергает каждую новую серию.
        let zero_limits: &[(&'static str, bool, &'static str)] = &[
            (
                "export.rate_limit_per_minute",
                self.export.enabled && self.export.rate_limit_per_minute == 0,
                "ни один запрос не получает токен, каждый ответ 429",
            ),
            (
                "export.max_series",
                self.export.enabled && self.export.max_series == 0,
                "ответ /metrics не содержит ни одной серии",
            ),
            (
                "export.max_series_per_metric",
                self.export.enabled && self.export.max_series_per_metric == 0,
                "ни одна метрика не попадает в ответ",
            ),
            (
                "store.max_series",
                self.store.max_series == 0,
                "история отвергает каждую новую серию",
            ),
        ];
        for (name, broken, consequence) in zero_limits {
            if *broken {
                return Err(ConfigError::ZeroLimit { name, consequence });
            }
        }

        Ok(())
    }

    /// Такт в секундах — используется при расчёте производных величин.
    #[must_use]
    pub fn interval_secs(&self) -> f64 {
        self.general.interval_ms as f64 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default()
            .validate()
            .expect("дефолт должен быть валиден");
    }

    #[test]
    fn defaults_are_secure() {
        let c = Config::default();
        assert!(
            c.export.bind.ip().is_loopback(),
            "bind обязан быть локальным"
        );
        assert_eq!(c.security.redact, RedactMode::Secrets);
        assert!(!c.security.allow_actions, "действия выключены по умолчанию");
        assert_eq!(c.export.processes.mode, ProcessExportMode::None);
    }

    #[test]
    fn remote_bind_without_token_is_rejected() {
        let mut c = Config::default();
        c.export.bind = "0.0.0.0:9099".parse().expect("адрес");
        let err = c.validate().expect_err("должно быть отклонено");
        assert!(matches!(err, ConfigError::RemoteBindWithoutToken(_)));
    }

    #[test]
    fn remote_bind_with_token_is_allowed() {
        let mut c = Config::default();
        c.export.bind = "0.0.0.0:9099".parse().expect("адрес");
        c.export.token_file = Some(PathBuf::from("/etc/pulse/token"));
        c.validate().expect("с токеном допустимо");
    }

    #[test]
    fn inverted_hysteresis_is_rejected() {
        let mut c = Config::default();
        c.rules.psi_io_clear = c.rules.psi_io_warn + 0.1;
        assert!(matches!(
            c.validate(),
            Err(ConfigError::Hysteresis { name: "psi_io", .. })
        ));
    }

    #[test]
    fn critical_threshold_must_not_be_below_warning() {
        let cases = [
            ("psi_cpu", {
                let mut c = Config::default();
                c.rules.psi_cpu_crit = c.rules.psi_cpu_warn - 0.01;
                c
            }),
            ("psi_memory", {
                let mut c = Config::default();
                c.rules.psi_memory_crit = c.rules.psi_memory_warn - 0.01;
                c
            }),
            ("psi_io", {
                let mut c = Config::default();
                c.rules.psi_io_crit = c.rules.psi_io_warn - 0.01;
                c
            }),
            ("throttle", {
                let mut c = Config::default();
                c.rules.throttle_crit = c.rules.throttle_warn - 0.01;
                c
            }),
            ("memory_util", {
                let mut c = Config::default();
                c.rules.memory_util_crit = c.rules.memory_util_warn - 0.01;
                c
            }),
            ("disk_await", {
                let mut c = Config::default();
                c.rules.disk_await_crit_ms = c.rules.disk_await_warn_ms - 0.1;
                c
            }),
            ("fd_util", {
                let mut c = Config::default();
                c.rules.fd_util_crit = c.rules.fd_util_warn - 0.01;
                c
            }),
            ("swap_util", {
                let mut c = Config::default();
                c.rules.swap_util_crit = c.rules.swap_util_warn - 0.01;
                c
            }),
        ];
        for (expected, config) in cases {
            assert!(
                matches!(
                    config.validate(),
                    Err(ConfigError::Severity { name, .. }) if name == expected
                ),
                "{expected}: crit ниже warn обязан быть отклонён"
            );
        }
    }

    #[test]
    fn zero_operational_limits_are_rejected_only_when_relevant() {
        let mut rate = Config::default();
        rate.export.rate_limit_per_minute = 0;
        assert!(matches!(
            rate.validate(),
            Err(ConfigError::ZeroLimit {
                name: "export.rate_limit_per_minute",
                ..
            })
        ));

        let mut disabled_export = rate.clone();
        disabled_export.export.enabled = false;
        disabled_export
            .validate()
            .expect("выключенный exporter не использует свои лимиты");

        let mut global = Config::default();
        global.export.max_series = 0;
        assert!(matches!(
            global.validate(),
            Err(ConfigError::ZeroLimit {
                name: "export.max_series",
                ..
            })
        ));

        let mut per_metric = Config::default();
        per_metric.export.max_series_per_metric = 0;
        assert!(matches!(
            per_metric.validate(),
            Err(ConfigError::ZeroLimit {
                name: "export.max_series_per_metric",
                ..
            })
        ));

        let mut store = Config::default();
        store.store.max_series = 0;
        assert!(matches!(
            store.validate(),
            Err(ConfigError::ZeroLimit {
                name: "store.max_series",
                ..
            })
        ));
    }

    #[test]
    fn out_of_range_threshold_is_rejected() {
        let mut c = Config::default();
        c.rules.psi_cpu_warn = 2.0;
        assert!(matches!(c.validate(), Err(ConfigError::Threshold { .. })));
    }

    #[test]
    fn nan_threshold_is_rejected() {
        let mut c = Config::default();
        c.rules.psi_cpu_warn = f64::NAN;
        assert!(matches!(c.validate(), Err(ConfigError::Threshold { .. })));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let toml = "[general]\ninterval_ms = 1000\nunknown_key = 5\n";
        let parsed: Result<Config, _> = toml::from_str(toml);
        assert!(parsed.is_err(), "неизвестные ключи должны отклоняться");
    }

    #[test]
    fn config_roundtrips_through_toml() {
        let c = Config::default();
        let text = toml::to_string(&c).expect("сериализация");
        let back: Config = toml::from_str(&text).expect("разбор");
        assert_eq!(back.general.interval_ms, c.general.interval_ms);
        assert_eq!(back.security.redact, c.security.redact);
    }
}
