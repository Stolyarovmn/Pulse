//! Экран сущностей (разделы 16, 128, 129 спецификации v0.7).
//!
//! На широком терминале таблица и инспектор стоят рядом; при нехватке ширины
//! инспектор становится вкладкой, а не узкой испорченной колонкой
//! (раздел 128). Отношения агрегируются: шесть почти одинаковых строк
//! `backed_by disk/sdX` менее полезны, чем `backed by 6 disks`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use pulse_core::entity::{Entity, EntityKind};
use pulse_core::snapshot::Snapshot;

use crate::app::{logical_relation_targets, App};
use crate::fold::LogicalRow;
use crate::layout::{bounded, BlockKind, LayoutPlan, Pane};
use crate::theme::Theme;
use crate::ui;

use super::{focused_section, rect, render_logical_table};

/// Отрисовка экрана сущностей.
///
/// Два режима представления (раздел 149): логический по умолчанию и технический
/// по `m`. Логический показывает операционные объекты со свёрнутыми
/// техническими частями, технический - полный инвентарь графа. Модель данных в
/// обоих режимах одна: меняется только группировка.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &mut App,
    plan: &LayoutPlan,
    theme: &Theme,
    history: Option<&pulse_store::History>,
) {
    // Производный вид считается один раз на такт: и ввод, и отрисовка читают
    // один результат.
    let derived = app.derived(snapshot);
    let rows: &[LogicalRow] = &derived.logical;
    app.entities.list_len = rows.len();
    let selected = app.entities.selected.min(rows.len().saturating_sub(1));
    app.entities.preview_relation_len = rows
        .get(selected)
        .map_or(0, |row| logical_relation_targets(snapshot, row).len());

    let head = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    let mode = if app.entities.technical_view {
        "technical"
    } else {
        "logical"
    };
    let ascii = matches!(theme.capability, crate::theme::Capability::Ascii);
    // Раздел 178: активный search обязан быть виден как filter, а не молча
    // сокращать список при подписи `filter:all`. Total рядом с visible count
    // показывает, что именно отфильтровано.
    let filter = match (
        app.entities.search_filter.as_deref(),
        app.entities.kind_filter,
    ) {
        (Some(query), _) => format!("search(\"{query}\")"),
        (None, Some(kind)) => kind.as_str().to_string(),
        (None, None) => "all".to_string(),
    };
    let total = snapshot
        .entities
        .iter()
        .filter(|entity| entity.kind != EntityKind::Host)
        .count();
    let count = if rows.len() == total {
        format!("{}", rows.len())
    } else {
        format!("{}/{total}", rows.len())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("ENTITIES {count}   "), theme.strong()),
            Span::styled(
                format!(
                    "sort:{}{}   filter:{filter}   view:{mode}",
                    app.entities.sort.label(),
                    app.entities.direction.symbol(ascii),
                ),
                theme.dim(),
            ),
        ])),
        rect(&head, 0),
    );

    let body = rect(&head, 1);
    if plan.inspector_beside {
        let (list_w, _) = bounded(BlockKind::EntityTable, body.width * 6 / 10);
        let preview_available = body.width.saturating_sub(list_w).saturating_sub(3);
        let (preview_w, _) = bounded(BlockKind::Selected, preview_available);
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(list_w),
                Constraint::Length(3),
                Constraint::Length(preview_w),
                Constraint::Min(0),
            ])
            .split(body);
        render_entity_list(
            frame,
            rect(&halves, 0),
            rows,
            (selected, app.pane == Pane::Primary),
            &super::overview::TrendSource { snapshot, history },
            plan,
            theme,
        );
        render_inspector_pane(
            frame,
            rect(&halves, 2),
            snapshot,
            rows.get(selected),
            (
                app.pane == Pane::Inspector,
                app.entities.preview_relation_selected,
            ),
            plan,
            theme,
        );
    } else {
        render_entity_list(
            frame,
            body,
            rows,
            (selected, true),
            &super::overview::TrendSource { snapshot, history },
            plan,
            theme,
        );
    }
}

fn render_entity_list(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[LogicalRow],
    // Курсор и фокус идут парой: у функции иначе восемь аргументов.
    view: (usize, bool),
    trend: &super::overview::TrendSource<'_>,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let (selected, focused) = view;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    frame.render_widget(
        Paragraph::new(focused_section(
            "ENTITY LIST",
            area.width,
            focused,
            plan,
            theme,
        )),
        rect(&chunks, 0),
    );
    render_logical_table(frame, rect(&chunks, 1), rows, selected, Some(trend), theme);
}

/// Панель инспектора: сводка, агрегированные связи, метки (раздел 128).
pub(crate) fn render_inspector_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    logical: Option<&LogicalRow>,
    interaction: (bool, usize),
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let (focused, relation_selected) = interaction;
    let Some(logical) = logical else {
        return;
    };
    let Some(entity) = snapshot.entity(logical.row.id) else {
        return;
    };

    let width = usize::from(area.width);
    let title = format!("PREVIEW / {}", entity.name);
    let mut lines = vec![focused_section(
        &ui::truncate(&title, width, theme.capability),
        area.width,
        focused,
        plan,
        theme,
    )];

    lines.push(Line::from(vec![
        Span::styled(entity.kind.as_str().to_string(), theme.dim()),
        Span::raw(" "),
        Span::styled(
            ui::truncate(&entity.name, width.saturating_sub(10), theme.capability),
            theme.strong(),
        ),
    ]));

    let severity = snapshot
        .problems_of(entity.id)
        .map(|problem| problem.severity)
        .max();
    let state = crate::rows::state_of(snapshot, entity, severity);
    lines.push(Line::from(vec![
        Span::styled("STATE ", theme.dim()),
        Span::styled(
            state.symbol(theme.capability).to_string(),
            state.style(theme),
        ),
    ]));
    lines.push(Line::from(""));

    // Числа: только те, что реально есть у этого вида сущности.
    lines.push(Line::from(Span::styled("SUMMARY", theme.dim())));
    for (label, value) in summary_rows(snapshot, entity) {
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<12}"), theme.dim()),
            Span::styled(value, theme.text()),
        ]));
    }

    let relations = logical_relation_targets(snapshot, logical);
    if !relations.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("RELATIONS", theme.dim())));
        for (index, target) in relations.iter().enumerate() {
            let selected = focused && index == relation_selected;
            lines.push(Line::from(vec![
                Span::styled(
                    ui::row_marker(selected),
                    if selected {
                        theme.selection()
                    } else {
                        theme.dim()
                    },
                ),
                Span::styled(format!("{:<12}", target.label), theme.dim()),
                Span::styled(
                    ui::truncate(&target.name, width.saturating_sub(15), theme.capability),
                    if selected {
                        theme.selection()
                    } else {
                        theme.text()
                    },
                ),
            ]));
        }
    }

    if !entity.labels.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("LABELS", theme.dim())));
        for (key, value) in entity.labels.iter().take(6) {
            lines.push(Line::from(vec![
                Span::styled(format!("{key:<12}"), theme.dim()),
                Span::styled(
                    ui::truncate(value, width.saturating_sub(13), theme.capability),
                    theme.text(),
                ),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Числовая сводка сущности.
fn summary_rows(snapshot: &Snapshot, entity: &Entity) -> Vec<(&'static str, String)> {
    use pulse_core::metric::ids;
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    let cpu = crate::rows::cpu_of(snapshot, entity);
    let memory = crate::rows::memory_of(snapshot, entity);
    rows.push(("CPU", crate::format::cores(cpu)));
    rows.push(("MEM", crate::format::bytes(memory)));

    match entity.kind {
        EntityKind::Cgroup | EntityKind::Unit | EntityKind::Container | EntityKind::Pod => {
            // §Resource semantics: у агрегата обязана быть явно названа область,
            // иначе `CPU 18.0c` у system.slice читается как нагрузка одного
            // объекта, а не суммы его потомков.
            rows.push(("scope", "descendants".to_string()));
            let limit = snapshot.value(entity.id, ids::CG_CPU_LIMIT_CORES);
            rows.push((
                "CPU limit",
                limit.map_or_else(|| "none".to_string(), crate::format::cores),
            ));
            if let Some(psi) = snapshot.value(entity.id, ids::CG_PSI_IO_FULL_AVG10) {
                rows.push(("IO PSI", crate::format::percent(psi)));
            }
        }
        EntityKind::Process => {
            if let Some(fds) = snapshot.value(entity.id, ids::PROC_FD_COUNT) {
                rows.push(("FD", crate::format::count(fds)));
            }
        }
        EntityKind::Disk => {
            if let Some(await_ms) = snapshot.value(entity.id, ids::DISK_AWAIT) {
                rows.push(("await", crate::format::millis(await_ms)));
            }
        }
        _ => {}
    }
    rows
}
