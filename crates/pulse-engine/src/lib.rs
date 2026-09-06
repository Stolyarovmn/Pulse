//! `pulse-engine` — диагностика.
//!
//! Две подсистемы, обе детерминированные и без обязательного участия ИИ:
//!
//! * [`Analyzer`] — правила обнаружения проблем с гистерезисом. Каждая проблема
//!   несёт доказательства: измеренные величины и пересечённые пороги, чтобы
//!   оператор видел, **почему** сформирован вывод.
//! * [`diff`] — семантическое сравнение двух моментов: структура графа,
//!   числовые изменения, события интервала и ранжированные доказательства.
//!
//! Причинность не утверждается. Инструмент показывает совпадения во времени и
//! в графе; вывод о причине делает человек.

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

pub mod analyzer;
pub mod diff;
pub mod rules;

pub use analyzer::Analyzer;
pub use diff::{
    diff, render_text, ChangeKind, DiffOptions, DiffReport, MetricChange, ScoredEvidence,
    StructuralChange,
};
pub use rules::{default_rules, Rule, RuleCtx, RuleHit, Shape, ThresholdRule};
