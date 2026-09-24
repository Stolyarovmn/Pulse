//! Палитра команд и справка — один список действий.
//!
//! Каждая команда — это подпись и клавиша, которую она нажимает. `Enter` в
//! палитре закрывает её и отдаёт клавишу тому же маршрутизатору, что и
//! клавиатура. Поэтому палитра не может разойтись с горячими клавишами: у них
//! один путь исполнения, а справка перечисляет ровно то, что работает.
//!
//! Палитра (`:`) показывает команды текущего места — экрана или инспектора —
//! и общие. Справка (`?`) — все команды всех мест: выбор команды другого
//! экрана сначала переводит на этот экран, затем нажимает клавишу.

use crossterm::event::KeyCode;

use crate::app::Screen;

/// Где команда действует.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Везде.
    Global,
    /// Только на своём экране.
    Screen(Screen),
    /// Только в открытом инспекторе.
    Inspector,
}

impl Scope {
    /// Подпись группы в списке.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "GLOBAL",
            Self::Inspector => "INSPECTOR",
            Self::Screen(Screen::Overview) => "OVERVIEW",
            Self::Screen(Screen::Problems) => "PROBLEMS",
            Self::Screen(Screen::Entities) => "ENTITIES",
            Self::Screen(Screen::Timeline) => "TIMELINE",
        }
    }
}

/// Что делает команда.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    /// Нажать клавишу — тот же путь, что у клавиатуры.
    Key(KeyCode),
    /// Вторичный поток сырых событий Timeline (§183): своей клавиши нет.
    RawEvents,
    /// Назад к истории инцидентов Timeline.
    Story,
}

/// Одна команда.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub title: &'static str,
    /// Как вызвать без палитры; пусто — только отсюда.
    pub keys: &'static str,
    pub hint: &'static str,
    pub scope: Scope,
    pub run: Run,
}

const fn key(
    title: &'static str,
    keys: &'static str,
    hint: &'static str,
    scope: Scope,
    code: KeyCode,
) -> Command {
    Command {
        title,
        keys,
        hint,
        scope,
        run: Run::Key(code),
    }
}

/// Все команды в порядке показа внутри группы.
pub const COMMANDS: &[Command] = &[
    // Инспектор: расследование.
    key(
        "Open selected",
        "Enter",
        "follow the selected row",
        Scope::Inspector,
        KeyCode::Enter,
    ),
    key(
        "Next list",
        "Tab",
        "inside → leads → around",
        Scope::Inspector,
        KeyCode::Tab,
    ),
    key(
        "Previous list",
        "Shift+Tab",
        "around → leads → inside",
        Scope::Inspector,
        KeyCode::BackTab,
    ),
    key(
        "Back along the trail",
        "Esc",
        "one step back; closes at the start",
        Scope::Inspector,
        KeyCode::Esc,
    ),
    // Overview.
    key(
        "Open selected",
        "Enter",
        "inspect the selected entity",
        Scope::Screen(Screen::Overview),
        KeyCode::Enter,
    ),
    key(
        "Next pane",
        "Tab",
        "entities ↔ selected preview",
        Scope::Screen(Screen::Overview),
        KeyCode::Tab,
    ),
    // Problems.
    key(
        "Inspect problem entity",
        "Enter",
        "open the entity of the selected problem",
        Scope::Screen(Screen::Problems),
        KeyCode::Enter,
    ),
    // Entities.
    key(
        "Open selected",
        "Enter",
        "inspect the selected entity",
        Scope::Screen(Screen::Entities),
        KeyCode::Enter,
    ),
    key(
        "Next sort",
        "s",
        "memory, cpu, name, relevance",
        Scope::Screen(Screen::Entities),
        KeyCode::Char('s'),
    ),
    key(
        "Next type filter",
        "f",
        "all, process, container, unit, cgroup",
        Scope::Screen(Screen::Entities),
        KeyCode::Char('f'),
    ),
    key(
        "Logical / technical view",
        "m",
        "fold processes into services or not",
        Scope::Screen(Screen::Entities),
        KeyCode::Char('m'),
    ),
    key(
        "Next pane",
        "Tab",
        "list ↔ preview",
        Scope::Screen(Screen::Entities),
        KeyCode::Tab,
    ),
    // Timeline.
    key(
        "Open selected event",
        "Enter",
        "inspect the entity of the event",
        Scope::Screen(Screen::Timeline),
        KeyCode::Enter,
    ),
    key(
        "Scrub back",
        "←",
        "one second into the past",
        Scope::Screen(Screen::Timeline),
        KeyCode::Left,
    ),
    key(
        "Scrub forward",
        "→",
        "one second towards now",
        Scope::Screen(Screen::Timeline),
        KeyCode::Right,
    ),
    key(
        "Back to LIVE",
        "F",
        "follow the present again",
        Scope::Screen(Screen::Timeline),
        KeyCode::Char('F'),
    ),
    key(
        "Zoom in",
        "+",
        "narrow the time window",
        Scope::Screen(Screen::Timeline),
        KeyCode::Char('+'),
    ),
    key(
        "Zoom out",
        "-",
        "widen the time window",
        Scope::Screen(Screen::Timeline),
        KeyCode::Char('-'),
    ),
    key(
        "Set mark A",
        "A",
        "comparison start",
        Scope::Screen(Screen::Timeline),
        KeyCode::Char('A'),
    ),
    key(
        "Set mark B",
        "B",
        "comparison end",
        Scope::Screen(Screen::Timeline),
        KeyCode::Char('B'),
    ),
    key(
        "Semantic A/B diff",
        "D",
        "what changed between the marks",
        Scope::Screen(Screen::Timeline),
        KeyCode::Char('D'),
    ),
    Command {
        title: "Raw events",
        keys: "",
        hint: "secondary stream of every observation",
        scope: Scope::Screen(Screen::Timeline),
        run: Run::RawEvents,
    },
    Command {
        title: "Incident story",
        keys: "",
        hint: "back from raw events",
        scope: Scope::Screen(Screen::Timeline),
        run: Run::Story,
    },
    key(
        "Next pane",
        "Tab",
        "rail, story, snapshot",
        Scope::Screen(Screen::Timeline),
        KeyCode::Tab,
    ),
    // Везде.
    key(
        "Overview",
        "1",
        "system state and relevant entities",
        Scope::Global,
        KeyCode::Char('1'),
    ),
    key(
        "Problems",
        "2",
        "open problems with evidence",
        Scope::Global,
        KeyCode::Char('2'),
    ),
    key(
        "Entities",
        "3",
        "full inventory",
        Scope::Global,
        KeyCode::Char('3'),
    ),
    key(
        "Timeline / Time Machine",
        "4",
        "story, lanes, A/B compare",
        Scope::Global,
        KeyCode::Char('4'),
    ),
    key(
        "Search",
        "/",
        "filter entities by name or command",
        Scope::Global,
        KeyCode::Char('/'),
    ),
    key(
        "Pipe",
        "g",
        "facts about the selected entity",
        Scope::Global,
        KeyCode::Char('g'),
    ),
    key(
        "Pause display",
        "p",
        "collection continues",
        Scope::Global,
        KeyCode::Char('p'),
    ),
    key(
        "Help",
        "? / F1",
        "every command of every screen",
        Scope::Global,
        KeyCode::Char('?'),
    ),
    key(
        "Quit",
        "q / Ctrl+C",
        "leave PULSE",
        Scope::Global,
        KeyCode::Char('q'),
    ),
];

/// Место, в котором открыта палитра.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Inspector,
    Screen(Screen),
}

impl Place {
    /// Действует ли команда здесь без перехода.
    #[must_use]
    pub fn covers(self, scope: Scope) -> bool {
        match (self, scope) {
            (_, Scope::Global) => true,
            (Place::Inspector, Scope::Inspector) => true,
            (Place::Screen(here), Scope::Screen(there)) => here == there,
            _ => false,
        }
    }
}

/// Команды, которые показывает палитра или справка, в порядке показа.
///
/// Сначала команды текущего места: за ними и открывают палитру. Справка
/// добавляет остальные места, а открытие справки из самой справки не
/// предлагается.
#[must_use]
pub fn listed(place: Place, help: bool, query: &str) -> Vec<&'static Command> {
    let order = |command: &Command| -> u8 {
        match command.scope {
            scope if scope != Scope::Global && place.covers(scope) => 0,
            Scope::Global => 1,
            Scope::Inspector => 2,
            Scope::Screen(Screen::Overview) => 3,
            Scope::Screen(Screen::Problems) => 4,
            Scope::Screen(Screen::Entities) => 5,
            Scope::Screen(Screen::Timeline) => 6,
        }
    };
    let mut out: Vec<&'static Command> = COMMANDS
        .iter()
        .filter(|command| help || place.covers(command.scope))
        .filter(|command| !(help && command.title == "Help"))
        .filter(|command| matches(command, query))
        .collect();
    // Стабильная сортировка: внутри группы сохраняется порядок таблицы.
    out.sort_by_key(|command| order(command));
    out
}

/// Подходит ли команда под запрос: каждое слово запроса есть в подписи,
/// подсказке, клавише или группе.
#[must_use]
pub fn matches(command: &Command, query: &str) -> bool {
    let haystack = format!(
        "{} {} {} {}",
        command.title,
        command.hint,
        command.keys,
        command.scope.label()
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Палитра показывает только то, что сработает здесь: команда чужого
    /// экрана в палитре — это Enter в пустоту.
    #[test]
    fn palette_lists_only_commands_of_this_place_and_global() {
        let listed = listed(Place::Screen(Screen::Entities), false, "");
        assert!(listed
            .iter()
            .all(|command| Place::Screen(Screen::Entities).covers(command.scope)));
        assert!(listed.iter().any(|command| command.title == "Next sort"));
        assert!(!listed.iter().any(|command| command.title == "Zoom in"));
        // Команды места идут первыми: за ними палитру и открывают.
        assert_eq!(
            listed.first().map(|command| command.scope),
            Some(Scope::Screen(Screen::Entities))
        );
    }

    /// Справка — все команды всех мест, и её фильтр ищет по подписи,
    /// подсказке и клавише одновременно.
    #[test]
    fn help_lists_everything_and_filters_by_words() {
        let all = listed(Place::Screen(Screen::Overview), true, "");
        for command in COMMANDS.iter().filter(|command| command.title != "Help") {
            assert!(all.contains(&command), "нет в справке: {}", command.title);
        }
        let found = listed(Place::Screen(Screen::Overview), true, "raw events");
        assert_eq!(
            found.first().map(|command| command.run),
            Some(Run::RawEvents)
        );
        let by_key = listed(Place::Inspector, true, "shift+tab");
        assert_eq!(
            by_key.first().map(|command| command.title),
            Some("Previous list")
        );
    }
}
