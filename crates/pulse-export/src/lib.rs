//! Экспорт телеметрии Pulse наружу.
//!
//! Единственная сетевая поверхность продукта, поэтому здесь самые строгие
//! требования безопасности: bind по умолчанию только на loopback, обязательный
//! bearer-токен для любого не-loopback адреса, проверка токена за константное
//! время и ДО сборки ответа, лимит частоты запросов, бюджет cardinality и
//! белый список меток (аргументы процессов не покидают хост никогда).
//!
//! Разделение на чистую функцию рендеринга и сетевой цикл сделано намеренно:
//! [`render_openmetrics`] тестируется без сокетов, а [`spawn`] отвечает только
//! за транспорт и защиту.

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

pub mod auth;
pub mod limits;
pub mod render;
pub mod server;

pub use auth::{constant_time_equal, load_token, parse_bearer};
pub use limits::{Budget, RateLimiter};
pub use render::{render_openmetrics, RenderStats};
pub use server::{spawn, ExportHandle, ExportRuntimeStats};
