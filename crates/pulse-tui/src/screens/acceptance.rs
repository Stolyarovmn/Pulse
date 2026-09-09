//! Deterministic render/state acceptance для v0.9 (§194, §197).
//!
//! Проверяется текстовый Buffer, а не исходный код: compilation не доказывает
//! visual hierarchy. Interaction tests проходят через тот же [`App::dispatch`],
//! который получает события в runtime.

#![cfg(test)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use pulse_core::entity::{EntityKey, EntityKind, EntitySpec};
use pulse_core::graph::EntityGraph;
use pulse_core::metric::ids;
use pulse_core::problem::{Evidence, Problem, ProblemId, RuleId, Severity};
use pulse_core::sample::SeriesKey;
use pulse_core::snapshot::{AgentStats, LatestValues, Snapshot};
use pulse_core::time::Timestamp;
use pulse_core::{Event, EventKind, RelationKind};

use crate::app::{Action, App, Overlay, Screen};
use crate::layout::{max_useful_width, BlockKind, Pane};
use crate::theme::{Capability, Theme};

const REQUIRED_SIZES: &[(u16, u16)] = &[(60, 18), (80, 24), (120, 30), (180, 40), (200, 50)];

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift_tab() -> KeyEvent {
    KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)
}

fn draw(width: u16, height: u16, snapshot: &Snapshot, app: &mut App) -> Vec<String> {
    draw_with(
        width,
        height,
        snapshot,
        app,
        &Theme::with_capability(Capability::TrueColor),
    )
}

fn draw_with(
    width: u16,
    height: u16,
    snapshot: &Snapshot,
    app: &mut App,
    theme: &Theme,
) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| super::render(frame, snapshot, app, theme, None))
        .expect("render");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|row| {
            (0..width)
                .map(|col| {
                    buffer
                        .cell((col, row))
                        .and_then(|cell| cell.symbol().chars().next())
                        .unwrap_or(' ')
                })
                .collect()
        })
        .collect()
}

fn joined(width: u16, height: u16, snapshot: &Snapshot, app: &mut App) -> String {
    draw(width, height, snapshot, app).join("\n")
}

/// Базовый снимок: baseline, logical service, workers, rates и relation graph.
fn healthy() -> Snapshot {
    let mut graph = EntityGraph::new("boot", "asuspc", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(2_000));
    let host = graph.host();
    let system = graph
        .upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 10 }, "system.slice").parent(host));
    let cgroup = graph.upsert(
        EntitySpec::new(EntityKey::Cgroup { cgroup_id: 11 }, "angie.service").parent(system),
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
    let disk =
        graph.upsert(EntitySpec::new(EntityKey::Disk { name: "sda".into() }, "sda").parent(host));
    graph.relate(cgroup, RelationKind::BackedBy, disk);

    let mut latest = LatestValues::new();
    latest.set(SeriesKey::new(host, ids::HOST_CPU_UTIL), 0.03);
    latest.set(SeriesKey::new(host, ids::HOST_MEM_UTIL), 0.11);
    latest.set(SeriesKey::new(host, ids::HOST_UPTIME), 90_000.0);
    latest.set(
        SeriesKey::new(unit, ids::CG_MEM_CURRENT),
        376.0 * 1024.0 * 1024.0,
    );
    latest.set(SeriesKey::new(cgroup, ids::CG_CPU_CORES), 0.28);
    latest.set(SeriesKey::new(cgroup, ids::CG_IO_READ_THROUGHPUT), 4096.0);
    latest.set(
        SeriesKey::new(cgroup, ids::CG_IO_READ_BYTES),
        9.1 * 1024.0 * 1024.0 * 1024.0,
    );
    latest.set(
        SeriesKey::new(host, ids::NETIF_RX_THROUGHPUT),
        207.0 * 1024.0,
    );
    latest.set(SeriesKey::new(host, ids::NETIF_TX_THROUGHPUT), 7.2 * 1024.0);

    for pid in 100..114_i32 {
        let process = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid,
                    start_ticks: 5,
                },
                "angie",
            )
            .parent(cgroup),
        );
        latest.set(
            SeriesKey::new(process, ids::PROC_RSS),
            49.0 * 1024.0 * 1024.0,
        );
    }

    let pulse = graph.upsert(
        EntitySpec::new(
            EntityKey::Process {
                pid: 999,
                start_ticks: 3,
            },
            "pulse",
        )
        .parent(host),
    );
    latest.set(SeriesKey::new(pulse, ids::PROC_RSS), 6.0 * 1024.0 * 1024.0);

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

/// Снимок, приближённый к реальному хосту efgis-dockerdev.
///
/// Здесь воспроизведены именно те условия, при которых экраны деградировали:
/// две тысячи технических сущностей, десятки per-cpu воркеров ядра с
/// непрерывным переименованием, контейнеры с хешами вместо имён и один
/// по-настоящему нагруженный сервис.
fn production_like() -> Snapshot {
    let mut graph = EntityGraph::new("boot", "efgis-dockerdev", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(2_000));
    let host = graph.host();
    let mut latest = LatestValues::new();
    latest.set(SeriesKey::new(host, ids::HOST_CPU_UTIL), 0.39);
    latest.set(SeriesKey::new(host, ids::HOST_MEM_UTIL), 0.65);
    latest.set(SeriesKey::new(host, ids::HOST_UPTIME), 11_000_000.0);

    let system = graph
        .upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 10 }, "system.slice").parent(host));
    latest.set(SeriesKey::new(system, ids::CG_CPU_CORES), 18.0);
    latest.set(
        SeriesKey::new(system, ids::CG_MEM_CURRENT),
        47.2 * 1024.0 * 1024.0 * 1024.0,
    );

    // Нагруженный сервис: 15 ядер и 14.7 GiB. Он обязан быть первым в Overview.
    let docker_cgroup = graph.upsert(
        EntitySpec::new(EntityKey::Cgroup { cgroup_id: 11 }, "docker.service").parent(system),
    );
    let docker = graph.upsert(
        EntitySpec::new(
            EntityKey::Unit {
                name: "docker.service".into(),
            },
            "docker.service",
        )
        .parent(docker_cgroup),
    );
    graph.relate(docker_cgroup, RelationKind::OwnedBy, docker);
    latest.set(SeriesKey::new(docker_cgroup, ids::CG_CPU_CORES), 15.0);
    latest.set(
        SeriesKey::new(docker_cgroup, ids::CG_MEM_CURRENT),
        14_700.0 * 1024.0 * 1024.0,
    );
    for pid in 1_000..1_075_i32 {
        let _ = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid,
                    start_ticks: 7,
                },
                "dockerd",
            )
            .parent(docker_cgroup),
        );
    }

    // Контейнеры названы хешами: узнаваемое имя приходит от главного процесса.
    const CONTAINERS: [(&str, &str, f64); 4] = [
        ("4368590e0411", "postgres", 2.1),
        ("a59afa2bbe21", "java", 1.7),
        ("1863014a5505", "redis-server", 1.5),
        ("08adcc62f976", "nginx", 1.2),
    ];
    for (index, (id, process, gib)) in CONTAINERS.iter().enumerate() {
        let cgroup_id = 100 + index as u64;
        // На реальном хосте контейнеры живут в собственных scope под
        // system.slice, а не внутри cgroup самого docker.service.
        let cgroup =
            graph.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id }, *id).parent(system));
        let container = graph.upsert(
            EntitySpec::new(
                EntityKey::Container {
                    id: (*id).into(),
                    runtime: pulse_core::Runtime::Docker,
                },
                *id,
            )
            .parent(cgroup),
        );
        graph.relate(cgroup, RelationKind::OwnedBy, container);
        let pid = 2_000 + i32::try_from(index).unwrap_or(0) * 10;
        let main = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid,
                    start_ticks: 9,
                },
                *process,
            )
            .parent(cgroup),
        );
        latest.set(
            SeriesKey::new(main, ids::PROC_RSS),
            gib * 1024.0 * 1024.0 * 1024.0,
        );
        latest.set(
            SeriesKey::new(cgroup, ids::CG_MEM_CURRENT),
            gib * 1024.0 * 1024.0 * 1024.0,
        );
    }

    // Потоки ядра: именно они забивали все экраны.
    let mut kworkers = Vec::new();
    for cpu in 0..64_i32 {
        let name = format!("kworker/{cpu}:0-events");
        let worker = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid: 5_000 + cpu,
                    start_ticks: 2,
                },
                name.clone(),
            )
            .parent(host),
        );
        latest.set(SeriesKey::new(worker, ids::PROC_CPU_CORES), 0.01);
        kworkers.push((worker, name));
    }

    // Наполнение до production-масштаба: 2000 технических сущностей.
    for pid in 10_000..11_800_i32 {
        let _ = graph.upsert(
            EntitySpec::new(
                EntityKey::Process {
                    pid,
                    start_ticks: 4,
                },
                format!("worker-{pid}"),
            )
            .parent(system),
        );
    }

    let at = Timestamp::from_millis(2_000);
    let mut events = vec![
        Event::new(at, EventKind::ObservationStarted, "efgis-dockerdev")
            .detail("baseline: 2012 entities; processes 1943"),
    ];
    // Пять переименований на каждый воркер: 320 сырых событий за такт.
    for round in 0..5 {
        for (worker, name) in &kworkers {
            events.push(
                Event::new(at, EventKind::MetadataChanged, name.clone())
                    .entity(*worker, EntityKind::Process)
                    .detail(format!("name: {name} -> kworker/{round}:0-mm_percpu_wq")),
            );
        }
    }
    events.push(
        Event::new(at, EventKind::Restarted, "docker.service")
            .entity(docker, EntityKind::Unit)
            .detail("logical entity restarted"),
    );

    let _ = graph.end_tick();
    Snapshot::build(
        &graph,
        latest,
        vec![],
        events,
        AgentStats::default(),
        "efgis-dockerdev",
        "boot",
    )
}

fn critical() -> Snapshot {
    let mut snapshot = healthy();
    let unit = snapshot
        .entities_of_kind(EntityKind::Unit)
        .next()
        .map(|entity| entity.id)
        .expect("unit fixture");
    snapshot.problems.push(Problem {
        id: ProblemId {
            rule: RuleId("memory.pressure"),
            entity: unit,
        },
        severity: Severity::Crit,
        entity_name: "angie.service".to_string(),
        title: "memory pressure 38%".to_string(),
        summary: "measured memory pressure".to_string(),
        evidence: vec![Evidence::new("PSI memory full", "38%").with_threshold("CRIT > 25%")],
        since: Timestamp::from_millis(1_500),
        last_seen: Timestamp::from_millis(2_000),
        streak: 5,
    });
    snapshot.events.push(
        Event::new(
            Timestamp::from_millis(1_700),
            EventKind::Restarted,
            "docker-desktop",
        )
        .detail("restart observed"),
    );
    snapshot
}

/// Циклический relation graph A -> B -> C -> A для Inspector.
fn cyclic() -> (Snapshot, EntityKey, EntityKey, EntityKey) {
    let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(2_000));
    let host = graph.host();
    let a_key = EntityKey::Cgroup { cgroup_id: 1 };
    let b_key = EntityKey::Cgroup { cgroup_id: 2 };
    let c_key = EntityKey::Cgroup { cgroup_id: 3 };
    let a = graph.upsert(EntitySpec::new(a_key.clone(), "A").parent(host));
    let b = graph.upsert(EntitySpec::new(b_key.clone(), "B").parent(a));
    let c = graph.upsert(EntitySpec::new(c_key.clone(), "C").parent(b));
    graph.relate(c, RelationKind::RunsIn, a);
    let batch = graph.end_tick();
    (
        Snapshot::build(
            &graph,
            LatestValues::new(),
            vec![],
            batch.events,
            AgentStats::default(),
            "host",
            "boot",
        ),
        a_key,
        b_key,
        c_key,
    )
}

fn follow_to(app: &mut App, snapshot: &Snapshot, key: &EntityKey) {
    let targets = app.inspector_relations(snapshot);
    let position = targets
        .iter()
        .position(|target| &target.key == key)
        .expect("visible relation target");
    if let Some(inspector) = &mut app.inspector {
        inspector.relation_selected = position;
        inspector.relation_len = targets.len();
    }
    let _ = app.dispatch(key_event_enter(), snapshot);
}

fn key_event_enter() -> KeyEvent {
    key(KeyCode::Enter)
}

#[test]
fn overview_180_matches_v09_structure() {
    let snapshot = healthy();
    let mut app = App::default();
    let lines = draw(180, 40, &snapshot, &mut app);
    let text = lines.join("\n");
    for required in [
        "LIVE ●",
        "OVERVIEW",
        "STATE",
        "ATTENTION",
        "SIGNALS",
        "RECENT CHANGES",
        "RELEVANT ENTITIES",
        "SELECTED /",
        "WHY",
        "RELEVANCE",
        "sort:relevance",
        "view:logical",
    ] {
        assert!(text.contains(required), "missing {required}: {text}");
    }
    assert!(!text.contains("LIVE ○"));
    assert!(!text.contains("Enter → baseline snapshot"));
    assert!(!text.contains("Enter -> baseline snapshot"));
    assert!(!text.contains("created angie"));
    assert!(!text.contains("9.1 GiB/s"), "counter rendered as rate");

    // Fixed 9x7 field with background dots.
    let glyph_rows = lines
        .iter()
        .filter(|line| {
            line.chars()
                .take(17)
                .filter(|ch| "·○●◉◇◆▲×".contains(*ch))
                .count()
                == 9
        })
        .count();
    assert_eq!(glyph_rows, 7, "large glyph must be exact 9x7");

    // Selected pane is separated from bounded entity pane.
    let lower_title = lines
        .iter()
        .find(|line| line.contains("RELEVANT ENTITIES") && line.contains("SELECTED /"))
        .expect("two lower panes on one row");
    let selected_byte = lower_title.find("SELECTED /").expect("selected column");
    let selected_col = lower_title[..selected_byte].chars().count();
    assert!(
        (80..=100).contains(&selected_col),
        "bounded entity pane: {selected_col}"
    );
}

#[test]
fn relevant_rows_are_explained_and_workers_folded() {
    let snapshot = healthy();
    let mut app = App::default();
    let text = joined(180, 40, &snapshot, &mut app);
    assert!(text.contains("WHY"));
    assert!(text.contains("angie.service"));
    assert!(!text
        .lines()
        .any(|line| line.trim_start().starts_with("angie ")));
    assert!(text.contains("memory") || text.contains("system") || text.contains("cpu"));
}

#[test]
fn baseline_is_one_compact_event_without_inline_hint() {
    let snapshot = healthy();
    let mut app = App::default();
    let text = joined(180, 40, &snapshot, &mut app);
    assert_eq!(text.matches("PULSE observation started").count(), 1);
    let baseline = text
        .lines()
        .find(|line| line.contains("PULSE observation started"))
        .expect("baseline row");
    assert!(baseline.contains("baseline"));
    assert!(!baseline.contains("cgroups"));
    assert!(!text.contains("Enter →"));
}

#[test]
fn problems_empty_is_intentional() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Problems;
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(text.contains("NO ACTIVE PROBLEMS"));
    assert!(text.contains("STATE"));
    assert!(text.contains("RECENT RESOLVED"));
    assert!(text.contains("PULSE started"));
    assert!(!text.contains("stable for"));
}

#[test]
fn entities_defaults_to_logical_with_explicit_state() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Entities;
    let text = joined(180, 40, &snapshot, &mut app);
    assert!(text.contains("sort:MEM↓"));
    assert!(text.contains("filter:all"));
    assert!(text.contains("view:logical"));
    assert!(text.contains("angie.service"));
    assert!(!text
        .lines()
        .any(|line| line.trim_start().starts_with("angie ")));
    assert!(text.contains("ENTITY LIST"));
    assert!(text.contains("PREVIEW /"));
}

#[test]
fn search_overlay_is_visible_and_owns_input() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = draw(120, 30, &snapshot, &mut app);
    app.overview.selected = usize::from(app.overview.list_len > 1);
    let previous_selection = app.overview.selected;
    let _ = app.dispatch(key(KeyCode::Char('/')), &snapshot);
    let opened = joined(120, 30, &snapshot, &mut app);
    assert!(opened.contains("SEARCH / █"));

    for ch in "angie".chars() {
        let _ = app.dispatch(key(KeyCode::Char(ch)), &snapshot);
    }
    // `4`, `e`, `j` while modal are text, never screen/navigation shortcuts.
    let screen = app.screen;
    let selection = app.overview.selected;
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(text.contains("SEARCH / angie█"));
    assert!(text.contains("matches"));
    assert_eq!(app.screen, screen);
    assert_eq!(app.overview.selected, selection);

    let _ = app.dispatch(key(KeyCode::Esc), &snapshot);
    assert!(app.overlay.is_none());
    assert_eq!(app.screen, Screen::Overview);
    assert_eq!(app.overview.selected, previous_selection);
}

#[test]
fn search_enter_applies_entities_filter_and_flow_opens_inspector() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('/')), &snapshot);
    for ch in "angie".chars() {
        let _ = app.dispatch(key(KeyCode::Char(ch)), &snapshot);
    }
    let _ = app.dispatch(key(KeyCode::Enter), &snapshot);
    assert_eq!(app.screen, Screen::Entities);
    assert_eq!(app.entities.search_filter.as_deref(), Some("angie"));
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(text.contains("angie.service"));
    let _ = app.dispatch(key(KeyCode::Enter), &snapshot);
    assert!(app.inspector.is_some());
    let _ = app.dispatch(key(KeyCode::Esc), &snapshot);
    assert!(app.inspector.is_none());
    assert_eq!(app.screen, Screen::Entities);
    assert_eq!(app.entities.search_filter.as_deref(), Some("angie"));
}

#[test]
fn applied_search_is_visible_as_filter_state() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('/')), &snapshot);
    for ch in "angie".chars() {
        let _ = app.dispatch(key(KeyCode::Char(ch)), &snapshot);
    }
    let _ = app.dispatch(key(KeyCode::Enter), &snapshot);
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(
        text.contains("filter:search(\"angie\")"),
        "applied search must be visible filter state: {text}"
    );
    let claims_all = text.contains("filter:all");
    assert!(
        !claims_all,
        "filter:all lies while search is active: {text}"
    );
    let header = text
        .lines()
        .find(|l| l.contains("ENTITIES") && l.contains("sort:"))
        .unwrap_or_default();
    assert!(
        header.contains('/'),
        "visible/total count expected: {header}"
    );
}

#[test]
fn repeated_entity_shortcuts_are_idempotent() {
    let snapshot = healthy();
    let mut app = App::default();
    for code in [KeyCode::Char('E'), KeyCode::Char('e'), KeyCode::Char('3')] {
        let _ = app.dispatch(key(code), &snapshot);
        assert_eq!(app.screen, Screen::Entities);
        assert!(app.inspector.is_none());
    }
    app.entities.selected = 2;
    let _ = app.dispatch(key(KeyCode::Char('e')), &snapshot);
    assert_eq!(app.entities.selected, 2, "same screen shortcut is no-op");
}

#[test]
fn tab_cycles_only_visible_panes_and_resize_repairs_focus() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = draw(180, 40, &snapshot, &mut app);
    assert_eq!(app.visible_panes(), &[Pane::Primary, Pane::Inspector]);
    let screen = app.screen;
    let _ = app.dispatch(key(KeyCode::Tab), &snapshot);
    assert_eq!(app.pane, Pane::Inspector);
    assert_eq!(app.screen, screen);
    let _ = app.dispatch(shift_tab(), &snapshot);
    assert_eq!(app.pane, Pane::Primary);

    app.pane = Pane::Inspector;
    let _ = draw(80, 24, &snapshot, &mut app);
    assert_eq!(app.visible_panes(), &[Pane::Primary]);
    assert_eq!(
        app.pane,
        Pane::Primary,
        "hidden preview cannot retain focus"
    );
    let _ = app.dispatch(key(KeyCode::Tab), &snapshot);
    assert_eq!(app.screen, screen);
    assert_eq!(app.pane, Pane::Primary);
}

#[test]
fn enter_never_uses_hidden_preview_selection() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Entities;
    let _ = draw(180, 40, &snapshot, &mut app);
    app.pane = Pane::Inspector;
    app.entities.preview_relation_selected = 0;
    let _ = draw(80, 24, &snapshot, &mut app); // hides preview and repairs focus
    assert_eq!(app.pane, Pane::Primary);
    let rows = crate::rows::entity_rows(&snapshot, &app);
    let folded = crate::fold::fold(&snapshot, &rows);
    let expected = snapshot
        .entity(folded[app.entities.selected].row.id)
        .expect("selected entity")
        .key
        .clone();
    let _ = app.dispatch(key(KeyCode::Enter), &snapshot);
    assert_eq!(
        app.inspector.as_ref().map(|session| &session.current),
        Some(&expected),
        "Enter acted on visible list, not retained hidden preview relation"
    );
}

#[test]
fn inspector_cycle_is_unreachable_by_drilling() {
    // Фикстура содержит замыкающую связь C -> A между равными объектами.
    // Раньше по ней можно было ходить кругами; теперь цепочка направленная,
    // и такой шаг просто не является звеном.
    let (snapshot, a, b, c) = cyclic();
    let mut app = App::default();
    app.screen = Screen::Entities;
    app.entities.selected = 2;
    app.open_inspector(a.clone());
    follow_to(&mut app, &snapshot, &b);
    follow_to(&mut app, &snapshot, &c);
    assert_eq!(app.inspector.as_ref().map(|s| s.path.len()), Some(3));

    let targets = app.inspector_relations(&snapshot);
    assert!(
        targets.iter().all(|target| target.key != a),
        "возврат в начало цепочки не предлагается: {:?}",
        targets.iter().map(|t| t.label).collect::<Vec<_>>()
    );
    assert!(
        targets.is_empty(),
        "C завершает цепочку: глубже него объектов нет"
    );
}

/// Защита пути остаётся на месте: повторный вход по canonical key усекает путь.
#[test]
fn inspector_path_truncates_on_repeated_key() {
    let (snapshot, a, b, _) = cyclic();
    let mut app = App::default();
    app.open_inspector(a.clone());
    follow_to(&mut app, &snapshot, &b);
    assert_eq!(app.inspector.as_ref().map(|s| s.path.len()), Some(2));

    // Прямой повторный вход в уже посещённый объект (боковой переход из
    // палитры или поиска) не наращивает путь, а усекает его до первого входа.
    if let Some(session) = &mut app.inspector {
        session.follow(a.clone());
    }
    let session = app.inspector.as_ref().expect("inspector remains open");
    assert_eq!(session.path, vec![a.clone()]);
    assert_eq!(session.current, a);
}

#[test]
fn inspector_esc_walks_unique_path_then_restores_origin() {
    let (snapshot, a, b, c) = cyclic();
    let mut app = App::default();
    app.screen = Screen::Entities;
    app.entities.selected = 1;
    app.entities.kind_filter = Some(EntityKind::Cgroup);
    app.pane = Pane::Primary;
    app.open_inspector(a);
    follow_to(&mut app, &snapshot, &b);
    follow_to(&mut app, &snapshot, &c);
    let _ = app.dispatch(key(KeyCode::Esc), &snapshot);
    assert_eq!(app.inspector.as_ref().map(|s| s.path.len()), Some(2));
    let _ = app.dispatch(key(KeyCode::Esc), &snapshot);
    assert_eq!(app.inspector.as_ref().map(|s| s.path.len()), Some(1));
    let _ = app.dispatch(key(KeyCode::Esc), &snapshot);
    assert!(app.inspector.is_none());
    assert_eq!(app.screen, Screen::Entities);
    assert_eq!(app.entities.selected, 1);
    assert_eq!(app.entities.kind_filter, Some(EntityKind::Cgroup));
    assert_eq!(app.pane, Pane::Primary);
}

#[test]
fn inspector_never_renders_empty_top_level_page() {
    let snapshot = healthy();
    let mut app = App::default();
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(!text.contains("entity not selected"));
    assert!(!text.contains("сущность не выбрана"));
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    assert_eq!(app.screen, Screen::Timeline, "4 is Timeline, not Inspector");
}

#[test]
fn help_is_overlay_and_restores_exact_state() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Entities;
    app.entities.selected = 2;
    app.pane = Pane::Primary;
    let _ = app.dispatch(key(KeyCode::Char('?')), &snapshot);
    assert_eq!(app.overlay, Some(Overlay::Help));
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(text.contains("HELP / NAVIGATION"));
    assert!(text.contains("Shift+Tab"));
    assert!(text.contains("Timeline / Time Machine"));
    assert!(!text.contains("1 … 5"));
    let _ = app.dispatch(key(KeyCode::Esc), &snapshot);
    assert!(app.overlay.is_none());
    assert_eq!(app.screen, Screen::Entities);
    assert_eq!(app.entities.selected, 2);
    assert_eq!(app.pane, Pane::Primary);
}

#[test]
fn footer_is_hotkey_legend_not_tab_bar() {
    let snapshot = healthy();
    let mut app = App::default();
    let lines = draw(180, 40, &snapshot, &mut app);
    let footer = lines.last().expect("footer");
    for hint in [
        "[1]Overview",
        "[2]Problems",
        "[3]Entities",
        "[4]Timeline",
        "[/]Search",
        "[?]Help",
    ] {
        assert!(footer.contains(hint), "missing {hint}: {footer}");
    }
    assert!(!footer.contains("P problems"));
}

#[test]
fn timeline_fresh_baseline_looks_like_time_machine() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Timeline;
    app.pane = Pane::Story;
    let text = joined(180, 40, &snapshot, &mut app);
    for required in [
        "TIME MACHINE",
        "observation started",
        "NOW",
        "STATE",
        "CPU",
        "MEM",
        "IO",
        "collecting history",
        "STORY",
        "SNAPSHOT /",
        "BASELINE",
    ] {
        assert!(text.contains(required), "missing {required}: {text}");
    }
    assert!(
        !text.contains(": raw events"),
        "raw view is not default label"
    );
    assert!(!text.contains("CPU   ─"), "no fake metric line");
    assert!(!text.contains("MEM   ─"), "no fake metric line");
}

#[test]
fn timeline_scrub_story_snapshot_and_live_flow() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Timeline;
    app.pane = Pane::Story;
    let _ = draw(180, 40, &snapshot, &mut app);
    let _ = app.dispatch(key(KeyCode::Left), &snapshot);
    assert!(app.timeline.time_cursor.is_some());
    let _ = app.dispatch(key(KeyCode::Tab), &snapshot);
    assert_eq!(app.pane, Pane::Inspector);
    let _ = app.dispatch(key(KeyCode::Char('F')), &snapshot);
    // F is pane-local Timeline action regardless of visible Timeline pane.
    assert!(app.timeline.time_cursor.is_none());
}

#[test]
fn selected_pane_follows_section_190_labels() {
    let snapshot = healthy();
    let mut app = App::default();
    let text = joined(180, 40, &snapshot, &mut app);
    let line = text
        .lines()
        .find(|l| l.contains("SELECTED /"))
        .expect("selected pane");
    assert!(line.contains("SELECTED /"), "{line}");

    // §190 фиксирует порядок и регистр подписей панели.
    let mut order = Vec::new();
    for label in ["STATE", "RELEVANCE", "CPU", "MEM", "RELATIONS", "RECENT"] {
        let at = text
            .find(&format!("{label}       "))
            .or_else(|| text.find(&format!("{label}   ")))
            .unwrap_or_else(|| panic!("label {label} missing in selected pane:\n{text}"));
        order.push((label, at));
    }
    let mut sorted = order.clone();
    sorted.sort_by_key(|(_, at)| *at);
    assert_eq!(
        order.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        sorted.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        "порядок подписей обязан совпадать с §190"
    );

    // RELEVANCE - одно значение, а не перечень совпавших признаков.
    assert_eq!(
        text.matches("RELEVANCE").count(),
        1,
        "RELEVANCE обязан быть одной строкой:\n{text}"
    );
    assert!(
        !text.contains("reason      "),
        "перечень reason-строк удалён в пользу §190:\n{text}"
    );
}

#[test]
fn header_rule_is_full_width_on_every_screen() {
    let snapshot = healthy();
    for screen in [
        Screen::Overview,
        Screen::Problems,
        Screen::Entities,
        Screen::Timeline,
    ] {
        for &(w, h) in REQUIRED_SIZES {
            let mut app = App::default();
            app.screen = screen;
            let lines = draw(w, h, &snapshot, &mut app);
            let rule = lines.get(1).cloned().unwrap_or_default();
            let trimmed = rule.trim_end();
            assert_eq!(
                trimmed.chars().count(),
                usize::from(w),
                "правило под шапкой обязано занимать всю ширину ({screen:?} {w}x{h}): {rule}"
            );
            assert!(
                trimmed.chars().all(|c| c == '\u{2500}'),
                "строка под шапкой обязана быть правилом ({screen:?} {w}x{h}): {rule}"
            );
        }
    }
}

#[test]
fn help_explains_state_alphabet() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('?')), &snapshot);
    let text = joined(120, 30, &snapshot, &mut app);
    assert!(text.contains("STATE ALPHABET"), "{text}");
    // §120: каждый символ алфавита обязан иметь расшифровку, иначе главный
    // экран нечитаем.
    for symbol in [
        '\u{b7}', '\u{25cb}', '\u{25cf}', '\u{25c9}', '\u{25c7}', '\u{25c6}', '\u{25b2}', '\u{d7}',
    ] {
        assert!(
            text.contains(symbol),
            "символ {symbol:?} обязан быть в легенде:\n{text}"
        );
    }
    assert!(text.contains("normal active mass"), "{text}");
    assert!(text.contains("lost / failed function"), "{text}");
}

#[test]
fn help_is_single_language() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('?')), &snapshot);
    let text = joined(120, 40, &snapshot, &mut app);
    let cyrillic: Vec<char> = text
        .chars()
        .filter(|c| ('\u{410}'..='\u{44f}').contains(c))
        .collect();
    assert!(
        cyrillic.is_empty(),
        "справка обязана быть на языке mockups: {cyrillic:?}"
    );
}

// --- P1: семантическое подавление шума на production-like снимке -----------

#[test]
fn kernel_worker_churn_is_absent_from_recent_changes() {
    let snapshot = production_like();
    assert!(
        snapshot.suppressed_noise >= 300,
        "фикстура обязана содержать churn: {}",
        snapshot.suppressed_noise
    );
    let mut app = App::default();
    let text = joined(180, 40, &snapshot, &mut app);
    assert!(
        !text.contains("kworker"),
        "Overview не имеет права показывать переименования потоков ядра:\n{text}"
    );
    assert!(
        text.contains("docker.service restarted"),
        "операционное изменение обязано остаться:\n{text}"
    );
    assert!(
        text.contains("routine observations suppressed"),
        "оператор обязан узнать, что сырой поток существует:\n{text}"
    );
}

#[test]
fn kernel_worker_churn_is_absent_from_incident_story() {
    let snapshot = production_like();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    let story = joined(180, 40, &snapshot, &mut app);
    assert!(story.contains("STORY"), "ожидался экран Timeline:\n{story}");
    assert!(
        !story.contains("kworker"),
        "Story обязана быть Time Machine, а не kernel event viewer:\n{story}"
    );
    assert!(
        story.contains("docker.service"),
        "перезапуск сервиса обязан быть в Story:\n{story}"
    );
}

#[test]
fn raw_event_stream_still_exposes_kernel_churn() {
    let snapshot = production_like();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    app.timeline.raw_events = true;
    let raw = joined(180, 40, &snapshot, &mut app);
    assert!(raw.contains("RAW EVENTS"), "{raw}");
    assert!(
        raw.contains("kworker"),
        "сырой поток обязан оставаться доступным эксперту:\n{raw}"
    );
    assert!(
        !snapshot.events.is_empty(),
        "сырые данные не уничтожаются семантической фильтрацией"
    );
}

#[test]
fn story_respects_information_budget() {
    let snapshot = production_like();
    assert!(
        snapshot.meaningful.len() <= pulse_core::snapshot::STORY_BUDGET,
        "бюджет Story нарушен: {}",
        snapshot.meaningful.len()
    );
    assert!(
        snapshot.meaningful.len() >= 2,
        "значимые события не должны исчезнуть целиком: {}",
        snapshot.meaningful.len()
    );
}

#[test]
fn relevance_is_not_dominated_by_changed() {
    let snapshot = production_like();
    let mut app = App::default();
    let text = joined(180, 40, &snapshot, &mut app);
    let why_rows: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("GiB") || line.contains("MiB"))
        .collect();
    assert!(!why_rows.is_empty(), "таблица сущностей пуста:\n{text}");
    let changed_rows = why_rows
        .iter()
        .filter(|line| line.contains("changed"))
        .count();
    assert!(
        changed_rows * 2 <= why_rows.len(),
        "`changed` снова доминирует в WHY: {changed_rows} из {}\n{text}",
        why_rows.len()
    );

    // Нагруженный сервис обязан быть выше kernel worker'ов.
    let rows = crate::fold::relevant(&snapshot, &crate::rows::entity_rows(&snapshot, &app), 12);
    let first = rows.first().expect("непустой список значимых");
    assert!(
        !first.row.name.starts_with("kworker"),
        "воркер ядра не может быть самой значимой сущностью: {}",
        first.row.name
    );
}

#[test]
fn logical_view_prefers_human_names_and_keeps_ids() {
    let snapshot = production_like();
    let app = App::default();
    let rows = crate::fold::fold(&snapshot, &crate::rows::entity_rows(&snapshot, &app));
    let renamed = rows
        .iter()
        .find(|row| row.technical_id.as_deref() == Some("4368590e0411"))
        .expect("контейнер с хешем обязан быть переименован");
    assert_eq!(
        renamed.row.name, "postgres",
        "logical view обязан показывать узнаваемое имя"
    );
    assert!(
        crate::fold::is_opaque_id("4368590e0411"),
        "хеш обязан распознаваться как технический идентификатор"
    );
    assert!(
        !crate::fold::is_opaque_id("docker.service"),
        "имя сервиса не является хешем"
    );
}

#[test]
fn state_river_is_not_driven_by_kernel_churn() {
    let snapshot = production_like();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    let text = joined(180, 40, &snapshot, &mut app);
    let river = text
        .lines()
        .find(|line| line.trim_start().starts_with("STATE"))
        .expect("полоса состояния");
    for abnormal in ['\u{25c7}', '\u{25c6}', '\u{25b2}', '\u{d7}'] {
        assert!(
            !river.contains(abnormal),
            "320 переименований воркеров не делают состояние ненормальным: {river:?}"
        );
    }
}

#[test]
fn event_groups_are_expandable_with_first_last_and_count() {
    let mut snapshot = production_like();
    // Четырнадцать перезапусков контейнеров — один факт с объёмом.
    let mut events = Vec::new();
    for index in 0..14_u64 {
        events.push(
            Event::new(
                Timestamp::from_millis(3_000 + index * 1_000),
                EventKind::Restarted,
                format!("container-{index}"),
            )
            .entity(snapshot.host, EntityKind::Container),
        );
    }
    let summary = pulse_core::semantic::summarize(&events, 20);
    snapshot.meaningful = summary.items;

    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    let text = joined(180, 40, &snapshot, &mut app);
    assert!(
        text.contains("\u{d7}14"),
        "группа обязана показывать объём:\n{text}"
    );
    assert!(
        text.contains("count") && text.contains("first") && text.contains("last"),
        "группа обязана раскрываться в детали:\n{text}"
    );
}

#[test]
fn aggregated_cpu_names_its_scope_and_units() {
    let snapshot = production_like();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('3')), &snapshot);
    let text = joined(180, 40, &snapshot, &mut app);
    assert!(
        text.contains("18.0c") || text.contains("15.0c"),
        "CPU обязан иметь единицу:\n{text}"
    );
    assert!(
        text.contains("descendants"),
        "область агрегации обязана быть названа:\n{text}"
    );
    // Число без единицы запрещено: `CPU 18` двусмысленно.
    let cpu_cells: Vec<&str> = text.lines().filter(|line| line.contains("CPU ")).collect();
    assert!(!cpu_cells.is_empty(), "{text}");
}

#[test]
fn healthy_timeline_stays_visually_calm() {
    let snapshot = healthy();
    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    let lines = draw(180, 40, &snapshot, &mut app);
    let story_rows = lines
        .iter()
        .skip_while(|line| !line.contains("STORY"))
        .skip(1)
        .filter(|line| line.contains(':') && line.contains("00:00"))
        .count();
    assert!(
        story_rows <= 5,
        "здоровый старт обязан выглядеть спокойно, строк: {story_rows}"
    );
}

#[test]
fn production_like_screens_render_on_every_required_size() {
    let snapshot = production_like();
    assert!(
        snapshot.entities.len() >= 1_900,
        "фикстура обязана быть production-масштаба: {}",
        snapshot.entities.len()
    );
    for &(w, h) in REQUIRED_SIZES {
        for screen in [
            Screen::Overview,
            Screen::Problems,
            Screen::Entities,
            Screen::Timeline,
        ] {
            let mut app = App::default();
            app.screen = screen;
            let lines = draw(w, h, &snapshot, &mut app);
            assert_eq!(
                lines.len(),
                usize::from(h),
                "кадр обязан быть полным ({screen:?} {w}x{h})"
            );
            for line in &lines {
                assert!(
                    line.chars().count() <= usize::from(w),
                    "строка выходит за кадр ({screen:?} {w}x{h}): {line:?}"
                );
            }
            if screen == Screen::Overview || screen == Screen::Timeline {
                let text = lines.join("\n");
                assert!(
                    !text.contains("kworker"),
                    "шум ядра не имеет права появляться ({screen:?} {w}x{h}):\n{text}"
                );
            }
        }
    }
}

#[test]
fn derived_view_is_invalidated_by_view_state() {
    // Кэш обязан различать виды: иначе фильтр или сортировка не применятся,
    // а это молчаливая ложь в интерфейсе.
    let snapshot = production_like();
    let mut app = App::default();
    let all = app.derived(&snapshot).rows.len();
    assert!(all > 100, "полный список: {all}");

    app.entities.kind_filter = Some(EntityKind::Container);
    let containers = app.derived(&snapshot).rows.len();
    assert!(
        containers < all && containers > 0,
        "фильтр обязан сузить список: {containers} из {all}"
    );

    app.entities.technical_view = true;
    let technical = app.derived(&snapshot).logical.len();
    app.entities.technical_view = false;
    let logical = app.derived(&snapshot).logical.len();
    assert!(
        technical >= logical,
        "технический вид не сворачивает: {technical} против {logical}"
    );

    app.entities.kind_filter = None;
    assert_eq!(
        app.derived(&snapshot).rows.len(),
        all,
        "возврат к прежнему виду обязан дать прежний результат"
    );
}

#[test]
fn memory_ceiling_is_reported_in_the_interface_not_in_the_terminal() {
    // На реальном хосте это состояние существовало только как `tracing::warn!`
    // и печаталось поверх кадра ratatui, оставаясь висеть при переключении
    // экранов. Место такого факта — блок внимания.
    let mut snapshot = healthy();
    snapshot.agent.history_evicted_hot_ticks = 13;
    snapshot.agent.history_eviction_no_progress = 4;
    snapshot.agent.store_bytes = 64 * 1024 * 1024;

    let mut app = App::default();
    let text = joined(180, 40, &snapshot, &mut app);
    assert!(
        text.contains("memory ceiling"),
        "состояние потолка обязано быть видно в интерфейсе:\n{text}"
    );
    assert!(
        text.contains("64.0 MiB held"),
        "число обязано быть подписано как удержание:\n{text}"
    );
    // Формат журнала в кадре недопустим.
    for marker in ["WARN", "max_bytes=", "evicted_buckets="] {
        assert!(
            !text.contains(marker),
            "строка журнала не имеет права попадать в кадр ({marker}):\n{text}"
        );
    }

    // Здоровая история не порождает строку вовсе.
    let clean = healthy();
    let mut app = App::default();
    let clean_text = joined(180, 40, &clean, &mut app);
    assert!(
        !clean_text.contains("memory ceiling"),
        "без вытеснений строки быть не должно:\n{clean_text}"
    );
}

/// Воспроизводит ровно те строки, которые были на скриншоте efgis-dockerdev:
/// `! a2dc50289218` и `! vethcf9dac0` в Story. События берутся из настоящего
/// жизненного цикла графа, а не собираются руками: только так проверяется, что
/// вид сущности доезжает до классификации.
#[test]
fn container_and_veth_lifecycle_never_reaches_story() {
    let mut graph = EntityGraph::new("boot", "efgis-dockerdev", Timestamp::from_millis(1_000));

    // Такт 1: объекты существуют. Baseline не создаёт событий Created.
    graph.begin_tick(Timestamp::from_millis(2_000));
    let host = graph.host();
    let veth = graph.upsert(
        EntitySpec::new(
            EntityKey::NetIf {
                name: "vethcf9dac0".into(),
            },
            "vethcf9dac0",
        )
        .parent(host),
    );
    let container = graph.upsert(
        EntitySpec::new(
            EntityKey::Container {
                id: "a2dc50289218".into(),
                runtime: pulse_core::Runtime::Docker,
            },
            "a2dc50289218",
        )
        .parent(host),
    );
    let eth = graph.upsert(
        EntitySpec::new(
            EntityKey::NetIf {
                name: "eth0".into(),
            },
            "eth0",
        )
        .parent(host),
    );
    let _ = graph.end_tick();
    let _ = (veth, container, eth);

    // Такт 2 и 3: контейнер и его veth-пара исчезли, eth0 остался.
    // Форы жизненного цикла хватает на один такт, поэтому тактов два.
    // События собираются со всех тактов, как их накапливает журнал истории:
    // удаление происходит на том такте, где истекает фора жизненного цикла, и
    // привязываться к номеру такта в тесте значило бы дублировать эту логику.
    let mut events = Vec::new();
    for at in [3_000_u64, 4_000, 5_000] {
        graph.begin_tick(Timestamp::from_millis(at));
        let host = graph.host();
        let _ = graph.upsert(
            EntitySpec::new(
                EntityKey::NetIf {
                    name: "eth0".into(),
                },
                "eth0",
            )
            .parent(host),
        );
        events.extend(graph.end_tick().events);
    }
    let deleted: Vec<&str> = events
        .iter()
        .filter(|event| event.kind == EventKind::Deleted)
        .map(|event| event.entity_name.as_str())
        .collect();
    assert!(
        deleted.contains(&"vethcf9dac0") && deleted.contains(&"a2dc50289218"),
        "фикстура обязана воспроизвести исходные события: {deleted:?}"
    );
    assert!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::Deleted)
            .all(|event| event.entity_kind.is_some()),
        "вид сущности обязан доезжать до классификации"
    );

    let snapshot = Snapshot::build(
        &graph,
        LatestValues::new(),
        vec![],
        events,
        AgentStats::default(),
        "efgis-dockerdev",
        "boot",
    );

    let mut app = App::default();
    let _ = app.dispatch(key(KeyCode::Char('4')), &snapshot);
    let story = joined(180, 40, &snapshot, &mut app);
    assert!(
        !story.contains("vethcf9dac0"),
        "veth-пара контейнерной сети не является операционным изменением:\n{story}"
    );
    assert!(
        !story.contains("a2dc50289218"),
        "неразрешённый хеш ничего не сообщает оператору:\n{story}"
    );

    // Сырой поток обязан сохранить оба факта.
    app.timeline.raw_events = true;
    let raw = joined(180, 40, &snapshot, &mut app);
    assert!(
        raw.contains("vethcf9dac0") && raw.contains("a2dc50289218"),
        "сырые данные не уничтожаются семантической фильтрацией:\n{raw}"
    );
}

#[test]
fn frame_cost_at_production_scale_is_bounded() {
    use std::time::Instant;

    let snapshot = production_like();
    let mut app = App::default();
    // Прогрев: первый кадр включает ленивую инициализацию.
    let _ = draw(180, 40, &snapshot, &mut app);

    let started = Instant::now();
    for _ in 0..20 {
        let _ = draw(180, 40, &snapshot, &mut app);
    }
    let per_frame = started.elapsed() / 20;

    let started = Instant::now();
    for _ in 0..20 {
        let rows = crate::rows::entity_rows(&snapshot, &app);
        let _ = crate::fold::relevant(&snapshot, &rows, 12);
    }
    let per_analytics = started.elapsed() / 20;

    eprintln!(
        "frame cost: entities={} frame={per_frame:?} analytics={per_analytics:?}",
        snapshot.entities.len()
    );
    assert!(
        per_frame.as_millis() < 60,
        "кадр на 2000 сущностях обязан оставаться интерактивным: {per_frame:?}"
    );
}

#[test]
fn stacked_section_rules_span_their_row() {
    let snapshot = healthy();
    // §123 и §190: разделитель секции, идущей в стопке, отделяет её от соседней
    // секции и потому идёт на всю ширину ряда. Линия, обрывающаяся посреди
    // пустого ряда, читается как дефект отрисовки.
    for &(w, h) in REQUIRED_SIZES {
        let mut app = App::default();
        let lines = draw(w, h, &snapshot, &mut app);
        for label in ["SIGNALS", "RECENT CHANGES"] {
            let Some(line) = lines.iter().find(|l| l.trim_start().starts_with(label)) else {
                continue;
            };
            let trimmed = line.trim_end();
            if !trimmed.contains('\u{2500}') {
                continue;
            }
            assert_eq!(
                trimmed.chars().count(),
                usize::from(w),
                "разделитель {label} обязан занимать ряд целиком ({w}x{h}): {line:?}"
            );
        }
    }
}

#[test]
fn owning_pane_rule_spans_row_while_content_stays_bounded() {
    let snapshot = healthy();
    let mut app = App::default();
    // 120 колонок: панель выбранного не показывается (нужно >=140), таблица
    // владеет рядом. Правило обязано идти на всю ширину (§123), содержимое -
    // остаться в границах полезной ширины (раздел 144). Это разные величины.
    let lines = draw(120, 30, &snapshot, &mut app);
    let head = lines
        .iter()
        .find(|l| l.contains("ENTITIES") && l.contains('\u{2500}'))
        .expect("заголовок таблицы с правилом");
    assert_eq!(
        head.trim_end().chars().count(),
        120,
        "правило владеющей панели обязано занимать ряд целиком: {head:?}"
    );

    let widest_row = lines
        .iter()
        .filter(|l| l.starts_with("> ") || l.starts_with("  NAME"))
        .map(|l| l.trim_end().chars().count())
        .max()
        .expect("строки таблицы");
    let cap = usize::from(max_useful_width(BlockKind::EntityTable));
    assert!(
        widest_row <= cap,
        "содержимое обязано остаться в полезной ширине {cap}: {widest_row}"
    );
}

#[test]
fn side_by_side_rules_stop_at_pane_edge() {
    let snapshot = healthy();
    let mut app = App::default();
    // На XL панели делят ряд: там линия обязана кончаться на краю своей панели,
    // иначе она затрёт соседа (§188, §189).
    let lines = draw(180, 40, &snapshot, &mut app);
    let head = lines
        .iter()
        .find(|l| l.contains("STATE") && l.contains("ATTENTION"))
        .expect("STATE и ATTENTION делят ряд на 180 колонках");
    let state_at = head.find("STATE").expect("STATE");
    let attention_at = head.find("ATTENTION").expect("ATTENTION");
    assert!(state_at < attention_at, "{head:?}");
    let between = head
        .get(state_at..attention_at)
        .expect("участок между подписями");
    assert!(
        between.contains('\u{2500}'),
        "у панели состояния обязана быть своя линия: {head:?}"
    );
    assert!(
        between.contains("  "),
        "линия обязана кончаться до соседней панели: {head:?}"
    );
}

#[test]
fn state_pane_never_repeats_zero_problems() {
    let snapshot = healthy();
    assert!(
        snapshot.problems.is_empty(),
        "фикстура обязана быть здоровой"
    );
    for &(w, h) in REQUIRED_SIZES {
        let mut app = App::default();
        let text = joined(w, h, &snapshot, &mut app);
        assert!(
            !text.contains("no active problem state"),
            "§163 запрещает дублировать нуль проблем ({w}x{h}):\n{text}"
        );
        assert!(
            !text.contains("0 problems"),
            "§163 запрещает счётчик 0 problems ({w}x{h}):\n{text}"
        );
        assert!(
            text.contains("SYSTEM NOMINAL"),
            "вердикт обязан остаться ({w}x{h}):\n{text}"
        );
    }
}

#[test]
fn raw_events_are_reachable_only_as_secondary_view() {
    let snapshot = critical();
    let mut app = App::default();
    app.screen = Screen::Timeline;
    let default_view = joined(180, 40, &snapshot, &mut app);
    assert!(default_view.contains("STORY"));
    let raw_by_default = default_view.contains("RAW EVENTS");
    assert!(!raw_by_default, "raw stream must not be the default view");

    // Palette command opens the secondary subview explicitly.
    let _ = app.dispatch(key(KeyCode::Char(':')), &snapshot);
    for ch in "raw events".chars() {
        let _ = app.dispatch(key(KeyCode::Char(ch)), &snapshot);
    }
    let _ = app.dispatch(key(KeyCode::Enter), &snapshot);
    assert!(app.timeline.raw_events);
    let raw_view = joined(180, 40, &snapshot, &mut app);
    assert!(raw_view.contains("RAW EVENTS"), "{raw_view}");
    assert!(app.timeline.story_len >= snapshot.events.len());
}

#[test]
fn metric_lanes_use_real_history_when_available() {
    use pulse_core::config::Store as StoreConfig;
    use pulse_core::sample::SeriesKey;
    use pulse_store::History;

    let snapshot = healthy();
    // Заполняем горячее окно настоящими точками через публичный ingest.
    let mut history = History::new(&StoreConfig::default());
    for tick in 1..=6_u64 {
        let at = Timestamp::from_millis(1_000 + tick * 1_000);
        let mut batch = pulse_core::graph::TickBatch {
            tick: pulse_core::time::TickId(tick),
            at,
            samples: vec![pulse_core::sample::Sample {
                series: SeriesKey::new(snapshot.host, ids::HOST_CPU_UTIL),
                value: f64::from(u32::try_from(tick).unwrap_or(1)) / 10.0,
            }],
            events: Vec::new(),
            records: Vec::new(),
            alive: vec![snapshot.host],
        };
        batch.samples.push(pulse_core::sample::Sample {
            series: SeriesKey::new(snapshot.host, ids::HOST_MEM_UTIL),
            value: 0.11,
        });
        history.ingest(&batch);
    }

    let mut app = App::default();
    app.screen = Screen::Timeline;
    app.timeline.zoom_ms = 60_000;
    let backend = TestBackend::new(180, 40);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let theme = Theme::with_capability(Capability::TrueColor);
    let mut snapshot_at = snapshot.clone();
    snapshot_at.at = Timestamp::from_millis(7_000);
    terminal
        .draw(|frame| super::render(frame, &snapshot_at, &mut app, &theme, Some(&history)))
        .expect("render");
    let buffer = terminal.backend().buffer().clone();
    let text: String = (0..40)
        .map(|row| {
            (0..180)
                .map(|col| {
                    buffer
                        .cell((col, row))
                        .and_then(|cell| cell.symbol().chars().next())
                        .unwrap_or(' ')
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    let cpu_lane = text
        .lines()
        .find(|line| line.trim_start().starts_with("CPU"))
        .expect("cpu lane");
    let still_collecting = cpu_lane.contains("collecting history");
    assert!(
        !still_collecting,
        "real history must replace placeholder: {cpu_lane}"
    );
    assert!(
        cpu_lane.chars().any(|ch| "▁▂▃▄▅▆▇█".contains(ch)),
        "sparkline expected: {cpu_lane}"
    );
    // IO без точек остаётся честным placeholder.
    let io_lane = text
        .lines()
        .find(|line| line.trim_start().starts_with("IO"))
        .expect("io lane");
    assert!(io_lane.contains("collecting history"), "{io_lane}");
}

#[test]
fn resize_roundtrip_preserves_semantic_context() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Entities;
    app.entities.selected = 1;
    app.entities.sort = crate::app::SortKey::Memory;
    app.entities.kind_filter = Some(EntityKind::Unit);
    app.entities.technical_view = true;
    app.timeline.time_cursor = Some(Timestamp::from_millis(1_500));
    app.timeline.mark_a = Some(Timestamp::from_millis(1_200));

    for (width, height) in [(180, 40), (80, 24), (180, 40)] {
        let _ = draw(width, height, &snapshot, &mut app);
    }
    assert_eq!(app.screen, Screen::Entities);
    assert_eq!(app.entities.selected, 1);
    assert_eq!(app.entities.sort, crate::app::SortKey::Memory);
    assert_eq!(app.entities.kind_filter, Some(EntityKind::Unit));
    assert!(app.entities.technical_view);
    assert_eq!(
        app.timeline.time_cursor,
        Some(Timestamp::from_millis(1_500))
    );
    assert_eq!(app.timeline.mark_a, Some(Timestamp::from_millis(1_200)));
}

#[test]
fn all_base_screens_render_at_required_sizes_without_wrap_or_overlap() {
    for snapshot in [healthy(), critical()] {
        for screen in [
            Screen::Overview,
            Screen::Problems,
            Screen::Entities,
            Screen::Timeline,
        ] {
            for (width, height) in REQUIRED_SIZES {
                let mut app = App::default();
                app.screen = screen;
                let lines = draw(*width, *height, &snapshot, &mut app);
                assert_eq!(lines.len(), usize::from(*height));
                assert!(
                    lines
                        .iter()
                        .all(|line| line.chars().count() <= usize::from(*width)),
                    "wrap/overflow on {screen:?} {width}x{height}"
                );
                let footer = lines.last().expect("footer");
                assert!(!footer.trim().is_empty(), "missing footer {width}x{height}");
            }
        }
    }
}

#[test]
fn ascii_mode_preserves_semantics_without_unicode_decoration() {
    let snapshot = critical();
    let mut app = App::default();
    let theme = Theme::with_capability(Capability::Ascii);
    let text = draw_with(120, 30, &snapshot, &mut app, &theme).join("\n");
    for forbidden in ['·', '○', '●', '◉', '◇', '◆', '▲', '×', '─'] {
        assert!(!text.contains(forbidden), "unicode leaked: {forbidden}");
    }
    assert!(text.contains("LIVE *"));
    assert!(text.contains("RX") && text.contains("TX"));
}

/// Manual visual review harness: deterministic buffers for the three normative
/// sizes. Normally silent; `PULSE_DUMP_SNAPSHOTS=1 cargo test ... --nocapture`
/// prints exact text without terminal log whitespace collapsing.
#[test]
fn dump_v09_snapshots_when_requested() {
    if std::env::var_os("PULSE_DUMP_SNAPSHOTS").is_none() {
        return;
    }
    let snapshot = healthy();
    for (width, height) in [(80, 24), (120, 30), (180, 40)] {
        for screen in [
            Screen::Overview,
            Screen::Problems,
            Screen::Entities,
            Screen::Timeline,
        ] {
            let mut app = App::default();
            app.screen = screen;
            let text = joined(width, height, &snapshot, &mut app);
            println!("\n=== {screen:?} {width}x{height} ===\n{text}");
        }
    }
}

#[test]
fn dump_v09_contexts_when_requested() {
    if std::env::var_os("PULSE_DUMP_SNAPSHOTS").is_none() {
        return;
    }
    let snapshot = critical();

    let mut search = App::default();
    let _ = search.dispatch(key(KeyCode::Char('/')), &snapshot);
    for ch in "angie".chars() {
        let _ = search.dispatch(key(KeyCode::Char(ch)), &snapshot);
    }
    println!(
        "\n=== Search 120x30 ===\n{}",
        joined(120, 30, &snapshot, &mut search)
    );

    let mut help = App::default();
    let _ = help.dispatch(key(KeyCode::Char('?')), &snapshot);
    println!(
        "\n=== Help 120x30 ===\n{}",
        joined(120, 30, &snapshot, &mut help)
    );

    let key = snapshot
        .entities_of_kind(EntityKind::Unit)
        .next()
        .expect("unit")
        .key
        .clone();
    let mut inspector = App::default();
    inspector.open_inspector(key);
    println!(
        "\n=== Inspector 120x30 ===\n{}",
        joined(120, 30, &snapshot, &mut inspector)
    );

    let mut problems = App::default();
    problems.screen = Screen::Problems;
    println!(
        "\n=== Problems critical 120x30 ===\n{}",
        joined(120, 30, &snapshot, &mut problems)
    );
}

// --- Восстановленные инварианты прежней приёмки, не отменённые v0.9 --------

/// §110: слишком маленький терминал получает объяснение, а не обрезки.
#[test]
fn too_small_terminal_explains_itself() {
    let snapshot = healthy();
    let mut app = App::default();
    let text = joined(43, 9, &snapshot, &mut app);
    assert!(text.contains("terminal too small"), "{text}");
    assert!(text.contains("50"), "minimum size expected: {text}");
    assert!(text.contains("43"), "current size expected: {text}");
    assert!(text.contains("q quit"), "exit hint expected: {text}");
}

/// §13: ячейки таблицы не склеиваются ни на одном поддерживаемом размере.
#[test]
fn table_cells_never_run_together() {
    let snapshot = healthy();
    for (width, height) in REQUIRED_SIZES {
        for screen in [Screen::Overview, Screen::Entities] {
            let mut app = App::default();
            app.screen = screen;
            let lines = draw(*width, *height, &snapshot, &mut app);
            let glued = lines.iter().any(|line| {
                line.contains("MiBunit")
                    || line.contains("MiBcgroup")
                    || line.contains("MiBprocess")
                    || line.contains("MiBdisk")
            });
            assert!(
                !glued,
                "glued cells on {screen:?} {width}x{height}: {lines:#?}"
            );
        }
    }
}

/// §111: критическое состояние не скрывается ни при каком размере.
#[test]
fn critical_state_is_never_hidden() {
    let snapshot = critical();
    for (width, height) in REQUIRED_SIZES {
        let mut app = App::default();
        let text = joined(*width, *height, &snapshot, &mut app);
        assert!(
            text.contains('▲') || text.contains('◆'),
            "critical state hidden at {width}x{height}: {text}"
        );
    }
}

/// §111: режим наблюдения виден всегда и отличает LIVE от HISTORY.
#[test]
fn live_or_history_is_always_visible() {
    let snapshot = healthy();
    for (width, height) in REQUIRED_SIZES {
        for paused in [false, true] {
            let mut app = App::default();
            app.paused = paused;
            let header = draw(*width, *height, &snapshot, &mut app)
                .first()
                .cloned()
                .unwrap_or_default();
            let expected = if paused { "HIST" } else { "LIVE" };
            assert!(
                header.contains(expected),
                "{expected} missing at {width}x{height}: {header}"
            );
        }
    }
}

/// §136: маркер `>` принадлежит только выбранной строке.
#[test]
fn only_selected_row_carries_marker() {
    let snapshot = healthy();
    let mut app = App::default();
    app.screen = Screen::Entities;
    let _ = draw(180, 40, &snapshot, &mut app);
    app.entities.selected = 1;
    let lines = draw(180, 40, &snapshot, &mut app);
    let markers = lines.iter().filter(|line| line.starts_with("> ")).count();
    assert_eq!(markers, 1, "exactly one selection marker: {lines:#?}");
    let legacy = lines.iter().any(|line| line.contains('▸'));
    assert!(!legacy, "no alternative selection markers");
}

/// §151, §152: утверждения не превышают окно наблюдения, аптайм не подменяет его.
#[test]
fn claims_never_exceed_observation_window() {
    let mut graph = EntityGraph::new("boot", "asuspc", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(3_000));
    let host = graph.host();
    let mut latest = LatestValues::new();
    // Аптайм 23 часа против двух секунд наблюдения.
    latest.set(SeriesKey::new(host, ids::HOST_UPTIME), 82_800.0);
    let batch = graph.end_tick();
    let snapshot = Snapshot::build(
        &graph,
        latest,
        vec![],
        batch.events,
        AgentStats::default(),
        "asuspc",
        "boot",
    );

    for screen in [Screen::Overview, Screen::Problems] {
        let mut app = App::default();
        app.screen = screen;
        let lines = draw(180, 40, &snapshot, &mut app);
        let claims: Vec<&String> = lines
            .iter()
            .filter(|line| {
                line.contains("observed nominal for") || line.contains("have been observed")
            })
            .collect();
        let missing = claims.is_empty();
        assert!(
            !missing,
            "{screen:?} must state the observation window: {lines:#?}"
        );
        for claim in claims {
            let overclaims = claim.contains("23h");
            assert!(!overclaims, "{screen:?} claims beyond observation: {claim}");
            assert!(
                claim.contains("2s") || claim.contains("0s"),
                "{screen:?} must name the real window: {claim}"
            );
        }
    }
}

/// §130: однотипные связи агрегируются, а не печатаются построчно.
#[test]
fn inspector_aggregates_repeated_relations() {
    let mut graph = EntityGraph::new("boot", "asuspc", Timestamp::from_millis(1_000));
    graph.begin_tick(Timestamp::from_millis(2_000));
    let host = graph.host();
    let cgroup =
        graph.upsert(EntitySpec::new(EntityKey::Cgroup { cgroup_id: 1 }, "/").parent(host));
    for name in ["sda", "sdb", "sdc", "sdd", "sde", "sdf"] {
        let disk =
            graph.upsert(EntitySpec::new(EntityKey::Disk { name: name.into() }, name).parent(host));
        graph.relate(cgroup, RelationKind::BackedBy, disk);
    }
    let batch = graph.end_tick();
    let snapshot = Snapshot::build(
        &graph,
        LatestValues::new(),
        vec![],
        batch.events,
        AgentStats::default(),
        "asuspc",
        "boot",
    );

    let cgroup_key = snapshot
        .entities_of_kind(EntityKind::Cgroup)
        .next()
        .expect("cgroup")
        .key
        .clone();
    let mut app = App::default();
    app.open_inspector(cgroup_key);
    let text = joined(120, 30, &snapshot, &mut app);

    // Шесть дисков не занимают шесть строк цепочки: диск не звено
    // расследования, а ресурс. Он живёт одной строкой контекста.
    assert!(
        text.contains("RESOURCES"),
        "ресурсы обязаны быть отдельным блоком: {text}"
    );
    let resource_lines = text
        .lines()
        .filter(|line| line.contains("sda") || line.contains("sdf"))
        .count();
    assert_eq!(
        resource_lines, 1,
        "все диски на одной строке контекста: {text}"
    );
    assert!(
        text.contains("sda") && text.contains("sdf"),
        "состояние каждого диска остаётся видимым: {text}"
    );
}

/// Дефект живого прогона: на экране пайпа `q` не завершал агента, потому что
/// overlay съедал все неизвестные клавиши. Выход обязан работать с любого
/// экрана, иначе пользователь ищет способ выйти вместо работы.
#[test]
fn quit_works_from_the_pipe_overlay() {
    let snapshot = healthy();
    let mut app = App::default();

    assert_eq!(
        app.dispatch(key(KeyCode::Char('g')), &snapshot),
        Action::None,
        "`g` открывает пайп"
    );
    assert!(
        matches!(app.overlay, Some(Overlay::Pipe(_))),
        "пайп обязан быть открыт"
    );
    assert_eq!(
        app.dispatch(key(KeyCode::Char('q')), &snapshot),
        Action::Quit,
        "`q` завершает агента и из пайпа"
    );
}

/// Цепочка и боковые переходы - разные списки, и Tab честно переключает их.
#[test]
fn chain_and_side_jumps_are_separate_lists() {
    let snapshot = production_like();
    let cgroup = snapshot
        .entities_of_kind(EntityKind::Cgroup)
        .find(|entity| entity.name.contains("docker"))
        .or_else(|| snapshot.entities_of_kind(EntityKind::Cgroup).next())
        .expect("cgroup в фикстуре")
        .key
        .clone();

    let mut app = App::default();
    app.open_inspector(cgroup);
    let text = joined(140, 44, &snapshot, &mut app);
    assert!(
        text.contains("CHAIN <ENTER>"),
        "по умолчанию Enter ведёт по цепечке: {text}"
    );

    let _ = app.dispatch(key(KeyCode::Tab), &snapshot);
    let text = joined(140, 44, &snapshot, &mut app);
    assert!(
        text.contains("RELATED <ENTER>") && text.contains("CHAIN <TAB>"),
        "Tab переключил активный список: {text}"
    );
}

/// Боковой переход начинает новое расследование, а не удлиняет цепочку.
#[test]
fn side_jump_restarts_the_investigation() {
    let (snapshot, a, b, _) = cyclic();
    let mut app = App::default();
    app.open_inspector(a.clone());
    follow_to(&mut app, &snapshot, &b);
    assert_eq!(app.inspector.as_ref().map(|s| s.path.len()), Some(2));

    // Владелец доступен только как боковой переход: вверх ведёт Esc,
    // а Enter по цепочке идёт вниз.
    let side = app.inspector_side_steps(&snapshot);
    let owner = side
        .iter()
        .position(|step| step.label == "owner")
        .expect("владелец в боковых переходах");
    if let Some(session) = &mut app.inspector {
        session.side_active = true;
        session.relation_selected = owner;
    }
    let _ = app.dispatch(key(KeyCode::Enter), &snapshot);

    let session = app.inspector.as_ref().expect("inspector открыт");
    assert_eq!(
        session.path.len(),
        1,
        "прыжок в сторону не продолжает цепочку: {:?}",
        session.path
    );
    assert_eq!(session.current, a, "новый корень - объект прыжка");
    assert!(
        !session.side_active,
        "после прыжка Enter снова ведёт по цепочке"
    );
}

/// Терминальный уровень цепочки: процесс обязан отвечать «кто и что держит».
#[test]
fn process_inspector_answers_user_exe_and_ports() {
    #[derive(Debug)]
    struct Fixed;
    impl pulse_core::ProcessDetailsSource for Fixed {
        fn details(&self, _of: pulse_core::ProcessIdentity) -> pulse_core::ProcessDetails {
            pulse_core::ProcessDetails {
                uid: Some(33),
                user: Some("www-data".into()),
                exe: Some("/usr/sbin/nginx".into()),
                cwd: Some("/var/www".into()),
                fd_total: 9,
                files: vec![pulse_core::OpenFile {
                    fd: 3,
                    target: "/var/log/nginx/access.log".into(),
                }],
                ports: vec![pulse_core::Port {
                    protocol: "tcp",
                    address: "0.0.0.0".into(),
                    port: 80,
                }],
                restricted: false,
                identity_changed: false,
            }
        }
    }

    let snapshot = healthy();
    let process = snapshot
        .entities_of_kind(EntityKind::Process)
        .next()
        .expect("процесс в фикстуре")
        .key
        .clone();
    let mut app = App::default().with_details(std::sync::Arc::new(Fixed));
    app.open_inspector(process);
    let text = joined(140, 40, &snapshot, &mut app);

    assert!(text.contains("PROCESS DETAILS"), "блок деталей: {text}");
    assert!(text.contains("www-data"), "пользователь: {text}");
    assert!(text.contains("/usr/sbin/nginx"), "исполняемый файл: {text}");
    assert!(text.contains("/var/www"), "рабочий каталог: {text}");
    assert!(text.contains("0.0.0.0:80"), "слушающий порт: {text}");
    assert!(
        text.contains("/var/log/nginx/access.log"),
        "открытый файл: {text}"
    );
}

/// Отказ ядра в правах обязан быть назван, а не выглядеть как отсутствие файлов.
#[test]
fn restricted_details_explain_the_reason() {
    #[derive(Debug)]
    struct Restricted;
    impl pulse_core::ProcessDetailsSource for Restricted {
        fn details(&self, _of: pulse_core::ProcessIdentity) -> pulse_core::ProcessDetails {
            pulse_core::ProcessDetails {
                uid: Some(0),
                user: Some("root".into()),
                restricted: true,
                ..pulse_core::ProcessDetails::default()
            }
        }
    }

    let snapshot = healthy();
    let process = snapshot
        .entities_of_kind(EntityKind::Process)
        .next()
        .expect("процесс в фикстуре")
        .key
        .clone();
    let mut app = App::default().with_details(std::sync::Arc::new(Restricted));
    app.open_inspector(process);
    let text = joined(140, 40, &snapshot, &mut app);

    assert!(
        text.contains("restricted"),
        "причина отказа обязана быть на экране: {text}"
    );
}

/// §159: остаток высоты принадлежит таблице, а не пустой рамке.
#[test]
fn overview_entities_take_remaining_height() {
    let snapshot = healthy();
    let mut app = App::default();
    let lines = draw(180, 40, &snapshot, &mut app);
    let table_title = lines
        .iter()
        .position(|line| line.contains("RELEVANT ENTITIES") || line.contains("KEY ENTITIES"))
        .expect("entity block");
    let remaining = lines.len().saturating_sub(table_title);
    assert!(
        remaining * 2 >= lines.len(),
        "entity area must own at least half the height: {remaining} of {}",
        lines.len()
    );
}
