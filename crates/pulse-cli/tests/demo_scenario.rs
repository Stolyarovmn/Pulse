//! Демонстрационный сценарий обязан работать через настоящий конвейер.
//!
//! Смысл демо — показать диагностику, которой на здоровом хосте нет. Если бы
//! оно рисовало проблемы напрямую, оно доказывало бы только само себя.
//! Поэтому тест прогоняет ту же цепочку, что и агент: коллекторы читают
//! `DemoFs`, граф собирает сущности, `Analyzer` применяет правила из
//! `Rules::default`, история хранит такты. Проверяется наблюдаемое: проблема
//! открывается в деградации и закрывается после восстановления.

// Тест обязан падать и указывать строку, поэтому `expect` здесь уместен;
// в рабочем коде запрет остаётся в силе (lints workspace).
#![allow(clippy::expect_used, clippy::panic)]

use pulse_collect::demo::{CYCLE_TICKS, DEGRADE_FROM};
use pulse_collect::{build_collectors, DemoFs, FsSource};
use pulse_core::snapshot::LatestValues;
use pulse_core::time::Timestamp;
use pulse_core::{CollectCtx, Config, EntityGraph};
use pulse_engine::Analyzer;
use pulse_store::History;
use std::sync::Arc;

/// Один шаг конвейера: сбор, граф, история, правила.
struct Pipeline {
    graph: EntityGraph,
    analyzer: Analyzer,
    history: History,
    collectors: Vec<Box<dyn pulse_core::Collector>>,
    tick: u64,
}

impl Pipeline {
    fn new(fs: Arc<dyn FsSource>) -> Self {
        let config = Config::default();
        let graph = EntityGraph::new("boot", "demo-host", Timestamp::from_millis(1_000));
        Pipeline {
            collectors: build_collectors(&config, fs),
            analyzer: Analyzer::new(config.rules.clone()),
            history: History::new(&config.store),
            graph,
            tick: 0,
        }
    }

    /// Прогоняет один такт и возвращает открытые проблемы.
    fn step(&mut self) -> Vec<pulse_core::problem::Problem> {
        self.tick += 1;
        let now = Timestamp::from_millis(1_000 + self.tick * 1_000);
        self.graph.begin_tick(now);
        for collector in &mut self.collectors {
            let mut ctx = CollectCtx::new(&mut self.graph, 1.0);
            let _ = collector.collect(&mut ctx);
        }
        let batch = self.graph.end_tick();
        let mut latest = LatestValues::default();
        for sample in &batch.samples {
            latest.set(sample.series, sample.value);
        }
        let problems = self
            .analyzer
            .evaluate(&self.graph, &latest, &self.history, now);
        self.history.ingest(&batch);
        problems
    }
}

/// Демо обязано довести правила до открытой проблемы: иначе оно не решает
/// задачу, ради которой появилось, — показать диагностику на здоровом хосте.
#[test]
fn demo_scenario_opens_and_closes_a_problem_through_real_rules() {
    // Такт задаётся явно, а не берётся из часов: иначе тест зависел бы от
    // скорости прогона.
    let fs = Arc::new(DemoScene::default());
    let mut pipeline = Pipeline::new(Arc::clone(&fs) as Arc<dyn FsSource>);

    // Такты идут подряд, без пропусков между циклами. Пропуск давал бы
    // приращение счётчиков сразу за десятки тактов, а правила считают долю
    // по приращению между соседними тактами (`d_throttled / d_periods`):
    // такая ложная дельта открывает проблему заново и сбрасывает выдержку
    // закрытия. Ошибка была бы тихой — тест «проходил» бы по запасу окна.
    let mut opened_at = None;
    let mut opened_kinds: Vec<String> = Vec::new();
    let mut still_open = usize::MAX;
    let last_tick = CYCLE_TICKS * 2 + DEGRADE_FROM;
    for tick in 0..=last_tick {
        fs.set_tick(tick);
        let problems = pipeline.step();
        if !problems.is_empty() && opened_at.is_none() {
            opened_at = Some(tick);
            opened_kinds = problems
                .iter()
                .map(|problem| problem.id.rule.to_string())
                .collect();
        }
        still_open = problems.len();
    }

    let opened_at = opened_at.expect(
        "за цикл сценария правила обязаны открыть хотя бы одну проблему: \
         иначе демо показывает «проблем нет» — то, из-за чего оно и понадобилось",
    );
    assert!(
        opened_at >= DEGRADE_FROM,
        "проблема обязана появиться в фазе деградации, а не в покое: такт {opened_at}"
    );
    assert!(
        !opened_kinds.is_empty(),
        "у открытой проблемы обязано быть имя правила"
    );
    // Последний такт прогона — конец фазы покоя, которая длиннее
    // `clear_after_ticks`: проблема обязана закрыться сама, по своему
    // условию снятия, а не по команде извне.
    assert_eq!(
        still_open, 0,
        "после восстановления и выдержки clear_after_ticks проблем быть не должно"
    );
}

/// Обёртка над [`DemoFs`] с управляемым номером такта.
///
/// Реальный `DemoFs` берёт такт из часов, и тест на нём зависел бы от
/// времени прогона. Здесь номер задаётся явно, а дерево остаётся то же.
#[derive(Debug, Default)]
struct DemoScene {
    scene: std::sync::Mutex<Option<(u64, DemoFs)>>,
    tick: std::sync::atomic::AtomicU64,
}

impl DemoScene {
    fn set_tick(&self, tick: u64) {
        self.tick.store(tick, std::sync::atomic::Ordering::Relaxed);
    }

    /// Выполняет чтение на дереве текущего такта.
    ///
    /// Дерево кешируется на такт: коллекторы делают десятки чтений за такт,
    /// и пересборка на каждое чтение превращала прогон в сотни тысяч сборок
    /// полного дерева.
    fn with_scene<T>(&self, f: impl FnOnce(&DemoFs) -> T) -> T {
        let tick = self.tick.load(std::sync::atomic::Ordering::Relaxed);
        let mut guard = self.scene.lock().expect("замок сцены");
        let needs_rebuild = guard.as_ref().is_none_or(|(built, _)| *built != tick);
        if needs_rebuild {
            *guard = Some((tick, DemoFs::at_tick(tick)));
        }
        let (_, scene) = guard.as_ref().expect("сцена собрана выше");
        f(scene)
    }
}

impl FsSource for DemoScene {
    fn read(&self, path: &std::path::Path, cap: usize) -> std::io::Result<Vec<u8>> {
        self.with_scene(|scene| scene.read(path, cap))
    }

    fn scan_dir(
        &self,
        path: &std::path::Path,
        visit: &mut dyn FnMut(&std::ffi::OsStr) -> bool,
    ) -> std::io::Result<()> {
        self.with_scene(|scene| scene.scan_dir(path, visit))
    }

    fn read_link(&self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        self.with_scene(|scene| scene.read_link(path))
    }

    fn inode(&self, path: &std::path::Path) -> std::io::Result<u64> {
        self.with_scene(|scene| scene.inode(path))
    }

    fn statfs(&self, path: &std::path::Path) -> std::io::Result<pulse_collect::fs::FsUsage> {
        self.with_scene(|scene| scene.statfs(path))
    }
}
