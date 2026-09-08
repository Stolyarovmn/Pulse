//! RED-репродьюсеры аудита для `pulse-collect`.
//!
//! Описывают желаемый контракт сбора и падают на текущей реализации.

// Тест обязан падать и указывать строку.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use pulse_collect::fs::{FixtureFs, FsSource};
use pulse_collect::HostCollector;
use pulse_core::time::Timestamp;
use pulse_core::{CollectCtx, Collector, EntityGraph};

/// PULSE-084 (PROMOTED): потеря критического источника обязана быть заявлена.
///
/// `/proc/stat` и `/proc/meminfo` — основа наблюдения за хостом. Раньше
/// внутренние методы при отсутствии файла просто выходили, а `collect`
/// всегда возвращал `Ok(())`: агент мог ослепнуть, не увеличив ни одного
/// счётчика ошибок и не породив ни одного события.
#[test]
fn audit_red_host_collector_reports_missing_critical_source() {
    // Всё на месте, кроме `/proc/stat`.
    let fs: Arc<dyn FsSource> = Arc::new(
        FixtureFs::new()
            .file(
                "/proc/meminfo",
                "MemTotal:       8000000 kB\nMemFree:        1000000 kB\nMemAvailable:   2000000 kB\n",
            )
            .file("/proc/loadavg", "1.25 1.32 1.18 2/1234 5678\n")
            .file("/proc/uptime", "123456.78 987654.32\n"),
    );
    let mut collector = HostCollector::new(Arc::clone(&fs), PathBuf::from("/proc"));
    let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(2_000));

    let (result, errors) = {
        let mut ctx = CollectCtx::new(&mut graph, 1.0);
        let result = collector.collect(&mut ctx);
        let errors = ctx.errors();
        (result, errors)
    };

    assert!(
        result.is_err() || errors > 0,
        "отсутствие /proc/stat обязано быть заявлено как отказ сбора, получено Ok и {errors} ошибок"
    );
}
