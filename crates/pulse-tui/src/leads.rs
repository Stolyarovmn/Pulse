//! Зацепки расследования: куда смотреть дальше и почему.
//!
//! Инспектор отвечает на «что это», цепочка — на «из чего оно состоит».
//! Зацепка отвечает на вопрос следователя: **где тут интересное**. Каждая —
//! измеренный факт о конкретном объекте, по которому можно перейти `Enter`:
//! открытая проблема внутри, ненормальное состояние, доминирующая доля
//! ресурса, сосед на том же диске, перезапуск.
//!
//! Слов о причинности здесь нет (раздел 47.2): «несёт 86 % CPU группы» —
//! факт, «вызвал нагрузку» — догадка. Зацепка показывает, куда идти, а вывод
//! делает оператор.

use std::collections::{HashMap, HashSet, VecDeque};

use pulse_core::entity::{Entity, EntityId, EntityKey, EntityKind};
use pulse_core::metric::ids;
use pulse_core::snapshot::Snapshot;
use pulse_core::{EventKind, RelationKind, Severity};

use crate::state::StateClass;

/// Сколько зацепок показывать: больше — уже список, а не подсказка.
pub const LEADS_SHOWN: usize = 5;

/// Глубина обхода вниз: от слайса до процесса контейнера — пять уровней.
const MAX_DEPTH: usize = 8;

/// Потолок обхода: дерево `system.slice` на stage-1 — около 1300 узлов.
const MAX_VISITED: usize = 20_000;

/// Доля, с которой ребёнок считается доминирующим.
const DOMINANT_SHARE: f64 = 0.5;

/// Одна зацепка.
#[derive(Clone, Debug, PartialEq)]
pub struct Lead {
    /// Куда ведёт `Enter`.
    pub key: EntityKey,
    /// Узнаваемое имя цели.
    pub name: String,
    /// Класс для символа и цвета.
    pub class: StateClass,
    /// Измеренный факт одной строкой.
    pub reason: String,
    weight: u64,
}

/// Объекты, которые для оператора — одно и то же: cgroup и её владелец.
///
/// Правила кладут проблему на владельца (`nginx.service`), а в инспектор
/// можно прийти и на его cgroup. Дело обязано показать беду в обоих случаях.
#[must_use]
pub fn twins(snapshot: &Snapshot, id: EntityId) -> Vec<EntityId> {
    let mut out = vec![id];
    for relation in snapshot.relations_of(id, Some(RelationKind::OwnedBy)) {
        let other = if relation.from == id {
            relation.to
        } else {
            relation.from
        };
        if !out.contains(&other) {
            out.push(other);
        }
    }
    out
}

/// Корень поддерева объекта: у unit/контейнера/pod это его cgroup.
#[must_use]
pub fn subtree_root(snapshot: &Snapshot, entity: &Entity) -> EntityId {
    if matches!(
        entity.kind,
        EntityKind::Unit | EntityKind::Container | EntityKind::Pod
    ) {
        if let Some(parent) = entity.parent.and_then(|id| snapshot.entity(id)) {
            if parent.kind == EntityKind::Cgroup {
                return parent.id;
            }
        }
    }
    entity.id
}

/// Потомки корня (без него самого) с глубиной, в порядке обхода в ширину.
fn descendants(snapshot: &Snapshot, root: EntityId) -> Vec<(EntityId, usize)> {
    let mut children: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
    for entity in &snapshot.entities {
        if let Some(parent) = entity.parent {
            children.entry(parent).or_default().push(entity.id);
        }
    }
    let mut out = Vec::new();
    let mut seen: HashSet<EntityId> = HashSet::from([root]);
    let mut queue = VecDeque::from([(root, 0_usize)]);
    while let Some((id, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH || out.len() >= MAX_VISITED {
            continue;
        }
        for child in children.get(&id).into_iter().flatten() {
            if seen.insert(*child) {
                out.push((*child, depth + 1));
                queue.push_back((*child, depth + 1));
            }
        }
    }
    out
}

/// Зацепки для объекта, сильнейшая первой, не больше `limit`.
#[must_use]
pub fn leads(snapshot: &Snapshot, id: EntityId, limit: usize) -> Vec<Lead> {
    let Some(entity) = snapshot.entity(id) else {
        return Vec::new();
    };
    let own = twins(snapshot, id);
    let root = subtree_root(snapshot, entity);
    let below = descendants(snapshot, root);
    let inside: HashSet<EntityId> = below.iter().map(|(id, _)| *id).collect();
    let mut found: Vec<Lead> = Vec::new();

    let lead = |target: &Entity, class: StateClass, reason: String, weight: u64| Lead {
        key: target.key.clone(),
        name: crate::fold::display_name(snapshot, target),
        class,
        reason,
        weight,
    };

    // 1. Открытые проблемы внутри: самая сильная зацепка.
    for problem in &snapshot.problems {
        let target = problem.id.entity;
        if own.contains(&target) || !inside.contains(&target) {
            continue;
        }
        let Some(target) = snapshot.entity(target) else {
            continue;
        };
        let weight = match problem.severity {
            Severity::Crit => 300_000,
            Severity::Warn => 200_000,
            Severity::Info => 100_000,
        };
        found.push(lead(
            target,
            StateClass::from_severity(problem.severity),
            problem.title.clone(),
            weight,
        ));
    }

    // 2. Ненормальное состояние без проблемы: ещё не подтверждено, но уже
    // выделяется среди соседей.
    for (child, depth) in &below {
        let Some(target) = snapshot.entity(*child) else {
            continue;
        };
        if target.kind == EntityKind::Host {
            continue;
        }
        let state = crate::rows::state_of(snapshot, target, None);
        if state.is_abnormal() {
            let weight = 50_000_u64.saturating_sub(*depth as u64 * 1_000);
            found.push(lead(
                target,
                state,
                format!("state {}", state.label()),
                weight,
            ));
        }
    }

    // 3. Доминирующая доля ресурса среди прямых детей.
    dominant_children(snapshot, root, &mut found);

    // 4. Перезапуски и OOM внутри.
    for event in &snapshot.meaningful {
        if !matches!(event.kind, EventKind::Restarted | EventKind::OomKill) {
            continue;
        }
        let Some(target) = event
            .entity
            .filter(|id| inside.contains(id) || own.contains(id))
        else {
            continue;
        };
        let Some(target) = snapshot.entity(target) else {
            continue;
        };
        let what = if event.kind == EventKind::OomKill {
            "OOM kill"
        } else {
            "restarted"
        };
        let reason = if event.count > 1 {
            format!("{what} ×{} · last {}", event.count, event.last_at)
        } else {
            format!("{what} · {}", event.at)
        };
        let class = if event.kind == EventKind::OomKill {
            StateClass::Critical
        } else {
            StateClass::Warning
        };
        found.push(lead(target, class, reason, 150_000));
    }

    // 5. Сосед по диску, который сейчас двигает больше всех данных.
    busiest_disk_neighbour(snapshot, entity, &own, &mut found);

    found.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.name.cmp(&b.name)));
    // Одна цель — одна зацепка, самая сильная; цель — логический объект,
    // чтобы Enter открывал сервис, а не его техническую cgroup.
    let mut seen: HashSet<EntityKey> = HashSet::new();
    for lead in &mut found {
        lead.key = logical_key(snapshot, &lead.key);
    }
    found.retain(|lead| seen.insert(lead.key.clone()));
    found.truncate(limit);
    found
}

/// Логическая идентичность цели: у cgroup с владельцем — владелец.
///
/// Unit и его cgroup — один объект для оператора. Без этого `docker.service`
/// попадал в зацепки дважды: как перезапущенный unit и как cgroup с
/// доминирующей долей CPU.
fn logical_key(snapshot: &Snapshot, key: &EntityKey) -> EntityKey {
    let Some(entity) = snapshot.entities.iter().find(|entity| &entity.key == key) else {
        return key.clone();
    };
    twins(snapshot, entity.id)
        .into_iter()
        .filter_map(|id| snapshot.entity(id))
        .find(|twin| {
            matches!(
                twin.kind,
                EntityKind::Unit | EntityKind::Container | EntityKind::Pod
            )
        })
        .map_or_else(|| key.clone(), |owner| owner.key.clone())
}

/// Ребёнок, который несёт больше половины CPU или памяти объекта.
fn dominant_children(snapshot: &Snapshot, root: EntityId, found: &mut Vec<Lead>) {
    let Some(root_entity) = snapshot.entity(root) else {
        return;
    };
    let rows: Vec<crate::rows::EntityRow> = snapshot
        .children(root)
        .map(|child| crate::rows::row_of(snapshot, child))
        .collect();
    let folded = crate::fold::fold(snapshot, &rows);
    // Доля от единственной части — всегда 100 %, это не зацепка.
    if folded.len() < 2 {
        return;
    }
    let total = crate::rows::row_of(snapshot, root_entity);
    for child in &folded {
        let Some(target) = snapshot.entity(child.row.id) else {
            continue;
        };
        let cpu_share = share(child.row.cpu, total.cpu, 0.05);
        let mem_share = if total.memory_measured && child.row.memory_measured {
            share(child.row.memory, total.memory, 64.0 * 1024.0 * 1024.0)
        } else {
            None
        };
        let reason = match (cpu_share, mem_share) {
            (Some(cpu), _) => format!(
                "{:.0}% of CPU here ({} of {})",
                cpu * 100.0,
                crate::format::cores(child.row.cpu),
                crate::format::cores(total.cpu)
            ),
            (None, Some(mem)) => format!(
                "{:.0}% of memory here ({} of {})",
                mem * 100.0,
                crate::format::bytes(child.row.memory),
                crate::format::bytes(total.memory)
            ),
            (None, None) => continue,
        };
        let weight = 20_000 + (cpu_share.or(mem_share).unwrap_or(0.0) * 1_000.0) as u64;
        found.push(Lead {
            key: target.key.clone(),
            name: child.row.name.clone(),
            class: StateClass::Saturated,
            reason,
            weight,
        });
    }
}

/// Доля части в целом, если целое заметно и доля доминирует.
fn share(part: f64, whole: f64, floor: f64) -> Option<f64> {
    if !(part.is_finite() && whole.is_finite()) || whole < floor {
        return None;
    }
    let ratio = part / whole;
    (DOMINANT_SHARE..=1.0 + 1e-9)
        .contains(&ratio)
        .then_some(ratio.min(1.0))
}

/// Самый активный сосед того же уровня на общем диске.
fn busiest_disk_neighbour(
    snapshot: &Snapshot,
    entity: &Entity,
    own: &[EntityId],
    found: &mut Vec<Lead>,
) {
    let rate = |id: EntityId| {
        snapshot
            .value(id, ids::CG_IO_READ_THROUGHPUT)
            .unwrap_or(0.0)
            + snapshot
                .value(id, ids::CG_IO_WRITE_THROUGHPUT)
                .unwrap_or(0.0)
    };
    let mut best: Option<(&Entity, &Entity, f64)> = None;
    for resource in crate::investigate::context_resources(snapshot, entity.id) {
        let Some(disk) = snapshot.entities.iter().find(|e| e.key == resource.key) else {
            continue;
        };
        for relation in snapshot.relations_of(disk.id, None) {
            let other = if relation.from == disk.id {
                relation.to
            } else {
                relation.from
            };
            if own.contains(&other) || crate::investigate::is_lineage(snapshot, entity.id, other) {
                continue;
            }
            let Some(neighbour) = snapshot.entity(other) else {
                continue;
            };
            if neighbour.kind != entity.kind {
                continue;
            }
            let moved = rate(other);
            if moved > 0.0 && best.is_none_or(|(_, _, top)| moved > top) {
                best = Some((neighbour, disk, moved));
            }
        }
    }
    if let Some((neighbour, disk, moved)) = best {
        found.push(Lead {
            key: neighbour.key.clone(),
            name: crate::fold::display_name(snapshot, neighbour),
            class: StateClass::Normal,
            reason: format!("same disk {} · {}", disk.name, crate::format::rate(moved)),
            weight: 5_000,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::EntitySpec;
    use pulse_core::problem::{Problem, ProblemId, RuleId};
    use pulse_core::sample::SeriesKey;
    use pulse_core::snapshot::{AgentStats, LatestValues};
    use pulse_core::time::Timestamp;
    use pulse_core::EntityGraph;

    /// Слайс с двумя сервисами: один тяжёлый, у второго открыта проблема
    /// на владельце; соседний сервис пишет на тот же диск.
    fn case() -> (Snapshot, EntityId, EntityId) {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let disk = graph
            .upsert(EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host));
        let slice = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 1 }, "system.slice").parent(host),
        );
        let mut service = |id: u64, name: &str| {
            let cgroup = graph
                .upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: id }, name).parent(slice));
            let unit = graph.upsert(
                EntitySpec::new(EntityKey::Unit { name: name.into() }, name).parent(cgroup),
            );
            graph.relate(cgroup, RelationKind::OwnedBy, unit);
            (cgroup, unit)
        };
        let (heavy, _) = service(2, "dockerd.service");
        let (_, sick) = service(3, "nginx.service");
        let (_, _) = service(4, "cron.service");
        graph.relate(slice, RelationKind::BackedBy, disk);
        let other = graph
            .upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 9 }, "user.slice").parent(host));
        graph.relate(other, RelationKind::BackedBy, disk);
        let _ = graph.end_tick();

        let mut latest = LatestValues::new();
        latest.set(SeriesKey::new(slice, ids::CG_CPU_CORES), 2.5);
        latest.set(SeriesKey::new(heavy, ids::CG_CPU_CORES), 2.1);
        latest.set(
            SeriesKey::new(other, ids::CG_IO_WRITE_THROUGHPUT),
            4.0 * 1024.0 * 1024.0,
        );
        let mut snapshot = Snapshot::build(
            &graph,
            latest,
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        );
        snapshot.problems.push(Problem {
            id: ProblemId {
                rule: RuleId("memory.pressure"),
                entity: sick,
            },
            severity: Severity::Crit,
            entity_name: "nginx.service".to_string(),
            title: "memory pressure 97%".to_string(),
            summary: String::new(),
            evidence: Vec::new(),
            since: Timestamp::from_millis(1_500),
            last_seen: Timestamp::from_millis(2_000),
            streak: 3,
        });
        (snapshot, slice, sick)
    }

    /// Порядок — сила факта: проблема внутри, затем доминирующая доля, затем
    /// сосед по диску. Причины — измеренные величины, без слов о причинности.
    #[test]
    fn leads_rank_problem_then_dominant_share_then_disk_neighbour() {
        let (snapshot, slice, _) = case();
        let found = leads(&snapshot, slice, LEADS_SHOWN);
        let summary: Vec<(&str, &str)> = found
            .iter()
            .map(|lead| (lead.name.as_str(), lead.reason.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("nginx.service", "memory pressure 97%"),
                ("dockerd.service", "84% of CPU here (2.10c of 2.50c)"),
                ("user.slice", "same disk sda · 4.0 MiB/s"),
            ],
            "{found:#?}"
        );
    }

    /// Проблема самого объекта — не зацепка «куда идти», она уже в деле.
    /// Открыть cgroup сервиса и открыть сам unit — одно и то же дело.
    #[test]
    fn own_problem_is_not_a_lead_even_through_the_cgroup_twin() {
        let (snapshot, _, sick) = case();
        let cgroup = snapshot
            .entity(sick)
            .and_then(|unit| unit.parent)
            .expect("cgroup");
        for subject in [sick, cgroup] {
            assert!(twins(&snapshot, subject).contains(&sick));
            let found = leads(&snapshot, subject, LEADS_SHOWN);
            assert!(
                found
                    .iter()
                    .all(|lead| lead.reason != "memory pressure 97%"),
                "{found:#?}"
            );
        }
    }

    /// Unit и его cgroup — один объект. Перезапуск unit и доминирующая доля
    /// его cgroup обязаны дать одну зацепку (сильнейшую), и `Enter` по ней
    /// открывает сервис, а не техническую cgroup.
    #[test]
    fn one_object_is_one_lead_even_when_unit_and_cgroup_both_qualify() {
        let (mut snapshot, slice, _) = case();
        let unit = snapshot
            .entities
            .iter()
            .find(|entity| entity.kind == EntityKind::Unit && entity.name == "dockerd.service")
            .map(|entity| (entity.id, entity.key.clone()))
            .expect("unit");
        snapshot.meaningful.push(pulse_core::MeaningfulEvent {
            at: Timestamp::from_millis(1_900),
            last_at: Timestamp::from_millis(1_900),
            kind: EventKind::Restarted,
            severity: Severity::Warn,
            significance: pulse_core::semantic::Significance::Operational,
            entity: Some(unit.0),
            entity_kind: Some(EntityKind::Unit),
            entity_name: "dockerd.service".to_string(),
            detail: String::new(),
            count: 1,
        });
        let found = leads(&snapshot, slice, LEADS_SHOWN);
        let docker: Vec<&Lead> = found
            .iter()
            .filter(|lead| lead.name == "dockerd.service")
            .collect();
        assert_eq!(docker.len(), 1, "{found:#?}");
        assert!(docker.iter().all(|lead| lead.key == unit.1), "{found:#?}");
        assert!(
            docker
                .iter()
                .all(|lead| lead.reason.starts_with("restarted")),
            "сильнейший факт — перезапуск: {found:#?}"
        );
    }
}
