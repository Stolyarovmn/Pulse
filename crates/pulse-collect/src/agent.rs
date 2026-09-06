//! Самонаблюдение агента.
//!
//! Без этого невозможно честно отвечать на вопрос «а сам инструмент сколько
//! стоит?» — а это один из критериев сравнения с конкурентами. Метрики агента
//! публикуются на сущности хоста и уходят и в TUI, и в `/metrics`, и в scorecard.

use std::path::PathBuf;
use std::sync::Arc;

use pulse_core::graph::{CollectCtx, CollectError, Collector};
use pulse_core::metric::ids;

use crate::fs::{FsSource, FsSourceExt};
use crate::parse;

/// Коллектор собственных ресурсов процесса Pulse.
#[derive(Debug)]
pub struct SelfCollector {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
    clock_ticks: f64,
}

impl SelfCollector {
    #[must_use]
    pub fn new(fs: Arc<dyn FsSource>, proc_root: PathBuf, clock_ticks: u64) -> Self {
        SelfCollector {
            fs,
            proc_root,
            clock_ticks: (clock_ticks.max(1)) as f64,
        }
    }
}

impl Collector for SelfCollector {
    fn name(&self) -> &'static str {
        "agent"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        let host = ctx.host();

        if let Some(text) = self.fs.read_opt(&self.proc_root.join("self/statm")) {
            if let Some(rss_pages) = text
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<f64>().ok())
            {
                ctx.sample(host, ids::AGENT_RSS, rss_pages * 4096.0);
            }
        }

        if let Some(text) = self.fs.read_opt(&self.proc_root.join("self/stat")) {
            if let Some(stat) = parse::parse_proc_stat(&text) {
                let ticks = stat.utime_ticks.saturating_add(stat.stime_ticks) as f64;
                ctx.sample(host, ids::AGENT_CPU_SECONDS, ticks / self.clock_ticks);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use pulse_core::sample::SeriesKey;
    use pulse_core::time::Timestamp;
    use pulse_core::EntityGraph;

    #[test]
    fn agent_rss_and_cpu_are_published() {
        let fs = Arc::new(
            FixtureFs::new()
                .file("/proc/self/statm", "10000 2500 500 100 0 1000 0\n")
                .file(
                    "/proc/self/stat",
                    "9 (pulse) R 1 9 9 0 -1 0 5 0 1 0 30 20 0 0 20 0 3 0 100 1000 2500",
                ),
        );
        let mut collector = SelfCollector::new(fs, PathBuf::from("/proc"), 100);
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор самометрик");
        }
        let batch = graph.end_tick();
        let host = graph.host();
        let rss = batch
            .samples
            .iter()
            .find(|s| s.series == SeriesKey::new(host, ids::AGENT_RSS))
            .map(|s| s.value)
            .unwrap_or_default();
        let cpu = batch
            .samples
            .iter()
            .find(|s| s.series == SeriesKey::new(host, ids::AGENT_CPU_SECONDS))
            .map(|s| s.value)
            .unwrap_or_default();
        assert!((rss - 2500.0 * 4096.0).abs() < 1.0, "rss = {rss}");
        assert!((cpu - 0.5).abs() < 1e-9, "cpu = {cpu}");
    }

    #[test]
    fn missing_self_files_are_not_an_error() {
        let fs = Arc::new(FixtureFs::new());
        let mut collector = SelfCollector::new(fs, PathBuf::from("/proc"), 100);
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let mut ctx = CollectCtx::new(&mut graph, 1.0);
        assert!(collector.collect(&mut ctx).is_ok());
    }
}
