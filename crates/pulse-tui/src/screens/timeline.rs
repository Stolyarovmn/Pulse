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

    render_rail(frame, rect(&top, 0), snapshot, &story, app, theme);
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

/// Отметка события на оси времени.
///
/// Ось без отметок — прямая линия, по которой нельзя понять, когда именно
/// что-то случилось: она сообщала только «наблюдение началось тогда,
/// сейчас — теперь». Отметки ставят события в их момент, а подписи дают
/// таймкод, то есть ось становится измерением, а не рамкой.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RailMark {
    /// Колонка внутри оси.
    column: usize,
    class: StateClass,
    at: Timestamp,
}

/// Раскладывает события по колонкам оси.
///
/// Отдельная чистая функция: раскладка проверяется тестом, а рисование
/// остаётся тривиальным. При совпадении колонок остаётся худшее состояние —
/// потерять `crit` из-за соседнего `normal` недопустимо.
fn rail_marks(story: &[StoryRow], start: Timestamp, end: Timestamp, width: usize) -> Vec<RailMark> {
    let inner = width.saturating_sub(2);
    if inner == 0 {
        return Vec::new();
    }
    let span = end.as_millis().saturating_sub(start.as_millis()).max(1);
    let mut marks: Vec<RailMark> = Vec::new();
    for row in story {
        let Some(class) = state_transition(row.kind) else {
            continue;
        };
        if row.at.as_millis() < start.as_millis() {
            continue;
        }
        let offset = row.at.as_millis().saturating_sub(start.as_millis());
        let column = usize::try_from(offset * (inner as u64) / span)
            .unwrap_or(inner - 1)
            .min(inner - 1)
            + 1;
        match marks.iter_mut().find(|mark| mark.column == column) {
            // Порядок `StateClass` идёт от нормы к отказу, поэтому «худшее»
            // это максимум, а не отдельная таблица приоритетов.
            Some(existing) if class > existing.class => {
                existing.class = class;
                existing.at = row.at;
            }
            Some(_) => {}
            None => marks.push(RailMark {
                column,
                class,
                at: row.at,
            }),
        }
    }
    marks.sort_unstable_by_key(|mark| mark.column);
    marks
}

/// Строка подписей: таймкод под отметкой, пока хватает места.
///
/// Подписать все отметки нельзя — таймкод занимает восемь колонок, и
/// подписи наложились бы друг на друга. Поэтому слева направо берутся те,
/// что укладываются без пересечения: подпись обязана читаться, а остальные
/// отметки остаются видимы на оси.
fn rail_labels(marks: &[RailMark], width: usize) -> String {
    let mut out = String::new();
    for mark in marks {
        let label = mark.at.to_string();
        let start_at = mark.column.saturating_sub(label.chars().count() / 2);
        if start_at < out.chars().count() + 1 {
            continue;
        }
        if start_at + label.chars().count() > width {
            break;
        }
        out.push_str(&" ".repeat(start_at - out.chars().count()));
        out.push_str(&label);
    }
    out
}

/// Observation boundary → LIVE cursor. До boundary состояние неизвестно.
fn render_rail(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    story: &[StoryRow],
    app: &App,
    theme: &Theme,
) {
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

    let marks = rail_marks(story, start, cursor, width);
    let mut axis: Vec<Span<'_>> = vec![Span::styled(start_mark.to_string(), theme.strong())];
    let mut column = 1;
    for mark in &marks {
        if mark.column > column {
            axis.push(Span::styled(
                std::iter::repeat_n(line_char, mark.column - column).collect::<String>(),
                theme.dim(),
            ));
            column = mark.column;
        }
        axis.push(Span::styled(
            mark.class.symbol(theme.capability).to_string(),
            mark.class.style(theme),
        ));
        column += 1;
    }
    if column <= inner {
        axis.push(Span::styled(
            std::iter::repeat_n(line_char, inner + 1 - column).collect::<String>(),
            theme.dim(),
        ));
    }
    axis.push(Span::styled(live_mark.to_string(), theme.strong()));

    // Третья строка несёт таймкоды отметок; пока отметок нет, она остаётся
    // прежней подписью границ наблюдения — пустой строки в кадре не бывает.
    let footer = if marks.is_empty() {
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
        ])
    } else {
        Line::from(Span::styled(rail_labels(&marks, width), theme.dim()))
    };

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(left, theme.dim()),
                Span::raw(" ".repeat(padding)),
                Span::styled(label, theme.strong()),
            ]),
            Line::from(axis),
            footer,
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
                    crate::format::ratio_lane(&points, width, (from, to), theme),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: EventKind, at_ms: u64) -> StoryRow {
        StoryRow {
            at: Timestamp::from_millis(at_ms),
            last_at: Timestamp::from_millis(at_ms),
            kind,
            name: "demo-host".to_string(),
            detail: String::new(),
            count: 1,
        }
    }

    /// Отметки обязаны стоять в момент события, а не в начале оси.
    ///
    /// Дефект с живого прогона: ось была прямой линией, по которой нельзя
    /// понять, когда именно открылась проблема.
    #[test]
    fn marks_stand_at_the_moment_of_the_event() {
        let start = Timestamp::from_millis(0);
        let end = Timestamp::from_millis(100_000);
        let story = [
            row(EventKind::ProblemOpened, 25_000),
            row(EventKind::ProblemClosed, 75_000),
        ];
        let marks = rail_marks(&story, start, end, 42);

        assert_eq!(marks.len(), 2, "оба события обязаны попасть на ось");
        assert!(
            marks[0].column < marks[1].column,
            "порядок на оси обязан совпадать с порядком во времени: {marks:?}"
        );
        // Четверть окна из сорока внутренних колонок — десятая колонка.
        assert_eq!(
            marks[0].column, 11,
            "позиция считается по времени: {marks:?}"
        );
        assert_eq!(marks[1].column, 31);
        assert_eq!(marks[0].class, StateClass::Warning);
        assert_eq!(marks[1].class, StateClass::Normal);
    }

    /// В одной колонке остаётся худшее состояние: потерять отказ из-за
    /// соседнего восстановления недопустимо.
    #[test]
    fn worst_state_wins_a_shared_column() {
        let story = [
            row(EventKind::ProblemClosed, 1_000),
            row(EventKind::CollectorError, 1_100),
        ];
        let marks = rail_marks(
            &story,
            Timestamp::from_millis(0),
            Timestamp::from_millis(100_000),
            20,
        );
        assert_eq!(marks.len(), 1, "события попали в одну колонку: {marks:?}");
        assert_eq!(marks[0].class, StateClass::Failed);
    }

    /// События до начала окна на ось не попадают: ось описывает окно,
    /// а не всю историю.
    #[test]
    fn events_before_the_window_are_dropped() {
        let story = [row(EventKind::ProblemOpened, 500)];
        let marks = rail_marks(
            &story,
            Timestamp::from_millis(10_000),
            Timestamp::from_millis(20_000),
            40,
        );
        assert!(marks.is_empty(), "{marks:?}");
    }

    /// Подписи обязаны читаться: наложение таймкодов запрещено, поэтому
    /// часть отметок остаётся без подписи, но видимой на оси.
    #[test]
    fn labels_never_overlap() {
        let marks = [
            RailMark {
                column: 3,
                class: StateClass::Warning,
                at: Timestamp::from_millis(3_000),
            },
            RailMark {
                column: 5,
                class: StateClass::Normal,
                at: Timestamp::from_millis(5_000),
            },
            RailMark {
                column: 40,
                class: StateClass::Critical,
                at: Timestamp::from_millis(40_000),
            },
        ];
        let line = rail_labels(&marks, 60);
        let stamps: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            stamps.len(),
            2,
            "вторая отметка стоит слишком близко и подписи не получает: {line:?}"
        );
        assert!(
            line.chars().count() <= 60,
            "строка подписей не имеет права выходить за кадр: {line:?}"
        );
    }
}
