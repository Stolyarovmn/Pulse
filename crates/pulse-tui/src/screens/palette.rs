//! Отрисовка палитры команд и справки (`crate::palette`).
//!
//! Модальное окно поверх экрана: рамка и строка запроса — цветом места,
//! команды сгруппированы по месту действия, выбранная строка подсвечена на
//! всю ширину. Справка — та же палитра со всеми местами и алфавитом фигуры
//! состояния внизу (§120): без легенды главный экран нечитаем.

use ratatui::layout::Rect;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, PaletteState};
use crate::palette::{listed, Command, Scope};
use crate::state::StateClass;
use crate::theme::{Capability, Theme};
use crate::ui;

/// Алфавит фигуры состояния в порядке возрастания тревоги.
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

/// Рамка из ASCII для старых терминалов.
const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Строка списка: заголовок группы или команда.
enum Row {
    Group(&'static str),
    Command(usize, &'static Command),
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    body: Rect,
    app: &App,
    state: &PaletteState,
    theme: &Theme,
) {
    let ascii = matches!(theme.capability, Capability::Ascii);
    let commands = listed(app.place(), state.help, &state.query);
    let selected = state.selected.min(commands.len().saturating_sub(1));

    let mut rows: Vec<Row> = Vec::new();
    let mut group: Option<Scope> = None;
    for (index, command) in commands.iter().enumerate() {
        if group != Some(command.scope) {
            group = Some(command.scope);
            rows.push(Row::Group(command.scope.label()));
        }
        rows.push(Row::Command(index, command));
    }

    // Справка занимает тело целиком, палитра — по содержимому.
    let alphabet_rows = if state.help {
        ALPHABET.len().div_ceil(2) + 2
    } else {
        0
    };
    let chrome = 2 + 2 + 2; // рамка, запрос с линией, подсказка с линией
    let wanted = u16::try_from(rows.len().max(1) + chrome + alphabet_rows).unwrap_or(u16::MAX);
    let height = if state.help {
        body.height
    } else {
        wanted.min(body.height)
    };
    let width = body
        .width
        .saturating_sub(4)
        .min(if state.help { 112 } else { 96 })
        .max(body.width.min(40));
    let area = Rect {
        x: body.x + body.width.saturating_sub(width) / 2,
        y: if state.help {
            body.y
        } else {
            body.y + body.height.saturating_sub(height) / 3
        },
        width,
        height,
    };

    let place = match app.place() {
        crate::palette::Place::Inspector => "INSPECTOR",
        crate::palette::Place::Screen(screen) => Scope::Screen(screen).label(),
    };
    let title = if state.help {
        format!(" HELP · every command · here: {place} ")
    } else {
        format!(" COMMANDS · {place} ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(if ascii { ASCII_BORDER } else { border::ROUNDED })
        .border_style(theme.accent())
        .title(Span::styled(title, theme.accent()));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let inner_width = usize::from(inner.width);
    let rule = || {
        Line::from(Span::styled(
            ui::rule(inner.width, theme.capability),
            theme.faint(),
        ))
    };
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled(if ascii { "> " } else { "› " }, theme.accent()),
            Span::styled(state.query.clone(), theme.strong()),
            Span::styled(if ascii { "_" } else { "█" }, theme.accent()),
            Span::styled(
                format!(
                    "{:>width$}",
                    format!("{} of {}", commands.len(), total(app, state)),
                    width = inner_width
                        .saturating_sub(ui::width_of(&state.query) + 3)
                        .max(1)
                ),
                theme.faint(),
            ),
        ]),
        rule(),
    ];

    // Окно списка следует за выбором.
    let room = usize::from(inner.height).saturating_sub(4 + alphabet_rows);
    let cursor = rows
        .iter()
        .position(|row| matches!(row, Row::Command(index, _) if *index == selected))
        .unwrap_or(0);
    let start = cursor.saturating_sub(room.saturating_sub(1));
    if commands.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  no command matches \"{}\"", state.query),
            theme.dim(),
        )));
    }
    for row in rows.iter().skip(start).take(room) {
        match row {
            Row::Group(label) => {
                lines.push(Line::from(Span::styled(
                    format!("  {label}"),
                    theme.faint(),
                )));
            }
            Row::Command(index, command) => {
                lines.push(command_line(
                    command,
                    *index == selected,
                    inner.width,
                    theme,
                ));
            }
        }
    }
    while lines.len() < 2 + room {
        lines.push(Line::from(""));
    }

    if state.help {
        lines.push(rule());
        lines.push(Line::from(Span::styled("  STATE ALPHABET", theme.faint())));
        let half = ALPHABET.len().div_ceil(2);
        for index in 0..half {
            let mut spans = vec![Span::raw("  ")];
            for class in [ALPHABET.get(index), ALPHABET.get(index + half)]
                .into_iter()
                .flatten()
            {
                spans.push(Span::styled(
                    format!("{}  ", class.symbol(theme.capability)),
                    class.style(theme),
                ));
                spans.push(Span::styled(format!("{:<34}", class.label()), theme.text()));
            }
            lines.push(Line::from(spans));
        }
    }

    lines.push(rule());
    let arrows = if ascii { "Up/Down" } else { "↑↓" };
    let close = if state.help {
        "? / Esc close"
    } else {
        "Esc close"
    };
    lines.push(Line::from(Span::styled(
        format!("  {arrows} select   Enter run   {close}   type to filter"),
        theme.dim(),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Сколько команд в этом режиме без фильтра.
fn total(app: &App, state: &PaletteState) -> usize {
    listed(app.place(), state.help, "").len()
}

/// Строка команды: подпись, клавиша цветом места, подсказка приглушённо.
fn command_line(command: &Command, selected: bool, width: u16, theme: &Theme) -> Line<'static> {
    const TITLE: usize = 28;
    const KEYS: usize = 12;
    let hint_width = usize::from(width).saturating_sub(TITLE + KEYS + 4);
    let marker = ui::row_marker(selected);
    let text = |value: &str| ascii_safe(value, theme.capability);
    let mut spans = vec![
        Span::styled(marker, theme.dim()),
        Span::styled(
            format!(
                "{:<TITLE$}",
                ui::truncate(&text(command.title), TITLE, theme.capability)
            ),
            theme.text(),
        ),
        Span::styled(
            format!(
                "{:<KEYS$}",
                ui::truncate(&text(command.keys), KEYS, theme.capability)
            ),
            theme.accent(),
        ),
        Span::styled(
            ui::truncate(&text(command.hint), hint_width, theme.capability),
            theme.dim(),
        ),
    ];
    if selected {
        let background = theme.selection().bg;
        let used: usize = spans.iter().map(|span| ui::width_of(&span.content)).sum();
        for span in &mut spans {
            span.style = ratatui::style::Style {
                bg: background,
                ..span.style
            };
        }
        spans.push(Span::styled(
            " ".repeat(usize::from(width).saturating_sub(used)),
            theme.selection(),
        ));
    }
    Line::from(spans)
}

/// Стрелки подсказок в ASCII-режиме: старый терминал покажет мусор вместо
/// `→`, а подсказка без направления теряет смысл.
fn ascii_safe(value: &str, capability: Capability) -> String {
    if !matches!(capability, Capability::Ascii) {
        return value.to_string();
    }
    value
        .replace('↔', "<->")
        .replace('→', "->")
        .replace('←', "<-")
}
