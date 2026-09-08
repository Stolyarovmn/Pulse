//! Help overlay по фактическому keymap (v0.9 §180, §181).
//!
//! Help не top-level screen и не Tab target. Пока overlay открыт, underlying
//! pane не получает ни одного события; `Esc`, `?` и `F1` закрывают его.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::layout::LayoutPlan;
use crate::state::StateClass;
use crate::theme::Theme;

use super::section;

const NAVIGATION: &[(&str, &str)] = &[
    ("1", "Overview"),
    ("2", "Problems"),
    ("3", "Entities"),
    ("4", "Timeline / Time Machine"),
    ("↑ ↓ / k j", "Select visible row"),
    ("Enter", "Open selected visible object"),
    ("Esc", "Back / close Inspector or overlay"),
    ("Tab", "Next visible pane"),
    ("Shift+Tab", "Previous visible pane"),
    ("/", "Search"),
    (":", "Command palette"),
    ("? / F1", "Help"),
    ("g", "Pipe: facts about the selected entity"),
    ("q / Ctrl+C", "Quit"),
];

const CONTEXT: &[(&str, &str)] = &[
    ("m", "Entities: logical / technical view"),
    ("s", "Entities: next sort"),
    ("f", "Entities: next type filter"),
    ("← →", "Timeline: scrub time"),
    ("+ / -", "Timeline: zoom window"),
    ("F", "Timeline: back to LIVE"),
    ("A / B", "Timeline: comparison marks"),
    ("D", "Semantic A/B diff"),
    ("p", "Pause display; collection continues"),
    (": raw events", "Timeline: secondary raw event stream"),
    (": story", "Timeline: back to Incident Story"),
    ("→ / ←", "Pipe: expand / collapse a branch"),
    ("v", "Pipe: boxes / indent view"),
];

/// Видимый алфавит фигуры состояния (§120), в порядке возрастания.
const ALPHABET: &[StateClass] = &[
    StateClass::Inactive,
    StateClass::Normal,
    StateClass::Active,
    StateClass::Saturated,
    StateClass::Degraded,
    StateClass::Warning,
    StateClass::Critical,
    StateClass::Failed,
];

pub(crate) fn render(frame: &mut Frame<'_>, area: Rect, plan: &LayoutPlan, theme: &Theme) {
    // Два списка клавиш в один столбец не влезают в тело 120x30, а §181
    // требует и полный keymap, и отдельную секцию контекстных клавиш. На
    // широком терминале списки идут в две колонки: это освобождает место под
    // легенду алфавита, без которой главный экран нечитаем.
    let columns = if area.width >= 96 { 2 } else { 1 };
    let cell = usize::from(area.width) / columns;

    let mut lines = vec![section("HELP / NAVIGATION", area.width, plan, theme)];
    lines.push(Line::from(""));
    lines.extend(key_block(NAVIGATION, columns, cell, theme));

    lines.push(Line::from(""));
    lines.push(section("CONTEXT-SPECIFIC", area.width, plan, theme));
    lines.extend(key_block(CONTEXT, columns, cell, theme));

    // §120 задаёт видимый алфавит фигуры. Урезать его с хвоста нельзя - там
    // как раз критические классы, поэтому он либо показан целиком, либо нет.
    let rows = ALPHABET.len().div_ceil(columns);
    let spare = usize::from(area.height).saturating_sub(lines.len());
    if spare >= rows + 2 {
        lines.push(Line::from(""));
        lines.push(section("STATE ALPHABET", area.width, plan, theme));
        for (index, first) in ALPHABET.iter().take(rows).enumerate() {
            let mut spans = alphabet_cell(*first, theme);
            for column in 1..columns {
                if let Some(next) = ALPHABET.get(index + rows * column) {
                    spans.extend(alphabet_cell(*next, theme));
                }
            }
            lines.push(Line::from(spans));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Список клавиш в заданное число колонок.
fn key_block<'a>(
    entries: &'a [(&'a str, &'a str)],
    columns: usize,
    cell: usize,
    theme: &'a Theme,
) -> Vec<Line<'a>> {
    let rows = entries.len().div_ceil(columns);
    (0..rows)
        .map(|row| {
            let mut spans = Vec::new();
            for column in 0..columns {
                if let Some((key, meaning)) = entries.get(row + rows * column) {
                    spans.extend(key_cell(key, meaning, cell, theme));
                }
            }
            Line::from(spans)
        })
        .collect()
}

fn key_cell<'a>(key: &str, meaning: &str, cell: usize, theme: &'a Theme) -> Vec<Span<'a>> {
    let text = cell.saturating_sub(15);
    vec![
        Span::styled(format!("{key:<14}"), theme.strong()),
        Span::styled(format!("{meaning:<text$}"), theme.text()),
    ]
}

/// Одна ячейка алфавита: символ своим цветом и его смысл.
fn alphabet_cell(class: StateClass, theme: &Theme) -> Vec<Span<'_>> {
    vec![
        Span::styled(
            format!("{:<3}", class.symbol(theme.capability)),
            class.style(theme),
        ),
        Span::styled(format!("{:<31}", class.label()), theme.text()),
    ]
}
