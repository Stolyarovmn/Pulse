//! Каждая произведённая метрика обязана быть описана в реестре.
//!
//! PULSE-077. Экспортёр молча пропускает образец, для которого нет
//! `MetricDesc` (`pulse-export/src/render.rs:90`): без `HELP`/`TYPE` строку
//! OpenMetrics составить нельзя. Молчание означает, что коллектор может
//! годами класть в историю метрику, которой нет ни в `/metrics`, ни в
//! документации, и обнаружится это только вопросом «а где мой график».
//!
//! Обратное направление — «у каждой объявленной метрики есть производитель» —
//! здесь намеренно не проверяется: набор произведённых метрик зависит от
//! подсистем хоста (PSI, cgroup-лимиты, наличие дисков и интерфейсов), и
//! такой тест был бы нестабилен от машины к машине, а не строг.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use pulse_collect::{build_collectors, DemoFs, FsSource, RealFs};
use pulse_core::metric::{describe, MetricId};
use pulse_core::time::Timestamp;
use pulse_core::{CollectCtx, Config, EntityGraph};

fn produced_metrics(fs: Arc<dyn FsSource>) -> BTreeSet<MetricId> {
    let config = Config::default();
    let mut collectors = build_collectors(&config, fs);
    let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
    let mut out = BTreeSet::new();

    // Два такта: производные величины (скорости, доли CPU) существуют только
    // при наличии предыдущего наблюдения.
    for tick in 1..=2_u64 {
        graph.begin_tick(Timestamp::from_millis(1_000 + tick * 1_000));
        for collector in &mut collectors {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            let _ = collector.collect(&mut ctx);
        }
        let batch = graph.end_tick();
        for sample in &batch.samples {
            out.insert(sample.series.metric);
        }
    }
    out
}

#[test]
fn every_produced_metric_is_described() {
    let mut produced = produced_metrics(Arc::new(DemoFs::new(Duration::from_millis(1_000))));
    if cfg!(target_os = "linux") {
        produced.extend(produced_metrics(Arc::new(RealFs)));
    }

    assert!(
        produced.len() > 20,
        "прогон обязан произвести заметный набор метрик, получено {}",
        produced.len()
    );

    let orphans: Vec<u16> = produced
        .iter()
        .filter(|id| describe(**id).is_none())
        .map(|id| id.0)
        .collect();
    assert!(
        orphans.is_empty(),
        "метрики без описания в реестре не попадут в /metrics: {orphans:?}"
    );
}
