//! Направленная цепочка расследования.
//!
//! Проблема, которую решает этот модуль: раньше `Enter` вёл по любой связи
//! графа. Из сервиса можно было провалиться в диск, из диска — в другой сервис,
//! из него снова в диск, и так бесконечно. Граф связен, но расследование — нет:
//! оператор идёт от общего к конкретному и обязан приходить к ответу, а не
//! возвращаться на тот же уровень.
//!
//! Модель:
//!
//! ```text
//! host → workload (unit/pod) → container → cgroup → process → detail
//! ```
//!
//! * `Enter` спускается строго на уровень глубже. Вверх ведёт только `Esc`.
//! * Диски и интерфейсы не являются звеньями цепочки: это ресурсы, которыми
//!   пользуется объект. Они показываются как контекст и не открываются, иначе
//!   становятся пересадочным узлом между несвязанными сервисами.
//! * Переход «в сторону» (смежный или влияющий объект) существует, но это не
//!   продолжение цепочки: он начинает новое расследование с нового корня.

use pulse_core::entity::{EntityKey, EntityKind};
use pulse_core::snapshot::Snapshot;
use pulse_core::{EntityId, RelationKind};

/// Уровень объекта в расследовании. Меньше — общее, больше — конкретнее.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Host,
    Workload,
    Container,
    Cgroup,
    Process,
    /// Ресурс: диск, интерфейс. Не звено цепочки, а её контекст.
    Resource,
}

impl Level {
    /// Уровень по виду сущности.
    #[must_use]
    pub const fn of(kind: EntityKind) -> Self {
        match kind {
            EntityKind::Host => Level::Host,
            EntityKind::Unit | EntityKind::Pod => Level::Workload,
            EntityKind::Container => Level::Container,
            EntityKind::Cgroup => Level::Cgroup,
            EntityKind::Process => Level::Process,
            EntityKind::Disk | EntityKind::NetIf => Level::Resource,
        }
    }

    /// Порядковый номер для сравнения глубины.
    #[must_use]
    pub const fn depth(self) -> u8 {
        match self {
            Level::Host => 0,
            Level::Workload => 1,
            Level::Container => 2,
            Level::Cgroup => 3,
            Level::Process => 4,
            // Ресурс вне вертикали: он не глубже и не выше, он рядом.
            Level::Resource => u8::MAX,
        }
    }

    /// Является ли уровень ресурсом, а не звеном цепочки.
    #[must_use]
    pub const fn is_resource(self) -> bool {
        matches!(self, Level::Resource)
    }

    /// Человекочитаемое имя уровня для подписи цепочки.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Level::Host => "host",
            Level::Workload => "workload",
            Level::Container => "container",
            Level::Cgroup => "cgroup",
            Level::Process => "process",
            Level::Resource => "resource",
        }
    }
}

/// Объект, доступный для перехода.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// Подпись отношения: `runs`, `child`, `owner`.
    pub label: &'static str,
    pub key: EntityKey,
    pub name: String,
    pub kind: EntityKind,
    /// Сколько однотипных объектов свёрнуто в эту строку.
    pub count: usize,
}

/// Звенья цепочки: объекты строго глубже текущего.
///
/// Именно этот список получает `Enter`. Разрешены два вида шагов: вниз по
/// уровню по любой связи и вниз по дереву владения (`parent`), даже если вид
/// объекта тот же — вложенные cgroup именно так и устроены, а дерево ациклично
/// по построению. Запрещён шаг «в бок» по связи между равными: ровно он и
/// создавал круги сервис → диск → другой сервис.
#[must_use]
pub fn drill_steps(snapshot: &Snapshot, id: EntityId) -> Vec<Step> {
    let Some(entity) = snapshot.entity(id) else {
        return Vec::new();
    };
    let current = Level::of(entity.kind);
    if current.is_resource() {
        // Ресурс — лист расследования. Из диска нельзя «провалиться» в сервис:
        // диском пользуются десятки несвязанных объектов, и такой переход
        // означал бы прыжок в произвольную сторону, а не углубление.
        return Vec::new();
    }

    // Сначала собираются уникальные объекты, и только потом однотипные
    // сворачиваются. Обратный порядок давал неверный счёт: свёрнутая строка
    // хранит ключ лишь первого объекта, поэтому остальные проходили проверку
    // «уже добавлен» второй раз и на живом хосте 40 процессов показывались
    // как «5 × process» из двух источников — связей и дерева владения.
    let mut candidates: Vec<(EntityKey, String, EntityKind)> = Vec::new();
    let mut push_candidate = |key: EntityKey, name: &str, kind: EntityKind| {
        if candidates.iter().any(|(existing, _, _)| *existing == key) {
            return;
        }
        candidates.push((key, name.to_string(), kind));
    };

    for relation in snapshot.relations_of(id, None) {
        let other_id = if relation.from == id {
            relation.to
        } else if relation.to == id {
            relation.from
        } else {
            continue;
        };
        let Some(other) = snapshot.entity(other_id) else {
            continue;
        };
        let level = Level::of(other.kind);
        if level.is_resource() || level.depth() <= current.depth() {
            continue;
        }
        push_candidate(other.key.clone(), &other.name, other.kind);
    }

    // Дети по дереву владения: связь `ParentOf` может отсутствовать, а
    // `parent` есть. Здесь допустим тот же уровень: `/system.slice` и
    // `/system.slice/angie.service` — оба cgroup, и это спуск, а не круг.
    for candidate in &snapshot.entities {
        if candidate.parent != Some(id) {
            continue;
        }
        let level = Level::of(candidate.kind);
        if level.is_resource() || level.depth() < current.depth() {
            continue;
        }
        push_candidate(candidate.key.clone(), &candidate.name, candidate.kind);
    }

    let mut steps: Vec<Step> = Vec::new();
    for (key, name, kind) in candidates {
        push_step(&mut steps, key, &name, kind);
    }

    // Сначала ближайший уровень: расследование спускается по одной ступени.
    steps.sort_by_key(|step| (Level::of(step.kind).depth(), step.name.clone()));
    steps
}

/// Ресурсы объекта: контекст, а не звенья цепочки.
///
/// Здесь нет свёртки однотипных, в отличие от [`drill_steps`]: каждый ресурс
/// занимает не строку, а одно слово, и у каждого своё состояние. Свёрнутое
/// «6 disks» скрыло бы, что деградировал ровно один из них.
#[must_use]
pub fn context_resources(snapshot: &Snapshot, id: EntityId) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    for relation in snapshot.relations_of(id, None) {
        let other_id = if relation.from == id {
            relation.to
        } else if relation.to == id {
            relation.from
        } else {
            continue;
        };
        let Some(other) = snapshot.entity(other_id) else {
            continue;
        };
        if !Level::of(other.kind).is_resource() {
            continue;
        }
        if out.iter().any(|step| step.key == other.key) {
            continue;
        }
        out.push(Step {
            label: forward_label(relation.kind),
            key: other.key.clone(),
            name: other.name.clone(),
            kind: other.kind,
            count: 1,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Боковые переходы: смежные объекты того же уровня и владельцы.
///
/// Это не продолжение цепочки. Открытие такого объекта начинает новое
/// расследование, поэтому в интерфейсе он живёт отдельным блоком и открывается
/// отдельной клавишей.
#[must_use]
pub fn side_steps(snapshot: &Snapshot, id: EntityId) -> Vec<Step> {
    let Some(entity) = snapshot.entity(id) else {
        return Vec::new();
    };
    let current = Level::of(entity.kind);
    let mut out: Vec<Step> = Vec::new();

    // Владелец: подъём как явный прыжок, а не как `Enter`.
    //
    // Родитель обязан быть не глубже текущего объекта. В реальном графе
    // `unit` — ребёнок своей cgroup, и без этого условия из сервиса
    // предлагался бы «владелец» на уровень ниже: техническая cgroup выдавала
    // бы себя за владельца сервиса, которым она как раз и владеема.
    if let Some(parent) = entity.parent.and_then(|parent| snapshot.entity(parent)) {
        let parent_level = Level::of(parent.kind);
        if !parent_level.is_resource() && parent_level.depth() <= current.depth() {
            push_labelled(
                &mut out,
                "owner",
                parent.key.clone(),
                &parent.name,
                parent.kind,
            );
        }
    }

    // Соседи по общему ресурсу: «кто ещё пользуется этим диском».
    for resource in context_resources(snapshot, id) {
        let Some(resource_id) = snapshot
            .entities
            .iter()
            .find(|candidate| candidate.key == resource.key)
            .map(|candidate| candidate.id)
        else {
            continue;
        };
        for relation in snapshot.relations_of(resource_id, None) {
            let neighbour_id = if relation.from == resource_id {
                relation.to
            } else {
                relation.from
            };
            if neighbour_id == id {
                continue;
            }
            let Some(neighbour) = snapshot.entity(neighbour_id) else {
                continue;
            };
            if Level::of(neighbour.kind) != current {
                continue;
            }
            push_labelled(
                &mut out,
                "shares",
                neighbour.key.clone(),
                &neighbour.name,
                neighbour.kind,
            );
        }
    }

    out.sort_by(|a, b| a.label.cmp(b.label).then(a.name.cmp(&b.name)));
    out
}

/// Добавляет звено цепочки, сворачивая однотипные объекты.
///
/// Подпись звена — вид цели, а не направление связи. Причина в реальном
/// графе: коллектор делает `unit` ребёнком своей cgroup, хотя семантически
/// unit владеет ею. Направленная подпись читалась бы как «owner
/// angie.service» на шаге вниз из `angie.service` — оператор видел бы круг
/// там, где происходит спуск. Вид цели однозначен: вниз, в cgroup.
fn push_step(steps: &mut Vec<Step>, key: EntityKey, name: &str, kind: EntityKind) {
    push_labelled(steps, kind.as_str(), key, name, kind);
}

/// Добавляет шаг с явной подписью: для боковых переходов направление важно.
fn push_labelled(
    steps: &mut Vec<Step>,
    label: &'static str,
    key: EntityKey,
    name: &str,
    kind: EntityKind,
) {
    if steps.iter().any(|step| step.key == key) {
        return;
    }
    if let Some(group) = steps
        .iter_mut()
        .find(|step| step.label == label && step.kind == kind)
    {
        group.count = group.count.saturating_add(1);
        // Счётная форма вместо суффикса `s`: `process` давал «40 processs»,
        // а вид сущности приходит из реестра и не обязан быть склоняемым.
        group.name = format!("{} × {}", group.count, kind.as_str());
        return;
    }
    steps.push(Step {
        label,
        key,
        name: name.to_string(),
        kind,
        count: 1,
    });
}

/// Подпись ресурса: чем он служит объекту.
///
/// Обратного варианта нет: из ресурса цепочка не идёт, а в боковых шагах
/// подписи заданы явно (`owner`, `shares`).
const fn forward_label(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::ParentOf => "child",
        RelationKind::RunsIn => "runs in",
        RelationKind::OwnedBy => "owner",
        RelationKind::MemberOf => "member of",
        RelationKind::BackedBy => "backed by",
        RelationKind::ConnectsTo => "connects",
        RelationKind::ScheduledOn => "node",
        RelationKind::ListensOn => "listens",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::EntitySpec;
    use pulse_core::snapshot::{AgentStats, LatestValues};
    use pulse_core::time::Timestamp;
    use pulse_core::EntityGraph;

    /// Хост с сервисом, его cgroup, процессами и общим диском.
    fn snapshot() -> (Snapshot, EntityId, EntityId, EntityId) {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let disk = graph
            .upsert(EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host));

        let cgroup = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 10 }, "docker.service").parent(host),
        );
        let unit = graph.upsert(
            EntitySpec::new(
                EntityKey::Unit {
                    name: "docker.service".into(),
                },
                "docker.service",
            )
            .parent(host),
        );
        graph.relate(cgroup, RelationKind::OwnedBy, unit);
        graph.relate(cgroup, RelationKind::BackedBy, disk);

        let process = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid: 100,
                    start_ticks: 5,
                },
                "dockerd",
            )
            .parent(cgroup),
        );
        graph.relate(process, RelationKind::RunsIn, cgroup);

        // Второй сервис на том же диске: сосед, а не звено цепочки.
        let other_cgroup = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 20 }, "postgres.service").parent(host),
        );
        graph.relate(other_cgroup, RelationKind::BackedBy, disk);

        let _ = graph.end_tick();
        let snapshot = Snapshot::build(
            &graph,
            LatestValues::new(),
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        );
        (snapshot, unit, cgroup, process)
    }

    #[test]
    fn chain_goes_only_deeper() {
        let (snapshot, unit, cgroup, process) = snapshot();

        let from_unit: Vec<&str> = drill_steps(&snapshot, unit)
            .iter()
            .map(|step| step.kind.as_str())
            .collect();
        assert!(
            from_unit.iter().all(|kind| *kind == "cgroup"),
            "из сервиса спускаемся в его cgroup: {from_unit:?}"
        );

        let from_cgroup: Vec<&str> = drill_steps(&snapshot, cgroup)
            .iter()
            .map(|step| step.kind.as_str())
            .collect();
        assert!(
            from_cgroup.contains(&"process"),
            "из cgroup спускаемся в процессы: {from_cgroup:?}"
        );

        // Процесс — последнее звено графа: глубже только детали, а они не
        // являются сущностями.
        assert!(
            drill_steps(&snapshot, process).is_empty(),
            "процесс завершает цепочку"
        );
    }

    #[test]
    fn resources_are_leaves_not_transit_hubs() {
        let (snapshot, _, cgroup, _) = snapshot();
        let disk_id = snapshot
            .entities
            .iter()
            .find(|entity| entity.kind == EntityKind::Disk)
            .map(|entity| entity.id)
            .expect("диск");

        assert!(
            drill_steps(&snapshot, disk_id).is_empty(),
            "из диска нельзя провалиться в сервисы: это и создавало круги"
        );
        let context = context_resources(&snapshot, cgroup);
        let resources: Vec<&str> = context.iter().map(|step| step.name.as_str()).collect();
        assert_eq!(resources, vec!["sda"], "диск остаётся видимым контекстом");
    }

    #[test]
    fn no_cycle_is_reachable_by_drilling() {
        let (snapshot, unit, _, _) = snapshot();
        // Полный обход вниз из сервиса обязан завершиться.
        let mut visited: Vec<EntityKey> = Vec::new();
        let mut frontier = vec![unit];
        let mut guard = 0;
        while let Some(current) = frontier.pop() {
            guard += 1;
            assert!(guard < 1_000, "обход не завершился: цикл");
            let Some(entity) = snapshot.entity(current) else {
                continue;
            };
            assert!(
                !visited.contains(&entity.key),
                "объект встретился дважды: {}",
                entity.name
            );
            visited.push(entity.key.clone());
            for step in drill_steps(&snapshot, current) {
                if let Some(next) = snapshot
                    .entities
                    .iter()
                    .find(|candidate| candidate.key == step.key)
                {
                    frontier.push(next.id);
                }
            }
        }
        assert!(
            visited.len() >= 3,
            "цепочка обязана иметь глубину: {visited:?}"
        );
    }

    #[test]
    fn side_steps_offer_neighbours_but_are_not_the_chain() {
        let (snapshot, _, cgroup, _) = snapshot();
        let side: Vec<&str> = side_steps(&snapshot, cgroup)
            .iter()
            .map(|step| step.label)
            .collect();
        assert!(
            side.contains(&"shares"),
            "сосед по диску доступен как боковой переход: {side:?}"
        );
        let drill: Vec<&str> = drill_steps(&snapshot, cgroup)
            .iter()
            .map(|step| step.label)
            .collect();
        assert!(
            !drill.contains(&"shares"),
            "но не как звено цепочки: {drill:?}"
        );
    }

    #[test]
    fn nested_hierarchy_of_same_kind_is_a_descent_not_a_circle() {
        // /system.slice -> /system.slice/angie.service: оба cgroup. Это
        // настоящее дерево, и запрет по уровню сделал бы его непроходимым.
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let slice = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 1 }, "system.slice").parent(host),
        );
        let service = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 2 }, "angie.service").parent(slice),
        );
        // Боковая связь между равными: сервис общается с другим сервисом.
        let peer = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 3 }, "postgres.service").parent(host),
        );
        graph.relate(service, RelationKind::ConnectsTo, peer);
        let _ = graph.end_tick();
        let snapshot = Snapshot::build(
            &graph,
            LatestValues::new(),
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        );

        let down = drill_steps(&snapshot, slice);
        let names: Vec<&str> = down.iter().map(|step| step.name.as_str()).collect();
        assert!(
            names.contains(&"angie.service"),
            "спуск во вложенный cgroup обязан работать: {names:?}"
        );

        let side = drill_steps(&snapshot, service);
        let from_service: Vec<&str> = side.iter().map(|step| step.name.as_str()).collect();
        assert!(
            !from_service.contains(&"postgres.service"),
            "связь между равными не является спуском: {from_service:?}"
        );
    }

    #[test]
    fn folded_step_uses_counting_form_not_broken_plural() {
        // Живой кадр показывал «40 processs»: суффикс `s` приклеивался к виду
        // сущности из реестра метрик. Вид не обязан быть склоняемым словом.
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let cgroup =
            graph.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 3 }, "svc").parent(host));
        for pid in 0..3_i32 {
            let _ = graph.upsert(
                EntitySpec::new(
                    EntityKey::Process {
                        pid: 100 + pid,
                        start_ticks: 5,
                    },
                    format!("worker{pid}"),
                )
                .parent(cgroup),
            );
        }
        let _ = graph.end_tick();
        let snapshot = Snapshot::build(
            &graph,
            LatestValues::new(),
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        );

        let steps = drill_steps(&snapshot, cgroup);
        let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
        assert!(
            names.iter().all(|name| !name.contains("processs")),
            "ломаный плюрал: {names:?}"
        );
        assert!(
            names.contains(&"3 × process"),
            "свёрнутые объекты названы счётной формой: {names:?}"
        );
    }

    #[test]
    fn unit_child_of_its_cgroup_is_a_descent_labelled_by_kind() {
        // Так устроен реальный граф: коллектор делает `unit` ребёнком своей
        // cgroup (`EntitySpec::parent(cgroup)`), хотя семантически unit
        // владеет ею. Живой кадр показывал из `unit angie.service` шаг
        // «owner angie.service» - оператор видел круг вместо спуска.
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let cgroup = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 7 }, "angie.service").parent(host),
        );
        let unit = graph.upsert(
            EntitySpec::new(
                EntityKey::Unit {
                    name: "system.slice/angie.service".into(),
                },
                "angie.service",
            )
            .parent(cgroup),
        );
        graph.relate(cgroup, RelationKind::OwnedBy, unit);
        graph.relate(unit, RelationKind::BackedBy, cgroup);
        let _ = graph.end_tick();
        let snapshot = Snapshot::build(
            &graph,
            LatestValues::new(),
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        );

        let down = drill_steps(&snapshot, unit);
        let labels: Vec<&str> = down.iter().map(|step| step.label).collect();
        assert!(
            labels.contains(&"cgroup"),
            "шаг вниз подписан видом цели: {labels:?}"
        );
        assert!(
            !labels.contains(&"owner"),
            "спуск не может называться подъёмом: {labels:?}"
        );

        // Обратно вверх цепочка не идёт: иначе unit и cgroup ходили бы кругом.
        let up = drill_steps(&snapshot, cgroup);
        let up_kinds: Vec<&str> = up.iter().map(|step| step.kind.as_str()).collect();
        assert!(
            !up_kinds.contains(&"unit"),
            "из cgroup обратно в unit цепочка не ведёт: {up_kinds:?}"
        );

        // И «владельцем» сервиса не объявляется его собственная cgroup.
        let side = side_steps(&snapshot, unit);
        let side_names: Vec<(&str, &str)> = side
            .iter()
            .map(|step| (step.label, step.name.as_str()))
            .collect();
        assert!(
            !side_names.iter().any(|(label, _)| *label == "owner"),
            "cgroup ниже сервиса и не может быть его владельцем: {side_names:?}"
        );
    }

    #[test]
    fn levels_are_ordered_from_general_to_specific() {
        assert!(Level::of(EntityKind::Host) < Level::of(EntityKind::Unit));
        assert!(Level::of(EntityKind::Unit) < Level::of(EntityKind::Container));
        assert!(Level::of(EntityKind::Container) < Level::of(EntityKind::Cgroup));
        assert!(Level::of(EntityKind::Cgroup) < Level::of(EntityKind::Process));
        assert!(Level::of(EntityKind::Disk).is_resource());
        assert!(Level::of(EntityKind::NetIf).is_resource());
    }
}
