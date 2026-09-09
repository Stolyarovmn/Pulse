//! Правила обнаружения проблем.
//!
//! Правила декларативны и детерминированы: из конфигурации приходят только
//! числовые пороги, логика живёт в коде. Мини-DSL сознательно не вводится — его
//! пришлось бы отлаживать вместо диагностики, а объяснимость («почему возникла
//! проблема») требует, чтобы каждое правило само формировало доказательства.
//!
//! Правило не занимается подавлением дребезга: оно сообщает «условие входа
//! выполнено» или «условие снятия выполнено», а гистерезис применяет
//! [`crate::analyzer::Analyzer`].

use pulse_core::config::Rules as RulesCfg;
use pulse_core::entity::{EntityId, EntityKind};
use pulse_core::metric::{ids, MetricId};
use pulse_core::problem::{Evidence, RuleId, Severity};
use pulse_core::snapshot::LatestValues;
use pulse_core::time::Timestamp;
use pulse_core::{EntityGraph, SeriesKey};
use pulse_store::History;

/// Контекст оценки правила.
pub struct RuleCtx<'a> {
    pub graph: &'a EntityGraph,
    pub latest: &'a LatestValues,
    pub history: &'a History,
    pub now: Timestamp,
    pub cfg: &'a RulesCfg,
}

impl std::fmt::Debug for RuleCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleCtx").field("now", &self.now).finish()
    }
}

impl RuleCtx<'_> {
    /// Значение метрики сущности.
    #[must_use]
    pub fn value(&self, entity: EntityId, metric: MetricId) -> Option<f64> {
        self.latest.get(entity, metric)
    }

    /// Скорость counter-метрики за последние `secs` секунд.
    #[must_use]
    pub fn rate(&self, entity: EntityId, metric: MetricId, secs: u64) -> Option<f64> {
        let from = self.now.saturating_sub_millis(secs.saturating_mul(1000));
        self.history
            .rate(SeriesKey::new(entity, metric), from, self.now)
    }

    /// Прирост counter-метрики за последние `secs` секунд.
    #[must_use]
    pub fn delta(&self, entity: EntityId, metric: MetricId, secs: u64) -> Option<f64> {
        let from = self.now.saturating_sub_millis(secs.saturating_mul(1000));
        let window = self
            .history
            .window(SeriesKey::new(entity, metric), from, self.now)?;
        Some((window.last - window.first).max(0.0))
    }

    /// Имя сущности для заголовка проблемы.
    #[must_use]
    pub fn name(&self, entity: EntityId) -> String {
        self.graph
            .get(entity)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// Срабатывание правила на одной сущности.
#[derive(Clone, Debug)]
pub struct RuleHit {
    pub entity: EntityId,
    pub entity_name: String,
    pub severity: Severity,
    /// Условие входа выполнено.
    pub enter: bool,
    /// Условие снятия выполнено.
    pub clear: bool,
    pub title: String,
    pub summary: String,
    pub evidence: Vec<Evidence>,
}

/// Правило.
pub trait Rule: Send {
    fn id(&self) -> RuleId;
    fn evaluate(&self, ctx: &RuleCtx<'_>) -> Vec<RuleHit>;
}

/// Как форматировать величину в заголовке и доказательствах.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Доля 0..1 показывается процентами.
    Percent,
    /// Миллисекунды.
    Millis,
    /// Ядра CPU.
    Cores,
}

impl Shape {
    fn format(self, value: f64) -> String {
        match self {
            Shape::Percent => format!("{:.0}%", value * 100.0),
            Shape::Millis => format!("{value:.1} ms"),
            Shape::Cores => format!("{value:.2} ядер"),
        }
    }
}

/// Пороговое правило: величина сравнивается с warn/crit/clear.
///
/// Одна структура покрывает большинство правил, потому что все они имеют одну
/// форму: «взять величину у подходящих сущностей и сравнить с порогами».
/// Различия — в метриках, форматировании и предусловии применимости.
#[derive(Debug)]
pub struct ThresholdRule {
    pub id: RuleId,
    /// Метрика на хосте (если применимо).
    pub host_metric: Option<MetricId>,
    /// Метрика на владельцах ресурсов и cgroup.
    pub owner_metric: Option<MetricId>,
    /// Метрика на процессах.
    pub process_metric: Option<MetricId>,
    /// Метрика на устройствах.
    pub device_metric: Option<MetricId>,
    /// Выбор порогов из конфигурации: `(warn, crit, clear)`.
    pub thresholds: fn(&RulesCfg) -> (f64, f64, f64),
    pub shape: Shape,
    /// Короткая подпись величины: `IO PSI full avg10`.
    pub label: &'static str,
    /// Заголовок проблемы: подставляется отформатированное значение.
    pub title_prefix: &'static str,
    /// Одна строка пояснения.
    pub summary: &'static str,
    /// Предусловие: правило вообще применимо к этой сущности.
    pub gate: Option<fn(&RuleCtx<'_>, EntityId) -> bool>,
    /// Дополнительные доказательства.
    pub extra: Option<fn(&RuleCtx<'_>, EntityId) -> Vec<Evidence>>,
}

impl ThresholdRule {
    fn candidates(&self, ctx: &RuleCtx<'_>) -> Vec<(EntityId, f64)> {
        let mut found = Vec::new();

        if let Some(metric) = self.host_metric {
            if let Some(value) = ctx.value(ctx.graph.host(), metric) {
                found.push((ctx.graph.host(), value));
            }
        }

        if let Some(metric) = self.owner_metric {
            for kind in [
                EntityKind::Unit,
                EntityKind::Container,
                EntityKind::Pod,
                EntityKind::Cgroup,
            ] {
                for entity in ctx.graph.entities_of_kind(kind) {
                    // Метрики cgroup намеренно дублируются на её владельца
                    // (`cgroup.rs`: `derived.publish`), чтобы оператор искал
                    // `nginx.service`, а не inode. Значит одна беда лежит на
                    // двух сущностях сразу, и правило обязано выбрать одну -
                    // иначе список показывает два одинаковых пункта. Выбран
                    // владелец: он и есть ответ на вопрос «что сломалось».
                    //
                    // Условие именно «владелец уже несёт это значение», а не
                    // «владелец существует»: иначе при отброшенном по лимиту
                    // серий сэмпле владельца проблема исчезла бы вовсе, то
                    // есть починка дублирования создала бы слепоту.
                    if kind == EntityKind::Cgroup && owner_reports(ctx, entity.id, metric) {
                        continue;
                    }
                    if let Some(value) = ctx.value(entity.id, metric) {
                        found.push((entity.id, value));
                    }
                }
            }
        }

        if let Some(metric) = self.process_metric {
            for entity in ctx.graph.entities_of_kind(EntityKind::Process) {
                if let Some(value) = ctx.value(entity.id, metric) {
                    found.push((entity.id, value));
                }
            }
        }

        if let Some(metric) = self.device_metric {
            for entity in ctx.graph.entities_of_kind(EntityKind::Disk) {
                if let Some(value) = ctx.value(entity.id, metric) {
                    found.push((entity.id, value));
                }
            }
        }

        found
    }
}

/// Несёт ли владелец cgroup то же значение метрики.
///
/// Владение — это связь `OwnedBy` от cgroup к владельцу, а не совпадение
/// имён: `api.service` как unit и как каталог cgroup называются одинаково,
/// но склейка по имени соединила бы и разные экземпляры одного unit из
/// системного и пользовательского менеджера.
fn owner_reports(ctx: &RuleCtx<'_>, cgroup: EntityId, metric: MetricId) -> bool {
    ctx.graph
        .relations_of(cgroup)
        .filter(|relation| {
            relation.from == cgroup && relation.kind == pulse_core::relation::RelationKind::OwnedBy
        })
        .any(|relation| ctx.value(relation.to, metric).is_some())
}

impl Rule for ThresholdRule {
    fn id(&self) -> RuleId {
        self.id
    }

    fn evaluate(&self, ctx: &RuleCtx<'_>) -> Vec<RuleHit> {
        let (warn, crit, clear) = (self.thresholds)(ctx.cfg);
        let mut hits = Vec::new();

        for (entity, value) in self.candidates(ctx) {
            if let Some(gate) = self.gate {
                if !gate(ctx, entity) {
                    continue;
                }
            }

            let enter = value > warn;
            let clear_now = value < clear;
            if !enter && !clear_now {
                // Зона гистерезиса: состояние не меняется.
                continue;
            }

            let severity = if value > crit {
                Severity::Crit
            } else {
                Severity::Warn
            };

            let formatted = self.shape.format(value);
            let mut evidence =
                vec![
                    Evidence::new(self.label, formatted.clone()).with_threshold(format!(
                        "WARN > {} / CRIT > {}",
                        self.shape.format(warn),
                        self.shape.format(crit)
                    )),
                ];
            if let Some(extra) = self.extra {
                evidence.extend(extra(ctx, entity));
            }

            hits.push(RuleHit {
                entity,
                entity_name: ctx.name(entity),
                severity,
                enter,
                clear: clear_now,
                title: format!("{} {}", self.title_prefix, formatted),
                summary: self.summary.to_string(),
                evidence,
            });
        }

        hits
    }
}

/// Правило OOM: событие, а не уровень.
///
/// Отличается от пороговых: любое ненулевое приращение счётчика убийств — уже
/// критично, дребезга здесь не бывает, а гасить событие по порогу нельзя.
#[derive(Debug)]
pub struct OomRule {
    pub id: RuleId,
    /// Окно, в котором приращение считается свежим, секунды.
    pub window_secs: u64,
}

impl Rule for OomRule {
    fn id(&self) -> RuleId {
        self.id
    }

    fn evaluate(&self, ctx: &RuleCtx<'_>) -> Vec<RuleHit> {
        let mut hits = Vec::new();

        let mut check = |entity: EntityId, metric: MetricId, scope: &str| {
            let Some(delta) = ctx.delta(entity, metric, self.window_secs) else {
                return;
            };
            let happened = delta > 0.0;
            hits.push(RuleHit {
                entity,
                entity_name: ctx.name(entity),
                severity: Severity::Crit,
                enter: happened,
                clear: !happened,
                title: format!("OOM kill: {delta:.0}"),
                summary: format!(
                    "ядро убивало процессы из-за нехватки памяти ({scope}) за последние {} с",
                    self.window_secs
                ),
                evidence: vec![Evidence::new("OOM kills", format!("{delta:.0}"))
                    .with_threshold("любое значение > 0".to_string())],
            });
        };

        check(ctx.graph.host(), ids::HOST_OOM_KILLS, "хост");
        for kind in [EntityKind::Unit, EntityKind::Container, EntityKind::Cgroup] {
            let entities: Vec<EntityId> = ctx.graph.entities_of_kind(kind).map(|e| e.id).collect();
            for entity in entities {
                check(entity, ids::CG_MEM_EVENTS_OOM_KILL, kind.as_str());
            }
        }

        hits
    }
}

/// Правило по swap: считается из двух метрик хоста, поэтому отдельным типом.
#[derive(Debug)]
pub struct SwapRule {
    pub id: RuleId,
}

impl Rule for SwapRule {
    fn id(&self) -> RuleId {
        self.id
    }

    fn evaluate(&self, ctx: &RuleCtx<'_>) -> Vec<RuleHit> {
        let host = ctx.graph.host();
        let total = ctx.value(host, ids::HOST_SWAP_TOTAL).unwrap_or(0.0);
        // Без swap правило неприменимо: иначе оно всегда «в норме» и только шумит.
        if total <= 0.0 {
            return Vec::new();
        }
        let used = ctx.value(host, ids::HOST_SWAP_USED).unwrap_or(0.0);
        let ratio = (used / total).clamp(0.0, 1.0);
        let (warn, crit, clear) = (
            ctx.cfg.swap_util_warn,
            ctx.cfg.swap_util_crit,
            ctx.cfg.swap_util_clear,
        );

        let enter = ratio > warn;
        let clear_now = ratio < clear;
        if !enter && !clear_now {
            return Vec::new();
        }

        vec![RuleHit {
            entity: host,
            entity_name: ctx.name(host),
            severity: if ratio > crit {
                Severity::Crit
            } else {
                Severity::Warn
            },
            enter,
            clear: clear_now,
            title: format!("swap занят {:.0}%", ratio * 100.0),
            summary: "система вытесняет страницы в swap".to_string(),
            evidence: vec![
                Evidence::new("swap занято", format!("{:.0}%", ratio * 100.0)).with_threshold(
                    format!("WARN > {:.0}% / CRIT > {:.0}%", warn * 100.0, crit * 100.0),
                ),
                Evidence::new(
                    "объём swap",
                    format!("{:.1} GiB", total / 1024.0 / 1024.0 / 1024.0),
                ),
            ],
        }]
    }
}

/// Предусловие для правила throttling: без квоты CPU оно бессмысленно.
fn has_cpu_quota(ctx: &RuleCtx<'_>, entity: EntityId) -> bool {
    ctx.value(entity, ids::CG_CPU_LIMIT_CORES)
        .is_some_and(|limit| limit > 0.0)
}

/// Дополнительные доказательства для throttling: лимит и фактическое потребление.
fn throttle_evidence(ctx: &RuleCtx<'_>, entity: EntityId) -> Vec<Evidence> {
    let mut out = Vec::new();
    if let Some(limit) = ctx.value(entity, ids::CG_CPU_LIMIT_CORES) {
        out.push(Evidence::new("лимит CPU", format!("{limit:.2} ядер")));
    }
    if let Some(cores) = ctx.value(entity, ids::CG_CPU_CORES) {
        out.push(Evidence::new("потребление CPU", format!("{cores:.2} ядер")));
    }
    out
}

/// Предусловие для правила задержки диска: без операций величина не определена.
fn has_disk_traffic(ctx: &RuleCtx<'_>, entity: EntityId) -> bool {
    let reads = ctx.rate(entity, ids::DISK_READ_OPS, 30).unwrap_or(0.0);
    let writes = ctx.rate(entity, ids::DISK_WRITE_OPS, 30).unwrap_or(0.0);
    reads + writes > 0.0
}

/// Доказательства для задержки диска: очередь и занятость устройства.
fn disk_evidence(ctx: &RuleCtx<'_>, entity: EntityId) -> Vec<Evidence> {
    let mut out = Vec::new();
    if let Some(queue) = ctx.value(entity, ids::DISK_QUEUE) {
        out.push(Evidence::new("очередь", format!("{queue:.2}")));
    }
    if let Some(util) = ctx.value(entity, ids::DISK_UTIL) {
        out.push(Evidence::new("занятость", format!("{:.0}%", util * 100.0)));
    }
    out
}

/// Доказательства для памяти: текущее потребление и лимит.
fn memory_evidence(ctx: &RuleCtx<'_>, entity: EntityId) -> Vec<Evidence> {
    let mut out = Vec::new();
    if let Some(current) = ctx.value(entity, ids::CG_MEM_CURRENT) {
        out.push(Evidence::new(
            "потребление",
            format!("{:.1} MiB", current / 1024.0 / 1024.0),
        ));
    }
    if let Some(limit) = ctx.value(entity, ids::CG_MEM_LIMIT) {
        if limit > 0.0 {
            out.push(Evidence::new(
                "лимит",
                format!("{:.1} MiB", limit / 1024.0 / 1024.0),
            ));
        }
    }
    out
}

/// Полный набор правил первой версии.
#[must_use]
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(ThresholdRule {
            id: RuleId("psi.cpu"),
            host_metric: Some(ids::HOST_PSI_CPU_SOME_AVG10),
            owner_metric: Some(ids::CG_PSI_CPU_SOME_AVG10),
            process_metric: None,
            device_metric: None,
            thresholds: |c| (c.psi_cpu_warn, c.psi_cpu_crit, c.psi_cpu_clear),
            shape: Shape::Percent,
            label: "CPU PSI some avg10",
            title_prefix: "нехватка CPU",
            summary: "задачи ждут процессорное время",
            gate: None,
            extra: None,
        }),
        Box::new(ThresholdRule {
            id: RuleId("psi.memory"),
            host_metric: Some(ids::HOST_PSI_MEM_FULL_AVG10),
            owner_metric: Some(ids::CG_PSI_MEM_FULL_AVG10),
            process_metric: None,
            device_metric: None,
            thresholds: |c| (c.psi_memory_warn, c.psi_memory_crit, c.psi_memory_clear),
            shape: Shape::Percent,
            label: "Memory PSI full avg10",
            title_prefix: "давление на память",
            summary: "все задачи простаивают из-за нехватки памяти",
            gate: None,
            extra: Some(memory_evidence),
        }),
        Box::new(ThresholdRule {
            id: RuleId("psi.io"),
            // Именно full, а не some: some срабатывает от одной ждущей задачи,
            // то есть практически всегда при любом нормальном вводе-выводе.
            host_metric: Some(ids::HOST_PSI_IO_FULL_AVG10),
            owner_metric: Some(ids::CG_PSI_IO_FULL_AVG10),
            process_metric: None,
            device_metric: None,
            thresholds: |c| (c.psi_io_warn, c.psi_io_crit, c.psi_io_clear),
            shape: Shape::Percent,
            label: "IO PSI full avg10",
            title_prefix: "давление ввода-вывода",
            summary: "задачи простаивают в ожидании диска",
            gate: None,
            extra: None,
        }),
        Box::new(ThresholdRule {
            id: RuleId("cgroup.throttle"),
            host_metric: None,
            owner_metric: Some(ids::CG_CPU_THROTTLE_RATIO),
            process_metric: None,
            device_metric: None,
            thresholds: |c| (c.throttle_warn, c.throttle_crit, c.throttle_clear),
            shape: Shape::Percent,
            label: "доля периодов с throttling",
            title_prefix: "CPU throttling",
            summary: "квота CPU исчерпывается, задачи принудительно останавливаются",
            gate: Some(has_cpu_quota),
            extra: Some(throttle_evidence),
        }),
        Box::new(ThresholdRule {
            id: RuleId("memory.pressure"),
            host_metric: Some(ids::HOST_MEM_UTIL),
            owner_metric: Some(ids::CG_MEM_UTIL),
            process_metric: None,
            device_metric: None,
            thresholds: |c| (c.memory_util_warn, c.memory_util_crit, c.memory_util_clear),
            shape: Shape::Percent,
            label: "доля занятой памяти",
            title_prefix: "память заполнена на",
            summary: "память близка к пределу",
            gate: None,
            extra: Some(memory_evidence),
        }),
        Box::new(ThresholdRule {
            id: RuleId("disk.latency"),
            host_metric: None,
            owner_metric: None,
            process_metric: None,
            device_metric: Some(ids::DISK_AWAIT),
            thresholds: |c| {
                (
                    c.disk_await_warn_ms,
                    c.disk_await_crit_ms,
                    c.disk_await_clear_ms,
                )
            },
            shape: Shape::Millis,
            label: "среднее время обслуживания",
            title_prefix: "задержка диска",
            summary: "устройство отвечает медленно",
            gate: Some(has_disk_traffic),
            extra: Some(disk_evidence),
        }),
        Box::new(ThresholdRule {
            id: RuleId("fd.pressure"),
            host_metric: Some(ids::HOST_FD_UTIL),
            owner_metric: None,
            process_metric: Some(ids::PROC_FD_UTIL),
            device_metric: None,
            thresholds: |c| (c.fd_util_warn, c.fd_util_crit, c.fd_util_clear),
            shape: Shape::Percent,
            label: "доля использованных дескрипторов",
            title_prefix: "дескрипторы исчерпаны на",
            summary: "приближается предел числа открытых файлов",
            gate: None,
            extra: None,
        }),
        Box::new(SwapRule {
            id: RuleId("swap.pressure"),
        }),
        Box::new(OomRule {
            id: RuleId("memory.oom"),
            window_secs: 60,
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::config::Store as StoreConfig;
    use pulse_core::entity::{EntityKey, EntitySpec};
    use pulse_core::EntityGraph;

    fn store_cfg() -> StoreConfig {
        StoreConfig::default()
    }

    /// Готовит граф с хостом, cgroup-владельцем и диском.
    struct Fixture {
        graph: EntityGraph,
        history: History,
        latest: LatestValues,
        now: Timestamp,
        unit: EntityId,
        disk: EntityId,
    }

    impl Fixture {
        fn new() -> Self {
            let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
            graph.begin_tick(Timestamp::from_millis(2_000));
            let host = graph.host();
            let unit = graph.upsert(
                EntitySpec::new(
                    EntityKey::Unit {
                        name: "nginx.service".into(),
                    },
                    "nginx.service",
                )
                .parent(host),
            );
            let disk = graph.upsert(
                EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host),
            );
            let _ = graph.end_tick();
            Fixture {
                graph,
                history: History::new(&store_cfg()),
                latest: LatestValues::new(),
                now: Timestamp::from_millis(2_000),
                unit,
                disk,
            }
        }

        fn set(&mut self, entity: EntityId, metric: MetricId, value: f64) {
            self.latest.set(SeriesKey::new(entity, metric), value);
        }

        /// Техническая cgroup, которой владеет unit фикстуры: ровно та
        /// топология, что строит коллектор на живом хосте.
        fn own_cgroup(&mut self, name: &str) -> EntityId {
            self.graph.begin_tick(self.now);
            let host = self.graph.host();
            let cgroup = self
                .graph
                .upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 4_242 }, name).parent(host));
            let owned = pulse_core::relation::RelationKind::OwnedBy;
            let backed = pulse_core::relation::RelationKind::BackedBy;
            self.graph.relate(cgroup, owned, self.unit);
            self.graph.relate(self.unit, backed, cgroup);
            let _ = self.graph.end_tick();
            cgroup
        }

        fn ctx<'a>(&'a self, cfg: &'a RulesCfg) -> RuleCtx<'a> {
            RuleCtx {
                graph: &self.graph,
                latest: &self.latest,
                history: &self.history,
                now: self.now,
                cfg,
            }
        }
    }

    fn rule(id: &str) -> Box<dyn Rule> {
        default_rules()
            .into_iter()
            .find(|r| r.id().0 == id)
            .expect("правило должно существовать")
    }

    #[test]
    fn psi_io_rule_uses_full_metric_only() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let host = fixture.graph.host();
        // some высокий, full ниже порога — правило молчит.
        fixture.set(host, ids::HOST_PSI_IO_SOME_AVG10, 0.9);
        fixture.set(host, ids::HOST_PSI_IO_FULL_AVG10, 0.01);
        let hits = rule("psi.io").evaluate(&fixture.ctx(&cfg));
        assert!(hits.iter().all(|h| !h.enter), "full ниже порога: {hits:?}");
    }

    #[test]
    fn threshold_crossings_produce_expected_severity() {
        let cfg = RulesCfg::default();
        let cases = [
            (0.05, false, Severity::Warn),
            (0.25, true, Severity::Warn),
            (0.60, true, Severity::Crit),
        ];
        for (value, expect_enter, expect_severity) in cases {
            let mut fixture = Fixture::new();
            let host = fixture.graph.host();
            fixture.set(host, ids::HOST_PSI_CPU_SOME_AVG10, value);
            let hits = rule("psi.cpu").evaluate(&fixture.ctx(&cfg));
            let hit = hits.iter().find(|h| h.entity == host);
            if expect_enter {
                let hit = hit.expect("должно быть срабатывание");
                assert!(hit.enter);
                assert_eq!(hit.severity, expect_severity, "value = {value}");
            } else {
                assert!(hit.is_some_and(|h| h.clear) || hit.is_none());
            }
        }
    }

    #[test]
    fn hysteresis_zone_produces_no_hit() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let host = fixture.graph.host();
        // Между clear (0.10) и warn (0.20) — ни входа, ни снятия.
        fixture.set(host, ids::HOST_PSI_CPU_SOME_AVG10, 0.15);
        let hits = rule("psi.cpu").evaluate(&fixture.ctx(&cfg));
        assert!(hits.is_empty(), "в зоне гистерезиса решений нет: {hits:?}");
    }

    #[test]
    fn throttle_rule_is_silent_without_quota() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let unit = fixture.unit;
        fixture.set(unit, ids::CG_CPU_THROTTLE_RATIO, 0.9);
        // Лимита нет.
        let hits = rule("cgroup.throttle").evaluate(&fixture.ctx(&cfg));
        assert!(hits.is_empty(), "без квоты правило обязано молчать");

        fixture.set(unit, ids::CG_CPU_LIMIT_CORES, 8.0);
        let hits = rule("cgroup.throttle").evaluate(&fixture.ctx(&cfg));
        assert_eq!(hits.len(), 1);
        let hit = hits.first().expect("срабатывание");
        assert!(hit.evidence.iter().any(|e| e.label.contains("лимит")));
    }

    /// PULSE-064: одна проблема на одну беду.
    ///
    /// Коллектор намеренно дублирует метрики cgroup на её владельца
    /// (`derived.publish`), чтобы оператор искал `nginx.service`, а не inode
    /// технической cgroup. Но правило обходило и `Unit`, и `Cgroup`, поэтому
    /// одна исчерпанная квота давала два пункта с одинаковым заголовком и
    /// одинаковым именем: список выглядел как сломанный.
    #[test]
    fn owned_cgroup_does_not_duplicate_the_problem_of_its_owner() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let cgroup = fixture.own_cgroup("nginx.service");
        let unit = fixture.unit;

        // Ровно то, что делает коллектор: значение публикуется на обеих.
        for entity in [cgroup, unit] {
            fixture.set(entity, ids::CG_CPU_THROTTLE_RATIO, 0.9);
            fixture.set(entity, ids::CG_CPU_LIMIT_CORES, 8.0);
        }

        let hits = rule("cgroup.throttle").evaluate(&fixture.ctx(&cfg));
        let entering: Vec<_> = hits.iter().filter(|hit| hit.enter).collect();

        assert_eq!(
            entering.len(),
            1,
            "одна исчерпанная квота обязана дать одну проблему: {entering:?}"
        );
        assert_eq!(
            entering.first().map(|hit| hit.entity),
            Some(unit),
            "проблему обязан нести владелец: оператор ищет сервис, а не inode"
        );
    }

    /// Обратная сторона починки дублирования: молчание тоже дефект.
    ///
    /// Если сэмпл владельца не дошёл (например, отброшен по лимиту серий),
    /// проблема обязана остаться видимой на cgroup. Иначе исправление
    /// дубликата превратилось бы в потерю наблюдения.
    #[test]
    fn cgroup_keeps_the_problem_when_its_owner_reports_nothing() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let cgroup = fixture.own_cgroup("nginx.service");

        // Значение есть только у cgroup: владелец молчит.
        fixture.set(cgroup, ids::CG_CPU_THROTTLE_RATIO, 0.9);
        fixture.set(cgroup, ids::CG_CPU_LIMIT_CORES, 8.0);

        let hits = rule("cgroup.throttle").evaluate(&fixture.ctx(&cfg));
        let entering: Vec<_> = hits.iter().filter(|hit| hit.enter).collect();

        assert_eq!(
            entering.iter().map(|hit| hit.entity).collect::<Vec<_>>(),
            vec![cgroup],
            "без значения у владельца проблему обязана нести cgroup: {entering:?}"
        );
    }

    #[test]
    fn disk_latency_rule_is_silent_without_traffic() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let disk = fixture.disk;
        fixture.set(disk, ids::DISK_AWAIT, 100.0);
        // История пуста, скорость операций нулевая.
        let hits = rule("disk.latency").evaluate(&fixture.ctx(&cfg));
        assert!(hits.is_empty(), "без операций задержка не диагностируется");
    }

    #[test]
    fn swap_rule_is_silent_without_swap() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let host = fixture.graph.host();
        fixture.set(host, ids::HOST_SWAP_TOTAL, 0.0);
        fixture.set(host, ids::HOST_SWAP_USED, 0.0);
        assert!(rule("swap.pressure")
            .evaluate(&fixture.ctx(&cfg))
            .is_empty());

        fixture.set(host, ids::HOST_SWAP_TOTAL, 8.0 * 1024.0 * 1024.0 * 1024.0);
        fixture.set(host, ids::HOST_SWAP_USED, 7.0 * 1024.0 * 1024.0 * 1024.0);
        let hits = rule("swap.pressure").evaluate(&fixture.ctx(&cfg));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits.first().map(|h| h.severity), Some(Severity::Crit));
    }

    #[test]
    fn memory_rule_reports_limit_in_evidence() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let unit = fixture.unit;
        fixture.set(unit, ids::CG_MEM_UTIL, 0.97);
        fixture.set(unit, ids::CG_MEM_CURRENT, 200.0 * 1024.0 * 1024.0);
        fixture.set(unit, ids::CG_MEM_LIMIT, 206.0 * 1024.0 * 1024.0);
        let hits = rule("memory.pressure").evaluate(&fixture.ctx(&cfg));
        let hit = hits.first().expect("срабатывание");
        assert_eq!(hit.severity, Severity::Crit);
        assert!(hit.evidence.iter().any(|e| e.label == "лимит"));
    }

    #[test]
    fn fd_rule_covers_host_and_processes() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let host = fixture.graph.host();
        fixture.set(host, ids::HOST_FD_UTIL, 0.99);
        let hits = rule("fd.pressure").evaluate(&fixture.ctx(&cfg));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits.first().map(|h| h.severity), Some(Severity::Crit));
    }

    #[test]
    fn evidence_always_carries_threshold() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let host = fixture.graph.host();
        fixture.set(host, ids::HOST_PSI_CPU_SOME_AVG10, 0.7);
        let hits = rule("psi.cpu").evaluate(&fixture.ctx(&cfg));
        let hit = hits.first().expect("срабатывание");
        assert!(
            hit.evidence
                .first()
                .and_then(|e| e.threshold.as_ref())
                .is_some(),
            "пользователь обязан видеть порог"
        );
    }

    #[test]
    fn titles_and_summaries_avoid_causal_claims() {
        let cfg = RulesCfg::default();
        let mut fixture = Fixture::new();
        let host = fixture.graph.host();
        fixture.set(host, ids::HOST_PSI_CPU_SOME_AVG10, 0.7);
        fixture.set(host, ids::HOST_MEM_UTIL, 0.99);
        fixture.set(host, ids::HOST_FD_UTIL, 0.99);
        let mut texts = Vec::new();
        for id in ["psi.cpu", "memory.pressure", "fd.pressure"] {
            for hit in rule(id).evaluate(&fixture.ctx(&cfg)) {
                texts.push(hit.title.to_lowercase());
                texts.push(hit.summary.to_lowercase());
            }
        }
        for text in texts {
            for forbidden in ["вызвал", "причина", "caused"] {
                assert!(
                    !text.contains(forbidden),
                    "недопустимое причинное утверждение в «{text}»"
                );
            }
        }
    }

    #[test]
    fn rule_set_has_stable_identifiers() {
        let ids_list: Vec<&str> = default_rules().iter().map(|r| r.id().0).collect();
        assert_eq!(
            ids_list,
            vec![
                "psi.cpu",
                "psi.memory",
                "psi.io",
                "cgroup.throttle",
                "memory.pressure",
                "disk.latency",
                "fd.pressure",
                "swap.pressure",
                "memory.oom",
            ]
        );
    }
}
