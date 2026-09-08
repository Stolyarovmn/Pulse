//! A/B diff обязан обнаруживать изменения метаданных между моментами.
//!
//! Это регрессионная защита исправления PULSE-004/044: пока журнал сущностей
//! перезаписывал запись целиком, `entities_at(A)` и `entities_at(B)`
//! возвращали одно и то же состояние, поэтому переименование и смена
//! родителя не обнаруживались в принципе — flagship-функция сравнения
//! молчала о самом заметном виде изменения.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use pulse_core::config::Store as StoreConfig;
use pulse_core::entity::{EntityKey, EntitySpec};
use pulse_core::time::Timestamp;
use pulse_core::{EntityGraph, TickBatch};
use pulse_engine::diff::{diff, ChangeKind, DiffOptions};
use pulse_store::History;

struct Feeder {
    graph: EntityGraph,
    history: History,
    tick: u64,
}

impl Feeder {
    fn new() -> Self {
        Self {
            graph: EntityGraph::new("boot", "host", Timestamp::from_millis(1_000)),
            history: History::new(&StoreConfig::default()),
            tick: 0,
        }
    }

    fn at(&self) -> Timestamp {
        Timestamp::from_millis(1_000 + self.tick * 1_000)
    }

    fn tick(&mut self, fill: impl FnOnce(&mut EntityGraph)) -> TickBatch {
        self.tick += 1;
        self.graph.begin_tick(self.at());
        fill(&mut self.graph);
        let batch = self.graph.end_tick();
        self.history.ingest(&batch);
        batch
    }
}

fn cgroup(name: &str, id: u64) -> EntitySpec {
    EntitySpec::new(EntityKey::Cgroup { cgroup_id: id }, name)
}

#[test]
fn diff_detects_reparent_between_moments() {
    let mut feeder = Feeder::new();
    let _ = feeder.tick(|graph| {
        let first = graph.upsert(cgroup("parent-one", 30));
        let _ = graph.upsert(cgroup("parent-two", 31));
        let _ = graph.upsert(cgroup("child", 32).parent(first));
    });
    let moment_a = feeder.at();

    for _ in 0..3 {
        let _ = feeder.tick(|graph| {
            let _ = graph.upsert(cgroup("parent-one", 30));
            let second = graph.upsert(cgroup("parent-two", 31));
            let _ = graph.upsert(cgroup("child", 32).parent(second));
        });
    }
    let moment_b = feeder.at();

    let report = diff(&feeder.history, moment_a, moment_b, &DiffOptions::default());
    let reparented: Vec<&str> = report
        .structural
        .iter()
        .filter(|change| change.kind == ChangeKind::Reparented)
        .map(|change| change.name.as_str())
        .collect();

    assert!(
        reparented.contains(&"child"),
        "смена родителя обязана попасть в отчёт: {:?}",
        report
            .structural
            .iter()
            .map(|change| (change.kind, change.name.clone()))
            .collect::<Vec<_>>()
    );
}
