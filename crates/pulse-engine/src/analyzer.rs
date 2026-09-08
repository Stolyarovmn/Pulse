//! Анализатор: применяет правила и подавляет дребезг.
//!
//! Гистерезис живёт здесь, а не в правилах: так подавление дребезга
//! настраивается один раз для всех правил, а правило остаётся чистой функцией
//! «значение → решение», которую легко тестировать.

use std::collections::{HashMap, HashSet};

use pulse_core::config::Rules as RulesCfg;
use pulse_core::event::{Event, EventKind};
use pulse_core::problem::{Hysteresis, Problem, ProblemId, Severity};
use pulse_core::snapshot::LatestValues;
use pulse_core::time::Timestamp;
use pulse_core::EntityGraph;
use pulse_store::History;

use crate::rules::{default_rules, Rule, RuleCtx, RuleHit};

/// Состояние одной потенциальной проблемы.
#[derive(Clone, Debug)]
struct State {
    hysteresis: Hysteresis,
    since: Timestamp,
    streak: u32,
    last_seen: Timestamp,
    /// Последняя материализованная проблема, пока гистерезис открыт.
    ///
    /// Обязательна, потому что правило не выдаёт попадание в нейтральной
    /// зоне: значение уже ниже порога входа, но ещё выше условия снятия.
    /// Раньше список проблем строился только из попаданий текущего такта,
    /// поэтому открытая проблема исчезала из снимка, хотя внутреннее
    /// состояние оставалось открытым и события о закрытии не было. Для
    /// оператора это выглядело как мигание, а «проблема исчезла» и
    /// «условие перестало подтверждаться» — разные утверждения.
    materialized: Option<Problem>,
}

/// Анализатор проблем.
pub struct Analyzer {
    cfg: RulesCfg,
    rules: Vec<Box<dyn Rule>>,
    state: HashMap<ProblemId, State>,
    events: Vec<Event>,
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer")
            .field("rules", &self.rules.len())
            .field("tracked", &self.state.len())
            .finish()
    }
}

impl Analyzer {
    #[must_use]
    pub fn new(cfg: RulesCfg) -> Self {
        Analyzer {
            cfg,
            rules: default_rules(),
            state: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Число отслеживаемых состояний. Нужно тестам: таблица не должна расти
    /// вместе с числом когда-либо существовавших процессов.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.state.len()
    }

    /// Оценивает правила и возвращает открытые проблемы.
    pub fn evaluate(
        &mut self,
        graph: &EntityGraph,
        latest: &LatestValues,
        history: &History,
        now: Timestamp,
    ) -> Vec<Problem> {
        let ctx = RuleCtx {
            graph,
            latest,
            history,
            now,
            cfg: &self.cfg,
        };

        let mut hits: Vec<(ProblemId, RuleHit)> = Vec::new();
        for rule in &self.rules {
            for hit in rule.evaluate(&ctx) {
                hits.push((
                    ProblemId {
                        rule: rule.id(),
                        entity: hit.entity,
                    },
                    hit,
                ));
            }
        }

        let enter_after = self.cfg.enter_after_ticks;
        let clear_after = self.cfg.clear_after_ticks;
        let mut problems: Vec<Problem> = Vec::new();
        let mut hit_ids: HashSet<ProblemId> = HashSet::new();

        for (id, hit) in hits {
            let _ = hit_ids.insert(id);
            let kind = graph.get(id.entity).map(|entity| entity.kind);
            let state = self.state.entry(id).or_insert_with(|| State {
                hysteresis: Hysteresis::default(),
                since: now,
                streak: 0,
                last_seen: now,
                materialized: None,
            });
            let was_open = state.hysteresis.open;
            let open = state
                .hysteresis
                .update(hit.enter, hit.clear, enter_after, clear_after);
            state.last_seen = now;

            if open {
                if !was_open {
                    state.since = now;
                    state.streak = 0;
                }
                state.streak = state.streak.saturating_add(1);
                let problem = Problem {
                    id,
                    severity: hit.severity,
                    entity_name: hit.entity_name.clone(),
                    title: hit.title.clone(),
                    summary: hit.summary,
                    evidence: hit.evidence,
                    since: state.since,
                    last_seen: now,
                    streak: state.streak,
                };
                state.materialized = Some(problem.clone());

                if !was_open {
                    // Идентичность сущности обязательна: имя ею не является
                    // (`dbus.socket` живёт и в системном, и в пользовательском
                    // менеджере, PID переиспользуются). Без `entity` событие
                    // невозможно связать с узлом графа.
                    let mut event =
                        Event::new(now, EventKind::ProblemOpened, hit.entity_name.clone())
                            .severity(hit.severity)
                            .detail(hit.title.clone())
                            .rule(id.rule);
                    if let Some(kind) = kind {
                        event = event.entity(id.entity, kind);
                    }
                    self.events.push(event);
                }

                problems.push(problem);
            } else if was_open {
                state.materialized = None;
                let mut event = Event::new(now, EventKind::ProblemClosed, hit.entity_name.clone())
                    .severity(Severity::Info)
                    .detail(hit.title.clone())
                    .rule(id.rule);
                if let Some(kind) = kind {
                    event = event.entity(id.entity, kind);
                }
                self.events.push(event);
            }
        }

        // Нейтральная зона: правило не выдало попадания, но проблема открыта.
        // Она обязана остаться в снимке, иначе оператор видит исчезновение
        // без события о закрытии.
        for (id, state) in &self.state {
            if hit_ids.contains(id) || !state.hysteresis.open {
                continue;
            }
            if let Some(problem) = state.materialized.clone() {
                problems.push(problem);
            }
        }

        // Сущность исчезла (процесс завершился, контейнер удалён) — состояние
        // больше не нужно. Без этой очистки таблица растёт вместе с churn.
        self.state.retain(|id, state| {
            let alive = graph.get(id.entity).is_some();
            alive && state.last_seen.as_millis() + 300_000 >= now.as_millis()
        });

        problems.sort_by_key(Problem::sort_key);
        problems
    }

    /// Забирает накопленные события открытия и закрытия проблем.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::config::Store as StoreConfig;
    use pulse_core::entity::{EntityKey, EntitySpec};
    use pulse_core::metric::ids;
    use pulse_core::SeriesKey;

    struct Harness {
        graph: EntityGraph,
        history: History,
        analyzer: Analyzer,
        now: u64,
    }

    impl Harness {
        fn new() -> Self {
            let graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
            Harness {
                graph,
                history: History::new(&StoreConfig::default()),
                analyzer: Analyzer::new(RulesCfg::default()),
                now: 1_000,
            }
        }

        /// Один такт с заданным значением PSI CPU у хоста.
        fn tick(&mut self, psi: f64) -> Vec<Problem> {
            self.now += 1_000;
            let at = Timestamp::from_millis(self.now);
            self.graph.begin_tick(at);
            let host = self.graph.host();
            self.graph.sample(host, ids::HOST_PSI_CPU_SOME_AVG10, psi);
            let batch = self.graph.end_tick();
            self.history.ingest(&batch);
            let mut latest = LatestValues::new();
            latest.set(SeriesKey::new(host, ids::HOST_PSI_CPU_SOME_AVG10), psi);
            self.analyzer
                .evaluate(&self.graph, &latest, &self.history, at)
        }
    }

    #[test]
    fn problem_opens_only_after_enter_streak() {
        let mut h = Harness::new();
        assert!(h.tick(0.9).is_empty(), "первое срабатывание не открывает");
        assert!(h.tick(0.9).is_empty(), "второе тоже");
        let problems = h.tick(0.9);
        assert_eq!(problems.len(), 1, "третье такт открывает проблему");
        assert_eq!(problems.first().map(|p| p.severity), Some(Severity::Crit));
    }

    #[test]
    fn problem_closes_only_after_clear_streak() {
        let mut h = Harness::new();
        for _ in 0..3 {
            let _ = h.tick(0.9);
        }
        // Значение упало ниже clear, но снятие требует clear_after_ticks тактов.
        for tick in 0..14 {
            let problems = h.tick(0.01);
            assert_eq!(problems.len(), 1, "такт {tick}: проблема ещё открыта");
        }
        assert!(h.tick(0.01).is_empty(), "после 15 тактов проблема закрыта");
    }

    #[test]
    fn events_are_emitted_on_transitions() {
        let mut h = Harness::new();
        for _ in 0..3 {
            let _ = h.tick(0.9);
        }
        let events = h.analyzer.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events.first().map(|e| e.kind),
            Some(EventKind::ProblemOpened)
        );
        assert!(events.first().and_then(|e| e.rule).is_some());

        for _ in 0..15 {
            let _ = h.tick(0.01);
        }
        let events = h.analyzer.take_events();
        assert!(events.iter().any(|e| e.kind == EventKind::ProblemClosed));
    }

    #[test]
    fn hysteresis_beats_naive_implementation_on_oscillation() {
        // Сигнал колеблется вокруг порога: наивная реализация переключалась бы
        // на каждом такте, гистерезис не должен открывать проблему вообще.
        let mut h = Harness::new();
        let mut transitions = 0;
        let mut open = false;
        let mut naive_transitions = 0;
        let mut naive_open = false;

        for i in 0..40 {
            let value = if i % 2 == 0 { 0.25 } else { 0.05 };
            let problems = h.tick(value);
            let now_open = !problems.is_empty();
            if now_open != open {
                transitions += 1;
                open = now_open;
            }
            // Наивная реализация: сравнение с одним порогом без гистерезиса.
            let naive_now = value > 0.20;
            if naive_now != naive_open {
                naive_transitions += 1;
                naive_open = naive_now;
            }
        }

        assert!(
            transitions < naive_transitions,
            "гистерезис ({transitions}) обязан переключаться реже наивной схемы ({naive_transitions})"
        );
        assert_eq!(
            transitions, 0,
            "дребезг вокруг порога подавляется полностью"
        );
    }

    #[test]
    fn state_of_dead_entities_is_released() {
        let mut h = Harness::new();
        // Процесс с высоким использованием дескрипторов.
        let mut latest = LatestValues::new();
        h.now += 1_000;
        let at = Timestamp::from_millis(h.now);
        h.graph.begin_tick(at);
        let host = h.graph.host();
        let pid = h.graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid: 42,
                    start_ticks: 1,
                },
                "leaky",
            )
            .parent(host),
        );
        h.graph.sample(pid, ids::PROC_FD_UTIL, 0.99);
        let batch = h.graph.end_tick();
        h.history.ingest(&batch);
        latest.set(SeriesKey::new(pid, ids::PROC_FD_UTIL), 0.99);
        let _ = h.analyzer.evaluate(&h.graph, &latest, &h.history, at);
        assert!(h.analyzer.tracked() > 0);

        // Процесс исчез: состояние правила должно освободиться.
        for _ in 0..3 {
            h.now += 1_000;
            let at = Timestamp::from_millis(h.now);
            h.graph.begin_tick(at);
            let batch = h.graph.end_tick();
            h.history.ingest(&batch);
            let _ = h
                .analyzer
                .evaluate(&h.graph, &LatestValues::new(), &h.history, at);
        }
        assert_eq!(
            h.analyzer.tracked(),
            0,
            "состояние мёртвых сущностей должно освобождаться"
        );
    }

    #[test]
    fn problem_identity_is_stable_across_ticks() {
        let mut h = Harness::new();
        for _ in 0..3 {
            let _ = h.tick(0.9);
        }
        let first = h.tick(0.9);
        let second = h.tick(0.9);
        let a = first.first().expect("проблема");
        let b = second.first().expect("проблема");
        assert_eq!(a.id, b.id, "проблема продолжается, а не создаётся заново");
        assert_eq!(a.since, b.since, "время начала не сдвигается");
        assert!(b.streak > a.streak, "длительность растёт");
    }
}
