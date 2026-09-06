//! Time Machine по normative layout v0.9 (§182–§186).
//!
//! Основной режим — time rail + State River + metric lanes + Incident Story +
//! Snapshot/Details. Raw events не являются default view. Снимок содержит лишь
//! текущие значения, поэтому metric history не выдумывается: до подключения
//! настоящего History source дорожки честно показывают `collecting history`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use pulse_core::snapshot::Snapshot;
use pulse_core::{Event, EventKind, Timestamp};

use crate::app::{raw_events, story_events, App};
use crate::layout::{LayoutPlan, Pane};
use crate::state::StateClass;
use crate::theme::{Capability, Theme};
use crate::ui;

use super::{focused_section, observation, rect};

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &mut App,
    plan: &LayoutPlan,
    theme: &Theme,
    history: Option<&pulse_store::History>,
) {
    // Raw поток — secondary subview из палитры; default остаётся Story (§183).
    let story: Vec<StoryRow> = if app.timeline.raw_events {
        raw_events(snapshot)
            .into_iter()
            .map(StoryRow::from_raw)
            .collect()
    } else {
        story_events(snapshot)
            .into_iter()
            .map(StoryRow::from_meaningful)
            .collect()
    };
    app.timeline.story_len = story.len();
    let selected = app
        .timeline
        .story_selected
        .min(story.len().saturating_sub(1));

    let top = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(3),
        ])
        .split(area);

    render_rail(frame, rect(&top, 0), snapshot, app, theme);
    render_state_river(frame, rect(&top, 1), snapshot, &story, theme);
    render_metric_lanes(frame, rect(&top, 2), snapshot, app, theme, history);

    let lower = rect(&top, 3);
    if app.pane_is_visible(Pane::Inspector) {
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(60),
                Constraint::Length(3),
                Constraint::Min(30),
            ])
            .split(lower);
        render_story(
            frame,
            rect(&halves, 0),
            &story,
            selected,
            (app.pane == Pane::Story, app.timeline.raw_events),
            plan,
            theme,
        );
        render_details(
            frame,
            rect(&halves, 2),
            story.get(selected),
            app.pane == Pane::Inspector,
            plan,
            theme,
        );
    } else {
        render_story(
            frame,
            lower,
            &story,
            selected,
            (app.pane == Pane::Story, app.timeline.raw_events),
            plan,
            theme,
        );
    }
}

/// Одна строка Story или Raw Events.
///
/// Два источника (сырой журнал и значимые события) рендерятся одним путём:
/// иначе Story и Raw расходятся по вёрстке и по семантике маркеров.
#[derive(Clone, Debug)]
pub(crate) struct StoryRow {
    at: Timestamp,
    last_at: Timestamp,
    kind: EventKind,
    name: String,
    detail: String,
    /// Сколько сырых наблюдений свёрнуто; 1 — одиночное событие.
    count: u32,
}

impl StoryRow {
    fn from_raw(event: &Event) -> Self {
        StoryRow {
            at: event.at,
            last_at: event.at,
            kind: event.kind,
            name: event.entity_name.clone(),
            detail: event.detail.clone(),
            count: 1,
        }
    }

    fn from_meaningful(event: &pulse_core::MeaningfulEvent) -> Self {
        StoryRow {
            at: event.at,
            last_at: event.last_at,
            kind: event.kind,
            name: event.entity_name.clone(),
            detail: event.detail.clone(),
            count: event.count,
        }
    }

    /// Группа раскрывается по `Enter`: у неё есть first/last/count.
    const fn is_group(&self) -> bool {
        self.count > 1
    }
}

/// Observation boundary → LIVE cursor. До boundary состояние неизвестно.
fn render_rail(frame: &mut Frame<'_>, area: Rect, snapshot: &Snapshot, app: &App, theme: &Theme) {
    let ascii = matches!(theme.capability, Capability::Ascii);
    let start = snapshot
        .entity(snapshot.host)
        .map_or(snapshot.at, |host| host.first_seen);
    let cursor = app.timeline.time_cursor.unwrap_or(snapshot.at);
    let label = if app.timeline.time_cursor.is_some() {
        format!("HISTORY ◀ {cursor}")
    } else {
        "NOW".to_string()
    };
    let width = usize::from(area.width).max(8);
    let left = start.to_string();
    let padding = width
        .saturating_sub(left.chars().count() + label.chars().count())
        .max(1);
    let line_char = if ascii { '-' } else { '─' };
    let start_mark = if ascii { '*' } else { '●' };
    let live_mark = if ascii { '*' } else { '●' };
    let inner = width.saturating_sub(2);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(left, theme.dim()),
                Span::raw(" ".repeat(padding)),
                Span::styled(label, theme.strong()),
            ]),
            Line::from(vec![
                Span::styled(start_mark.to_string(), theme.strong()),
                Span::styled(
                    std::iter::repeat_n(line_char, inner).collect::<String>(),
                    theme.dim(),
                ),
                Span::styled(live_mark.to_string(), theme.strong()),
            ]),
            Line::from(vec![
                Span::styled("observation started", theme.dim()),
                Span::raw(" ".repeat(width.saturating_sub(28))),
                Span::styled(
                    if app.timeline.time_cursor.is_some() {
                        "cursor"
                    } else {
                        "live"
                    },
                    theme.strong(),
                ),
            ]),
        ]),
        area,
    );
}

/// Spaced State River только из наблюдавшихся state transitions.
fn render_state_river(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    story: &[StoryRow],
    theme: &Theme,
) {
    let mut classes: Vec<StateClass> = story
        .iter()
        .rev()
        .filter_map(|event| state_transition(event.kind))
        .collect();
    let current = snapshot
        .worst_severity()
        .map_or(StateClass::Normal, StateClass::from_severity);
    classes.push(current);

    let mut spans = vec![Span::styled("STATE  ", theme.dim())];
    for (index, class) in classes.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            class.symbol(theme.capability).to_string(),
            class.style(theme),
        ));
    }
    if classes.len() <= 1 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!(
                "observed {} · collecting state history",
                pulse_core::time::format_duration(observation(snapshot))
            ),
            theme.dim(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Metric lanes: реальные точки из горячего окна истории.
///
/// Пока точек меньше двух, форма кривой неизвестна, и §186 требует честного
/// `collecting history` вместо ровной линии-заглушки.
fn render_metric_lanes(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &App,
    theme: &Theme,
    history: Option<&pulse_store::History>,
) {
    let dot = if matches!(theme.capability, Capability::Ascii) {
        "."
    } else {
        "·"
    };
    let to = app.timeline.time_cursor.unwrap_or(snapshot.at);
    let from = to.saturating_sub_millis(app.timeline.zoom_ms);
    // Шесть колонок на подпись, пять на текущее значение: без числа дорожка
    // по абсолютной шкале выглядит мёртвой (9% CPU навсегда у пола), а врать
    // амплитудой относительной нормировки нельзя - §186.
    let width = usize::from(area.width).saturating_sub(12).max(4);

    let lanes = [
        ("CPU", pulse_core::metric::ids::HOST_CPU_UTIL),
        ("MEM", pulse_core::metric::ids::HOST_MEM_UTIL),
        ("IO", pulse_core::metric::ids::HOST_PSI_IO_FULL_AVG10),
    ];
    let lines: Vec<Line<'_>> = lanes
        .iter()
        .map(|(label, metric)| {
            let points = history.map_or_else(Vec::new, |history| {
                history.series_points(
                    pulse_core::sample::SeriesKey::new(snapshot.host, *metric),
                    from,
                    to,
                )
            });
            let current = points
                .last()
                .map(|(_, value)| *value)
                .or_else(|| snapshot.value(snapshot.host, *metric));
            let reading = current.map_or_else(
                || "   — ".to_string(),
                |value| format!("{:>4} ", crate::format::percent(value)),
            );
            let lane = if points.len() >= 2 {
                // Дорожка рисуется по абсолютной шкале доли и приглушённым
                // цветом: три ярких ряда на всю ширину читаются как полотно,
                // а не как измерение.
                Span::styled(
                    crate::format::ratio_lane(&points, width, theme),
                    theme.dim(),
                )
            } else {
                Span::styled(format!("{dot} collecting history"), theme.dim())
            };
            Line::from(vec![
                Span::styled(format!("{label:<6}"), theme.dim()),
                Span::styled(reading, theme.text()),
                lane,
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_story(
    frame: &mut Frame<'_>,
    area: Rect,
    story: &[StoryRow],
    selected: usize,
    view: (bool, bool),
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let (focused, raw) = view;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    frame.render_widget(
        Paragraph::new(focused_section(
            if raw { "RAW EVENTS" } else { "STORY" },
            area.width,
            focused,
            plan,
            theme,
        )),
        rect(&chunks, 0),
    );

    let body = rect(&chunks, 1);
    let mut lines: Vec<Line<'_>> = Vec::new();
    if story.is_empty() {
        lines.push(Line::from(Span::styled(
            "no meaningful changes observed",
            theme.dim(),
        )));
    }
    let visible = usize::from(body.height);
    let start = selected.saturating_sub(visible.saturating_sub(1));
    for (index, event) in story.iter().enumerate().skip(start).take(visible) {
        let selected_row = index == selected;
        let marker = event_marker(event.kind, theme.capability);
        let summary = if event.kind == EventKind::ObservationStarted {
            "PULSE observation started".to_string()
        } else if event.is_group() {
            // Группа сообщает факт и его объём, а не повторяет одну строку N раз.
            format!("{} ×{}", event.name, event.count)
        } else if event.detail.is_empty() {
            event.name.clone()
        } else {
            format!("{} — {}", event.name, event.detail)
        };
        let text = format!(
            "{}{} {} {}",
            ui::row_marker(selected_row),
            event.at,
            marker,
            ui::truncate(
                &summary,
                usize::from(body.width).saturating_sub(15),
                theme.capability
            )
        );
        lines.push(Line::from(Span::styled(
            text,
            if selected_row {
                theme.selection()
            } else {
                theme.text()
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines), body);
}

fn render_details(
    frame: &mut Frame<'_>,
    area: Rect,
    selected: Option<&StoryRow>,
    focused: bool,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let title = selected.map_or("SNAPSHOT / DETAILS".to_string(), |event| {
        format!("SNAPSHOT / {}", event.at)
    });
    let mut lines = vec![focused_section(
        &ui::truncate(&title, usize::from(area.width), theme.capability),
        area.width,
        focused,
        plan,
        theme,
    )];
    let Some(event) = selected else {
        lines.push(Line::from(Span::styled("nothing selected", theme.dim())));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };

    if event.kind == EventKind::ObservationStarted {
        lines.push(Line::from(Span::styled("BASELINE", theme.strong())));
        let total = event
            .detail
            .split(';')
            .next()
            .unwrap_or(&event.detail)
            .trim_start_matches("baseline:")
            .split_whitespace()
            .next()
            .unwrap_or("0");
        lines.push(pair("entities", total.to_string(), theme));
        for (name, count) in baseline_breakdown(&event.detail) {
            lines.push(pair(&name, count.to_string(), theme));
        }
    } else {
        let class = state_transition(event.kind);
        if let Some(class) = class {
            lines.push(pair(
                "STATE",
                class.symbol(theme.capability).to_string(),
                theme,
            ));
        }
        lines.push(pair("event", event.kind.as_str().to_string(), theme));
        if !event.name.is_empty() {
            lines.push(pair("entity", event.name.clone(), theme));
        }
        if event.is_group() {
            // Раскрытие группы: сколько повторов и в каких границах времени.
            lines.push(pair("count", event.count.to_string(), theme));
            lines.push(pair("first", event.at.to_string(), theme));
            lines.push(pair("last", event.last_at.to_string(), theme));
        }
        if !event.detail.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(event.detail.clone(), theme.text())));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn pair<'a>(label: &str, value: String, theme: &'a Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:<14}"), theme.dim()),
        Span::styled(value, theme.text()),
    ])
}

#[must_use]
pub(crate) fn baseline_breakdown(detail: &str) -> Vec<(String, u64)> {
    let Some((_, tail)) = detail.split_once(';') else {
        return Vec::new();
    };
    tail.split(',')
        .filter_map(|part| {
            let (name, count) = part.trim().rsplit_once(' ')?;
            Some((name.to_string(), count.parse().ok()?))
        })
        .collect()
}

const fn state_transition(kind: EventKind) -> Option<StateClass> {
    match kind {
        EventKind::ProblemOpened => Some(StateClass::Warning),
        EventKind::OomKill => Some(StateClass::Critical),
        EventKind::CollectorError => Some(StateClass::Failed),
        EventKind::ProblemClosed => Some(StateClass::Normal),
        _ => None,
    }
}

fn event_marker(kind: EventKind, capability: Capability) -> char {
    let ascii = matches!(capability, Capability::Ascii);
    match kind {
        EventKind::ObservationStarted => {
            if ascii {
                '*'
            } else {
                '●'
            }
        }
        EventKind::ProblemClosed => {
            if ascii {
                'o'
            } else {
                '○'
            }
        }
        EventKind::ProblemOpened => {
            if ascii {
                '*'
            } else {
                '◆'
            }
        }
        EventKind::CollectorError => {
            if ascii {
                'x'
            } else {
                '×'
            }
        }
        // Restart/OOM/deploy-like lifecycle facts are discrete events.
        // `!` is event punctuation, never a State Glyph cell (§183–186).
        _ => '!',
    }
}
