//! RED-репродьюсеры аудита для `pulse-store`.
//!
//! Тесты описывают желаемый контракт истории и потому падают на текущей
//! реализации. Помечены `#[ignore]`: обязательный прогон остаётся зелёным,
//! а доказательство дефекта даёт `scripts/audit-red.sh`.

// Тест обязан падать и указывать строку.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use pulse_core::config::Store as StoreConfig;
use pulse_core::entity::{EntityKey, EntitySpec};
use pulse_core::metric::ids;
use pulse_core::sample::SeriesKey;
use pulse_core::time::Timestamp;
use pulse_core::{EntityGraph, TickBatch};
use pulse_store::History;

/// Конфигурация с коротким горячим кольцом: warm-слой начинает работать
/// быстро, поэтому границу hot/warm можно пересечь без тысяч тактов.
fn store_config() -> StoreConfig {
    StoreConfig {
        hot_ticks: 10,
        warm_buckets: 60,
        warm_bucket_ticks: 5,
        ..StoreConfig::default()
    }
}

/// Подаёт такты в историю и возвращает граф вместе с идентификатором cgroup.
struct Feeder {
    graph: EntityGraph,
    history: History,
    tick: u64,
}

impl Feeder {
    fn new() -> Self {
        Self {
            graph: EntityGraph::new("boot", "host", Timestamp::from_millis(1_000)),
            history: History::new(&store_config()),
            tick: 0,
        }
    }

    fn at(&self) -> Timestamp {
        Timestamp::from_millis(1_000 + self.tick * 1_000)
    }

    /// Один такт: сущности и значения задаёт вызывающий.
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

/// PULSE-002 / PULSE-048 (PROMOTED): значение, которое перестали наблюдать,
/// не имеет права выглядеть текущим.
///
/// История сообщает момент наблюдения каждой серии, а окно свежести задаёт
/// вызывающий: оно зависит от интервала сбора, о котором история не решает.
/// Раньше `LatestValues` хранил только `SeriesKey → f64`, поэтому
/// исчезновение источника выглядело как неизменное значение.
#[test]
fn audit_red_stale_value_is_not_current() {
    let mut feeder = Feeder::new();
    let _ = feeder.tick(|graph| {
        let id = graph.upsert(cgroup("api.service", 10));
        graph.sample(id, ids::CG_MEM_UTIL, 0.9);
    });
    let entity = feeder
        .history
        .entities_at(feeder.at())
        .iter()
        .find(|record| record.name == "api.service")
        .map(|record| record.id)
        .expect("сущность наблюдалась");
    let observed_at = feeder.at();

    // Пока наблюдение свежее, значение обязано быть видно.
    let fresh = feeder.history.latest().with_freshness(feeder.at(), 2_000);
    assert_eq!(fresh.get(entity, ids::CG_MEM_UTIL), Some(0.9));
    assert!(!fresh.is_stale(entity, ids::CG_MEM_UTIL));

    // Сущность жива, но метрику больше не собирают: источник исчез.
    for _ in 0..5 {
        let _ = feeder.tick(|graph| {
            let _ = graph.upsert(cgroup("api.service", 10));
        });
    }

    let latest = feeder.history.latest().with_freshness(feeder.at(), 2_000);
    assert!(
        latest.get(entity, ids::CG_MEM_UTIL).is_none(),
        "значение без свежего наблюдения не имеет права возвращаться как текущее: {:?}",
        latest.get(entity, ids::CG_MEM_UTIL)
    );
    assert!(
        latest.is_stale(entity, ids::CG_MEM_UTIL),
        "серия обязана быть названа устаревшей, а не просто отсутствующей"
    );
    assert_eq!(
        latest.observed_at(entity, ids::CG_MEM_UTIL),
        Some(observed_at),
        "момент последнего наблюдения обязан сохраняться для диагностики"
    );
    // Устаревшее наблюдение не имеет права утечь через перебор серий:
    // экспортёр отдал бы его как актуальное.
    assert!(
        !latest
            .iter()
            .any(|(key, _)| key.metric == ids::CG_MEM_UTIL && key.entity == entity),
        "перебор обязан пропускать устаревшие наблюдения"
    );
}

/// RED-05 / PULSE-010, PULSE-035: окно, пересекающее warm и hot, обязано
/// покрываться целиком.
///
/// `History::series` возвращает hot-точки и обращается к warm только если
/// hot пуст, поэтому warm-префикс запрошенного интервала теряется.
#[test]
#[ignore = "audit RED: PULSE-010/035 — promote when remediation lands"]
fn audit_red_window_crossing_warm_and_hot_keeps_prefix() {
    let mut feeder = Feeder::new();
    let mut key = None;
    for step in 0..40_u64 {
        let _ = feeder.tick(|graph| {
            let id = graph.upsert(cgroup("api.service", 11));
            graph.sample(id, ids::CG_MEM_UTIL, 0.1 + (step as f64) / 100.0);
        });
        if key.is_none() {
            key = feeder
                .history
                .entities_at(feeder.at())
                .first()
                .map(|record| SeriesKey::new(record.id, ids::CG_MEM_UTIL));
        }
    }
    let key = key.expect("серия существует");

    let from = Timestamp::from_millis(2_000);
    let to = feeder.at();
    let points = feeder.history.series(key, from, to);
    let oldest_point = points
        .first()
        .map(|(at, _)| at.as_millis())
        .expect("история не пуста");

    assert!(
        oldest_point <= 6_000,
        "начало запрошенного окна обязано остаться в ответе, получено {oldest_point} мс"
    );
}

/// RED-15 / PULSE-038: `History::oldest` обязан отражать всю сохранённую
/// историю, включая warm-слой.
#[test]
#[ignore = "audit RED: PULSE-038 — promote when remediation lands"]
fn audit_red_oldest_reflects_warm_retention() {
    let mut feeder = Feeder::new();
    for step in 0..40_u64 {
        let _ = feeder.tick(|graph| {
            let id = graph.upsert(cgroup("api.service", 12));
            graph.sample(id, ids::CG_MEM_UTIL, (step as f64) / 100.0);
        });
    }

    let oldest = feeder.history.oldest().as_millis();
    assert!(
        oldest <= 6_000,
        "oldest обязан учитывать warm-слой, получено {oldest} мс"
    );
}

/// RED-03 / PULSE-004, PULSE-044: историческое имя обязано сохраняться.
///
/// `Entities` перезаписывает запись целиком, поэтому `entities_at(A)` отдаёт
/// метаданные последнего такта, а не наблюдавшиеся в момент A.
#[test]
#[ignore = "audit RED: PULSE-004/044 — promote when remediation lands"]
fn audit_red_entities_at_returns_historical_name() {
    let mut feeder = Feeder::new();
    let _ = feeder.tick(|graph| {
        let _ = graph.upsert(cgroup("old-name", 20));
    });
    let moment_a = feeder.at();

    for _ in 0..3 {
        let _ = feeder.tick(|graph| {
            let _ = graph.upsert(cgroup("new-name", 20));
        });
    }

    let name_at_a = feeder
        .history
        .entities_at(moment_a)
        .first()
        .map(|record| record.name.clone())
        .expect("сущность наблюдалась в момент A");
    assert_eq!(
        name_at_a, "old-name",
        "entities_at обязан вернуть имя, наблюдавшееся в момент A"
    );
}

/// RED-04 / PULSE-004, PULSE-044: исторический родитель обязан сохраняться,
/// иначе A/B diff не способен обнаружить reparent.
#[test]
#[ignore = "audit RED: PULSE-004/044 — promote when remediation lands"]
fn audit_red_entities_at_returns_historical_parent() {
    let mut feeder = Feeder::new();
    let _ = feeder.tick(|graph| {
        let first = graph.upsert(cgroup("parent-one", 30));
        let _ = graph.upsert(cgroup("child", 32).parent(first));
    });
    let moment_a = feeder.at();
    let parent_one = feeder
        .history
        .entities_at(moment_a)
        .iter()
        .find(|record| record.name == "parent-one")
        .map(|record| record.id)
        .expect("первый родитель наблюдался");

    for _ in 0..3 {
        let _ = feeder.tick(|graph| {
            let _ = graph.upsert(cgroup("parent-one", 30));
            let second = graph.upsert(cgroup("parent-two", 31));
            let _ = graph.upsert(cgroup("child", 32).parent(second));
        });
    }

    let parent_at_a = feeder
        .history
        .entities_at(moment_a)
        .iter()
        .find(|record| record.name == "child")
        .and_then(|record| record.parent)
        .expect("у ребёнка был родитель в момент A");
    assert_eq!(
        parent_at_a, parent_one,
        "entities_at обязан вернуть родителя, наблюдавшегося в момент A"
    );
}

/// PULSE-039 (UNEXPECTED_GREEN): скорость счётчика через warm-слой считается
/// по фактическим приращениям и переживает сброс счётчика.
///
/// Аудит ожидал здесь дефект, но контракт выполняется: `History::rate`
/// обходит warm-бакеты и не использует их среднее как точку счётчика.
/// Поэтому тест не помечен `#[ignore]` — он остаётся защитой от регрессии.
#[test]
fn audit_red_counter_rate_through_warm_layer() {
    let mut feeder = Feeder::new();
    // Неравномерный рост со сбросом счётчика внутри warm-слоя: истинная
    // скорость считается как сумма положительных приращений на фактический
    // интервал, а среднее значение бакета такой ряд описать не может.
    let mut value = 0.0_f64;
    let mut increments = 0.0_f64;
    for step in 1..=40_u64 {
        if step == 12 {
            // Перезапуск источника: счётчик начинается заново.
            value = 0.0;
        } else {
            let increment = if step % 3 == 0 { 3_000.0 } else { 200.0 };
            value += increment;
            increments += increment;
        }
        let sample = value;
        let _ = feeder.tick(|graph| {
            let host = graph.host();
            graph.sample(host, ids::HOST_CPU_SECONDS, sample);
        });
    }
    let key = SeriesKey::new(feeder.graph.host(), ids::HOST_CPU_SECONDS);

    let from = Timestamp::from_millis(2_000);
    let to = feeder.at();
    let elapsed = (to.as_millis() - from.as_millis()) as f64 / 1_000.0;
    let expected = increments / elapsed;
    let rate = feeder
        .history
        .rate(key, from, to)
        .expect("скорость счётчика вычислима");
    assert!(
        (rate - expected).abs() < expected * 0.1,
        "скорость обязана считаться по приращениям всего окна: ожидалось около {expected}, получено {rate}"
    );
}
