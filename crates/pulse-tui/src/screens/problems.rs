//! Экран проблем (разделы 14, 15, 126, 127 спецификации v0.7).
//!
//! Пустой экран запрещён (раздел 126): отсутствие проблем - это тоже ответ, и
//! он обязан выглядеть как утверждение «всё в норме столько-то времени», а не
//! как пустое полотно.
//!
//! При наличии проблем экран делится на список и разбор выбранной проблемы
//! (раздел 127): `WHY`, `AFFECTED`, `RECENT`. Слова «вызвал» и «привёл к» не
//! используются - PULSE не имеет права выдавать совпадение за причину
//! (раздел 47.2).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use pulse_core::problem::{Problem, Severity};
use pulse_core::snapshot::Snapshot;
use pulse_core::time::format_duration;
use pulse_core::EventKind;

use crate::app::App;
use crate::glyph::GlyphSurface;
use crate::layout::{bounded, BlockKind, LayoutPlan, Priority, WidthClass};
use crate::state::{StateClass, StateGlyph};
use crate::theme::{Capability, Theme};
use crate::ui;

use super::{rect, section};

/// Отрисовка экрана проблем.
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &mut App,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    app.problems.list_len = snapshot.problems.len();
    if snapshot.problems.is_empty() {
        render_empty(frame, area, snapshot, plan, theme);
        return;
    }
    render_active(frame, area, snapshot, app, plan, theme);
}

/// Пустое состояние (разделы 126, 151, 159).
///
/// Экран отвечает утверждением, а не пустотой. Формулировка нормативная:
/// «observed nominal for N» - это утверждение о длине наблюдения, а не о
/// прошлом системы (раздел 151). Аптайм хоста здесь не участвует вовсе
/// (раздел 152): он относится ко времени, когда PULSE ещё не смотрел.
fn render_empty(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let ascii = matches!(theme.capability, Capability::Ascii);
    let mark = if ascii { "ok" } else { "✓" };
    let observed = format_duration(super::observation(snapshot));
    // Линия раздела не тянется на всю ширину под одной фразой (раздел 144).
    let (width, _) = bounded(BlockKind::Attention, area.width);

    let mut lines = vec![
        Line::from(Span::styled(
            format!("{mark} NO ACTIVE PROBLEMS"),
            theme.strong(),
        )),
        Line::from(""),
    ];

    let resolved = resolved_lines(snapshot, theme);
    if resolved.is_empty() {
        // Истории решённых проблем нет: говорим только то, что знаем.
        lines.push(Line::from(Span::styled(
            format!("No problems have been observed since PULSE started {observed} ago."),
            theme.text(),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("observed nominal for {observed}"),
            theme.text(),
        )));
    }
    lines.push(Line::from(""));

    // Фигура состояния: подтверждение вердикта, а не украшение.
    lines.push(section("STATE", width, plan, theme));
    let glyph = StateGlyph::from_snapshot(snapshot);
    let surface = GlyphSurface::build(&glyph, plan.glyph);
    for row in surface.cells() {
        let mut spans: Vec<Span<'_>> = Vec::new();
        for (index, class) in row.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                class.symbol(theme.capability).to_string(),
                class.style(theme),
            ));
        }
        lines.push(Line::from(spans));
    }

    if area.height >= 14 {
        lines.push(Line::from(""));
        lines.push(section("RECENT RESOLVED", width, plan, theme));
        if resolved.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("no resolved problems in {observed} of observation"),
                theme.dim(),
            )));
        } else {
            lines.extend(resolved);
            lines.push(Line::from(""));
            let arrow = if ascii { "->" } else { "↵" };
            lines.push(Line::from(Span::styled(
                format!("{arrow} open incident history"),
                theme.dim(),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Закрытые проблемы из журнала событий (раздел 126).
fn resolved_lines<'a>(snapshot: &Snapshot, theme: &'a Theme) -> Vec<Line<'a>> {
    snapshot
        .events
        .iter()
        .rev()
        .filter(|event| event.kind == EventKind::ProblemClosed)
        .take(3)
        .map(|event| {
            Line::from(vec![
                Span::styled(format!("{}  ", event.at), theme.dim()),
                Span::styled(
                    StateClass::Degraded.symbol(theme.capability).to_string(),
                    theme.severity(Severity::Info),
                ),
                Span::raw(" "),
                Span::styled(event.entity_name.clone(), theme.text()),
                Span::raw("  "),
                Span::styled("resolved".to_string(), theme.dim()),
            ])
        })
        .collect()
}

/// Экран с активными проблемами (раздел 127).
fn render_active(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &App,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let mut problems: Vec<&Problem> = snapshot.problems.iter().collect();
    problems.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.title.cmp(&b.title)));
    let selected = app.problems.selected.min(problems.len().saturating_sub(1));

    // Список сверху, разбор снизу: сначала «что», потом «почему».
    let list_h = u16::try_from(problems.len() * 2 + 1)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(6).max(3));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(list_h), Constraint::Min(3)])
        .split(area);

    render_list(frame, rect(&chunks, 0), &problems, selected, plan, theme);
    if let Some(problem) = problems.get(selected) {
        render_detail(frame, rect(&chunks, 1), snapshot, problem, plan, theme);
    }
}

/// Список проблем с состоянием слева (раздел 127).
fn render_list(
    frame: &mut Frame<'_>,
    area: Rect,
    problems: &[&Problem],
    selected: usize,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let mut lines = vec![section("CURRENT PROBLEMS", area.width, plan, theme)];
    let width = usize::from(area.width).saturating_sub(6);

    for (index, problem) in problems.iter().enumerate() {
        let class = StateClass::from_severity(problem.severity);
        let is_selected = index == selected;
        lines.push(Line::from(vec![
            Span::styled(
                ui::row_marker(is_selected),
                if is_selected {
                    theme.selection()
                } else {
                    theme.dim()
                },
            ),
            Span::styled(
                format!("{} ", class.symbol(theme.capability)),
                class.style(theme),
            ),
            Span::styled(
                ui::truncate(&problem.title, width, theme.capability),
                if is_selected {
                    theme.selection()
                } else {
                    theme.text()
                },
            ),
        ]));
        if plan.shows(Priority::P2) {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    ui::truncate(&problem.entity_name, width, theme.capability),
                    theme.dim(),
                ),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Разбор выбранной проблемы (раздел 127).
fn render_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    problem: &Problem,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let title = if plan.width_class >= WidthClass::Medium {
        format!("SELECTED / {}", problem.title.to_uppercase())
    } else {
        "SELECTED".to_string()
    };
    let mut lines = vec![section(&title, area.width, plan, theme)];

    // WHY: только измеренные доказательства, без слов о причинности.
    lines.push(Line::from(Span::styled("WHY", theme.dim())));
    if problem.evidence.is_empty() {
        lines.push(Line::from(Span::styled(
            problem.summary.clone(),
            theme.text(),
        )));
    } else {
        for evidence in &problem.evidence {
            let mut spans = vec![
                Span::styled(format!("{}: ", evidence.label), theme.dim()),
                Span::styled(evidence.value.clone(), theme.text()),
            ];
            if let Some(threshold) = evidence.threshold.as_ref() {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(threshold.clone(), theme.dim()));
            }
            lines.push(Line::from(spans));
        }
    }

    // AFFECTED: сущности, у которых есть свои открытые проблемы.
    if plan.shows(Priority::P2) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("AFFECTED", theme.dim())));
        let mut affected: Vec<Span<'_>> = Vec::new();
        for other in &snapshot.problems {
            if other.id == problem.id {
                continue;
            }
            let class = StateClass::from_severity(other.severity);
            if !affected.is_empty() {
                affected.push(Span::raw("    "));
            }
            affected.push(Span::styled(other.entity_name.clone(), theme.text()));
            affected.push(Span::raw(" "));
            affected.push(Span::styled(
                class.symbol(theme.capability).to_string(),
                class.style(theme),
            ));
        }
        if affected.is_empty() {
            affected.push(Span::styled(problem.entity_name.clone(), theme.text()));
        }
        lines.push(Line::from(affected));
    }

    // RECENT: последовательность фактов. Порядок - не причинность (раздел 47).
    if plan.shows(Priority::P3) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("RECENT", theme.dim())));
        let since = format_duration(std::time::Duration::from_millis(
            snapshot
                .at
                .as_millis()
                .saturating_sub(problem.since.as_millis()),
        ));
        lines.push(Line::from(vec![
            Span::styled(format!("{}  ", problem.since), theme.dim()),
            Span::styled(
                StateClass::from_severity(problem.severity)
                    .symbol(theme.capability)
                    .to_string(),
                theme.severity(problem.severity),
            ),
            Span::raw(" "),
            Span::styled(format!("held for {since}"), theme.text()),
            Span::raw("  "),
            Span::styled(
                format!("consecutive ticks: {}", problem.streak),
                theme.dim(),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}
