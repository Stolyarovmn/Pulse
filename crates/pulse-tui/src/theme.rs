//! Визуальный язык.
//!
//! Монохромная сетка: цвет используется только как носитель состояния
//! (норма/предупреждение/критично), а не для украшения. Это делает экран
//! читаемым и в ssh-сессии, и на монохромном терминале, и снимает у оператора
//! задачу «расшифровать палитру».
//!
//! Три уровня возможностей терминала. Различие не косметическое: в ASCII-режиме
//! не должно остаться ни одного символа вне ASCII, иначе вывод в старом
//! терминале превращается в кашу.

use ratatui::style::{Color, Modifier, Style};

use pulse_core::problem::Severity;

/// Уровень возможностей терминала.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// 24-битный цвет.
    TrueColor,
    /// 256 цветов.
    Ansi256,
    /// Только ASCII и базовые цвета.
    Ascii,
}

impl Capability {
    /// Определяет возможности по переменным окружения.
    ///
    /// `ascii_forced` приходит из конфигурации: явное требование пользователя
    /// важнее автоопределения.
    #[must_use]
    pub fn detect(ascii_forced: bool) -> Self {
        if ascii_forced {
            return Capability::Ascii;
        }
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            return Capability::TrueColor;
        }
        let term = std::env::var("TERM").unwrap_or_default();
        if term.contains("256color") {
            return Capability::Ansi256;
        }
        if term.is_empty() || term == "dumb" {
            return Capability::Ascii;
        }
        Capability::Ansi256
    }
}

/// Набор глифов. В ASCII-режиме — только ASCII.
#[derive(Clone, Copy, Debug)]
pub struct Glyphs {
    pub bar_full: char,
    pub bar_empty: char,
    pub arrow_right: &'static str,
    pub selected: &'static str,
    pub bullet: &'static str,
    pub blocks: &'static [char],
}

impl Glyphs {
    #[must_use]
    pub fn for_capability(capability: Capability) -> Self {
        if capability == Capability::Ascii {
            Glyphs {
                bar_full: '#',
                bar_empty: '.',
                arrow_right: "->",
                selected: ">",
                bullet: "*",
                blocks: &['.', ':', '-', '=', '+', '*', '#', '@'],
            }
        } else {
            Glyphs {
                bar_full: '█',
                bar_empty: '░',
                arrow_right: "→",
                selected: "▸",
                bullet: "•",
                blocks: &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'],
            }
        }
    }
}

/// Тема оформления.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub capability: Capability,
    pub glyphs: Glyphs,
}

impl Theme {
    #[must_use]
    pub fn new(ascii_forced: bool) -> Self {
        let capability = Capability::detect(ascii_forced);
        Theme {
            capability,
            glyphs: Glyphs::for_capability(capability),
        }
    }

    #[must_use]
    pub fn with_capability(capability: Capability) -> Self {
        Theme {
            capability,
            glyphs: Glyphs::for_capability(capability),
        }
    }

    /// Основной текст.
    #[must_use]
    pub fn text(&self) -> Style {
        Style::default().fg(Color::Gray)
    }

    /// Акцентированный текст: числа, имена сущностей.
    #[must_use]
    pub fn strong(&self) -> Style {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    }

    /// Второстепенный текст: подписи, единицы.
    #[must_use]
    pub fn dim(&self) -> Style {
        Style::default().fg(Color::DarkGray)
    }

    /// Выделенная строка списка.
    #[must_use]
    pub fn selection(&self) -> Style {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Gray)
            .add_modifier(Modifier::BOLD)
    }

    /// Цвет штатной активной массы глифа.
    ///
    /// Отдельно от [`Theme::strong`]: белый акцент на девяти строках фигуры
    /// превращает её в световое пятно и убивает читаемость severity.
    #[must_use]
    pub fn nominal(&self) -> Style {
        Style::default().fg(Color::Green)
    }

    /// Цвет состояния. Единственное место, где цвет несёт смысл.
    #[must_use]
    pub fn severity(&self, severity: Severity) -> Style {
        let color = match severity {
            Severity::Info => Color::Cyan,
            Severity::Warn => Color::Yellow,
            Severity::Crit => Color::Red,
        };
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    }

    /// Цвет для доли: спокойный до 0.7, предупреждение до 0.9, дальше критично.
    #[must_use]
    pub fn ratio(&self, value: f64) -> Style {
        if value >= 0.9 {
            self.severity(Severity::Crit)
        } else if value >= 0.7 {
            self.severity(Severity::Warn)
        } else {
            self.text()
        }
    }

    /// Горизонтальная полоса заполнения.
    #[must_use]
    pub fn bar(&self, value: f64, width: usize) -> String {
        let clamped = value.clamp(0.0, 1.0);
        let filled = ((clamped * width as f64).round() as usize).min(width);
        let mut out = String::with_capacity(width);
        for _ in 0..filled {
            out.push(self.glyphs.bar_full);
        }
        for _ in filled..width {
            out.push(self.glyphs.bar_empty);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_mode_uses_only_ascii_glyphs() {
        let theme = Theme::with_capability(Capability::Ascii);
        assert!(theme.glyphs.bar_full.is_ascii());
        assert!(theme.glyphs.bar_empty.is_ascii());
        assert!(theme.glyphs.arrow_right.is_ascii());
        assert!(theme.glyphs.selected.is_ascii());
        assert!(theme.glyphs.blocks.iter().all(char::is_ascii));
        assert!(theme.bar(0.5, 10).is_ascii());
    }

    #[test]
    fn unicode_mode_uses_block_glyphs() {
        let theme = Theme::with_capability(Capability::TrueColor);
        assert_eq!(theme.glyphs.bar_full, '█');
        assert_eq!(theme.glyphs.blocks.len(), 8);
    }

    #[test]
    fn bar_length_is_exact_and_clamped() {
        let theme = Theme::with_capability(Capability::Ascii);
        assert_eq!(theme.bar(0.0, 10).chars().count(), 10);
        assert_eq!(theme.bar(1.0, 10).chars().count(), 10);
        assert_eq!(theme.bar(2.0, 10), "##########");
        assert_eq!(theme.bar(-1.0, 4), "....");
        assert_eq!(theme.bar(0.5, 4), "##..");
    }

    #[test]
    fn ascii_forced_overrides_detection() {
        assert_eq!(Capability::detect(true), Capability::Ascii);
    }

    #[test]
    fn severity_colors_are_distinct() {
        let theme = Theme::with_capability(Capability::TrueColor);
        let info = theme.severity(Severity::Info);
        let warn = theme.severity(Severity::Warn);
        let crit = theme.severity(Severity::Crit);
        assert_ne!(info.fg, warn.fg);
        assert_ne!(warn.fg, crit.fg);
    }

    #[test]
    fn ratio_style_escalates_with_value() {
        let theme = Theme::with_capability(Capability::TrueColor);
        assert_eq!(theme.ratio(0.1).fg, theme.text().fg);
        assert_eq!(theme.ratio(0.75).fg, theme.severity(Severity::Warn).fg);
        assert_eq!(theme.ratio(0.95).fg, theme.severity(Severity::Crit).fg);
    }
}
