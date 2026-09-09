//! Рендер снапшота в текст OpenMetrics.
//!
//! Экспортёр работает только с опубликованным `Snapshot` и конфигурацией экспорта:
//! он не трогает граф, стор или коллекторы. Кардинальность контролируется на входе
//! (схема лейблов фиксирована по виду сущности) и на выходе — лимитами
//! `max_series` / `max_series_per_metric` и общим бюджетом ответа.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use pulse_core::config::{Export, ProcessExportMode};
use pulse_core::entity::{Entity, EntityId, EntityKey, EntityKind};
use pulse_core::metric::{
    describe, ids, ExportPolicy, MetricDesc, MetricId, MetricKind, MetricScope,
};
use pulse_core::redact::sanitize_display;
use pulse_core::sample::SeriesKey;
use pulse_core::snapshot::Snapshot;

use crate::limits::Budget;

/// Жёсткий предел размера ответа в байтах. Защита от «снежного кома» при ошибке
/// кардинальности: даже если лимиты конфига заданы чрезмерно, ответ остаётся bounded.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Статистика одного рендера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderStats {
    /// Серии, фактически помещённые в ответ.
    pub series: usize,
    /// Серии, отброшенные бюджетами (`max_series`, `max_series_per_metric`, размер ответа).
    pub dropped: u64,
    /// Итоговый размер ответа в байтах, включая `# EOF`.
    pub bytes: usize,
}

/// Рендерит снапшот в OpenMetrics-текст.
///
/// Политика:
/// * метрики `ExportPolicy::Never` не экспортируются никогда;
/// * `OptIn` (уровень процессов) — только при `ProcessExportMode::Top` и только для
///   top-N процессов по `process_cpu_seconds`;
/// * agent-метрики всегда рендерятся по хосту из `AgentStats`;
/// * порядок: метрика → сущность, чтобы HELP/TYPE не повторялись;
/// * лимиты: не более `cfg.max_series` серий, не более `cfg.max_series_per_metric`
///   серий на метрику, не более `MAX_RESPONSE_BYTES` в ответе.
#[must_use]
pub fn render_openmetrics(snapshot: &Snapshot, cfg: &Export) -> (String, RenderStats) {
    // Резервируем место под финальную `# EOF\n`, чтобы итоговый ответ не превысил предел.
    let budget = Budget::new(
        MAX_RESPONSE_BYTES.saturating_sub("# EOF\n".len()),
        cfg.max_series,
    );

    let entity_map: HashMap<EntityId, &Entity> =
        snapshot.entities.iter().map(|e| (e.id, e)).collect();
    let selected_processes = select_top_processes(snapshot, &entity_map, cfg);

    let mut points: Vec<(SeriesKey, f64)> = Vec::new();
    for (key, value) in snapshot.latest.iter() {
        if is_agent_metric(key.metric) {
            // Agent-метрики берутся из `AgentStats` отдельно — они могут отсутствовать
            // в `latest`, если коллектор агента ещё не публиковал их как серии.
            continue;
        }
        let Some(entity) = entity_map.get(&key.entity) else {
            continue;
        };
        if allowed(describe(key.metric), entity, cfg, &selected_processes) {
            points.push((key, value));
        }
    }
    push_agent_points(&mut points, snapshot, &entity_map);

    // Детерминированный порядок: сначала метрика (HELP/TYPE), затем сущность.
    points.sort_unstable_by(|a, b| {
        a.0.metric
            .cmp(&b.0.metric)
            .then_with(|| a.0.entity.cmp(&b.0.entity))
    });

    let mut out = String::new();
    let mut series = 0usize;
    let mut dropped = 0u64;
    let mut current_metric: Option<MetricId> = None;
    let mut series_for_metric = 0usize;
    let mut line_count = 0usize;

    for (index, (key, value)) in points.iter().enumerate() {
        let Some(desc) = describe(key.metric) else {
            continue;
        };
        let Some(entity) = entity_map.get(&key.entity) else {
            continue;
        };

        if current_metric != Some(key.metric) {
            if cfg.max_series_per_metric == 0 {
                current_metric = Some(key.metric);
                series_for_metric = 0;
            } else {
                let header = header_for(desc);
                if out.len().saturating_add(header.len()) > budget.max_bytes {
                    // Дальнейшие серии не влезут — считаем весь остаток отброшенным.
                    dropped = dropped.saturating_add((points.len() - index) as u64);
                    break;
                }
                out.push_str(&header);
                current_metric = Some(key.metric);
                series_for_metric = 0;
            }
        }

        if series_for_metric >= cfg.max_series_per_metric {
            dropped = dropped.saturating_add(1);
            continue;
        }

        let line = sample_line(desc, entity, *value);
        if !budget.can_push_line(out.len(), line.len(), line_count) {
            dropped = dropped.saturating_add((points.len() - index) as u64);
            break;
        }

        out.push_str(&line);
        out.push('\n');
        line_count += 1;
        series += 1;
        series_for_metric += 1;
    }

    out.push_str("# EOF\n");
    let bytes = out.len();
    (
        out,
        RenderStats {
            series,
            dropped,
            bytes,
        },
    )
}

/// Заголовок метрики в OpenMetrics: HELP + TYPE.
fn header_for(desc: &'static MetricDesc) -> String {
    let name = desc.prom_name();
    let ty = match desc.kind {
        MetricKind::Gauge => "gauge",
        MetricKind::Counter => "counter",
    };
    format!("# HELP {name} {}\n# TYPE {name} {ty}\n", desc.help)
}

/// Строка образца. Лейблы экранируются по OpenMetrics; значения — конечные числа,
/// NaN/±Inf допускаются форматом OpenMetrics.
fn sample_line(desc: &'static MetricDesc, entity: &Entity, value: f64) -> String {
    let name = desc.prom_name();
    let mut s = String::with_capacity(name.len() + 48);
    s.push_str(&name);
    s.push_str(&label_fragment(entity));
    s.push(' ');
    s.push_str(&format_value(value));
    s
}

/// Фрагмент лейблов. Схема фиксирована ключом сущности — переменные имена процессов
/// и аргументы командной строки в лейблы не попадают.
fn label_fragment(entity: &Entity) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    match &entity.key {
        EntityKey::Host { .. } => {
            // Намеренно без меток: цель scrape уже идентифицирует машину
            // (Prometheus добавляет instance/job), а boot_id отдаётся
            // отдельной метрикой pulse_build_info.
        }
        EntityKey::Cgroup { cgroup_id } => {
            pairs.push(("cgroup", cgroup_id.to_string()));
        }
        EntityKey::Unit { name } => {
            pairs.push(("unit", escape_label_value(name)));
        }
        EntityKey::Process { pid, start_ticks } => {
            // Идентичность процесса — пара `(pid, start_ticks)`, а не PID:
            // ядро переиспользует номера, и после перезапуска другой процесс
            // получал тот же ряд. Prometheus склеивал два разных процесса в
            // одну серию, а counter выглядел как сброс счётчика.
            pairs.push(("pid", pid.to_string()));
            pairs.push(("start_ticks", start_ticks.to_string()));
        }
        EntityKey::Container { runtime, id } => {
            pairs.push(("container", escape_label_value(id)));
            pairs.push(("runtime", runtime.as_str().to_string()));
        }
        EntityKey::Pod { namespace, name } => {
            pairs.push(("pod", escape_label_value(name)));
            pairs.push(("namespace", escape_label_value(namespace)));
        }
        EntityKey::Disk { name } => {
            pairs.push(("device", escape_label_value(name)));
        }
        EntityKey::NetIf { name } => {
            pairs.push(("device", escape_label_value(name)));
        }
    }
    if pairs.is_empty() {
        String::new()
    } else {
        let mut out = String::with_capacity(pairs.len() * 24 + 2);
        out.push('{');
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(k);
            out.push_str("=\"");
            out.push_str(v);
            out.push('"');
        }
        out.push('}');
        out
    }
}

/// Экранирует значение лейбла по OpenMetrics: `\\`, `\"`, `\n`.
/// Пропускает через `sanitize_display`, чтобы в вывод не попали управляющие символы.
fn escape_label_value(raw: &str) -> String {
    let clean = sanitize_display(raw);
    let mut out = String::with_capacity(clean.len());
    for ch in clean.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Формат значения: `NaN`, `+Inf`, `-Inf` либо обычное десятичное представление.
fn format_value(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_string()
    } else if value.is_infinite() {
        if value < 0.0 {
            "-Inf".to_string()
        } else {
            "+Inf".to_string()
        }
    } else {
        format!("{:?}", value)
    }
}

/// Допускается ли метрика для сущности в соответствии с политикой экспорта.
fn allowed(
    desc: Option<&'static MetricDesc>,
    entity: &Entity,
    cfg: &Export,
    selected_processes: &HashSet<EntityId>,
) -> bool {
    let Some(desc) = desc else {
        return false;
    };
    match desc.export {
        ExportPolicy::Never => false,
        ExportPolicy::Always => true,
        ExportPolicy::OptIn => {
            entity.kind == EntityKind::Process
                && cfg.processes.mode == ProcessExportMode::Top
                && selected_processes.contains(&entity.id)
        }
    }
}

/// Метрики scope `Agent` берутся из `AgentStats`, а не из `latest`.
fn is_agent_metric(metric: MetricId) -> bool {
    matches!(describe(metric).map(|d| d.scope), Some(MetricScope::Agent))
}

/// Top-N процессов по `process_cpu_seconds`. Порядок детерминирован:
/// сначала значение, затем `EntityId`.
fn select_top_processes(
    snapshot: &Snapshot,
    entity_map: &HashMap<EntityId, &Entity>,
    cfg: &Export,
) -> HashSet<EntityId> {
    if cfg.processes.mode != ProcessExportMode::Top || cfg.processes.limit == 0 {
        return HashSet::new();
    }
    let mut scores: Vec<(EntityId, f64)> = snapshot
        .latest
        .iter()
        .filter(|(key, _)| key.metric == ids::PROC_CPU_CORES)
        .filter_map(|(key, value)| {
            let entity = entity_map.get(&key.entity)?;
            (entity.kind == EntityKind::Process).then_some((key.entity, value))
        })
        .collect();
    scores.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scores
        .into_iter()
        .take(cfg.processes.limit)
        .map(|(id, _)| id)
        .collect()
}

/// Agent-метрики (`AGENT_*`), всегда для хоста.
fn push_agent_points(
    points: &mut Vec<(SeriesKey, f64)>,
    snapshot: &Snapshot,
    entity_map: &HashMap<EntityId, &Entity>,
) {
    if entity_map.get(&snapshot.host).is_none() {
        return;
    }
    let a = snapshot.agent;
    let host = snapshot.host;
    points.push((
        SeriesKey::new(host, ids::AGENT_TICK_DURATION),
        a.tick_duration_ms,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_TICK_SKIPPED),
        a.ticks_skipped as f64,
    ));
    points.push((SeriesKey::new(host, ids::AGENT_TICKS), a.ticks_total as f64));
    points.push((
        SeriesKey::new(host, ids::AGENT_ENTITIES_LIVE),
        snapshot.graph.live_entities as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_SERIES_LIVE),
        a.series_live as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_SAMPLES_STORED),
        a.samples_stored as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_STORE_BYTES),
        a.store_bytes as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_EVENTS_TOTAL),
        a.events_total as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_PROBLEMS_OPEN),
        snapshot.problems.len() as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_COLLECTOR_ERRORS),
        a.collector_errors as f64,
    ));
    points.push((SeriesKey::new(host, ids::AGENT_RSS), a.rss_bytes as f64));
    points.push((SeriesKey::new(host, ids::AGENT_CPU_SECONDS), a.cpu_seconds));
    points.push((
        SeriesKey::new(host, ids::AGENT_EXPORT_SERIES),
        a.export_series as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_EXPORT_DROPPED),
        a.export_dropped as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_EXPORT_REQUESTS),
        a.export_requests as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_EXPORT_REJECTED),
        a.export_rejected as f64,
    ));
    points.push((
        SeriesKey::new(host, ids::AGENT_REDACTIONS),
        a.redactions as f64,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::{EntityKey, EntityKind, Labels};
    use pulse_core::problem::{Problem, ProblemId, RuleId, Severity};
    use pulse_core::sample::SeriesKey;
    use pulse_core::snapshot::AgentStats;
    use pulse_core::time::Timestamp;

    fn export_cfg() -> Export {
        Export::default()
    }

    fn make_entity(id: EntityId, kind: EntityKind, key: EntityKey, name: &str) -> Entity {
        Entity {
            id,
            key,
            kind,
            name: name.to_string(),
            parent: None,
            labels: Labels::new(),
            logical: None,
            first_seen: Timestamp::default(),
            last_seen: Timestamp::default(),
            alive: true,
        }
    }

    fn host_entity(id: EntityId) -> Entity {
        make_entity(
            id,
            EntityKind::Host,
            EntityKey::Host {
                boot_id: "boot-1".into(),
            },
            "test-host",
        )
    }

    fn process_entity(id: EntityId, pid: i32) -> Entity {
        make_entity(
            id,
            EntityKind::Process,
            EntityKey::Process {
                pid,
                start_ticks: 1000,
            },
            &format!("proc-{pid}"),
        )
    }

    fn disk_entity(id: EntityId, name: &str) -> Entity {
        make_entity(
            id,
            EntityKind::Disk,
            EntityKey::Disk { name: name.into() },
            name,
        )
    }

    fn netif_entity(id: EntityId, name: &str) -> Entity {
        make_entity(
            id,
            EntityKind::NetIf,
            EntityKey::NetIf { name: name.into() },
            name,
        )
    }

    fn snapshot_with_host() -> Snapshot {
        let mut snap = Snapshot::default();
        snap.host = EntityId::new(1, 1);
        snap.entities = vec![host_entity(snap.host)];
        snap
    }

    #[test]
    fn renders_host_metric_without_labels() {
        let mut snap = snapshot_with_host();
        snap.latest
            .set(SeriesKey::new(snap.host, ids::HOST_CPU_SECONDS), 12.5);

        let (body, stats) = render_openmetrics(&snap, &export_cfg());

        assert!(body.contains("pulse_host_cpu_seconds_total 12.5"));
        assert!(!body.contains("{"));
        assert!(body.ends_with("# EOF\n"));
        assert!(stats.series >= 1);
        assert_eq!(stats.dropped, 0);
        assert_eq!(stats.bytes, body.len());
    }

    #[test]
    fn excludes_process_metrics_by_default() {
        let mut snap = snapshot_with_host();
        let pid = EntityId::new(2, 1);
        snap.entities.push(process_entity(pid, 42));
        snap.latest
            .set(SeriesKey::new(snap.host, ids::HOST_CPU_SECONDS), 1.0);
        snap.latest
            .set(SeriesKey::new(pid, ids::PROC_CPU_CORES), 3.0);
        snap.latest
            .set(SeriesKey::new(pid, ids::PROC_CPU_SECONDS), 9.0);

        let (body, _) = render_openmetrics(&snap, &export_cfg());

        assert!(!body.contains("pulse_process_cpu_cores"));
        assert!(!body.contains("pulse_process_cpu_seconds_total"));
        assert!(!body.contains("pid=\"42\""));
    }

    #[test]
    fn top_process_mode_exports_only_top_n() {
        let mut snap = snapshot_with_host();
        let low = EntityId::new(2, 1);
        let high = EntityId::new(3, 1);
        let snap2 = EntityId::new(4, 1);
        snap.entities.push(process_entity(low, 1));
        snap.entities.push(process_entity(high, 2));
        snap.entities.push(process_entity(snap2, 3));
        snap.latest
            .set(SeriesKey::new(low, ids::PROC_CPU_SECONDS), 1.0);
        snap.latest
            .set(SeriesKey::new(low, ids::PROC_CPU_CORES), 0.5);
        snap.latest
            .set(SeriesKey::new(high, ids::PROC_CPU_CORES), 2.0);
        snap.latest
            .set(SeriesKey::new(snap2, ids::PROC_CPU_CORES), 1.0);

        let mut cfg = export_cfg();
        cfg.processes.mode = ProcessExportMode::Top;
        cfg.processes.limit = 1;

        let (body, _) = render_openmetrics(&snap, &cfg);

        assert!(body.contains("pid=\"2\""));
        assert!(!body.contains("pid=\"1\""));
        assert!(!body.contains("pid=\"3\""));
    }

    #[test]
    fn process_state_is_never_exported() {
        let mut snap = snapshot_with_host();
        let pid = EntityId::new(2, 1);
        snap.entities.push(process_entity(pid, 42));
        snap.latest
            .set(SeriesKey::new(pid, ids::PROC_STATE_CODE), 3.0);

        let mut cfg = export_cfg();
        cfg.processes.mode = ProcessExportMode::Top;
        cfg.processes.limit = 10;

        let (body, _) = render_openmetrics(&snap, &cfg);
        assert!(!body.contains("pulse_process_state_code"));
    }

    #[test]
    fn disk_and_netif_use_device_label() {
        let mut snap = snapshot_with_host();
        let disk = EntityId::new(10, 1);
        let net = EntityId::new(20, 1);
        snap.entities.push(disk_entity(disk, "sda"));
        snap.entities.push(netif_entity(net, "eth0"));
        snap.latest
            .set(SeriesKey::new(disk, ids::DISK_READ_BYTES), 123.0);
        snap.latest
            .set(SeriesKey::new(net, ids::NETIF_TX_BYTES), 456.0);

        let (body, _) = render_openmetrics(&snap, &export_cfg());

        assert!(body.contains("pulse_disk_read_bytes_total{device=\"sda\"} 123"));
        assert!(body.contains("pulse_netif_transmit_bytes_total{device=\"eth0\"} 456"));
    }

    #[test]
    fn escapes_label_values() {
        let mut snap = snapshot_with_host();
        let unit = EntityId::new(30, 1);
        snap.entities.push(make_entity(
            unit,
            EntityKind::Unit,
            EntityKey::Unit {
                name: "we\"ird\\name".into(),
            },
            "unit",
        ));
        snap.latest
            .set(SeriesKey::new(unit, ids::CG_MEM_CURRENT), 7.0);
        snap.agent = AgentStats {
            tick_duration_ms: 12.5,
            tick_duration_p95_ms: 20.0,
            ticks_total: 100,
            ticks_skipped: 1,
            collector_errors: 2,
            series_live: 30,
            samples_stored: 400,
            store_bytes: 5000,
            events_total: 7,
            history_evicted_buckets: 0,
            history_evicted_hot_ticks: 0,
            history_evicted_events: 0,
            history_eviction_no_progress: 0,
            history_eviction_iterations: 0,
            history_peak_bytes: 0,
            rss_bytes: 8000,
            cpu_seconds: 1.25,
            redactions: 3,
            export_requests: 9,
            export_rejected: 1,
            export_series: 25,
            export_dropped: 2,
        };
        snap.graph.live_entities = 12;
        snap.problems.push(Problem {
            id: ProblemId {
                rule: RuleId("test_rule"),
                entity: snap.host,
            },
            severity: Severity::Warn,
            entity_name: "host".into(),
            title: "t".into(),
            summary: "s".into(),
            evidence: vec![],
            since: Timestamp::default(),
            last_seen: Timestamp::default(),
            streak: 1,
        });

        let (body, _) = render_openmetrics(&snap, &export_cfg());

        assert!(body.contains("pulse_agent_ticks_total 100"));
        assert!(body.contains("pulse_agent_export_requests_total 9"));
        assert!(body.contains("pulse_agent_export_requests_rejected_total 1"));
        assert!(body.contains("pulse_agent_problems_open 1"));
    }

    #[test]
    fn max_series_per_metric_drops_excess() {
        let mut snap = snapshot_with_host();
        let mut ids = Vec::new();
        for i in 0..10u32 {
            let id = EntityId::new(100 + i, 1);
            snap.entities.push(disk_entity(id, &format!("d{i}")));
            snap.latest
                .set(SeriesKey::new(id, ids::DISK_READ_BYTES), 1.0);
            ids.push(id);
        }

        let mut cfg = export_cfg();
        cfg.max_series_per_metric = 3;

        let (body, stats) = render_openmetrics(&snap, &cfg);

        // Заголовок один, но серий ровно 3.
        assert_eq!(
            body.matches("# TYPE pulse_disk_read_bytes_total").count(),
            1
        );
        // stats.series считает все серии ответа, включая самометрики агента,
        // поэтому проверяем именно дисковые серии и число отброшенных.
        assert_eq!(stats.dropped, 7);
        assert!(
            body.lines()
                .filter(|l| l.starts_with("pulse_disk_read_bytes_total{"))
                .count()
                == 3
        );
    }

    #[test]
    fn max_series_budget_drops_excess() {
        let mut snap = snapshot_with_host();
        // Разные метрики, чтобы не упереться в per-metric лимит.
        snap.latest
            .set(SeriesKey::new(snap.host, ids::HOST_CPU_SECONDS), 1.0);
        snap.latest
            .set(SeriesKey::new(snap.host, ids::HOST_CPU_USER), 0.1);
        snap.latest
            .set(SeriesKey::new(snap.host, ids::HOST_MEM_TOTAL), 1024.0);

        let mut cfg = export_cfg();
        cfg.max_series = 2;

        let (_body, stats) = render_openmetrics(&snap, &cfg);
        assert_eq!(stats.series, 2);
        assert!(stats.dropped >= 1);
    }

    #[test]
    fn output_ends_with_eof() {
        let snap = snapshot_with_host();
        let (body, stats) = render_openmetrics(&snap, &export_cfg());
        assert!(body.ends_with("# EOF\n"));
        assert_eq!(stats.bytes, body.len());
    }
}
