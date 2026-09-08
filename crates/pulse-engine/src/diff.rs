//! Семантический A/B diff — главная функция расследования.
//!
//! Отличие от «replay графиков»: сравниваются не только числовые серии, но и
//! **структура графа** (что появилось, исчезло, перезапустилось, сменило
//! владельца) и события интервала. Результат — ранжированный список
//! доказательств.
//!
//! Формулировки строго корреляционные. Pulse говорит «вместе с этим изменилось»,
//! но не «это вызвало»: причинность из корреляции не следует, а ложная
//! уверенность в диагнозе дороже отсутствия диагноза.
//!
//! Формула ранжирования:
//!
//! ```text
//! score     = 0.35*magnitude + 0.25*temporal + 0.25*graph + 0.15*resource
//! magnitude = min(|z|, 4) / 4,           z = (mean_B - mean_A) / stddev_A
//! temporal  = max(0, 1 - |t_anchor - t_evidence| / 120s), будущее -> 0
//! graph     = 1/(1+hops) по цепочкам родителей, hops > 4 -> 0
//! resource  = 1 при прямой ресурсной зависимости, иначе 0
//! ```

use std::collections::{HashMap, HashSet};

use pulse_core::entity::{EntityId, EntityKind, EntityRecord};
use pulse_core::event::{Event, EventKind};
use pulse_core::metric::{describe, MetricId, Unit};
use pulse_core::time::Timestamp;
use pulse_core::SeriesKey;
use pulse_store::History;

/// Вес слагаемых. Константы, а не настройки: подгонка весов без данных
/// инцидентов — это самообман, а фиксированные веса хотя бы объяснимы.
const W_MAGNITUDE: f64 = 0.35;
const W_TEMPORAL: f64 = 0.25;
const W_GRAPH: f64 = 0.25;
const W_RESOURCE: f64 = 0.15;

/// Верхняя граница z-оценки. Линейное отсечение, а не экспонента: границы
/// проверяются точными тестами.
const Z_MAX: f64 = 4.0;

/// Окно временной близости.
const TEMPORAL_WINDOW_MS: f64 = 120_000.0;

/// Максимальное расстояние по графу, при котором связь ещё учитывается.
const MAX_HOPS: u32 = 4;

/// Параметры diff.
#[derive(Clone, Copy, Debug)]
pub struct DiffOptions {
    /// Ширина окна усреднения вокруг точек A и B, секунды.
    pub baseline_secs: u64,
    pub max_evidence: usize,
    pub min_score: f64,
}

impl Default for DiffOptions {
    fn default() -> Self {
        DiffOptions {
            baseline_secs: 30,
            max_evidence: 5,
            min_score: 0.2,
        }
    }
}

/// Вид структурного изменения.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Deleted,
    Restarted,
    Reparented,
    MetadataChanged,
}

impl ChangeKind {
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            ChangeKind::Created => "+",
            ChangeKind::Deleted => "-",
            ChangeKind::Restarted => "↻",
            ChangeKind::Reparented => "→",
            ChangeKind::MetadataChanged => "~",
        }
    }
}

/// Структурное изменение графа.
#[derive(Clone, Debug)]
pub struct StructuralChange {
    pub kind: ChangeKind,
    pub entity: EntityId,
    pub entity_kind: EntityKind,
    pub name: String,
    pub detail: String,
}

/// Изменение числовой величины.
#[derive(Clone, Debug)]
pub struct MetricChange {
    pub entity: EntityId,
    pub name: String,
    pub metric: MetricId,
    pub before: f64,
    pub after: f64,
    pub delta: f64,
    /// Нормированное изменение относительно разброса в окне A.
    pub z: f64,
}

/// Ранжированное доказательство.
#[derive(Clone, Debug)]
pub struct ScoredEvidence {
    pub score: f64,
    pub text: String,
    pub at: Option<Timestamp>,
}

/// Результат сравнения двух моментов.
#[derive(Clone, Debug)]
pub struct DiffReport {
    pub a: Timestamp,
    pub b: Timestamp,
    pub structural: Vec<StructuralChange>,
    pub metrics: Vec<MetricChange>,
    pub events: Vec<Event>,
    pub evidence: Vec<ScoredEvidence>,
}

/// Величина изменения: нормированная z-оценка.
#[must_use]
pub fn magnitude_score(z: f64) -> f64 {
    if !z.is_finite() {
        return 0.0;
    }
    (z.abs().min(Z_MAX)) / Z_MAX
}

/// Временная близость доказательства к точке B.
///
/// Событие позже точки B получает 0: будущее не объясняет прошлое.
#[must_use]
pub fn temporal_score(anchor: Timestamp, evidence: Timestamp) -> f64 {
    let anchor_ms = anchor.as_millis() as i128;
    let evidence_ms = evidence.as_millis() as i128;
    // Ровно как в контракте: любое доказательство позже anchor получает 0.
    // Допуск здесь означал бы, что событие из будущего объясняет прошлое.
    if evidence_ms > anchor_ms {
        return 0.0;
    }
    let distance = (anchor_ms - evidence_ms).abs() as f64;
    (1.0 - distance / TEMPORAL_WINDOW_MS).max(0.0)
}

/// Близость по графу: `1/(1+hops)`, за пределами `MAX_HOPS` — ноль.
#[must_use]
pub fn graph_score(hops: Option<u32>) -> f64 {
    match hops {
        Some(hops) if hops <= MAX_HOPS => 1.0 / (1.0 + f64::from(hops)),
        _ => 0.0,
    }
}

/// Прямая ресурсная зависимость между двумя сущностями.
///
/// Раньше здесь сравнивались только *виды*: любое изменение любого диска
/// получало полный балл против любого потребителя. Такое слагаемое ничего не
/// различает — оно добавляло одинаковую константу всем парам и создавало
/// видимость работающей функции. Теперь зависимость должна быть подтверждена:
///
/// * потребитель ссылается на устройство меткой `disks` (её ставит
///   `pulse-collect` по `io.stat`, то есть по факту трафика) — балл 1;
/// * либо две сущности-потребителя делят родителя-cgroup — балл 1;
/// * иначе 0.
///
/// Метки, а не рёбра графа, потому что связи не переживают такт в истории, а
/// `EntityRecord.labels` сохраняются и доступны diff-у после смерти сущности.
#[must_use]
pub fn resource_score(a: &EntityRecord, b: &EntityRecord) -> f64 {
    let resource = |kind: EntityKind| matches!(kind, EntityKind::Disk | EntityKind::NetIf);
    let consumer = |kind: EntityKind| {
        matches!(
            kind,
            EntityKind::Process
                | EntityKind::Container
                | EntityKind::Unit
                | EntityKind::Pod
                | EntityKind::Cgroup
        )
    };
    // Ссылается ли потребитель на это устройство по имени.
    let uses = |consumer_record: &EntityRecord, device: &EntityRecord| -> bool {
        consumer_record
            .labels
            .get("disks")
            .is_some_and(|list| list.split(',').any(|name| name == device.name))
    };

    if resource(a.kind) && consumer(b.kind) {
        return f64::from(u8::from(uses(b, a)));
    }
    if resource(b.kind) && consumer(a.kind) {
        return f64::from(u8::from(uses(a, b)));
    }
    if consumer(a.kind) && consumer(b.kind) && a.parent.is_some() && a.parent == b.parent {
        return 1.0;
    }
    0.0
}

/// Итоговая оценка доказательства.
#[must_use]
pub fn score(magnitude: f64, temporal: f64, graph: f64, resource: f64) -> f64 {
    W_MAGNITUDE * magnitude + W_TEMPORAL * temporal + W_GRAPH * graph + W_RESOURCE * resource
}

/// Метрики, по которым имеет смысл искать изменения.
///
/// Список ограничен намеренно: сравнивать все серии — это тысячи строк шума,
/// в которых утонет содержательное изменение. Берётся из реестра, а не
/// дублируется здесь: ровно эти метрики хранятся в тёплом слое, и если бы список
/// разъехался с ретеншном, diff запрашивал бы данные, которых нет.
const WATCHED: &[MetricId] = pulse_core::metric::LONG_WINDOW;

/// Сравнивает два момента.
#[must_use]
pub fn diff(history: &History, a: Timestamp, b: Timestamp, opts: &DiffOptions) -> DiffReport {
    let (a, b) = if a <= b { (a, b) } else { (b, a) };

    let at_a: Vec<EntityRecord> = history.entities_at(a).into_iter().cloned().collect();
    let at_b: Vec<EntityRecord> = history.entities_at(b).into_iter().cloned().collect();
    let map_a: HashMap<EntityId, &EntityRecord> = at_a.iter().map(|r| (r.id, r)).collect();
    let map_b: HashMap<EntityId, &EntityRecord> = at_b.iter().map(|r| (r.id, r)).collect();

    let events: Vec<Event> = history.events_between(a, b).into_iter().cloned().collect();

    let mut structural = Vec::new();

    for record in &at_b {
        match map_a.get(&record.id) {
            None => structural.push(StructuralChange {
                kind: ChangeKind::Created,
                entity: record.id,
                entity_kind: record.kind,
                name: record.name.clone(),
                detail: String::new(),
            }),
            Some(before) => {
                if before.parent != record.parent {
                    structural.push(StructuralChange {
                        kind: ChangeKind::Reparented,
                        entity: record.id,
                        entity_kind: record.kind,
                        name: record.name.clone(),
                        detail: "сменился владелец".to_string(),
                    });
                }
                if before.name != record.name {
                    structural.push(StructuralChange {
                        kind: ChangeKind::MetadataChanged,
                        entity: record.id,
                        entity_kind: record.kind,
                        name: record.name.clone(),
                        detail: format!("имя: {} -> {}", before.name, record.name),
                    });
                }
            }
        }
    }

    for record in &at_a {
        if !map_b.contains_key(&record.id) {
            structural.push(StructuralChange {
                kind: ChangeKind::Deleted,
                entity: record.id,
                entity_kind: record.kind,
                name: record.name.clone(),
                detail: String::new(),
            });
        }
    }

    // Перезапуски видны только через события: логическая сущность выживает,
    // поэтому структурное сравнение двух моментов их не покажет.
    for event in events.iter().filter(|e| e.kind == EventKind::Restarted) {
        structural.push(StructuralChange {
            kind: ChangeKind::Restarted,
            entity: event.entity.unwrap_or(EntityId::NONE),
            entity_kind: event.entity_kind.unwrap_or(EntityKind::Cgroup),
            name: event.entity_name.clone(),
            detail: event.detail.clone(),
        });
    }

    // Числовые изменения: сущности, живые в обеих точках.
    let window = opts.baseline_secs.saturating_mul(1000);
    let mut metrics: Vec<MetricChange> = Vec::new();
    for record in &at_b {
        if !map_a.contains_key(&record.id) {
            continue;
        }
        for metric in WATCHED {
            let key = SeriesKey::new(record.id, *metric);
            let Some(before) = history.window(key, a.saturating_sub_millis(window), a) else {
                continue;
            };
            let Some(after) = history.window(key, b.saturating_sub_millis(window), b) else {
                continue;
            };
            if before.count == 0 || after.count == 0 {
                continue;
            }
            let delta = after.mean - before.mean;
            if delta.abs() < f64::EPSILON {
                continue;
            }
            // z считается только по сырым точкам. Если окно A обслужено
            // агрегатами тёплого слоя (`approximate`), его stddev занижен, и
            // деление на него превратило бы штатный шум в «значимое
            // изменение». Тот же путь — при вырожденном разбросе: вместо
            // деления на ноль берём относительное изменение по шкале z.
            let z = if before.stddev > 1e-9 && !before.approximate {
                delta / before.stddev
            } else {
                let base = before.mean.abs().max(1e-9);
                (delta / base) * Z_MAX
            };
            metrics.push(MetricChange {
                entity: record.id,
                name: record.name.clone(),
                metric: *metric,
                before: before.mean,
                after: after.mean,
                delta,
                z,
            });
        }
    }
    metrics.sort_by(|x, y| {
        y.z.abs()
            .partial_cmp(&x.z.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Производные cgroup намеренно публикуются и на техническом cgroup, и на
    // его владельце (unit/container). Две одинаковые строки в отчёте — не два
    // независимых доказательства, поэтому зеркало сворачивается до ранжирования.
    deduplicate_mirrored_metrics(&mut metrics, &map_b);

    let evidence = rank_evidence(&structural, &metrics, &events, &map_b, b, opts);

    DiffReport {
        a,
        b,
        structural,
        metrics,
        events,
        evidence,
    }
}
/// Сворачивает зеркальные изменения метрики: то же значение, опубликованное и
/// на сущности-владельце, и на его cgroup.
///
/// Признак зеркала — структурный, а не текстовый: сущности обязаны быть связаны
/// как родитель и ребёнок (владелец создаётся с `parent = cgroup`), носить одно
/// имя и иметь побитово равные значения. Одного совпадения имени недостаточно:
/// два `dbus.socket` из разных user-manager — разные сущности, и совпадение их
/// значений не повод потерять одну из строк. Одного родства тоже недостаточно:
/// вложенные slice с разными именами и случайно равными значениями остаются
/// отдельными фактами.
///
/// Из пары удаляется родитель (технический cgroup), остаётся владелец: именно
/// его имя оператор ищет в отчёте.
fn deduplicate_mirrored_metrics(
    metrics: &mut Vec<MetricChange>,
    records: &HashMap<EntityId, &EntityRecord>,
) {
    type MirrorKey = (MetricId, u64, u64);
    let key_of = |change: &MetricChange| -> MirrorKey {
        (
            change.metric,
            change.before.to_bits(),
            change.after.to_bits(),
        )
    };

    // Группируем по «метрика + одинаковые значения»: зеркало живёт внутри группы.
    let mut groups: HashMap<MirrorKey, Vec<(EntityId, &str)>> =
        HashMap::with_capacity(metrics.len());
    for change in metrics.iter() {
        groups
            .entry(key_of(change))
            .or_default()
            .push((change.entity, change.name.as_str()));
    }

    let dropped: HashSet<EntityId> = groups
        .values()
        .flat_map(|group| {
            group.iter().filter_map(|(id, name)| {
                // Есть ли в группе ребёнок этой сущности с тем же именем?
                let mirrored = group.iter().any(|(other, other_name)| {
                    other != id
                        && other_name == name
                        && records.get(other).and_then(|r| r.parent) == Some(*id)
                });
                mirrored.then_some(*id)
            })
        })
        .collect();

    metrics.retain(|change| !dropped.contains(&change.entity));
}

/// Расстояние по цепочкам родителей между двумя сущностями.
fn hops_between(
    records: &HashMap<EntityId, &EntityRecord>,
    from: EntityId,
    to: EntityId,
) -> Option<u32> {
    if from == to {
        return Some(0);
    }
    let ancestry = |mut id: EntityId| -> Vec<EntityId> {
        let mut chain = vec![id];
        for _ in 0..MAX_HOPS + 2 {
            match records.get(&id).and_then(|r| r.parent) {
                Some(parent) => {
                    chain.push(parent);
                    id = parent;
                }
                None => break,
            }
        }
        chain
    };
    let chain_from = ancestry(from);
    let chain_to = ancestry(to);
    for (i, a) in chain_from.iter().enumerate() {
        for (j, b) in chain_to.iter().enumerate() {
            if a == b {
                let hops = u32::try_from(i + j).unwrap_or(u32::MAX);
                return Some(hops);
            }
        }
    }
    None
}

/// Ранжирует доказательства относительно точки B.
fn rank_evidence(
    structural: &[StructuralChange],
    metrics: &[MetricChange],
    events: &[Event],
    records: &HashMap<EntityId, &EntityRecord>,
    anchor: Timestamp,
    opts: &DiffOptions,
) -> Vec<ScoredEvidence> {
    // Опорная сущность — та, у которой самое сильное числовое изменение.
    let focus = metrics.first().map(|m| m.entity);
    let focus_record = focus.and_then(|id| records.get(&id).copied());

    let mut scored: Vec<ScoredEvidence> = Vec::new();

    for change in metrics {
        let magnitude = magnitude_score(change.z);
        let hops = focus.and_then(|f| hops_between(records, f, change.entity));
        let graph = graph_score(hops);
        let resource = match (focus_record, records.get(&change.entity)) {
            (Some(anchor_record), Some(other)) => resource_score(anchor_record, other),
            _ => 0.0,
        };
        // Числовое изменение относится к точке B, поэтому по времени оно точное.
        let total = score(magnitude, 1.0, graph, resource);
        if total < opts.min_score {
            continue;
        }
        scored.push(ScoredEvidence {
            score: total,
            text: format!(
                "вместе с этим изменилось: {} {} {} -> {}",
                change.name,
                metric_label(change.metric),
                format_value(change.metric, change.before),
                format_value(change.metric, change.after)
            ),
            at: Some(anchor),
        });
    }

    for change in structural {
        let hops = focus.and_then(|f| hops_between(records, f, change.entity));
        let graph = graph_score(hops);
        let kind = change.entity_kind;
        // Структурное изменение ранжируется без ресурсного слагаемого: у
        // исчезнувшей сущности метки уже могут быть недоступны, а выдавать
        // недоказанную зависимость за доказанную нельзя.
        let resource = 0.0;
        // Структурное изменение весомо само по себе: величина берётся как половина шкалы.
        let total = score(0.5, 1.0, graph, resource);
        if total < opts.min_score {
            continue;
        }
        scored.push(ScoredEvidence {
            score: total,
            text: format!(
                "в интервале изменилась структура: {} {} {}",
                change.kind.glyph(),
                kind.as_str(),
                change.name
            ),
            at: None,
        });
    }

    for event in events {
        if matches!(event.kind, EventKind::ProblemOpened | EventKind::OomKill)
            || event.kind.is_structural()
        {
            let temporal = temporal_score(anchor, event.at);
            if temporal <= 0.0 {
                continue;
            }
            let entity = event.entity.unwrap_or(EntityId::NONE);
            let hops = focus.and_then(|f| hops_between(records, f, entity));
            let total = score(0.5, temporal, graph_score(hops), 0.0);
            if total < opts.min_score {
                continue;
            }
            scored.push(ScoredEvidence {
                score: total,
                text: format!(
                    "событие {} в {}: {} {}",
                    event.kind.as_str(),
                    event.at,
                    event.entity_name,
                    event.detail
                ),
                at: Some(event.at),
            });
        }
    }

    scored.sort_by(|x, y| {
        y.score
            .partial_cmp(&x.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.text.cmp(&y.text))
    });
    scored.truncate(opts.max_evidence);
    scored
}

fn metric_label(metric: MetricId) -> &'static str {
    describe(metric).map_or("значение", |d| d.name)
}

fn format_value(metric: MetricId, value: f64) -> String {
    match describe(metric).map(|d| d.unit) {
        Some(Unit::Ratio) => format!("{:.1}%", value * 100.0),
        Some(Unit::Bytes) => format!("{:.1} MiB", value / 1024.0 / 1024.0),
        Some(Unit::Milliseconds) => format!("{value:.1} ms"),
        Some(Unit::Cores) => format!("{value:.2} ядер"),
        _ => format!("{value:.2}"),
    }
}

/// Текстовый отчёт для CLI и TUI.
#[must_use]
pub fn render_text(report: &DiffReport) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(&format!("A {}   B {}\n\n", report.a, report.b));

    out.push_str("entities\n");
    if report.structural.is_empty() {
        out.push_str("  без изменений\n");
    } else {
        for change in report.structural.iter().take(20) {
            out.push_str(&format!(
                "  {} {} {}\n",
                change.kind.glyph(),
                change.entity_kind.as_str(),
                change.name
            ));
        }
    }

    out.push_str("\nresources\n");
    if report.metrics.is_empty() {
        out.push_str("  без изменений\n");
    } else {
        for change in report.metrics.iter().take(12) {
            out.push_str(&format!(
                "  {:<24} {:<28} {} -> {}\n",
                truncate(&change.name, 24),
                metric_label(change.metric),
                format_value(change.metric, change.before),
                format_value(change.metric, change.after)
            ));
        }
    }

    out.push_str("\nbetween A and B\n");
    if report.events.is_empty() {
        out.push_str("  событий нет\n");
    } else {
        for event in report.events.iter().take(20) {
            out.push_str(&format!(
                "  {} {} {} {}\n",
                event.at,
                event.kind.glyph(),
                event.entity_name,
                event.detail
            ));
        }
    }

    out.push_str("\ncorrelated changes\n");
    if report.evidence.is_empty() {
        out.push_str("  нет достаточно значимых совпадений\n");
    } else {
        for item in &report.evidence {
            out.push_str(&format!("  [{:.2}] {}\n", item.score, item.text));
        }
    }

    out
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::config::Store as StoreConfig;
    use pulse_core::entity::{EntityKey, EntitySpec, Runtime};
    use pulse_core::metric::ids;
    use pulse_core::EntityGraph;

    #[test]
    fn magnitude_is_clamped_at_z_max() {
        assert!((magnitude_score(0.0) - 0.0).abs() < 1e-12);
        assert!((magnitude_score(2.0) - 0.5).abs() < 1e-12);
        assert!((magnitude_score(4.0) - 1.0).abs() < 1e-12);
        assert!((magnitude_score(10.0) - 1.0).abs() < 1e-12);
        assert!((magnitude_score(-10.0) - 1.0).abs() < 1e-12);
        assert!((magnitude_score(f64::NAN) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn temporal_decays_linearly_and_ignores_future() {
        let anchor = Timestamp::from_millis(1_000_000);
        assert!((temporal_score(anchor, anchor) - 1.0).abs() < 1e-12);
        let half = Timestamp::from_millis(1_000_000 - 60_000);
        assert!((temporal_score(anchor, half) - 0.5).abs() < 1e-12);
        let edge = Timestamp::from_millis(1_000_000 - 120_000);
        assert!((temporal_score(anchor, edge) - 0.0).abs() < 1e-12);
        let older = Timestamp::from_millis(1_000_000 - 300_000);
        assert!((temporal_score(anchor, older) - 0.0).abs() < 1e-12);
        let future = Timestamp::from_millis(1_000_000 + 5_000);
        assert!(
            (temporal_score(anchor, future) - 0.0).abs() < 1e-12,
            "будущее не объясняет прошлое"
        );
        let just_after = Timestamp::from_millis(1_000_000 + 500);
        assert!(
            (temporal_score(anchor, just_after) - 0.0).abs() < 1e-12,
            "полсекунды после anchor — тоже будущее, без допуска"
        );
    }

    #[test]
    fn graph_score_decays_with_hops_and_cuts_off() {
        assert!((graph_score(Some(0)) - 1.0).abs() < 1e-12);
        assert!((graph_score(Some(1)) - 0.5).abs() < 1e-12);
        assert!((graph_score(Some(4)) - 0.2).abs() < 1e-12);
        assert!((graph_score(Some(5)) - 0.0).abs() < 1e-12);
        assert!((graph_score(None) - 0.0).abs() < 1e-12);
    }

    /// Инвариант: всё, что diff сравнивает, обязано храниться в тёплом слое.
    /// Иначе A/B за пределами горячего окна молча вернёт пустой отчёт.
    #[test]
    fn watched_metrics_are_retained_long() {
        for metric in WATCHED {
            assert!(
                pulse_core::metric::retains_long_window(*metric),
                "метрика {} сравнивается, но не имеет длинного ретеншна",
                metric.0
            );
        }
        // Список задан константой, поэтому проверяем не пустоту, а то, что
        // цикл выше действительно что-то проверил.
        assert!(WATCHED.len() >= 3, "перечень сравниваемых метрик усох");
    }

    /// Ресурсная зависимость обязана быть подтверждённой, а не выведенной из
    /// вида сущности: иначе слагаемое ничего не различает.
    #[test]
    fn resource_score_requires_a_declared_dependency() {
        let mut consumer = record(EntityId::new(1, 1), "nginx.service", None);
        consumer.kind = EntityKind::Unit;
        consumer.labels.set("disks", "sda,sdb");

        let mut sda = record(EntityId::new(2, 1), "sda", None);
        sda.kind = EntityKind::Disk;
        let mut sdz = record(EntityId::new(3, 1), "sdz", None);
        sdz.kind = EntityKind::Disk;

        // Направление не важно, важно наличие зависимости.
        assert!((resource_score(&sda, &consumer) - 1.0).abs() < 1e-12);
        assert!((resource_score(&consumer, &sda) - 1.0).abs() < 1e-12);
        assert!(
            (resource_score(&sdz, &consumer) - 0.0).abs() < 1e-12,
            "диск, которым сервис не пользуется, не ресурсная зависимость"
        );

        let mut unlabelled = record(EntityId::new(4, 1), "cron.service", None);
        unlabelled.kind = EntityKind::Unit;
        assert!(
            (resource_score(&sda, &unlabelled) - 0.0).abs() < 1e-12,
            "без подтверждённого трафика балл не выдаётся"
        );
    }

    #[test]
    fn shared_parent_consumers_are_resource_related() {
        let parent = EntityId::new(50, 1);
        let mut first = record(EntityId::new(51, 1), "a.service", Some(parent));
        first.kind = EntityKind::Unit;
        let mut second = record(EntityId::new(52, 1), "b.service", Some(parent));
        second.kind = EntityKind::Unit;
        assert!((resource_score(&first, &second) - 1.0).abs() < 1e-12);

        let orphan_a = record(EntityId::new(53, 1), "a.service", None);
        let orphan_b = record(EntityId::new(54, 1), "b.service", None);
        assert!(
            (resource_score(&orphan_a, &orphan_b) - 0.0).abs() < 1e-12,
            "отсутствие родителя не делает сущности связанными"
        );
    }

    #[test]
    fn score_weights_sum_to_one() {
        let total = W_MAGNITUDE + W_TEMPORAL + W_GRAPH + W_RESOURCE;
        assert!((total - 1.0).abs() < 1e-12);
        assert!((score(1.0, 1.0, 1.0, 1.0) - 1.0).abs() < 1e-12);
        assert!((score(0.0, 0.0, 0.0, 0.0) - 0.0).abs() < 1e-12);
    }

    /// Синтетическая история: контейнер перезапускается, процесс исчезает,
    /// задержка диска растёт.
    fn scenario() -> (History, Timestamp, Timestamp) {
        let mut history = History::new(&StoreConfig::default());
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let mut now = 1_000u64;
        let mut mark_a = Timestamp::ZERO;

        // Фаза 1: всё спокойно, 40 тактов.
        for tick in 0..40 {
            now += 1_000;
            let at = Timestamp::from_millis(now);
            graph.begin_tick(at);
            let host = graph.host();
            let disk = graph.upsert(
                EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host),
            );
            let container = graph.upsert(
                EntitySpec::new(
                    EntityKey::Container {
                        runtime: Runtime::Docker,
                        id: "aaaaaaaaaaaa".into(),
                    },
                    "payment-api",
                )
                .parent(host)
                .logical("payment-api"),
            );
            let process = graph.upsert(
                EntitySpec::new(
                    EntityKey::Process {
                        pid: 500,
                        start_ticks: 10,
                    },
                    "java",
                )
                .parent(container),
            );
            graph.sample(disk, ids::DISK_AWAIT, 2.0 + (tick % 3) as f64 * 0.1);
            graph.sample(disk, ids::DISK_READ_OPS, 100.0 * (tick + 1) as f64);
            graph.sample(container, ids::CG_CPU_CORES, 1.0 + (tick % 2) as f64 * 0.05);
            graph.sample(process, ids::PROC_RSS, 100.0 * 1024.0 * 1024.0);
            let batch = graph.end_tick();
            history.ingest(&batch);
            if tick == 30 {
                mark_a = at;
            }
        }

        // Фаза 2: контейнер перезапущен под новым id, процесс исчез,
        // задержка диска выросла в 20 раз.
        for tick in 0..40 {
            now += 1_000;
            let at = Timestamp::from_millis(now);
            graph.begin_tick(at);
            let host = graph.host();
            let disk = graph.upsert(
                EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host),
            );
            let container = graph.upsert(
                EntitySpec::new(
                    EntityKey::Container {
                        runtime: Runtime::Docker,
                        id: "bbbbbbbbbbbb".into(),
                    },
                    "payment-api",
                )
                .parent(host)
                .logical("payment-api"),
            );
            graph.sample(disk, ids::DISK_AWAIT, 45.0 + (tick % 3) as f64);
            graph.sample(disk, ids::DISK_READ_OPS, 100.0 * (tick + 41) as f64);
            graph.sample(container, ids::CG_CPU_CORES, 3.0);
            let batch = graph.end_tick();
            history.ingest(&batch);
        }

        let mark_b = Timestamp::from_millis(now);
        (history, mark_a, mark_b)
    }

    #[test]
    fn golden_diff_finds_structure_metrics_and_events() {
        let (history, a, b) = scenario();
        let report = diff(&history, a, b, &DiffOptions::default());

        assert!(
            report
                .structural
                .iter()
                .any(|c| c.kind == ChangeKind::Deleted && c.name == "java"),
            "исчезновение процесса должно быть замечено: {:?}",
            report.structural
        );
        assert!(
            report
                .structural
                .iter()
                .any(|c| c.kind == ChangeKind::Restarted || c.kind == ChangeKind::Created),
            "перезапуск контейнера должен быть виден"
        );
        let disk_change = report
            .metrics
            .iter()
            .find(|m| m.metric == ids::DISK_AWAIT)
            .expect("рост задержки диска обязан попасть в отчёт");
        assert!(disk_change.after > disk_change.before * 5.0);
        assert!(!report.evidence.is_empty(), "должны быть доказательства");
        assert!(
            report.evidence.windows(2).all(|w| {
                w.first().map(|e| e.score).unwrap_or_default()
                    >= w.get(1).map(|e| e.score).unwrap_or_default()
            }),
            "доказательства обязаны быть отсортированы по убыванию"
        );
        assert!(report.evidence.len() <= DiffOptions::default().max_evidence);
    }

    #[test]
    fn diff_arguments_can_be_swapped() {
        let (history, a, b) = scenario();
        let forward = diff(&history, a, b, &DiffOptions::default());
        let backward = diff(&history, b, a, &DiffOptions::default());
        assert_eq!(forward.a, backward.a);
        assert_eq!(forward.b, backward.b);
    }

    #[test]
    fn text_report_has_all_sections_and_no_causal_claims() {
        let (history, a, b) = scenario();
        let report = diff(&history, a, b, &DiffOptions::default());
        let text = render_text(&report).to_lowercase();
        for section in [
            "entities",
            "resources",
            "between a and b",
            "correlated changes",
        ] {
            assert!(text.contains(section), "нет секции {section}");
        }
        for forbidden in ["вызвал", "причина", "caused", "because"] {
            assert!(
                !text.contains(forbidden),
                "недопустимое причинное утверждение: {forbidden}"
            );
        }
    }

    #[test]
    fn empty_history_gives_empty_report_without_panic() {
        let history = History::new(&StoreConfig::default());
        let report = diff(
            &history,
            Timestamp::from_millis(1_000),
            Timestamp::from_millis(2_000),
            &DiffOptions::default(),
        );
        assert!(report.structural.is_empty());
        assert!(report.metrics.is_empty());
        assert!(report.evidence.is_empty());
        let text = render_text(&report);
        assert!(text.contains("без изменений"));
    }

    #[test]
    fn degenerate_stddev_does_not_produce_infinite_z() {
        let (history, a, b) = scenario();
        let report = diff(&history, a, b, &DiffOptions::default());
        assert!(
            report.metrics.iter().all(|m| m.z.is_finite()),
            "z обязан быть конечным даже при нулевом разбросе"
        );
    }

    #[test]
    fn min_score_filters_weak_evidence() {
        let (history, a, b) = scenario();
        let strict = DiffOptions {
            min_score: 0.95,
            ..DiffOptions::default()
        };
        let report = diff(&history, a, b, &strict);
        assert!(
            report.evidence.len() <= 2,
            "жёсткий порог должен резко сокращать список: {:?}",
            report.evidence
        );
    }
    /// Запись сущности для проверок дедупа: важны только id, имя и родитель.
    fn record(id: EntityId, name: &str, parent: Option<EntityId>) -> EntityRecord {
        EntityRecord {
            id,
            key: pulse_core::entity::EntityKey::Unit { name: name.into() },
            kind: EntityKind::Unit,
            name: name.to_string(),
            parent,
            labels: pulse_core::entity::Labels::new(),
            first_seen: Timestamp::ZERO,
            last_seen: Timestamp::ZERO,
            alive: true,
        }
    }

    fn change(entity: EntityId, name: &str, after: f64) -> MetricChange {
        MetricChange {
            entity,
            name: name.to_string(),
            metric: ids::CG_CPU_CORES,
            before: 1.0,
            after,
            delta: after - 1.0,
            z: after - 1.0,
        }
    }

    #[test]
    fn mirrored_owner_and_cgroup_metrics_count_as_one_fact() {
        let cgroup = EntityId::new(1, 1);
        let owner = EntityId::new(2, 1);
        let other = EntityId::new(3, 1);
        let records = [
            record(cgroup, "nginx.service", None),
            record(owner, "nginx.service", Some(cgroup)),
            record(other, "nginx.service", None),
        ];
        let map: HashMap<EntityId, &EntityRecord> = records.iter().map(|r| (r.id, r)).collect();

        let mut changes = vec![
            change(cgroup, "nginx.service", 2.0),
            change(owner, "nginx.service", 2.0),
            change(other, "nginx.service", 3.0),
        ];
        deduplicate_mirrored_metrics(&mut changes, &map);

        assert_eq!(changes.len(), 2, "зеркало обязано свернуться: {changes:?}");
        assert!(
            changes.iter().all(|c| c.entity != cgroup),
            "удаляется технический cgroup, остаётся владелец"
        );
        assert!(changes.iter().any(|c| c.entity == owner));
        assert!(changes.iter().any(|c| c.entity == other));
    }

    /// Регрессия на кейс, из-за которого дедуп по имени был опасен: два
    /// `dbus.socket` из разных user-manager — независимые сущности, и равенство
    /// их значений не повод потерять строку.
    #[test]
    fn same_named_unrelated_entities_are_never_folded() {
        let first = EntityId::new(10, 1);
        let second = EntityId::new(11, 1);
        let records = [
            record(first, "dbus.socket", Some(EntityId::new(20, 1))),
            record(second, "dbus.socket", Some(EntityId::new(21, 1))),
        ];
        let map: HashMap<EntityId, &EntityRecord> = records.iter().map(|r| (r.id, r)).collect();

        let mut changes = vec![
            change(first, "dbus.socket", 2.0),
            change(second, "dbus.socket", 2.0),
        ];
        deduplicate_mirrored_metrics(&mut changes, &map);

        assert_eq!(
            changes.len(),
            2,
            "одноимённые, но не родственные: {changes:?}"
        );
    }

    /// Родство без совпадения имени тоже не зеркало: вложенные slice со
    /// случайно равными значениями остаются отдельными фактами.
    #[test]
    fn parent_child_with_different_names_is_not_a_mirror() {
        let parent = EntityId::new(30, 1);
        let child = EntityId::new(31, 1);
        let records = [
            record(parent, "system.slice", None),
            record(child, "cron.service", Some(parent)),
        ];
        let map: HashMap<EntityId, &EntityRecord> = records.iter().map(|r| (r.id, r)).collect();

        let mut changes = vec![
            change(parent, "system.slice", 2.0),
            change(child, "cron.service", 2.0),
        ];
        deduplicate_mirrored_metrics(&mut changes, &map);

        assert_eq!(changes.len(), 2, "разные имена — разные факты: {changes:?}");
    }
}
