//! RED-репродьюсеры аудита для `pulse-export`.

// Тест обязан падать и указывать строку.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use pulse_core::config::{Export, ProcessExportMode};
use pulse_core::entity::{EntityKey, EntitySpec};
use pulse_core::metric::ids;
use pulse_core::snapshot::{AgentStats, LatestValues, Snapshot};
use pulse_core::time::Timestamp;
use pulse_core::EntityGraph;
use pulse_export::render_openmetrics;

/// RED-14 / PULSE-070: экспорт обязан различать инкарнации процесса.
///
/// Идентичность процесса в Pulse — пара `(pid, start_ticks)`, но метка серии
/// содержит только `pid`. После переиспользования PID два разных процесса
/// получают одну и ту же идентичность в OpenMetrics, и ряды склеиваются.
#[test]
#[ignore = "audit RED: PULSE-070 — promote when remediation lands"]
fn audit_red_process_export_keeps_incarnation_identity() {
    let mut graph = EntityGraph::new("boot", "test-host", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(2_000));
    let first = graph.upsert(EntitySpec::new(
        EntityKey::Process {
            pid: 123,
            start_ticks: 100,
        },
        "worker-old",
    ));
    let second = graph.upsert(EntitySpec::new(
        EntityKey::Process {
            pid: 123,
            start_ticks: 200,
        },
        "worker-new",
    ));
    let _ = graph.end_tick();

    let mut latest = LatestValues::new();
    latest.set(
        pulse_core::sample::SeriesKey::new(first, ids::PROC_CPU_CORES),
        1.0,
    );
    latest.set(
        pulse_core::sample::SeriesKey::new(second, ids::PROC_CPU_CORES),
        2.0,
    );
    let snapshot = Snapshot::build(
        &graph,
        latest,
        Vec::new(),
        Vec::new(),
        AgentStats::default(),
        "test-host",
        "boot",
    );

    let mut cfg = Export::default();
    cfg.processes.mode = ProcessExportMode::Top;
    cfg.processes.limit = 10;
    let (body, _) = render_openmetrics(&snapshot, &cfg);
    // Сравнивать нужно идентичность ряда, а не строку целиком: значения
    // у инкарнаций разные, и сравнение полных строк скрыло бы дефект.
    let identities: Vec<&str> = body
        .lines()
        .filter(|line| line.contains("process_cpu_cores{"))
        .filter_map(|line| line.split(' ').next())
        .collect();
    assert_eq!(
        identities.len(),
        2,
        "две инкарнации обязаны дать два ряда: {identities:?}; вывод:\n{body}"
    );
    assert!(
        identities.first() != identities.get(1),
        "идентичность ряда обязана включать start_ticks: {identities:?}"
    );
}
