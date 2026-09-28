//! Окно настроек интерфейса.
//!
//! Значения применяются сразу к [`crate::app::App`]. Запись конфигурации из
//! TUI намеренно не делается: процесс не знает, какой из возможных config-файлов
//! владелец считает главным, и не имеет права молча переписывать его.

use ratatui::layout::Rect;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use pulse_core::config::IconSet;

use crate::app::{App, SettingsState};
use crate::icons::Icon;
use crate::theme::Theme;

const WIDTH: u16 = 56;
const HEIGHT: u16 = 13;

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

pub fn render(frame: &mut Frame<'_>, body: Rect, app: &App, state: SettingsState, theme: &Theme) {
    let ascii = matches!(theme.capability, crate::theme::Capability::Ascii);
    let area = centered(body, WIDTH.min(body.width), HEIGHT.min(body.height));
    frame.render_widget(Clear, area);
    let title = crate::i18n::translate(theme.language, "SETTINGS");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(if ascii { ASCII_BORDER } else { border::ROUNDED })
        .padding(Padding::horizontal(1))
        .title(format!(" {title} "))
        .style(theme.text());

    let language = crate::i18n::language_name(theme.language, app.language);
    let icons = crate::i18n::icon_name(theme.language, app.icons);
    let visible_icons = if ascii { IconSet::Off } else { app.icons };
    let preview = if ascii {
        "-".to_string()
    } else {
        preview(visible_icons)
    };
    let mut lines = vec![
        setting_row(
            state.selected == 0,
            crate::i18n::choose(theme.language, "Language", "Язык"),
            language,
            ascii,
            theme,
        ),
        setting_row(
            state.selected == 1,
            crate::i18n::choose(theme.language, "Icons", "Иконки"),
            icons,
            ascii,
            theme,
        ),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!(
                    "{}  ",
                    crate::i18n::choose(theme.language, "Preview", "Предпросмотр")
                ),
                theme.dim(),
            ),
            Span::styled(preview, theme.accent()),
        ]),
    ];
    lines.push(Line::from(Span::styled(
        match (ascii, app.icons) {
            (true, IconSet::Nerd | IconSet::Unicode) => crate::i18n::choose(
                theme.language,
                "ASCII mode hides icons",
                "ASCII-режим скрывает иконки",
            ),
            (_, IconSet::Nerd) => crate::i18n::choose(
                theme.language,
                "Three different icons = Nerd Font works",
                "Три разные иконки = Nerd Font работает",
            ),
            (_, IconSet::Off) => {
                crate::i18n::choose(theme.language, "Icons are disabled", "Иконки выключены")
            }
            (_, IconSet::Unicode) => crate::i18n::choose(
                theme.language,
                "Works with a regular monospace font",
                "Работает с обычным моноширинным шрифтом",
            ),
        },
        theme.dim(),
    )));
    if !ascii && app.icons == IconSet::Nerd {
        lines.push(Line::from(Span::styled(
            crate::i18n::choose(
                theme.language,
                "Select a Nerd Font Mono in the terminal",
                "Выберите Nerd Font Mono в терминале",
            ),
            theme.dim(),
        )));
    } else {
        lines.push(Line::from(""));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            crate::i18n::choose(
                theme.language,
                "Applies now; persist with ui.language / ui.icons",
                "Применено сейчас; сохраните ui.language / ui.icons",
            ),
            theme.dim(),
        )),
        Line::from(Span::styled(
            if ascii {
                crate::i18n::choose(
                    theme.language,
                    "Up/Down select   Left/Right change   Esc close",
                    "Up/Down выбор   Left/Right изменить   Esc закрыть",
                )
            } else {
                crate::i18n::choose(
                    theme.language,
                    "↑↓ select   ←→ change   Esc close",
                    "↑↓ выбор   ←→ изменить   Esc закрыть",
                )
            },
            theme.accent(),
        )),
    ]);

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn setting_row(
    selected: bool,
    label: &'static str,
    value: &'static str,
    ascii: bool,
    theme: &Theme,
) -> Line<'static> {
    let marker = match (selected, ascii) {
        (true, true) => ">",
        (true, false) => "▸",
        (false, _) => " ",
    };
    let (left, right) = if ascii { ("<", ">") } else { ("←", "→") };
    Line::from(vec![
        Span::styled(
            format!("{marker} {label:<16}"),
            if selected {
                theme.accent()
            } else {
                theme.dim()
            },
        ),
        Span::styled(
            format!("{left}  {value:<12}  {right}"),
            if selected {
                theme.strong()
            } else {
                theme.text()
            },
        ),
    ])
}

fn preview(set: IconSet) -> String {
    if set == IconSet::Off {
        return "—".to_string();
    }
    [Icon::Overview, Icon::ProblemsScreen, Icon::Entities]
        .iter()
        .map(|icon| icon.glyph(set))
        .collect::<Vec<_>>()
        .join("   ")
}

fn centered(outer: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: outer
            .x
            .saturating_add(outer.width.saturating_sub(width) / 2),
        y: outer
            .y
            .saturating_add(outer.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_uses_three_distinct_nerd_glyphs() {
        let glyphs = preview(IconSet::Nerd);
        let distinct: std::collections::HashSet<char> =
            glyphs.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert_eq!(distinct.len(), 3);
        assert!(distinct
            .iter()
            .all(|ch| ('\u{e000}'..='\u{f8ff}').contains(ch)));
    }

    #[test]
    fn measured_copy_fits_the_settings_box() {
        let inner = usize::from(WIDTH.saturating_sub(4));
        for text in [
            "Three different icons = Nerd Font works",
            "Works with a regular monospace font",
            "Applies now; persist with ui.language / ui.icons",
            "Три разные иконки = Nerd Font работает",
            "Работает с обычным моноширинным шрифтом",
            "Применено сейчас; сохраните ui.language / ui.icons",
        ] {
            assert!(text.chars().count() <= inner, "{text:?} не помещается");
        }
    }
}
