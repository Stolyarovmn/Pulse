//! Снимок состояния для потребителей: TUI, экспортёр, CLI.
//!
//! Снимок неизменяем и публикуется целиком: читатель никогда не видит
//! полусобранное состояние и не блокирует поток сбора.

use std::collections::HashMap;

use crate::entity::{Entity, EntityId, EntityKind};
use crate::event::Event;
use crate::graph::{EntityGraph, GraphStats};
use crate::metric::MetricId;
use crate::problem::{Problem, Severity};
use crate::relation::{Relation, RelationKind};
use crate::sample::SeriesKey;
use crate::semantic::MeaningfulEvent;
use crate::time::{TickId, Timestamp};

/// Информационный бюджет Story на одно видимое окно (§ Timeline event budget).
///
/// Верхняя граница «5–20 meaningful items»: больше строк оператор не читает,
/// он ищет в них главное. Всё сверх бюджета учитывается счётчиком.
pub const STORY_BUDGET: usize = 20;

/// Информационный бюджет Recent Changes на Overview (§ Recent Changes).
pub const RECENT_CHANGES_BUDGET: usize = 8;

/// Источник актуального снимка. Читатели (TUI, экспортёр, CLI) получают снимок
/// без блокировки потока сбора: реализация в `pulse-cli` построена на `ArcSwap`.
pub type SnapshotSource = std::sync::Arc<dyn Fn() -> std::sync::Arc<Snapshot> + Send + Sync>;

/// Одно наблюдение: значение и момент, когда его действительно измерили.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Observation {
    pub value: f64,
    pub at: Timestamp,
}

/// Последние значения серий вместе со свежестью наблюдения.
///
/// Момент наблюдения обязателен, а не удобен. Раньше хранилось только
/// `SeriesKey → f64`, поэтому исчезновение источника выглядело как
/// неизменное значение: коллектор не смог прочитать `/proc/diskstats`, а
/// кадр и правила продолжали видеть последнее удачное измерение как
/// текущее. «Не наблюдали» и «столько же, сколько было» — разные
/// утверждения, и различать их обязано хранилище значений, а не каждый
/// вызывающий.
#[derive(Clone, Debug, Default)]
pub struct LatestValues {
    values: HashMap<SeriesKey, Observation>,
    /// Момент кадра: относительно него считается устаревание.
    as_of: Timestamp,
    /// Возраст, после которого наблюдение больше не считается текущим.
    ///
    /// `None` — проверка выключена: так работают модульные тесты и вызовы,
    /// которые сами наполняют набор значениями текущего такта.
    stale_after_ms: Option<u64>,
}

impl LatestValues {
    #[must_use]
    pub fn new() -> Self {
        LatestValues::default()
    }

    /// Задаёт момент кадра и допустимый возраст наблюдения.
    ///
    /// Допуск нужен из-за нормальной работы: значение приходит раз в такт,
    /// поэтому «свежим» обязано считаться и наблюдение предыдущего такта,
    /// иначе каждый кадр объявлял бы половину метрик пропавшими.
    #[must_use]
    pub fn with_freshness(mut self, as_of: Timestamp, stale_after_ms: u64) -> Self {
        self.as_of = as_of;
        self.stale_after_ms = Some(stale_after_ms);
        self
    }

    /// Значение, наблюдённое в момент кадра.
    pub fn set(&mut self, series: SeriesKey, value: f64) {
        let at = self.as_of;
        self.values.insert(series, Observation { value, at });
    }

    /// Значение с явным моментом наблюдения.
    pub fn set_observed(&mut self, series: SeriesKey, value: f64, at: Timestamp) {
        self.values.insert(series, Observation { value, at });
    }

    /// Текущее значение: только если наблюдение не устарело.
    #[must_use]
    pub fn get(&self, entity: EntityId, metric: MetricId) -> Option<f64> {
        let observation = self.values.get(&SeriesKey::new(entity, metric))?;
        match self.stale_after_ms {
            Some(limit) if self.age_ms(observation.at) > limit => None,
            _ => Some(observation.value),
        }
    }

    #[must_use]
    pub fn get_or(&self, entity: EntityId, metric: MetricId, default: f64) -> f64 {
        self.get(entity, metric).unwrap_or(default)
    }

    /// Момент последнего наблюдения серии, даже если оно устарело.
    ///
    /// Нужен интерфейсу и диагностике: «последний раз видели минуту назад»
    /// информативнее, чем просто отсутствие значения.
    #[must_use]
    pub fn observed_at(&self, entity: EntityId, metric: MetricId) -> Option<Timestamp> {
        self.values
            .get(&SeriesKey::new(entity, metric))
            .map(|observation| observation.at)
    }

    /// Серия наблюдалась, но наблюдение больше не считается текущим.
    #[must_use]
    pub fn is_stale(&self, entity: EntityId, metric: MetricId) -> bool {
        let Some(observation) = self.values.get(&SeriesKey::new(entity, metric)) else {
            return false;
        };
        match self.stale_after_ms {
            Some(limit) => self.age_ms(observation.at) > limit,
            None => false,
        }
    }

    fn age_ms(&self, at: Timestamp) -> u64 {
        self.as_of.as_millis().saturating_sub(at.as_millis())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Пары «серия — текущее значение». Устаревшие наблюдения не выдаются:
    /// иначе экспортёр отдал бы их как актуальные.
    pub fn iter(&self) -> impl Iterator<Item = (SeriesKey, f64)> + '_ {
        self.values
            .iter()
            .filter_map(|(key, observation)| match self.stale_after_ms {
                Some(limit) if self.age_ms(observation.at) > limit => None,
                _ => Some((*key, observation.value)),
            })
    }

    pub fn retain_entities(&mut self, keep: &dyn Fn(EntityId) -> bool) {
        self.values.retain(|k, _| keep(k.entity));
    }
}

/// Самонаблюдение агента: попадает и в TUI, и в `/metrics`, и в scorecard.
#[derive(Copy, Clone, Debug, Default)]
pub struct AgentStats {
    pub tick_duration_ms: f64,
    pub tick_duration_p95_ms: f64,
    pub ticks_total: u64,
    pub ticks_skipped: u64,
    pub collector_errors: u64,
    /// Сколько раз сбор дал корректный, но неполный результат из-за бюджета.
    ///
    /// Отдельно от `collector_errors`: усечение по настроенному лимиту — это
    /// неполнота наблюдения, а не отказ подсистемы. Смешивать их нельзя,
    /// иначе на большой ноде агент с дефолтным `max_cgroups` выглядел бы
    /// сломанным, а настоящий отказ сбора терялся бы среди усечений.
    pub collector_truncations: u64,
    pub series_live: usize,
    pub samples_stored: u64,
    pub store_bytes: u64,
    pub events_total: u64,
    /// Число сокращений тёплого слоя по потолку памяти.
    pub history_evicted_buckets: u64,
    /// Число вытесненных горячих тактов по потолку памяти.
    pub history_evicted_hot_ticks: u64,
    /// Сколько сырых событий вытеснено потолком памяти.
    pub history_evicted_events: u64,
    /// Сколько раз eviction завершился без прогресса (бюджет недостижим).
    ///
    /// Ненулевое значение означает «минимальное живое состояние больше лимита»,
    /// а не «цикл крутится»: цикл в этом случае обязан выйти сразу.
    pub history_eviction_no_progress: u64,
    /// Сколько bounded-шагов eviction выполнено за всё время.
    pub history_eviction_iterations: u64,
    /// Пик оценки памяти истории до вытеснения.
    pub history_peak_bytes: u64,
    pub rss_bytes: u64,
    pub cpu_seconds: f64,
    pub redactions: u64,
    pub export_requests: u64,
    pub export_rejected: u64,
    pub export_series: usize,
    pub export_dropped: u64,
}

/// Неизменяемый снимок.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tick: TickId,
    pub at: Timestamp,
    pub hostname: String,
    pub boot_id: String,
    pub host: EntityId,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub latest: LatestValues,
    pub problems: Vec<Problem>,
    /// Хвост сырого журнала событий, свежие последними.
    ///
    /// Это наблюдение, а не изменение: kernel worker churn остаётся здесь и
    /// доступен через отдельный вид. Экраны обязаны читать [`Snapshot::meaningful`].
    pub events: Vec<Event>,
    /// Значимые изменения: Tier0-шум подавлен, повторы свёрнуты в группы.
    pub meaningful: Vec<MeaningfulEvent>,
    /// Сколько сырых событий подавлено как рутинный шум.
    pub suppressed_noise: u64,
    /// Сколько сырых событий свёрнуто в группы повторов.
    pub grouped_events: u64,
    /// Сколько значимых событий не поместилось в информационный бюджет.
    pub over_budget: u64,
    pub graph: GraphStats,
    pub agent: AgentStats,
    index: HashMap<EntityId, usize>,
    /// Индекс связей по сущности: позиции в `relations` для обеих сторон.
    ///
    /// Без него `relations_of` сканировал весь вектор связей на каждый вызов, а
    /// вызывается он на каждую сущность в каждом кадре: на хосте с 2000
    /// сущностей это давало квадратичный путь и кадр в 32 мс.
    relations_index: HashMap<EntityId, Vec<u32>>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            tick: TickId::FIRST,
            at: Timestamp::ZERO,
            hostname: String::new(),
            boot_id: String::new(),
            host: EntityId::NONE,
            entities: Vec::new(),
            relations: Vec::new(),
            latest: LatestValues::new(),
            problems: Vec::new(),
            events: Vec::new(),
            meaningful: Vec::new(),
            suppressed_noise: 0,
            grouped_events: 0,
            over_budget: 0,
            graph: GraphStats::default(),
            agent: AgentStats::default(),
            index: HashMap::new(),
            relations_index: HashMap::new(),
        }
    }
}

impl Snapshot {
    /// Собирает снимок из графа. Сущности сортируются устойчиво:
    /// по виду, затем по имени — чтобы список в TUI не «прыгал».
    #[must_use]
    pub fn build(
        graph: &EntityGraph,
        latest: LatestValues,
        problems: Vec<Problem>,
        events: Vec<Event>,
        agent: AgentStats,
        hostname: &str,
        boot_id: &str,
    ) -> Self {
        let mut entities: Vec<Entity> = graph.entities().cloned().collect();
        entities.sort_by(|a, b| {
            a.kind
                .rank()
                .cmp(&b.kind.rank())
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.as_u64().cmp(&b.id.as_u64()))
        });
        let index = entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id, i))
            .collect();
        let mut problems = problems;
        // Семантическая обработка живёт в снимке, а не в экранах: иначе каждый
        // экран решал бы «что значимо» сам и они бы расходились. Бюджет здесь
        // максимальный из потребителей (Story); Recent Changes режет короче.
        let summary = crate::semantic::summarize(&events, STORY_BUDGET);

        problems.sort_by_key(Problem::sort_key);

        let relations: Vec<Relation> = graph.relations().copied().collect();
        let mut relations_index: HashMap<EntityId, Vec<u32>> = HashMap::new();
        for (position, relation) in relations.iter().enumerate() {
            let position = u32::try_from(position).unwrap_or(u32::MAX);
            relations_index
                .entry(relation.from)
                .or_default()
                .push(position);
            if relation.to != relation.from {
                relations_index
                    .entry(relation.to)
                    .or_default()
                    .push(position);
            }
        }

        Snapshot {
            tick: graph.tick(),
            at: graph.now(),
            hostname: hostname.to_string(),
            boot_id: boot_id.to_string(),
            host: graph.host(),
            entities,
            relations,
            latest,
            problems,
            events,
            meaningful: summary.items,
            suppressed_noise: summary.suppressed,
            grouped_events: summary.grouped,
            over_budget: summary.over_budget,
            graph: graph.stats(),
            agent,
            index,
            relations_index,
        }
    }

    #[must_use]
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.index.get(&id).and_then(|i| self.entities.get(*i))
    }

    #[must_use]
    pub fn value(&self, entity: EntityId, metric: MetricId) -> Option<f64> {
        self.latest.get(entity, metric)
    }

    #[must_use]
    pub fn value_or(&self, entity: EntityId, metric: MetricId, default: f64) -> f64 {
        self.latest.get_or(entity, metric, default)
    }

    pub fn entities_of_kind(&self, kind: EntityKind) -> impl Iterator<Item = &Entity> + '_ {
        self.entities.iter().filter(move |e| e.kind == kind)
    }

    pub fn children(&self, id: EntityId) -> impl Iterator<Item = &Entity> + '_ {
        self.entities.iter().filter(move |e| e.parent == Some(id))
    }

    /// Связи сущности, отфильтрованные по типу.
    pub fn relations_of(
        &self,
        id: EntityId,
        kind: Option<RelationKind>,
    ) -> impl Iterator<Item = &Relation> + '_ {
        self.relations_index
            .get(&id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(move |position| self.relations.get(usize::try_from(*position).ok()?))
            .filter(move |r| kind.is_none_or(|k| r.kind == k))
    }

    /// Цепочка родителей до корня — breadcrumbs.
    #[must_use]
    pub fn ancestry(&self, id: EntityId) -> Vec<EntityId> {
        let mut chain = Vec::new();
        let mut cursor = Some(id);
        let mut guard = 0;
        while let Some(current) = cursor {
            chain.push(current);
            guard += 1;
            if guard > 64 {
                break;
            }
            cursor = self.entity(current).and_then(|e| e.parent);
        }
        chain.reverse();
        chain
    }

    /// Наибольшая серьёзность среди открытых проблем.
    #[must_use]
    pub fn worst_severity(&self) -> Option<Severity> {
        self.problems.iter().map(|p| p.severity).max()
    }

    /// Проблемы, относящиеся к сущности.
    pub fn problems_of(&self, id: EntityId) -> impl Iterator<Item = &Problem> + '_ {
        self.problems.iter().filter(move |p| p.id.entity == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityKey, EntitySpec};
    use crate::metric::ids;

    fn sample_graph() -> EntityGraph {
        let mut g = EntityGraph::new("boot", "host-1", Timestamp::from_millis(1_000));
        g.begin_tick(Timestamp::from_millis(2_000));
        let host = g.host();
        let cg =
            g.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 5 }, "web.slice").parent(host));
        let _ = g.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid: 42,
                    start_ticks: 9,
                },
                "nginx",
            )
            .parent(cg),
        );
        g.sample(host, ids::HOST_CPU_UTIL, 0.42);
        let _ = g.end_tick();
        g
    }

    #[test]
    fn snapshot_indexes_entities() {
        let g = sample_graph();
        let mut latest = LatestValues::new();
        latest.set(SeriesKey::new(g.host(), ids::HOST_CPU_UTIL), 0.42);
        let snap = Snapshot::build(
            &g,
            latest,
            Vec::new(),
            Vec::new(),
            AgentStats::default(),
            "host-1",
            "boot",
        );
        assert_eq!(snap.entities.len(), 3);
        assert_eq!(snap.value(snap.host, ids::HOST_CPU_UTIL), Some(0.42));
        let host = snap.entity(snap.host).expect("хост в снимке");
        assert_eq!(host.kind, EntityKind::Host);
    }

    #[test]
    fn snapshot_sorting_is_stable_by_kind_then_name() {
        let g = sample_graph();
        let snap = Snapshot::build(
            &g,
            LatestValues::new(),
            Vec::new(),
            Vec::new(),
            AgentStats::default(),
            "host-1",
            "boot",
        );
        let kinds: Vec<EntityKind> = snap.entities.iter().map(|e| e.kind).collect();
        let mut sorted = kinds.clone();
        sorted.sort_by_key(|k| k.rank());
        assert_eq!(kinds, sorted);
    }

    #[test]
    fn ancestry_walks_to_host() {
        let g = sample_graph();
        let snap = Snapshot::build(
            &g,
            LatestValues::new(),
            Vec::new(),
            Vec::new(),
            AgentStats::default(),
            "host-1",
            "boot",
        );
        let process = snap
            .entities_of_kind(EntityKind::Process)
            .next()
            .expect("процесс");
        let chain = snap.ancestry(process.id);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain.first().copied(), Some(snap.host));
    }
}
