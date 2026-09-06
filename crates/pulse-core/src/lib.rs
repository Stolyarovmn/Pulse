//! `pulse-core` — доменная модель Pulse.
//!
//! Центральная идея: в основе не `HashMap<MetricName, TimeSeries>`, а **temporal
//! entity graph**. Сущности (`host`, `cgroup`, `unit`, `process`, `container`,
//! `disk`, `netif`) имеют устойчивую идентичность и время жизни; телеметрия
//! ссылается на сущность; события описывают изменения графа. Из этой модели
//! естественно выводятся навигация в TUI, семантический A/B diff, корреляция
//! проблем и совместимое с OpenMetrics представление.
//!
//! Границы ответственности:
//!
//! | Крейт | Отвечает за |
//! |---|---|
//! | `pulse-core` | модель, граф, реестр метрик, санитизация, конфигурация |
//! | `pulse-collect` | чтение `/proc`, `/sys`, cgroup v2 |
//! | `pulse-store` | история, журналы сущностей и событий, запросы |
//! | `pulse-engine` | правила проблем, семантический diff |
//! | `pulse-export` | OpenMetrics-эндпоинт |
//! | `pulse-tui` | интерфейс |
//! | `pulse-cli` | сборка конвейера и подкоманды |
//!
//! Безопасность заложена в тип-уровне модели: любая строка, пришедшая из ядра,
//! проходит [`redact::sanitize_display`] при попадании в граф, а метрики уровня
//! процесса по умолчанию имеют политику экспорта [`metric::ExportPolicy::OptIn`].

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

pub mod config;
pub mod details;
pub mod entity;
pub mod event;
pub mod graph;
pub mod metric;
pub mod problem;
pub mod redact;
pub mod relation;
pub mod sample;
pub mod semantic;
pub mod snapshot;
pub mod time;

pub use config::{Config, ConfigError};
pub use details::{DetailsCache, OpenFile, Port, ProcessDetails, ProcessDetailsSource};
pub use entity::{
    Entity, EntityId, EntityKey, EntityKind, EntityRecord, EntitySpec, Labels, Runtime,
};
pub use event::{Event, EventKind};
pub use graph::{CollectCtx, CollectError, Collector, EntityGraph, GraphStats, TickBatch};
pub use metric::{ids, ExportPolicy, MetricDesc, MetricId, MetricKind, MetricScope, Unit};
pub use problem::{Evidence, Hysteresis, Problem, ProblemId, RuleId, Severity};
pub use redact::{parse_cmdline, redact_argv, sanitize_display, RedactMode};
pub use relation::{Relation, RelationKind};
pub use sample::{Sample, SeriesKey};
pub use semantic::{significance, summarize, MeaningfulEvent, Significance, Summary};
pub use snapshot::{AgentStats, LatestValues, Snapshot, SnapshotSource};
pub use time::{format_duration, TickId, Timestamp};

/// Версия крейта — попадает в `/metrics` и в заголовок TUI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Имя продукта в интерфейсах.
pub const PRODUCT: &str = "pulse";
