//! RED-репродьюсеры аудита для `pulse-engine`.
//!
//! Тесты описывают желаемый контракт правил и потому падают на текущей
//! реализации. Помечены `#[ignore]`; доказательство даёт
//! `scripts/audit-red.sh`.

// Тест обязан падать и указывать строку.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use pulse_core::config::{Rules as RulesCfg, Store as StoreConfig};
use pulse_core::metric::ids;
use pulse_core::problem::{Problem, Severity};
use pulse_core::sample::SeriesKey;
use pulse_core::snapshot::LatestValues;
use pulse_core::time::Timestamp;
use pulse_core::{EntityGraph, EventKind};
use pulse_engine::Analyzer;
use pulse_store::History;

/// Стенд: один хост, одно давление CPU, настоящие правила и гистерезис.
struct Harness {
    graph: EntityGraph,
    analyzer: Analyzer,
    history: History,
    tick: u64,
}

impl Harness {
    fn new() -> Self {
        Self {
            graph: EntityGraph::new("boot", "host", Timestamp::from_millis(1_000)),
            analyzer: Analyzer::new(RulesCfg::default()),
            history: History::new(&StoreConfig::default()),
            tick: 0,
        }
    }

    /// Такт с заданным давлением CPU; возвращает открытые проблемы.
    fn tick(&mut self, psi: f64) -> Vec<Problem> {
        self.tick += 1;
        let at = Timestamp::from_millis(1_000 + self.tick * 1_000);
        self.graph.begin_tick(at);
        let host = self.graph.host();
        let batch = self.graph.end_tick();
        self.history.ingest(&batch);
        let mut latest = LatestValues::new();
        latest.set(SeriesKey::new(host, ids::HOST_PSI_CPU_SOME_AVG10), psi);
        self.analyzer
            .evaluate(&self.graph, &latest, &self.history, at)
    }
}

/// PULSE-003 / PULSE-045 (PROMOTED): открытая проблема обязана оставаться
/// видимой в нейтральной зоне гистерезиса.
///
/// `psi_cpu_warn` = 0.20, `psi_cpu_clear` = 0.10. Значение 0.15 не входит ни
/// в условие входа, ни в условие снятия, поэтому правило не выдаёт `RuleHit`.
/// Раньше список проблем строился только из попаданий текущего такта, и
/// проблема исчезала из снимка при открытом внутреннем состоянии и без
/// события о закрытии.
#[test]
fn audit_red_open_problem_survives_hysteresis_dead_zone() {
    let mut harness = Harness::new();
    for _ in 0..3 {
        let _ = harness.tick(0.9);
    }
    let opened = harness.analyzer.take_events();
    assert_eq!(
        opened
            .iter()
            .filter(|event| event.kind == EventKind::ProblemOpened)
            .count(),
        1,
        "открытие обязано быть ровно одно"
    );

    // Нейтральная зона: подтверждения нет, но и снятия нет.
    let problems = harness.tick(0.15);
    let events = harness.analyzer.take_events();
    assert!(
        !problems.is_empty(),
        "в нейтральной зоне проблема обязана оставаться открытой"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.kind == EventKind::ProblemClosed),
        "закрытия не было, значит проблема не имеет права исчезать из снимка"
    );

    // Возврат выше порога не имеет права выглядеть как новая проблема:
    // иначе один инцидент разваливается на серию открытий.
    let problems = harness.tick(0.9);
    let events = harness.analyzer.take_events();
    assert_eq!(problems.len(), 1);
    assert!(
        !events
            .iter()
            .any(|event| event.kind == EventKind::ProblemOpened),
        "повторный вход не создаёт второе открытие: {:?}",
        events.iter().map(|event| event.kind).collect::<Vec<_>>()
    );

    // Снятие по-прежнему работает: выдержка clear_after_ticks обязательна.
    for _ in 0..15 {
        let _ = harness.tick(0.01);
    }
    let events = harness.analyzer.take_events();
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::ProblemClosed),
        "после выдержки проблема обязана закрыться"
    );
    assert!(
        harness.tick(0.01).is_empty(),
        "закрытая проблема не имеет права оставаться в снимке"
    );
}

/// PULSE-059 (PROMOTED): события проблемы обязаны сохранять идентичность
/// сущности, а не только её имя.
///
/// Имя не является идентичностью: `dbus.socket` существует одновременно в
/// системном и пользовательском менеджере, а процессы переиспользуют PID.
/// Без `entity`/`entity_kind` событие невозможно связать с узлом графа.
#[test]
fn audit_red_problem_events_keep_entity_identity() {
    let mut harness = Harness::new();
    let host = harness.graph.host();
    for _ in 0..3 {
        let _ = harness.tick(0.9);
    }

    let events = harness.analyzer.take_events();
    let opened = events
        .iter()
        .find(|event| event.kind == EventKind::ProblemOpened)
        .expect("проблема открылась");

    assert_eq!(
        opened.entity,
        Some(host),
        "событие обязано указывать сущность графа"
    );
    assert!(
        opened.entity_kind.is_some(),
        "событие обязано указывать вид сущности"
    );
    assert!(opened.rule.is_some(), "событие обязано указывать правило");
    assert_eq!(opened.severity, Severity::Crit);
}
