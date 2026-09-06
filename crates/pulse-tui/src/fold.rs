//! Логическая свёртка сущностей и отбор значимых (разделы 147-150).
//!
//! Низкоуровневая модель данных не меняется: граф по-прежнему содержит юнит,
//! cgroup и каждый процесс отдельно (раздел 6 задания прямо это требует).
//! Меняется представление: по умолчанию оператор видит логический объект.
//!
//! Причина: `init.scope unit` и `init.scope cgroup` - две технические части
//! одного объекта, а двенадцать строк `angie` - один сервис. Показывать их
//! равноправными соседями означает выдавать устройство реализации за состояние
//! системы.
//!
//! Отбор значимых (раздел 147) отвечает на вопрос «на что смотреть», а не
//! «что вообще есть»: полный инвентарь живёт на экране Entities.

use std::collections::HashMap;

use pulse_core::entity::{Entity, EntityId, EntityKind};
use pulse_core::snapshot::Snapshot;

use crate::rows::EntityRow;
use crate::state::StateClass;

/// Объяснимая причина присутствия в Overview (раздел 165).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelevanceReason {
    Problem,
    Critical,
    Warning,
    Changed,
    Cpu,
    Memory,
    Io,
    Network,
    System,
    Pinned,
    Selected,
}

impl RelevanceReason {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Problem => "problem",
            Self::Critical => "critical",
            Self::Warning => "warning",
            Self::Changed => "changed",
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Io => "io",
            Self::Network => "network",
            Self::System => "system",
            Self::Pinned => "pinned",
            Self::Selected => "selected",
        }
    }
}

/// Логическая строка: объект и свёрнутые в него технические части.
#[derive(Clone, Debug)]
pub struct LogicalRow {
    /// Представитель объекта: сущность, которую открывает `Enter`.
    pub row: EntityRow,
    /// Сколько процессов свёрнуто в объект.
    pub processes: usize,
    /// Сколько из них в ненормальном состоянии (раздел 150).
    pub abnormal: usize,
    /// Сколько технических частей свёрнуто помимо процессов: cgroup, unit.
    pub technical: usize,
    /// Идентификаторы всех свёрнутых сущностей: нужны для раскрытия.
    pub members: Vec<EntityId>,
    /// Непрозрачный технический идентификатор, если имя было им заменено.
    ///
    /// Logical view показывает узнаваемое имя, но идентификатор не выбрасывается:
    /// он нужен для `docker inspect` и остаётся видимым в technical view и в
    /// панели выбранного объекта.
    pub technical_id: Option<String>,
    /// Почему объект попал в Overview. Пусто только у fallback KEY ENTITIES.
    pub reasons: Vec<RelevanceReason>,
}

impl LogicalRow {
    /// Подпись вида для колонки `KIND` (раздел 148).
    ///
    /// Компактная форма `unit+12p` честнее, чем просто `unit`: она сразу
    /// говорит, что за строкой стоит дюжина процессов.
    #[must_use]
    pub fn kind_label(&self) -> String {
        if self.processes > 0 && self.row.kind != EntityKind::Process {
            format!("{}+{}p", self.row.kind.as_str(), self.processes)
        } else {
            self.row.kind.as_str().to_string()
        }
    }

    /// Есть ли что раскрывать по `Enter`.
    #[must_use]
    pub fn is_folded(&self) -> bool {
        self.members.len() > 1
    }
}

/// Ключ логического объекта.
///
/// Юнит, его cgroup и его процессы имеют одно имя логического объекта, поэтому
/// группировка идёт по имени владельца, а не по виду сущности.
fn logical_key(snapshot: &Snapshot, entity: &Entity) -> String {
    // Процесс принадлежит логическому объекту своего владельца.
    if entity.kind == EntityKind::Process {
        if let Some(owner) = owning_name(snapshot, entity) {
            return owner;
        }
        // Процесс без владельца - самостоятельный объект.
        return format!("process/{}", entity.name);
    }

    // Технический cgroup служебного контейнера носит длинный идентификатор:
    // логическое имя берётся у владельца, если он есть.
    if entity.kind == EntityKind::Cgroup {
        if let Some(owner) = owning_name(snapshot, entity) {
            return owner;
        }
    }
    entity.name.clone()
}

/// Имя логического владельца сущности: unit, контейнер или pod.
fn owning_name(snapshot: &Snapshot, entity: &Entity) -> Option<String> {
    // Прямые связи владения важнее иерархии: cgroup явно указывает владельца.
    for relation in snapshot.relations_of(entity.id, Some(pulse_core::RelationKind::OwnedBy)) {
        if relation.from == entity.id {
            if let Some(owner) = snapshot.entity(relation.to) {
                if is_logical_owner(owner.kind) {
                    return Some(owner.name.clone());
                }
            }
        }
    }
    // Затем вверх по родителям до первого логического владельца.
    for id in snapshot.ancestry(entity.id) {
        if id == entity.id {
            continue;
        }
        let Some(candidate) = snapshot.entity(id) else {
            continue;
        };
        if is_logical_owner(candidate.kind) {
            return Some(candidate.name.clone());
        }
        // У cgroup спрашиваем его собственного владельца.
        if candidate.kind == EntityKind::Cgroup {
            for relation in
                snapshot.relations_of(candidate.id, Some(pulse_core::RelationKind::OwnedBy))
            {
                if relation.from == candidate.id {
                    if let Some(owner) = snapshot.entity(relation.to) {
                        if is_logical_owner(owner.kind) {
                            return Some(owner.name.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Виды, которые оператор считает объектами, а не деталями реализации.
const fn is_logical_owner(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Unit | EntityKind::Container | EntityKind::Pod
    )
}

/// Приоритет представителя объекта: чем меньше, тем лучше как «лицо» объекта.
const fn representative_rank(kind: EntityKind) -> u8 {
    match kind {
        EntityKind::Pod => 0,
        EntityKind::Container => 1,
        EntityKind::Unit => 2,
        EntityKind::Disk | EntityKind::NetIf => 3,
        EntityKind::Process => 4,
        EntityKind::Cgroup => 5,
        EntityKind::Host => 6,
    }
}

/// Сворачивает технические строки в логические (разделы 148-150).
///
/// Порядок входных строк сохраняется по первому появлению объекта: сортировку
/// выполняет вызывающий, и свёртка не имеет права её незаметно менять.
#[must_use]
pub fn fold(snapshot: &Snapshot, rows: &[EntityRow]) -> Vec<LogicalRow> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, LogicalRow> = HashMap::new();

    for row in rows {
        let Some(entity) = snapshot.entity(row.id) else {
            continue;
        };
        let key = logical_key(snapshot, entity);

        let is_process = row.kind == EntityKind::Process;
        let abnormal = usize::from(row.state.is_abnormal());

        match groups.get_mut(&key) {
            Some(group) => {
                group.members.push(row.id);
                if is_process {
                    group.processes += 1;
                    group.abnormal += abnormal;
                } else {
                    group.technical += 1;
                }

                // Числа объекта - сумма частей: память сервиса это память его
                // процессов, а не память самой крупной из них.
                if is_process {
                    group.row.cpu += row.cpu;
                    group.row.memory += row.memory;
                }
                // Потоки берутся с той части, где они измерены: у процессов их
                // нет, у cgroup есть.
                if group.row.io.is_none() {
                    group.row.io = row.io;
                }
                if group.row.net.is_none() {
                    group.row.net = row.net;
                }
                // Состояние объекта - худшее среди частей: одна проблемная
                // часть делает проблемным весь объект.
                group.row.state = group.row.state.max(row.state);
                group.row.severity = group.row.severity.max(row.severity);

                // Представителем становится наиболее «логическая» часть.
                if representative_rank(row.kind) < representative_rank(group.row.kind) {
                    let carried = (
                        group.row.cpu,
                        group.row.memory,
                        group.row.state,
                        group.row.severity,
                        group.row.io,
                        group.row.net,
                    );
                    group.row = row.clone();
                    group.row.cpu = carried.0;
                    group.row.memory = carried.1;
                    group.row.state = carried.2;
                    group.row.severity = carried.3;
                    group.row.io = carried.4;
                    group.row.net = carried.5;
                }
            }
            None => {
                order.push(key.clone());
                groups.insert(
                    key,
                    LogicalRow {
                        row: row.clone(),
                        processes: usize::from(is_process),
                        abnormal: if is_process { abnormal } else { 0 },
                        technical: usize::from(!is_process),
                        members: vec![row.id],
                        technical_id: None,
                        reasons: Vec::new(),
                    },
                );
            }
        }
    }

    let by_id: HashMap<EntityId, &EntityRow> = rows.iter().map(|row| (row.id, row)).collect();
    let mut result: Vec<LogicalRow> = order
        .into_iter()
        .filter_map(|key| groups.remove(&key))
        .collect();
    for group in &mut result {
        let mut process_cpu: f64 = 0.0;
        let mut process_memory: f64 = 0.0;
        let mut aggregate_cpu: f64 = 0.0;
        let mut aggregate_memory: f64 = 0.0;
        for member in &group.members {
            let Some(row) = by_id.get(member) else {
                continue;
            };
            if row.kind == EntityKind::Process {
                process_cpu += row.cpu;
                process_memory += row.memory;
            } else {
                aggregate_cpu = aggregate_cpu.max(row.cpu);
                aggregate_memory = aggregate_memory.max(row.memory);
            }
        }
        // cgroup/unit gauges уже агрегируют процессы. Складывать их ещё раз
        // означало бы двойной учёт; process sum — fallback без aggregate gauge.
        group.row.cpu = if aggregate_cpu > 0.0 {
            aggregate_cpu
        } else {
            process_cpu
        };
        group.row.memory = if aggregate_memory > 0.0 {
            aggregate_memory
        } else {
            process_memory
        };

        // §Logical view: экран сущностей не должен состоять из хешей. Приоритет
        // источников имени задан спецификацией; здесь доступен последний
        // работающий уровень — имя главного процесса внутри объекта. Docker/CRI
        // API не реализованы, поэтому выдумывать имя контейнера неоткуда.
        if is_opaque_id(&group.row.name) {
            if let Some(name) = dominant_process_name(&group.members, &by_id) {
                group.technical_id = Some(group.row.name.clone());
                group.row.name = name;
            }
        }
    }
    result
}

/// Имя выглядит как непрозрачный технический идентификатор.
///
/// Признак — длинная шестнадцатеричная строка: так выглядят идентификаторы
/// контейнеров Docker/containerd. Имя сервиса или процесса под это правило не
/// попадает, потому что содержит буквы вне `[0-9a-f]`, точку или дефис.
#[must_use]
pub fn is_opaque_id(name: &str) -> bool {
    let core = name
        .trim_start_matches("docker-")
        .trim_start_matches("crio-")
        .trim_start_matches("cri-containerd-")
        .trim_start_matches("libpod-")
        .trim_end_matches(".scope");
    core.len() >= 8 && core.chars().all(|c| c.is_ascii_hexdigit())
}

/// Имя самого крупного по памяти процесса объекта.
fn dominant_process_name(
    members: &[EntityId],
    by_id: &HashMap<EntityId, &EntityRow>,
) -> Option<String> {
    members
        .iter()
        .filter_map(|id| by_id.get(id))
        .filter(|row| row.kind == EntityKind::Process && !row.name.is_empty())
        .max_by(|a, b| {
            a.memory
                .partial_cmp(&b.memory)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|row| row.name.clone())
}

/// Взвешенный вклад одной причины в значимость строки.
///
/// Почему веса, а не порядок булевых флагов: прежняя реализация ставила
/// «недавнее изменение» третьим ключом сортировки и брала его из сырого
/// журнала. На реальном хосте события есть у каждой сущности, поэтому
/// `WHY changed` оказывался у всех, а kernel worker с сотней переименований
/// обгонял `docker.service` с 15 ядрами. Вес изменения ограничен так, чтобы
/// он не мог перевесить реальное потребление ресурсов.
#[must_use]
pub fn scored_reasons(snapshot: &Snapshot, row: &LogicalRow) -> Vec<(RelevanceReason, u64)> {
    // Поток ядра сам по себе не является операционным объектом: на реальном
    // хосте их десятки, каждый занимает сотые доли ядра и все они попадали в
    // Relevant Entities только из-за порога CPU. Он остаётся видимым в полном
    // инвентаре и в technical view, а также возвращается сюда, если у него
    // действительно проблема или ненормальное состояние.
    let background_kernel = pulse_core::semantic::is_kernel_thread(&row.row.name)
        && !row.row.state.is_abnormal()
        && !row
            .members
            .iter()
            .any(|id| snapshot.problems_of(*id).next().is_some());
    if background_kernel {
        return Vec::new();
    }

    let mut out: Vec<(RelevanceReason, u64)> = Vec::new();

    if row
        .members
        .iter()
        .any(|id| snapshot.problems_of(*id).next().is_some())
    {
        out.push((RelevanceReason::Problem, 100_000));
    }
    match row.row.state {
        StateClass::Failed | StateClass::Critical => {
            out.push((RelevanceReason::Critical, 80_000));
        }
        StateClass::Warning | StateClass::Degraded => {
            out.push((RelevanceReason::Warning, 40_000));
        }
        _ => {}
    }

    // Значимое изменение, а не любое наблюдение: источник — семантический
    // конвейер снимка, где Tier0-churn уже подавлен.
    if snapshot
        .meaningful
        .iter()
        .any(|event| event.entity.is_some_and(|id| row.members.contains(&id)))
    {
        out.push((RelevanceReason::Changed, 6_000));
    }

    // Ресурсы дают непрерывный вклад: 1 ядро = 1000, 16 MiB = 1.
    // Так `docker.service` с 15 ядрами и 14.7 GiB заведомо обгоняет объект,
    // который лишь недавно менялся.
    if row.row.cpu >= 0.01 {
        let weight = (row.row.cpu * 1_000.0).min(60_000.0) as u64;
        out.push((RelevanceReason::Cpu, weight.max(1)));
    }
    if row.row.memory >= 32.0 * 1024.0 * 1024.0 {
        let weight = (row.row.memory / (16.0 * 1024.0 * 1024.0)).min(20_000.0) as u64;
        out.push((RelevanceReason::Memory, weight.max(1)));
    }
    if let Some(io) = row.row.io.filter(|value| *value > 0.0) {
        let weight = (io / (1024.0 * 1024.0)).min(10_000.0) as u64;
        out.push((RelevanceReason::Io, weight.max(300)));
    }
    if let Some(net) = row.row.net.filter(|value| *value > 0.0) {
        let weight = (net / (1024.0 * 1024.0)).min(10_000.0) as u64;
        out.push((RelevanceReason::Network, weight.max(200)));
    }

    // Операционная роль: сервис, контейнер, pod и корневые слайсы объясняют
    // «кто это», но сами по себе не выводят объект вперёд.
    if matches!(
        row.row.kind,
        EntityKind::Unit | EntityKind::Container | EntityKind::Pod
    ) || matches!(
        row.row.name.as_str(),
        "system.slice" | "init.scope" | "user.slice" | "pulse"
    ) {
        out.push((RelevanceReason::System, 1_500));
    }

    // Порядок по вкладу: первая причина — та, которая действительно вывела
    // объект в список, и именно она показывается в колонке `WHY`.
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Объяснимые причины присутствия в Overview, сильнейшая первой (§165).
#[must_use]
pub fn reasons(snapshot: &Snapshot, row: &LogicalRow) -> Vec<RelevanceReason> {
    scored_reasons(snapshot, row)
        .into_iter()
        .map(|(reason, _)| reason)
        .collect()
}

/// Числовой ключ ранжирования: состояние, затем сумма взвешенных причин.
///
/// Состояние остаётся первым ключом: ненормальный объект обязан быть выше
/// любого нагруженного здорового. Внутри одного состояния решает сумма весов.
#[must_use]
pub fn relevance(snapshot: &Snapshot, row: &LogicalRow) -> (u8, u64) {
    let state_rank = match row.row.state {
        StateClass::Failed => 5,
        StateClass::Critical => 4,
        StateClass::Warning => 3,
        StateClass::Degraded => 2,
        StateClass::Saturated => 1,
        _ => 0,
    };
    let score = scored_reasons(snapshot, row)
        .into_iter()
        .fold(0_u64, |acc, (_, weight)| acc.saturating_add(weight));
    (state_rank, score)
}

/// Значимые логические строки для главного экрана (раздел 165).
#[must_use]
pub fn relevant(snapshot: &Snapshot, rows: &[EntityRow], limit: usize) -> Vec<LogicalRow> {
    let mut folded = fold(snapshot, rows);
    for row in &mut folded {
        row.reasons = reasons(snapshot, row);
    }
    folded.sort_by(|a, b| {
        relevance(snapshot, b)
            .cmp(&relevance(snapshot, a))
            .then_with(|| a.row.name.cmp(&b.row.name))
    });
    // Реально объяснимые сущности идут первыми. Если их нет, оставляем
    // ограниченный fallback; renderer назовёт его KEY ENTITIES, не RELEVANT.
    let explainable = folded.iter().filter(|row| !row.reasons.is_empty()).count();
    if explainable == 0 {
        // KEY ENTITIES fallback: renderer честно меняет заголовок.
        folded.truncate(limit.min(5));
    } else {
        // RELEVANT означает: у каждой строки есть объяснимая причина (§165).
        folded.retain(|row| !row.reasons.is_empty());
        folded.truncate(limit);
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::rows::entity_rows;
    use pulse_core::entity::{EntityKey, EntitySpec};
    use pulse_core::graph::EntityGraph;
    use pulse_core::metric::ids;
    use pulse_core::sample::SeriesKey;
    use pulse_core::snapshot::{AgentStats, LatestValues, Snapshot};
    use pulse_core::time::Timestamp;
    use pulse_core::RelationKind;

    /// Хост с сервисом angie: unit + cgroup + двенадцать процессов.
    fn snapshot_with_service() -> Snapshot {
        let mut graph = EntityGraph::new("boot", "asuspc", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();

        let cgroup = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 10 }, "angie.service").parent(host),
        );
        let unit = graph.upsert(
            EntitySpec::new(
                EntityKey::Unit {
                    name: "angie.service".into(),
                },
                "angie.service",
            )
            .parent(cgroup),
        );
        graph.relate(cgroup, RelationKind::OwnedBy, unit);

        let mut latest = LatestValues::new();
        latest.set(
            SeriesKey::new(cgroup, ids::CG_MEM_CURRENT),
            376.0 * 1024.0 * 1024.0,
        );

        for pid in 0..12_i32 {
            let process = graph.upsert(
                EntitySpec::new(
                    EntityKey::Process {
                        pid,
                        start_ticks: 7,
                    },
                    "angie",
                )
                .parent(cgroup),
            );
            latest.set(
                SeriesKey::new(process, ids::PROC_RSS),
                30.0 * 1024.0 * 1024.0,
            );
            latest.set(SeriesKey::new(process, ids::PROC_CPU_CORES), 0.01);
        }

        // Отдельный самостоятельный процесс: сворачиваться ему не с кем.
        let lone = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid: 999,
                    start_ticks: 3,
                },
                "pulse",
            )
            .parent(host),
        );
        latest.set(SeriesKey::new(lone, ids::PROC_RSS), 6.0 * 1024.0 * 1024.0);

        let batch = graph.end_tick();
        Snapshot::build(
            &graph,
            latest,
            vec![],
            batch.events,
            AgentStats::default(),
            "asuspc",
            "boot",
        )
    }

    /// Раздел 150: двенадцать процессов сворачиваются в один сервис.
    #[test]
    fn repeated_processes_fold_into_one_service() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let folded = fold(&snapshot, &rows);

        let angie = folded
            .iter()
            .find(|row| row.row.name == "angie.service")
            .expect("сервис в списке");
        assert_eq!(angie.processes, 12, "все процессы свёрнуты");
        assert_eq!(angie.kind_label(), "unit+12p");
        assert!(angie.is_folded());

        // Строк с именем `angie` в логическом виде быть не должно.
        let bare = folded.iter().filter(|row| row.row.name == "angie").count();
        assert_eq!(bare, 0, "отдельные процессы сервиса не показываются");
    }

    /// Раздел 148: unit и cgroup одного объекта - одна строка.
    #[test]
    fn unit_and_cgroup_twins_fold_together() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let folded = fold(&snapshot, &rows);

        let same_name = folded
            .iter()
            .filter(|row| row.row.name == "angie.service")
            .count();
        assert_eq!(same_name, 1, "две технические части дают одну строку");

        let angie = folded
            .iter()
            .find(|row| row.row.name == "angie.service")
            .expect("сервис есть");
        // Представителем стал unit, а не cgroup: он логичнее для оператора.
        assert_eq!(angie.row.kind, EntityKind::Unit);
    }

    /// Числа объекта - сумма его процессов, а не одной части.
    #[test]
    fn folded_row_sums_member_resources() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let folded = fold(&snapshot, &rows);
        let angie = folded
            .iter()
            .find(|row| row.row.name == "angie.service")
            .expect("сервис есть");
        // cgroup gauge уже агрегирует процессы: используем 376 MiB без double-count.
        let expected = 376.0 * 1024.0 * 1024.0;
        assert!(
            (angie.row.memory - expected).abs() < 1.0,
            "память сервиса складывается: {} вместо {expected}",
            angie.row.memory
        );
        assert!(
            (angie.row.cpu - 0.12).abs() < 1e-9,
            "CPU сервиса складывается: {}",
            angie.row.cpu
        );
    }

    /// Самостоятельный процесс остаётся отдельной строкой.
    #[test]
    fn standalone_process_is_its_own_object() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let folded = fold(&snapshot, &rows);
        let lone = folded.iter().find(|row| row.row.name == "pulse");
        assert!(lone.is_some(), "процесс без владельца виден: {folded:#?}");
    }

    /// Раздел 147: список значимых ограничен и отсортирован по значимости.
    #[test]
    fn relevant_list_is_bounded_and_ordered() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let relevant = relevant(&snapshot, &rows, 5);
        assert!(relevant.len() <= 5, "список ограничен");
        // Сервис с 376 MiB важнее одинокого процесса на 6 MiB.
        let names: Vec<&str> = relevant.iter().map(|r| r.row.name.as_str()).collect();
        let angie = names.iter().position(|n| *n == "angie.service");
        let pulse = names.iter().position(|n| *n == "pulse");
        if let (Some(angie), Some(pulse)) = (angie, pulse) {
            assert!(angie < pulse, "нагруженный объект выше: {names:?}");
        }
    }

    /// Свёртка детерминирована: одинаковый вход даёт одинаковый порядок.
    #[test]
    fn folding_is_deterministic() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let first: Vec<String> = fold(&snapshot, &rows)
            .iter()
            .map(|r| r.row.name.clone())
            .collect();
        let second: Vec<String> = fold(&snapshot, &rows)
            .iter()
            .map(|r| r.row.name.clone())
            .collect();
        assert_eq!(first, second);
    }

    /// Технический режим сохраняет полный инвентарь (раздел 149).
    #[test]
    fn technical_rows_keep_every_entity() {
        let snapshot = snapshot_with_service();
        let app = App::default();
        let rows = entity_rows(&snapshot, &app);
        let processes = rows
            .iter()
            .filter(|row| row.kind == EntityKind::Process)
            .count();
        assert_eq!(processes, 13, "низкоуровневая модель не урезана");
    }
}
