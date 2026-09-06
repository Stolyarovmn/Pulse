//! Текстовые отчёты для `top` и `scorecard`.
//!
//! Текстовый режим сохраняет ту же иерархию, что TUI: проблемы раньше таблицы
//! потребителей. Это важно для SSH, пайпов и терминалов без alternate screen.

use pulse_core::metric::ids;
use pulse_core::snapshot::Snapshot;
use pulse_core::time::format_duration;
use pulse_tui::app::{App, SortKey};
use pulse_tui::fold::relevant;
use pulse_tui::format;
use pulse_tui::rows::entity_rows;

/// Однократный человекочитаемый снимок.
#[must_use]
pub fn render_top(snapshot: &Snapshot, limit: usize) -> String {
    let host = snapshot.host;
    let cpu = snapshot.value_or(host, ids::HOST_CPU_UTIL, 0.0);
    let memory = snapshot.value_or(host, ids::HOST_MEM_UTIL, 0.0);
    let psi_cpu = snapshot.value_or(host, ids::HOST_PSI_CPU_SOME_AVG10, 0.0);
    let psi_memory = snapshot.value_or(host, ids::HOST_PSI_MEM_FULL_AVG10, 0.0);
    let psi_io = snapshot.value_or(host, ids::HOST_PSI_IO_FULL_AVG10, 0.0);
    let uptime = snapshot.value_or(host, ids::HOST_UPTIME, 0.0).max(0.0) as u64;

    let mut out = String::with_capacity(4096);
    out.push_str(&format!(
        "PULSE  {}  {}  up {}  tick {}\n",
        snapshot.hostname,
        snapshot.at,
        format_duration(std::time::Duration::from_secs(uptime)),
        snapshot.tick
    ));
    out.push_str(&format!(
        "CPU {:>6}  MEM {:>6}  PSI cpu {:>6} mem {:>6} io {:>6}\n\n",
        format::percent(cpu),
        format::percent(memory),
        format::percent(psi_cpu),
        format::percent(psi_memory),
        format::percent(psi_io),
    ));

    out.push_str(&format!("PROBLEMS {}\n", snapshot.problems.len()));
    if snapshot.problems.is_empty() {
        out.push_str("  no active problems\n");
    }
    for problem in &snapshot.problems {
        out.push_str(&format!(
            "  {:<5} {:<24} {}\n",
            problem.severity.as_str(),
            format::truncate(&problem.entity_name, 24),
            problem.title
        ));
        for evidence in problem.evidence.iter().take(3) {
            out.push_str(&format!("        {} = {}", evidence.label, evidence.value));
            if let Some(threshold) = &evidence.threshold {
                out.push_str(&format!("  [{threshold}]"));
            }
            out.push('\n');
        }
    }

    out.push_str("\nRELEVANT ENTITIES\n");
    out.push_str("  NAME                     KIND       CPU      MEM        OWNER\n");
    // Текстовый снимок показывает те же логические объекты, что и главный
    // экран (разделы 147-149 спецификации): один продукт не может описывать
    // систему двумя разными моделями. Полный инвентарь доступен в TUI по `m`.
    let mut app = App::default();
    app.entities.sort = SortKey::Relevance;
    let technical = entity_rows(snapshot, &app);
    for logical in relevant(snapshot, &technical, limit) {
        let row = &logical.row;
        out.push_str(&format!(
            "  {:<24} {:<10} {:>7}  {:>9}  {}\n",
            format::truncate(&row.name, 24),
            logical.kind_label(),
            format::cores(row.cpu),
            format::bytes(row.memory),
            // Текстовый снимок всегда ASCII: его перенаправляют в файл и
            // читают чем угодно, поэтому тире здесь недопустимо.
            if row.owner.is_empty() {
                "-"
            } else {
                &row.owner
            },
        ));
    }

    out.push_str(&format!(
        "\nAGENT  tick {:.2} ms  p95 {:.2} ms  rss {}  series {}  stored {}\n",
        snapshot.agent.tick_duration_ms,
        snapshot.agent.tick_duration_p95_ms,
        format::bytes(snapshot.agent.rss_bytes as f64),
        snapshot.agent.series_live,
        snapshot.agent.samples_stored,
    ));
    out
}

/// Число подтверждённых зависимостей cgroup → disk в актуальном снимке.
///
/// Нужна `pulse check`: отсутствие ошибки коллектора не доказывает, что
/// топология ядра действительно разрешилась в рёбра.
#[must_use]
pub fn disk_dependency_count(snapshot: &Snapshot) -> usize {
    snapshot
        .relations
        .iter()
        .filter(|relation| {
            relation.kind == pulse_core::RelationKind::BackedBy
                && snapshot
                    .entity(relation.to)
                    .is_some_and(|entity| entity.kind == pulse_core::EntityKind::Disk)
        })
        .count()
}

/// Измеримый отчёт, которым проект сравнивает себя с конкурентами.
#[must_use]
pub fn render_scorecard(snapshot: &Snapshot, elapsed: std::time::Duration) -> String {
    let interval_ms = if snapshot.agent.ticks_total == 0 {
        0.0
    } else {
        elapsed.as_secs_f64() * 1_000.0 / snapshot.agent.ticks_total as f64
    };
    let overhead_ratio = if interval_ms > 0.0 {
        snapshot.agent.tick_duration_p95_ms / interval_ms
    } else {
        0.0
    };

    let mut out = String::with_capacity(1024);
    out.push_str("Pulse scorecard\n");
    out.push_str(&format!("host: {}\n", snapshot.hostname));
    out.push_str(&format!("ticks: {}\n", snapshot.agent.ticks_total));
    out.push_str(&format!(
        "tick_duration_ms: {:.3}\n",
        snapshot.agent.tick_duration_ms
    ));
    out.push_str(&format!(
        "tick_duration_p95_ms: {:.3}\n",
        snapshot.agent.tick_duration_p95_ms
    ));
    out.push_str(&format!("agent_rss_bytes: {}\n", snapshot.agent.rss_bytes));
    out.push_str(&format!("series_live: {}\n", snapshot.agent.series_live));
    out.push_str(&format!(
        "samples_stored: {}\n",
        snapshot.agent.samples_stored
    ));
    out.push_str(&format!(
        "store_bytes_approx: {}\n",
        snapshot.agent.store_bytes
    ));
    out.push_str(&format!(
        "history_evicted_buckets: {}\n",
        snapshot.agent.history_evicted_buckets
    ));
    out.push_str(&format!(
        "history_evicted_events: {}\n",
        snapshot.agent.history_evicted_events
    ));
    out.push_str(&format!(
        "history_eviction_iterations: {}\n",
        snapshot.agent.history_eviction_iterations
    ));
    out.push_str(&format!(
        "history_eviction_no_progress: {}\n",
        snapshot.agent.history_eviction_no_progress
    ));
    out.push_str(&format!(
        "history_peak_bytes: {}\n",
        snapshot.agent.history_peak_bytes
    ));
    out.push_str(&format!(
        "suppressed_noise: {}\n",
        snapshot.suppressed_noise
    ));
    out.push_str(&format!("grouped_events: {}\n", snapshot.grouped_events));
    out.push_str(&format!(
        "meaningful_events: {}\n",
        snapshot.meaningful.len()
    ));
    out.push_str(&format!(
        "meaningful_over_budget: {}\n",
        snapshot.over_budget
    ));
    out.push_str(&format!(
        "history_evicted_hot_ticks: {}\n",
        snapshot.agent.history_evicted_hot_ticks
    ));
    out.push_str(&format!(
        "collector_errors: {}\n",
        snapshot.agent.collector_errors
    ));
    out.push_str(&format!(
        "ticks_skipped: {}\n",
        snapshot.agent.ticks_skipped
    ));
    out.push_str(&format!("overhead_ratio_estimate: {overhead_ratio:.6}\n"));
    out.push_str("note: overhead_ratio_estimate compares collection p95 with the mean interval; it is not CPU utilization\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::{EntityKey, EntitySpec};
    use pulse_core::snapshot::{AgentStats, LatestValues};
    use pulse_core::time::Timestamp;
    use pulse_core::{EntityGraph, SeriesKey};

    fn snapshot() -> Snapshot {
        let mut graph = EntityGraph::new("boot", "host-a", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let process = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid: 1,
                    start_ticks: 2,
                },
                "init",
            )
            .parent(host),
        );
        let mut latest = LatestValues::new();
        latest.set(SeriesKey::new(host, ids::HOST_CPU_UTIL), 0.5);
        latest.set(SeriesKey::new(process, ids::PROC_CPU_CORES), 0.25);
        let _ = graph.end_tick();
        Snapshot::build(
            &graph,
            latest,
            Vec::new(),
            Vec::new(),
            AgentStats {
                ticks_total: 2,
                tick_duration_ms: 1.0,
                tick_duration_p95_ms: 1.5,
                ..AgentStats::default()
            },
            "host-a",
            "boot",
        )
    }

    #[test]
    fn top_keeps_problem_first_order() {
        let text = render_top(&snapshot(), 20);
        assert!(text.find("PROBLEMS") < text.find("ENTITIES"));
        assert!(text.contains("host-a"));
        assert!(text.contains("init"));
    }

    #[test]
    fn scorecard_explains_estimate() {
        let text = render_scorecard(&snapshot(), std::time::Duration::from_secs(2));
        assert!(text.contains("tick_duration_p95_ms"));
        assert!(text.contains("not CPU utilization"));
        assert!(!text.contains("NaN"));
    }
}
