//! Temporal entity graph — центральная модель данных Pulse.
//!
//! Коллекторы не владеют графом: они описывают наблюдения, а граф решает,
//! появилась сущность, изменилась или исчезла. Отсюда естественно возникают
//! события жизненного цикла, A/B-diff и навигация в TUI.
//!
//! Инварианты:
//! * идентичность определяется только [`EntityKey`], а не порядком обхода;
//! * имя сущности всегда санитизировано (см. [`crate::redact::sanitize_display`]);
//! * `EntityId` не может «переехать» на другую сущность: слот арены получает
//!   новое поколение при переиспользовании;
//! * образцы принимаются только для живых сущностей текущего такта.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::entity::{Entity, EntityId, EntityKind, EntityRecord, EntitySpec, Labels};
use crate::event::{Event, EventKind};
use crate::metric::MetricId;
use crate::problem::Severity;
use crate::redact::sanitize_display;
use crate::relation::{Relation, RelationKind};
use crate::sample::Sample;
use crate::time::{TickId, Timestamp};

/// Окно, внутри которого исчезновение и появление одной логической сущности
/// считается перезапуском, а не независимыми событиями.
pub const DEFAULT_RESTART_WINDOW_MS: u64 = 30_000;

/// Сколько тактов сущность может «пропасть» до объявления её удалённой.
/// Один такт форы гасит гонки чтения `/proc` на занятой машине.
pub const DEFAULT_GRACE_TICKS: u64 = 1;

struct Slot {
    generation: u32,
    entity: Option<Entity>,
    last_tick: u64,
}

#[derive(Clone, Debug)]
struct DeadLogical {
    at: Timestamp,
    kind: EntityKind,
}

/// Результат такта: всё, что нужно передать в хранилище.
#[derive(Debug, Default)]
pub struct TickBatch {
    pub tick: TickId,
    pub at: Timestamp,
    pub samples: Vec<Sample>,
    pub events: Vec<Event>,
    /// Созданные, изменённые и удалённые сущности этого такта.
    pub records: Vec<EntityRecord>,
    /// Все живые сущности после такта — хранилище продлевает им `last_seen`.
    pub alive: Vec<EntityId>,
}

/// Статистика графа для самонаблюдения.
#[derive(Copy, Clone, Debug, Default)]
pub struct GraphStats {
    pub live_entities: usize,
    pub relations: usize,
    pub created_total: u64,
    pub deleted_total: u64,
    pub restarted_total: u64,
}

/// Ошибка коллектора. Никогда не роняет такт: логируется и превращается в событие.
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    #[error("{source_name}: {message}")]
    Source {
        source_name: &'static str,
        message: String,
    },
    #[error("подсистема недоступна: {0}")]
    Unavailable(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl CollectError {
    #[must_use]
    pub fn source(source_name: &'static str, message: impl Into<String>) -> Self {
        CollectError::Source {
            source_name,
            message: message.into(),
        }
    }
}

/// Коллектор наблюдений. Реализации живут в `pulse-collect`.
pub trait Collector: Send {
    /// Стабильное имя для журналов и самометрик.
    fn name(&self) -> &'static str;

    /// Один проход сбора. Ошибка не прерывает такт.
    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError>;
}

/// Граф сущностей.
pub struct EntityGraph {
    slots: Vec<Slot>,
    free: Vec<u32>,
    by_key: HashMap<crate::entity::EntityKey, EntityId>,
    relations: Vec<Relation>,
    rel_index: HashSet<(EntityId, RelationKind, EntityId)>,
    adjacency: HashMap<EntityId, Vec<EntityId>>,
    recent_dead: HashMap<String, DeadLogical>,
    host: EntityId,
    tick: TickId,
    now: Timestamp,
    samples: Vec<Sample>,
    events: Vec<Event>,
    changed: Vec<EntityId>,
    restart_window_ms: u64,
    grace_ticks: u64,
    stats: GraphStats,
    /// Идёт ли базовая инвентаризация (разделы 125, 132).
    ///
    /// Пока флаг поднят, появление сущности не является событием: объекты
    /// существовали до подключения наблюдателя, и момент запуска PULSE не
    /// является моментом их создания.
    in_baseline: bool,
}

impl std::fmt::Debug for EntityGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntityGraph")
            .field("live_entities", &self.stats.live_entities)
            .field("relations", &self.relations.len())
            .field("tick", &self.tick)
            .finish()
    }
}

impl EntityGraph {
    /// Создаёт граф с корневой сущностью-хостом.
    #[must_use]
    pub fn new(boot_id: &str, hostname: &str, now: Timestamp) -> Self {
        let mut graph = EntityGraph {
            slots: Vec::with_capacity(1024),
            free: Vec::new(),
            by_key: HashMap::with_capacity(1024),
            relations: Vec::new(),
            rel_index: HashSet::new(),
            adjacency: HashMap::new(),
            recent_dead: HashMap::new(),
            host: EntityId::NONE,
            tick: TickId::FIRST,
            now,
            samples: Vec::with_capacity(4096),
            events: Vec::new(),
            changed: Vec::new(),
            restart_window_ms: DEFAULT_RESTART_WINDOW_MS,
            grace_ticks: DEFAULT_GRACE_TICKS,
            stats: GraphStats::default(),
            in_baseline: true,
        };
        let host = graph.upsert(EntitySpec::new(
            crate::entity::EntityKey::Host {
                boot_id: boot_id.into(),
            },
            hostname,
        ));
        graph.host = host;
        graph
    }

    #[must_use]
    pub const fn host(&self) -> EntityId {
        self.host
    }

    #[must_use]
    pub const fn tick(&self) -> TickId {
        self.tick
    }

    #[must_use]
    pub const fn now(&self) -> Timestamp {
        self.now
    }

    #[must_use]
    pub const fn stats(&self) -> GraphStats {
        self.stats
    }

    pub fn set_restart_window_ms(&mut self, ms: u64) {
        self.restart_window_ms = ms;
    }

    /// Начинает новый такт. Буферы образцов и событий очищаются.
    pub fn begin_tick(&mut self, now: Timestamp) {
        self.tick = self.tick.next();
        self.now = now;
        self.samples.clear();
        self.events.clear();
        self.changed.clear();
        // Хост наблюдается всегда: он не может «исчезнуть» между тактами.
        self.touch(self.host);
    }

    /// Создаёт или обновляет сущность.
    pub fn upsert(&mut self, spec: EntitySpec) -> EntityId {
        let kind = spec.key.kind();
        let name = sanitize_display(&spec.name);

        if let Some(&id) = self.by_key.get(&spec.key) {
            if let Some(slot) = self.slots.get_mut(id.index() as usize) {
                slot.last_tick = self.tick.0;
                if let Some(entity) = slot.entity.as_mut() {
                    entity.last_seen = self.now;
                    entity.alive = true;

                    let renamed = entity.name != name;
                    let old_parent = entity.parent;
                    let reparented = spec.parent.is_some() && spec.parent != old_parent;

                    if renamed {
                        let previous = std::mem::replace(&mut entity.name, name.clone());
                        self.events.push(
                            Event::new(self.now, EventKind::MetadataChanged, name.clone())
                                .entity(id, kind)
                                .detail(format!("name: {previous} -> {name}")),
                        );
                    }
                    if reparented {
                        entity.parent = spec.parent;
                        self.events.push(
                            Event::new(self.now, EventKind::Reparented, name.clone())
                                .entity(id, kind)
                                .detail(match old_parent {
                                    Some(p) => format!(
                                        "owner: {p} -> {}",
                                        spec.parent
                                            .map(|x| x.to_string())
                                            .unwrap_or_else(|| "none".into())
                                    ),
                                    None => "owner assigned".to_string(),
                                }),
                        );
                    }
                    if !spec.labels.is_empty() {
                        entity.labels = spec.labels;
                    }
                    if spec.logical.is_some() {
                        entity.logical = spec.logical;
                    }
                    if renamed || reparented {
                        self.changed.push(id);
                    }
                }
            }
            return id;
        }

        // Новая сущность.
        let id = self.allocate(Entity {
            id: EntityId::NONE,
            key: spec.key.clone(),
            kind,
            name: name.clone(),
            parent: spec.parent,
            labels: spec.labels,
            logical: spec.logical.clone(),
            first_seen: self.now,
            last_seen: self.now,
            alive: true,
        });
        self.by_key.insert(spec.key, id);
        self.changed.push(id);
        self.stats.created_total = self.stats.created_total.saturating_add(1);

        // Детекция перезапуска: та же логическая сущность вернулась в окне.
        let restarted = spec.logical.as_ref().and_then(|logical| {
            self.recent_dead.get(logical).and_then(|dead| {
                let within = self.now.saturating_sub(dead.at).as_millis()
                    <= u128::from(self.restart_window_ms);
                (within && dead.kind == kind).then_some(())
            })
        });

        if restarted.is_some() {
            if let Some(logical) = spec.logical.as_ref() {
                self.recent_dead.remove(logical);
            }
            self.stats.restarted_total = self.stats.restarted_total.saturating_add(1);
            self.events.push(
                Event::new(self.now, EventKind::Restarted, name)
                    .entity(id, kind)
                    .severity(Severity::Warn)
                    .detail("logical entity restarted"),
            );
        } else if !self.in_baseline {
            // На базовой инвентаризации событие не создаётся: см. `in_baseline`.
            self.events
                .push(Event::new(self.now, EventKind::Created, name).entity(id, kind));
        }

        if let Some(parent) = spec.parent {
            self.relate(parent, RelationKind::ParentOf, id);
        }
        id
    }

    /// Отмечает сущность наблюдённой в этом такте без изменения метаданных.
    pub fn touch(&mut self, id: EntityId) {
        if let Some(slot) = self.slots.get_mut(id.index() as usize) {
            if slot.generation == id.generation() {
                slot.last_tick = self.tick.0;
                if let Some(entity) = slot.entity.as_mut() {
                    entity.last_seen = self.now;
                }
            }
        }
    }

    /// Записывает образец. Образцы для неизвестных или мёртвых сущностей отбрасываются.
    pub fn sample(&mut self, entity: EntityId, metric: MetricId, value: f64) {
        if !value.is_finite() {
            return;
        }
        if self.get(entity).is_some() {
            self.samples.push(Sample::new(entity, metric, value));
        }
    }

    /// Добавляет связь. Повторные вызовы в пределах жизни связи ничего не делают.
    pub fn relate(&mut self, from: EntityId, kind: RelationKind, to: EntityId) {
        if from == to || self.get(from).is_none() || self.get(to).is_none() {
            return;
        }
        if self.rel_index.insert((from, kind, to)) {
            self.relations.push(Relation::new(from, kind, to, self.now));
            self.adjacency.entry(from).or_default().push(to);
            self.adjacency.entry(to).or_default().push(from);
            self.stats.relations = self.relations.len();
        }
    }

    pub fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Событие ошибки коллектора: пустой график не должен выглядеть как «всё хорошо».
    pub fn push_collector_error(&mut self, collector: &'static str, message: &str) {
        let host = self.host;
        self.events.push(
            Event::new(self.now, EventKind::CollectorError, collector)
                .entity(host, EntityKind::Host)
                .severity(Severity::Warn)
                .detail(sanitize_display(message)),
        );
    }

    /// Завершает такт: помечает исчезнувшие сущности и формирует пакет для хранилища.
    pub fn end_tick(&mut self) -> TickBatch {
        let current = self.tick.0;
        let mut dead: Vec<EntityId> = Vec::new();
        let mut alive: Vec<EntityId> = Vec::new();

        for (index, slot) in self.slots.iter().enumerate() {
            let Some(entity) = slot.entity.as_ref() else {
                continue;
            };
            let id = EntityId::new(u32::try_from(index).unwrap_or(u32::MAX), slot.generation);
            if current.saturating_sub(slot.last_tick) > self.grace_ticks {
                dead.push(id);
            } else {
                alive.push(entity.id);
            }
        }

        let mut records: Vec<EntityRecord> = Vec::with_capacity(self.changed.len() + dead.len());
        for id in std::mem::take(&mut self.changed) {
            if let Some(entity) = self.get(id) {
                records.push(EntityRecord::from_entity(entity));
            }
        }

        for id in dead {
            if let Some(record) = self.retire(id) {
                records.push(record);
            }
        }

        self.prune_recent_dead();
        self.stats.live_entities = alive.len();

        if self.in_baseline {
            self.in_baseline = false;
            let event = self.baseline_event(alive.len());
            // Событие ставится первым в такте: наблюдение началось до всего
            // остального, что произошло в этом же такте.
            self.events.insert(0, event);
        }

        TickBatch {
            tick: self.tick,
            at: self.now,
            samples: std::mem::take(&mut self.samples),
            events: std::mem::take(&mut self.events),
            records,
            alive,
        }
    }

    /// Одно событие вместо сотен `Created` (раздел 132).
    ///
    /// В `detail` кладётся разбор по видам: экран Timeline раскрывает его по
    /// `Enter` как `BASELINE SNAPSHOT`, не запрашивая ничего дополнительно.
    fn baseline_event(&self, total: usize) -> Event {
        let count = |kind: EntityKind| self.entities_of_kind(kind).count();
        let processes = count(EntityKind::Process);
        let cgroups = count(EntityKind::Cgroup);
        let units = count(EntityKind::Unit);
        let containers = count(EntityKind::Container);
        let pods = count(EntityKind::Pod);
        let disks = count(EntityKind::Disk);
        let netifs = count(EntityKind::NetIf);
        let named = processes + cgroups + units + containers + pods + disks + netifs;
        let other = total.saturating_sub(named);
        Event::new(
            self.now,
            EventKind::ObservationStarted,
            "PULSE observation started",
        )
        .entity(self.host, EntityKind::Host)
        .detail(format!(
            "baseline: {total} entities; processes {processes}, cgroups {cgroups}, \
             units {units}, containers {containers}, pods {pods}, disks {disks}, \
             netifs {netifs}, other {other}"
        ))
    }

    /// Живая сущность по идентификатору.
    #[must_use]
    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        let slot = self.slots.get(id.index() as usize)?;
        if slot.generation != id.generation() {
            return None;
        }
        slot.entity.as_ref()
    }

    #[must_use]
    pub fn get_by_key(&self, key: &crate::entity::EntityKey) -> Option<&Entity> {
        self.by_key.get(key).and_then(|id| self.get(*id))
    }

    /// Все живые сущности.
    pub fn entities(&self) -> impl Iterator<Item = &Entity> + '_ {
        self.slots.iter().filter_map(|slot| slot.entity.as_ref())
    }

    /// Живые сущности указанного вида.
    pub fn entities_of_kind(&self, kind: EntityKind) -> impl Iterator<Item = &Entity> + '_ {
        self.entities().filter(move |e| e.kind == kind)
    }

    #[must_use]
    pub fn live_count(&self) -> usize {
        self.entities().count()
    }

    /// Активные связи.
    pub fn relations(&self) -> impl Iterator<Item = &Relation> + '_ {
        self.relations.iter().filter(|r| r.until.is_none())
    }

    /// Связи, инцидентные сущности.
    pub fn relations_of(&self, id: EntityId) -> impl Iterator<Item = &Relation> + '_ {
        self.relations
            .iter()
            .filter(move |r| r.until.is_none() && (r.from == id || r.to == id))
    }

    /// Прямые потомки в иерархии `ParentOf`.
    pub fn children(&self, id: EntityId) -> impl Iterator<Item = EntityId> + '_ {
        self.relations
            .iter()
            .filter(move |r| r.until.is_none() && r.kind == RelationKind::ParentOf && r.from == id)
            .map(|r| r.to)
    }

    /// Цепочка родителей от сущности к хосту — breadcrumbs в TUI.
    #[must_use]
    pub fn ancestry(&self, id: EntityId) -> Vec<EntityId> {
        let mut chain = Vec::new();
        let mut cursor = Some(id);
        let mut guard = 0;
        while let Some(current) = cursor {
            chain.push(current);
            guard += 1;
            if guard > 64 {
                break; // защита от цикла в данных
            }
            cursor = self.get(current).and_then(|e| e.parent);
        }
        chain.reverse();
        chain
    }

    /// Расстояние в рёбрах между сущностями, не более `max_hops`.
    ///
    /// Используется скорингом доказательств: чем ближе по графу, тем весомее связь.
    #[must_use]
    pub fn hops(&self, from: EntityId, to: EntityId, max_hops: u32) -> Option<u32> {
        if from == to {
            return Some(0);
        }
        let mut visited: HashSet<EntityId> = HashSet::new();
        let mut queue: VecDeque<(EntityId, u32)> = VecDeque::new();
        queue.push_back((from, 0));
        visited.insert(from);
        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            if let Some(neighbours) = self.adjacency.get(&current) {
                for &next in neighbours {
                    if next == to {
                        return Some(depth + 1);
                    }
                    if visited.insert(next) {
                        queue.push_back((next, depth + 1));
                    }
                }
            }
        }
        None
    }

    fn allocate(&mut self, mut entity: Entity) -> EntityId {
        if let Some(index) = self.free.pop() {
            if let Some(slot) = self.slots.get_mut(index as usize) {
                slot.generation = slot.generation.wrapping_add(1);
                let id = EntityId::new(index, slot.generation);
                entity.id = id;
                slot.entity = Some(entity);
                slot.last_tick = self.tick.0;
                return id;
            }
        }
        let index = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
        let id = EntityId::new(index, 0);
        entity.id = id;
        self.slots.push(Slot {
            generation: 0,
            entity: Some(entity),
            last_tick: self.tick.0,
        });
        id
    }

    /// Выводит сущность из графа, сохраняя её последнюю запись для истории.
    fn retire(&mut self, id: EntityId) -> Option<EntityRecord> {
        let slot = self.slots.get_mut(id.index() as usize)?;
        if slot.generation != id.generation() {
            return None;
        }
        let mut entity = slot.entity.take()?;
        entity.alive = false;
        slot.last_tick = 0;
        self.free.push(id.index());

        self.by_key.remove(&entity.key);
        if let Some(logical) = entity.logical.clone() {
            self.recent_dead.insert(
                logical,
                DeadLogical {
                    at: self.now,
                    kind: entity.kind,
                },
            );
        }

        // Закрываем инцидентные связи и чистим индексы.
        for relation in self.relations.iter_mut() {
            if relation.until.is_none() && (relation.from == id || relation.to == id) {
                relation.until = Some(self.now);
                self.rel_index.remove(&relation.triple());
            }
        }
        self.relations.retain(|r| r.until.is_none());
        self.adjacency.remove(&id);
        for neighbours in self.adjacency.values_mut() {
            neighbours.retain(|n| *n != id);
        }
        self.stats.relations = self.relations.len();
        self.stats.deleted_total = self.stats.deleted_total.saturating_add(1);

        self.events.push(
            Event::new(self.now, EventKind::Deleted, entity.name.clone()).entity(id, entity.kind),
        );

        Some(EntityRecord {
            id,
            key: entity.key,
            kind: entity.kind,
            name: entity.name,
            parent: entity.parent,
            labels: entity.labels,
            first_seen: entity.first_seen,
            last_seen: self.now,
            alive: false,
        })
    }

    fn prune_recent_dead(&mut self) {
        let window = u128::from(self.restart_window_ms);
        let now = self.now;
        self.recent_dead
            .retain(|_, dead| now.saturating_sub(dead.at).as_millis() <= window);
    }
}

/// Контекст сбора, который видит коллектор.
pub struct CollectCtx<'a> {
    graph: &'a mut EntityGraph,
    interval_secs: f64,
    errors: u32,
}

impl std::fmt::Debug for CollectCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectCtx")
            .field("interval_secs", &self.interval_secs)
            .field("errors", &self.errors)
            .finish()
    }
}

impl<'a> CollectCtx<'a> {
    #[must_use]
    pub fn new(graph: &'a mut EntityGraph, interval_secs: f64) -> Self {
        CollectCtx {
            graph,
            interval_secs,
            errors: 0,
        }
    }

    #[must_use]
    pub const fn interval_secs(&self) -> f64 {
        self.interval_secs
    }

    #[must_use]
    pub const fn now(&self) -> Timestamp {
        self.graph.now()
    }

    #[must_use]
    pub const fn tick(&self) -> TickId {
        self.graph.tick()
    }

    #[must_use]
    pub const fn host(&self) -> EntityId {
        self.graph.host()
    }

    pub fn upsert(&mut self, spec: EntitySpec) -> EntityId {
        self.graph.upsert(spec)
    }

    pub fn sample(&mut self, entity: EntityId, metric: MetricId, value: f64) {
        self.graph.sample(entity, metric, value);
    }

    pub fn relate(&mut self, from: EntityId, kind: RelationKind, to: EntityId) {
        self.graph.relate(from, kind, to);
    }

    pub fn event(&mut self, event: Event) {
        self.graph.push_event(event);
    }

    pub fn touch(&mut self, id: EntityId) {
        self.graph.touch(id);
    }

    #[must_use]
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.graph.get(id)
    }

    #[must_use]
    pub fn entity_by_key(&self, key: &crate::entity::EntityKey) -> Option<&Entity> {
        self.graph.get_by_key(key)
    }

    /// Живые сущности указанного вида. Нужен коллекторам для связывания
    /// наблюдений разных подсистем (например, `io.stat` cgroup с Disk,
    /// созданным disk-коллектором раньше в том же такте).
    pub fn entities_of_kind(&self, kind: EntityKind) -> impl Iterator<Item = &Entity> + '_ {
        self.graph.entities_of_kind(kind)
    }
    /// Сообщает о нефатальной ошибке подсистемы.
    pub fn note_error(&mut self, collector: &'static str, message: &str) {
        self.errors = self.errors.saturating_add(1);
        self.graph.push_collector_error(collector, message);
    }

    #[must_use]
    pub const fn errors(&self) -> u32 {
        self.errors
    }

    /// Пустые метки — короткая форма для коллекторов.
    #[must_use]
    pub fn labels() -> Labels {
        Labels::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityKey, Runtime};
    use crate::metric::ids;

    fn graph_at(ms: u64) -> EntityGraph {
        EntityGraph::new("boot-1", "worker-03", Timestamp::from_millis(ms))
    }

    fn process(pid: i32, start: u64) -> EntityKey {
        EntityKey::Process {
            pid,
            start_ticks: start,
        }
    }

    #[test]
    fn host_entity_exists_after_construction() {
        let g = graph_at(1_000);
        let host = g.get(g.host()).expect("хост должен существовать");
        assert_eq!(host.kind, EntityKind::Host);
        assert_eq!(host.name, "worker-03");
    }

    #[test]
    fn same_key_returns_same_id_across_ticks() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let a = g.upsert(EntitySpec::new(process(10, 55), "nginx"));
        let _ = g.end_tick();
        g.begin_tick(Timestamp::from_millis(3_000));
        let b = g.upsert(EntitySpec::new(process(10, 55), "nginx"));
        let _ = g.end_tick();
        assert_eq!(a, b);
    }

    #[test]
    fn pid_reuse_creates_new_entity() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let first = g.upsert(EntitySpec::new(process(10, 55), "nginx"));
        let _ = g.end_tick();
        g.begin_tick(Timestamp::from_millis(3_000));
        let second = g.upsert(EntitySpec::new(process(10, 99), "python"));
        let _ = g.end_tick();
        assert_ne!(
            first, second,
            "тот же PID с другим starttime — другая сущность"
        );
    }

    #[test]
    fn disappearing_entity_is_deleted_after_grace() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let id = g.upsert(EntitySpec::new(process(11, 7), "worker"));
        let _ = g.end_tick();

        // Такт без наблюдения — работает фора.
        g.begin_tick(Timestamp::from_millis(3_000));
        let batch = g.end_tick();
        assert!(
            g.get(id).is_some(),
            "фора должна сохранить сущность на один такт"
        );
        assert!(!batch.events.iter().any(|e| e.kind == EventKind::Deleted));

        // Второй такт без наблюдения — удаление.
        g.begin_tick(Timestamp::from_millis(4_000));
        let batch = g.end_tick();
        assert!(g.get(id).is_none());
        assert!(batch.events.iter().any(|e| e.kind == EventKind::Deleted));
        let record = batch
            .records
            .iter()
            .find(|r| r.id == id)
            .expect("запись об удалении");
        assert!(!record.alive);
        assert_eq!(record.last_seen, Timestamp::from_millis(4_000));
    }

    #[test]
    fn stale_entity_id_is_not_resolved_after_slot_reuse() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let old = g.upsert(EntitySpec::new(process(12, 1), "a"));
        let _ = g.end_tick();
        for step in 3..=5 {
            g.begin_tick(Timestamp::from_millis(step * 1_000));
            let _ = g.end_tick();
        }
        g.begin_tick(Timestamp::from_millis(6_000));
        let new = g.upsert(EntitySpec::new(process(13, 2), "b"));
        let _ = g.end_tick();
        assert_eq!(new.index(), old.index(), "слот должен переиспользоваться");
        assert_ne!(new.generation(), old.generation());
        assert!(g.get(old).is_none(), "устаревший id не должен разрешаться");
    }

    #[test]
    fn restart_is_detected_within_window() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        g.upsert(
            EntitySpec::new(
                EntityKey::Container {
                    runtime: Runtime::Docker,
                    id: "aaa".into(),
                },
                "payment-api",
            )
            .logical("payment-api"),
        );
        let _ = g.end_tick();

        for step in 3..=4 {
            g.begin_tick(Timestamp::from_millis(step * 1_000));
            let _ = g.end_tick();
        }

        g.begin_tick(Timestamp::from_millis(5_000));
        g.upsert(
            EntitySpec::new(
                EntityKey::Container {
                    runtime: Runtime::Docker,
                    id: "bbb".into(),
                },
                "payment-api",
            )
            .logical("payment-api"),
        );
        let batch = g.end_tick();
        assert!(
            batch.events.iter().any(|e| e.kind == EventKind::Restarted),
            "ожидалось событие перезапуска, получено: {:?}",
            batch.events.iter().map(|e| e.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn restart_is_not_detected_outside_window() {
        let mut g = graph_at(1_000);
        g.set_restart_window_ms(2_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        g.upsert(
            EntitySpec::new(
                EntityKey::Container {
                    runtime: Runtime::Docker,
                    id: "aaa".into(),
                },
                "svc",
            )
            .logical("svc"),
        );
        let _ = g.end_tick();
        for step in 3..=10 {
            g.begin_tick(Timestamp::from_millis(step * 1_000));
            let _ = g.end_tick();
        }
        g.begin_tick(Timestamp::from_millis(11_000));
        g.upsert(
            EntitySpec::new(
                EntityKey::Container {
                    runtime: Runtime::Docker,
                    id: "bbb".into(),
                },
                "svc",
            )
            .logical("svc"),
        );
        let batch = g.end_tick();
        assert!(!batch.events.iter().any(|e| e.kind == EventKind::Restarted));
        assert!(batch.events.iter().any(|e| e.kind == EventKind::Created));
    }

    /// Раздел 124: инвентаризация запуска не является потоком изменений.
    #[test]
    fn baseline_does_not_emit_created_events() {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        for pid in 0..50_i32 {
            graph.upsert(
                EntitySpec::new(
                    crate::entity::EntityKey::Process {
                        pid,
                        start_ticks: 1,
                    },
                    "proc",
                )
                .parent(host),
            );
        }
        let batch = graph.end_tick();
        let has_created = batch.events.iter().any(|e| e.kind == EventKind::Created);
        assert!(
            !has_created,
            "существовавшие до наблюдения объекты не создавались при нас"
        );
    }

    /// Раздел 132: одно событие baseline с разбором по видам.
    #[test]
    fn baseline_emits_single_observation_event() {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        for pid in 0..7_i32 {
            graph.upsert(
                EntitySpec::new(
                    crate::entity::EntityKey::Process {
                        pid,
                        start_ticks: 1,
                    },
                    "proc",
                )
                .parent(host),
            );
        }
        let batch = graph.end_tick();
        let baseline: Vec<&Event> = batch
            .events
            .iter()
            .filter(|e| e.kind == EventKind::ObservationStarted)
            .collect();
        assert_eq!(baseline.len(), 1, "ровно одно событие начала наблюдения");
        let event = baseline[0];
        assert_eq!(event.at, Timestamp::from_millis(2_000));
        let detail = event.detail.as_str();
        assert!(detail.contains("baseline: 8 entities"), "детали: {detail}");
        assert!(detail.contains("processes 7"), "детали: {detail}");
        assert_eq!(
            batch.events.first().map(|e| e.kind),
            Some(EventKind::ObservationStarted),
            "наблюдение началось раньше всего прочего в такте"
        );
    }

    /// После baseline обычные появления снова являются событиями: иначе
    /// перезапуск сервиса стал бы невидимым.
    #[test]
    fn created_events_resume_after_baseline() {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let _ = graph.end_tick();

        graph.begin_tick(Timestamp::from_millis(3_000));
        graph.upsert(
            EntitySpec::new(
                crate::entity::EntityKey::Process {
                    pid: 42,
                    start_ticks: 9,
                },
                "late",
            )
            .parent(host),
        );
        let batch = graph.end_tick();
        assert!(
            batch.events.iter().any(|e| e.kind == EventKind::Created),
            "появление после baseline - настоящее событие"
        );
        let repeated = batch
            .events
            .iter()
            .any(|e| e.kind == EventKind::ObservationStarted);
        assert!(!repeated, "наблюдение начинается один раз");
    }

    #[test]
    fn names_are_sanitized_on_ingest() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let id = g.upsert(EntitySpec::new(process(20, 1), "\u{1b}[2Jevil\u{0}name"));
        let entity = g.get(id).expect("сущность");
        assert!(!entity.name.contains('\u{1b}'));
        assert!(!entity.name.contains('\u{0}'));
        assert!(entity.name.contains("evil"));
    }

    #[test]
    fn samples_for_unknown_entity_are_dropped() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        g.sample(EntityId::new(999, 0), ids::HOST_CPU_UTIL, 0.5);
        let batch = g.end_tick();
        assert!(batch.samples.is_empty());
    }

    #[test]
    fn non_finite_samples_are_dropped() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let host = g.host();
        g.sample(host, ids::HOST_CPU_UTIL, f64::NAN);
        g.sample(host, ids::HOST_CPU_UTIL, f64::INFINITY);
        g.sample(host, ids::HOST_CPU_UTIL, 0.25);
        let batch = g.end_tick();
        assert_eq!(batch.samples.len(), 1);
    }

    #[test]
    fn relations_are_deduplicated() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let host = g.host();
        let proc_id = g.upsert(EntitySpec::new(process(30, 1), "svc"));
        g.relate(host, RelationKind::ParentOf, proc_id);
        g.relate(host, RelationKind::ParentOf, proc_id);
        assert_eq!(g.relations().count(), 1);
    }

    #[test]
    fn hops_measures_graph_distance() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let host = g.host();
        let cgroup =
            g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 7 }, "svc.slice").parent(host));
        let proc_id = g.upsert(EntitySpec::new(process(40, 1), "svc").parent(cgroup));
        assert_eq!(g.hops(host, host, 4), Some(0));
        assert_eq!(g.hops(host, cgroup, 4), Some(1));
        assert_eq!(g.hops(host, proc_id, 4), Some(2));
        assert_eq!(g.hops(host, proc_id, 1), None);
    }

    #[test]
    fn ancestry_returns_path_from_root() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let host = g.host();
        let cgroup =
            g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 7 }, "svc.slice").parent(host));
        let proc_id = g.upsert(EntitySpec::new(process(41, 1), "svc").parent(cgroup));
        assert_eq!(g.ancestry(proc_id), vec![host, cgroup, proc_id]);
    }

    #[test]
    fn reparenting_emits_event_and_keeps_identity() {
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let host = g.host();
        let cg1 =
            g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 1 }, "a.slice").parent(host));
        let cg2 =
            g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 2 }, "b.slice").parent(host));
        let pid = g.upsert(EntitySpec::new(process(50, 1), "svc").parent(cg1));
        let _ = g.end_tick();

        g.begin_tick(Timestamp::from_millis(3_000));
        let same = g.upsert(EntitySpec::new(process(50, 1), "svc").parent(cg2));
        let batch = g.end_tick();
        assert_eq!(
            pid, same,
            "идентичность процесса не меняется при репарентинге"
        );
        assert!(batch.events.iter().any(|e| e.kind == EventKind::Reparented));
    }

    #[test]
    fn ancestry_is_guarded_against_cycles() {
        // Искусственный цикл: сущность становится родителем сама себе через две связи.
        let mut g = graph_at(1_000);
        g.begin_tick(Timestamp::from_millis(2_000));
        let a = g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 1 }, "a"));
        let b = g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 2 }, "b").parent(a));
        // Замыкаем цикл через upsert с родителем b.
        let _ = g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 1 }, "a").parent(b));
        let chain = g.ancestry(b);
        assert!(chain.len() <= 65, "обход не должен зависать на цикле");
    }
}
