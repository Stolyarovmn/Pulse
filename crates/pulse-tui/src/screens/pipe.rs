//! Экран пайпа расследования (overlay по клавише `g`).
//!
//! Две панели: слева список сущностей, справа факты о выбранной. Список
//! остаётся на экране не для красоты: пайп отвечает на «что известно про этот
//! объект», и без окружения оператор теряет, где он находится и что рядом.
//!
//! Раскладка и текст пайпа живут в [`crate::pipe`], здесь только перенос строк
//! в кадр и раскраска: экран рисует состояние, а не считает его.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use pulse_core::{EntityId, Snapshot};

use crate::app::App;
use crate::layout::LayoutPlan;
use crate::pipe::{self, PipeState};
use crate::theme::Theme;

use super::section;

/// Ширина панели сущностей. Меньше — имена сервисов перестают читаться.
const LIST_COLS: u16 = 34;
/// Общий контекст кадра: иначе у функций отрисовки восемь параметров.
struct Ctx<'a> {
    snapshot: &'a Snapshot,
    app: &'a App,
    plan: &'a LayoutPlan,
    theme: &'a Theme,
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &App,
    state: &PipeState,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    frame.render_widget(Clear, area);

    let Some(entity) = app.focus_entity(snapshot) else {
        frame.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                "нет выбранной сущности",
                theme.dim(),
            ))]),
            area,
        );
        return;
    };

    // Узкий кадр отдаёт всю ширину пайпу: список важен, но ответ важнее.
    let (list_area, pipe_area) =
        if usize::from(area.width) >= usize::from(LIST_COLS) + pipe::GRAPH_MIN_COLS {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(LIST_COLS), Constraint::Min(20)])
                .split(area);
            (
                chunks.first().copied(),
                chunks.get(1).copied().unwrap_or(area),
            )
        } else {
            (None, area)
        };

    let ctx = Ctx {
        snapshot,
        app,
        plan,
        theme,
    };
    if let Some(list_area) = list_area {
        render_list(frame, list_area, &ctx, entity);
    }
    render_pipe(frame, pipe_area, &ctx, entity, state);
}

/// Список сущностей вокруг фокуса: сам объект, его владельцы и его процессы.
fn render_list(frame: &mut Frame<'_>, area: Rect, ctx: &Ctx<'_>, focus: EntityId) {
    let theme = ctx.theme;
    let mut text: Vec<Line<'_>> = vec![section("ENTITIES", area.width, ctx.plan, theme)];
    let rows = ctx.app.derived(ctx.snapshot);
    let width = usize::from(area.width).saturating_sub(2);
    // Вид сущности в списке обязателен: unit и его cgroup носят одно имя, и
    // без тега строки читаются как дубли — так и выглядело на живом кадре.
    let set = if matches!(theme.capability, crate::theme::Capability::Ascii) {
        pulse_core::config::IconSet::Off
    } else {
        ctx.app.icons
    };
    // Колонка имени сужается на ширину иконки, иначе включение набора
    // сдвигает проценты и кадр перестаёт совпадать с кадром без иконок.
    let icon_width = if matches!(set, pulse_core::config::IconSet::Off) {
        0
    } else {
        2
    };
    let name_width = width.saturating_sub(15 + icon_width);
    for row in rows
        .rows
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
    {
        let marker = if row.id == focus { "▸" } else { " " };
        let line = format!(
            "{marker} {}{:<5} {:<name_width$} {:>5}",
            crate::icons::prefix(crate::icons::for_kind(row.kind), set),
            kind_tag(row.kind),
            truncate(&row.name, name_width),
            crate::format::percent(row.cpu),
        );
        let style = if row.id == focus {
            theme.strong()
        } else {
            theme.text()
        };
        text.push(Line::from(Span::styled(line, style)));
    }
    frame.render_widget(Paragraph::new(text), area);
}

/// Короткий тег вида: колонка узкая, а различать сущности обязательно.
const fn kind_tag(kind: pulse_core::EntityKind) -> &'static str {
    match kind {
        pulse_core::EntityKind::Host => "host",
        pulse_core::EntityKind::Cgroup => "cg",
        pulse_core::EntityKind::Unit => "unit",
        pulse_core::EntityKind::Process => "proc",
        pulse_core::EntityKind::Container => "ctr",
        pulse_core::EntityKind::Pod => "pod",
        pulse_core::EntityKind::Disk => "disk",
        pulse_core::EntityKind::NetIf => "net",
    }
}

fn render_pipe(
    frame: &mut Frame<'_>,
    area: Rect,
    ctx: &Ctx<'_>,
    entity: EntityId,
    state: &PipeState,
) {
    let snapshot = ctx.snapshot;
    let theme = ctx.theme;
    let title = snapshot
        .entity(entity)
        .map(|found| format!("{} {}", found.kind.as_str(), found.name))
        .unwrap_or_else(|| "неизвестная сущность".to_string());

    let details = ctx.app.pipe_details(snapshot, entity, state);
    let nodes = pipe::build(snapshot, entity, details.as_ref(), state);
    // Кадр знает свою ширину, поэтому именно он сообщает сетку курсору.
    ctx.app
        .set_pipe_columns(PipeState::columns_for(area.width as usize, state.boxes));
    let rendered = pipe::render(
        &title,
        &nodes,
        state.selected_branch(),
        area.width as usize,
        pipe::Look {
            capability: theme.capability,
            boxes: state.boxes,
            icons: ctx.app.icons,
        },
    );

    let mut text: Vec<Line<'_>> = vec![section(
        &format!("PIPE · {title}"),
        area.width,
        ctx.plan,
        theme,
    )];
    for (line, severity) in rendered.iter().skip(1) {
        let style = match severity {
            Some(severity) => theme.severity(*severity),
            None => theme.text(),
        };
        text.push(Line::from(Span::styled(line.clone(), style)));
    }
    text.push(Line::from(Span::styled(
        "↑↓←→ ветка   ⎵ раскрыть   v вид   Esc выход",
        theme.dim(),
    )));

    frame.render_widget(Paragraph::new(text), area);
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let mut out: String = value.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}
