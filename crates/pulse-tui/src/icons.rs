//! Иконки интерфейса: один словарь на два набора.
//!
//! Набор выбирается настройкой `ui.icons` и действует на весь интерфейс, а не
//! только на пайп: вкладки, виды сущностей, серьёзность проблем, режим
//! наблюдения. Словарь один, потому что иначе наборы расходятся — часть
//! экранов получает глифы, часть остаётся без них, и оператор не может
//! опереться на форму символа.
//!
//! Два набора существуют по измеримой причине: глифы Nerd Font лежат
//! в приватной области Unicode, и в шрифте без патча вместо иконки рисуется
//! пустой прямоугольник. Запасной набор взят целиком из блока геометрических
//! фигур `U+25xx`, который есть в любом моноширинном шрифте и всюду одинарной
//! ширины — рамки от него не разъезжаются.

use pulse_core::config::IconSet;

/// Смысл иконки. Не символ, а роль: символ выбирает набор.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    // Категории фактов пайпа.
    Exe,
    User,
    Cwd,
    Ports,
    Owner,
    Limits,
    Files,
    Procs,
    Log,
    Journal,
    Problems,
    Resources,
    // Виды сущностей.
    Host,
    Unit,
    Cgroup,
    Process,
    Container,
    Pod,
    Disk,
    NetIf,
    // Экраны и режимы.
    Overview,
    ProblemsScreen,
    Entities,
    Timeline,
    Search,
    Commands,
    Help,
    Live,
    Paused,
    // Серьёзность.
    Warn,
    Crit,
    Ok,
}

impl Icon {
    /// Полный перечень: нужен тестам покрытия набора.
    pub const ALL: [Self; 32] = [
        Self::Exe,
        Self::User,
        Self::Cwd,
        Self::Ports,
        Self::Owner,
        Self::Limits,
        Self::Files,
        Self::Procs,
        Self::Log,
        Self::Journal,
        Self::Problems,
        Self::Resources,
        Self::Host,
        Self::Unit,
        Self::Cgroup,
        Self::Process,
        Self::Container,
        Self::Pod,
        Self::Disk,
        Self::NetIf,
        Self::Overview,
        Self::ProblemsScreen,
        Self::Entities,
        Self::Timeline,
        Self::Search,
        Self::Commands,
        Self::Help,
        Self::Live,
        Self::Paused,
        Self::Warn,
        Self::Crit,
        Self::Ok,
    ];

    /// Символ для выбранного набора; `IconSet::Off` даёт пустую строку.
    #[must_use]
    pub const fn glyph(self, set: IconSet) -> &'static str {
        match set {
            IconSet::Off => "",
            IconSet::Nerd => self.nerd(),
            IconSet::Unicode => self.geometric(),
        }
    }

    /// Глиф Nerd Font из диапазона Font Awesome (`U+F000…U+F2FF`).
    ///
    /// Диапазон выбран не случайно: он присутствует во всех патченных шрифтах
    /// проекта, тогда как более экзотические блоки (Material, Weather) есть
    /// только в полных сборках.
    const fn nerd(self) -> &'static str {
        match self {
            Self::Exe => "\u{f085}",
            Self::User => "\u{f007}",
            Self::Cwd => "\u{f07b}",
            Self::Ports => "\u{f1e6}",
            Self::Owner => "\u{f0e8}",
            Self::Limits => "\u{f0e4}",
            Self::Files => "\u{f15c}",
            Self::Procs => "\u{f0ae}",
            Self::Log => "\u{f0f6}",
            Self::Journal => "\u{f02d}",
            Self::Problems => "\u{f071}",
            Self::Resources => "\u{f1c0}",
            Self::Host => "\u{f109}",
            Self::Unit => "\u{f013}",
            Self::Cgroup => "\u{f247}",
            Self::Process => "\u{f04b}",
            Self::Container => "\u{f1b3}",
            Self::Pod => "\u{f1b2}",
            Self::Disk => "\u{f0a0}",
            Self::NetIf => "\u{f0ac}",
            Self::Overview => "\u{f200}",
            Self::ProblemsScreen => "\u{f06a}",
            Self::Entities => "\u{f03a}",
            Self::Timeline => "\u{f017}",
            Self::Search => "\u{f002}",
            Self::Commands => "\u{f120}",
            Self::Help => "\u{f059}",
            Self::Live => "\u{f111}",
            Self::Paused => "\u{f04c}",
            Self::Warn => "\u{f071}",
            Self::Crit => "\u{f057}",
            Self::Ok => "\u{f058}",
        }
    }

    /// Запасной набор: только блок `U+25xx`.
    ///
    /// Ограничение блоком — не вкусовое: символы этого блока одинарной ширины
    /// в любом моноширинном шрифте, поэтому включение набора не сдвигает
    /// колонки и не ломает рамки боксов.
    const fn geometric(self) -> &'static str {
        match self {
            Self::Exe => "▣",
            Self::User => "◉",
            Self::Cwd => "▸",
            Self::Ports => "◇",
            Self::Owner => "▲",
            Self::Limits => "◍",
            Self::Files => "▤",
            Self::Procs => "▪",
            Self::Log => "▨",
            Self::Journal => "▧",
            Self::Problems => "◆",
            Self::Resources => "○",
            Self::Host => "▩",
            Self::Unit => "◈",
            Self::Cgroup => "▥",
            Self::Process => "▮",
            Self::Container => "▦",
            Self::Pod => "▯",
            Self::Disk => "◗",
            Self::NetIf => "◌",
            Self::Overview => "◰",
            Self::ProblemsScreen => "◭",
            Self::Entities => "▬",
            Self::Timeline => "◷",
            Self::Search => "◎",
            Self::Commands => "▷",
            Self::Help => "◊",
            Self::Live => "●",
            Self::Paused => "◫",
            Self::Warn => "◬",
            Self::Crit => "◙",
            Self::Ok => "◦",
        }
    }
}

/// Иконка вида сущности.
#[must_use]
pub const fn for_kind(kind: pulse_core::EntityKind) -> Icon {
    match kind {
        pulse_core::EntityKind::Host => Icon::Host,
        pulse_core::EntityKind::Unit => Icon::Unit,
        pulse_core::EntityKind::Cgroup => Icon::Cgroup,
        pulse_core::EntityKind::Process => Icon::Process,
        pulse_core::EntityKind::Container => Icon::Container,
        pulse_core::EntityKind::Pod => Icon::Pod,
        pulse_core::EntityKind::Disk => Icon::Disk,
        pulse_core::EntityKind::NetIf => Icon::NetIf,
    }
}

/// Иконка серьёзности.
#[must_use]
pub const fn for_severity(severity: pulse_core::Severity) -> Icon {
    match severity {
        pulse_core::Severity::Info => Icon::Ok,
        pulse_core::Severity::Warn => Icon::Warn,
        pulse_core::Severity::Crit => Icon::Crit,
    }
}

/// Готовый префикс: иконка и пробел либо пустая строка.
///
/// Отдельная функция, потому что каждый вызывающий иначе повторяет одну и ту
/// же сборку строки и рано или поздно забывает пробел.
#[must_use]
pub fn prefix(icon: Icon, set: IconSet) -> String {
    let glyph = icon.glyph(set);
    if glyph.is_empty() {
        String::new()
    } else {
        format!("{glyph} ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Набор Nerd покрывает интерфейс целиком и только глифами приватной
    /// области: иначе «включил nerd» тихо рисует обычные символы.
    #[test]
    fn nerd_set_is_complete_and_private_use() {
        for icon in Icon::ALL {
            let glyph = icon.glyph(IconSet::Nerd);
            let mut chars = glyph.chars();
            let symbol = chars.next().expect("глиф набора Nerd");
            assert!(chars.next().is_none(), "{icon:?}: иконка — один символ");
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&symbol),
                "{icon:?}: не глиф Nerd Font: {symbol:?}"
            );
        }
    }

    /// Запасной набор покрывает интерфейс целиком и держится блока `U+25xx`.
    ///
    /// Проверка блока и есть гарантия одинарной ширины: символ из другого
    /// блока может оказаться двойной ширины и разъехать рамки.
    #[test]
    fn geometric_set_is_complete_and_single_width() {
        for icon in Icon::ALL {
            let glyph = icon.glyph(IconSet::Unicode);
            let mut chars = glyph.chars();
            let symbol = chars.next().expect("глиф запасного набора");
            assert!(chars.next().is_none(), "{icon:?}: иконка — один символ");
            assert!(
                ('\u{2500}'..='\u{25ff}').contains(&symbol),
                "{icon:?}: вне блока геометрических фигур: {symbol:?}"
            );
        }
    }

    /// Выключенный набор не даёт ничего: кадр обязан совпадать с кадром
    /// сборки без иконок вообще.
    #[test]
    fn off_set_is_empty_everywhere() {
        for icon in Icon::ALL {
            assert!(icon.glyph(IconSet::Off).is_empty(), "{icon:?}");
            assert!(prefix(icon, IconSet::Off).is_empty(), "{icon:?}");
        }
    }

    /// Символы внутри набора не повторяются: одинаковая иконка у разных
    /// смыслов делает набор бесполезным — ровно та жалоба, с которой
    /// начиналась работа над иконками.
    #[test]
    fn glyphs_are_distinct_within_each_set() {
        for set in [IconSet::Nerd, IconSet::Unicode] {
            let mut seen: Vec<&'static str> = Vec::new();
            for icon in Icon::ALL {
                let glyph = icon.glyph(set);
                // Проблема как категория пайпа и как серьёзность — один смысл,
                // поэтому единственное намеренное совпадение.
                if matches!(icon, Icon::Warn) {
                    continue;
                }
                assert!(
                    !seen.contains(&glyph),
                    "{set:?}: {icon:?} повторяет уже занятый символ {glyph:?}"
                );
                seen.push(glyph);
            }
        }
    }

    #[test]
    fn kind_and_severity_map_to_their_own_icons() {
        assert_eq!(for_kind(pulse_core::EntityKind::Unit), Icon::Unit);
        assert_eq!(for_kind(pulse_core::EntityKind::Disk), Icon::Disk);
        assert_eq!(for_severity(pulse_core::Severity::Crit), Icon::Crit);
        assert_eq!(
            prefix(Icon::Unit, IconSet::Unicode),
            format!("{} ", Icon::Unit.glyph(IconSet::Unicode))
        );
    }
}
