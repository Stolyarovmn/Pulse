//! Единое состояние интерфейса и однократная маршрутизация ввода.
//!
//! Спецификация v0.9 (разделы 164, 179, 195, 196) фиксирует инвариант:
//!
//! ```text
//! visible UI state == focus state == keyboard target
//! ```
//!
//! Поэтому renderer и input router читают один [`App`]. Renderer публикует
//! фактически видимые панели через [`App::set_visible_panes`], а router может
//! передать `Enter` только текущей видимой панели. Inspector и Help больше не
//! top-level screens: Inspector — contextual session, Help/Search/Palette —
//! overlays, которые полностью владеют вводом, пока открыты.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use pulse_core::details::{ProcessDetails, ProcessDetailsSource};
use pulse_core::entity::{EntityKey, EntityKind};
use pulse_core::snapshot::Snapshot;
use pulse_core::time::Timestamp;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::fold::{fold, relevant, LogicalRow};
use crate::layout::Pane;
use crate::rows::{entity_rows, EntityRow};

/// Четыре top-level экрана (разделы 168, 169).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Problems,
    Entities,
    Timeline,
}

impl Screen {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Overview => "OVERVIEW",
            Self::Problems => "PROBLEMS",
            Self::Entities => "ENTITIES",
            Self::Timeline => "TIME MACHINE",
        }
    }
}

/// Порядок сортировки списка сущностей.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Relevance,
    Cpu,
    Memory,
    Name,
}

impl SortKey {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Cpu => "CPU",
            Self::Memory => "MEM",
            Self::Name => "NAME",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Relevance => Self::Cpu,
            Self::Cpu => Self::Memory,
            Self::Memory => Self::Name,
            Self::Name => Self::Relevance,
        }
    }
}

/// Направление сортировки всегда видно на list screens (раздел 176).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    #[must_use]
    pub const fn symbol(self, ascii: bool) -> &'static str {
        match (self, ascii) {
            (Self::Ascending, true) => "^",
            (Self::Descending, true) => "v",
            (Self::Ascending, false) => "↑",
            (Self::Descending, false) => "↓",
        }
    }
}

/// Действие, которое выполняет внешний цикл приложения.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    RunDiff,
}

/// Modal overlay. Пока он открыт, underlying screen не получает ввод.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlay {
    Search(SearchOverlay),
    Palette {
        query: String,
    },
    Help,
    /// Пайп расследования: факты о выбранной сущности.
    Pipe(crate::pipe::PipeState),
}

/// Состояние Search modal и точка точного возврата по Esc (разделы 177, 178).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchOverlay {
    pub query: String,
    previous_screen: Screen,
    previous_pane: Pane,
    previous_selection: usize,
    previous_filter: Option<String>,
}

/// Состояние Overview.
#[derive(Clone, Debug, Default)]
pub struct OverviewState {
    pub selected: usize,
    pub list_len: usize,
    pub preview_relation_selected: usize,
    pub preview_relation_len: usize,
}

/// Состояние Problems.
#[derive(Clone, Debug, Default)]
pub struct ProblemsState {
    pub selected: usize,
    pub list_len: usize,
}

/// Состояние полного inventory screen.
#[derive(Clone, Debug)]
pub struct EntitiesState {
    pub selected: usize,
    pub list_len: usize,
    pub preview_relation_selected: usize,
    pub preview_relation_len: usize,
    pub sort: SortKey,
    pub direction: SortDirection,
    pub kind_filter: Option<EntityKind>,
    pub technical_view: bool,
    /// Применённый Search filter. Активный modal query хранится в overlay.
    pub search_filter: Option<String>,
}

impl Default for EntitiesState {
    fn default() -> Self {
        Self {
            selected: 0,
            list_len: 0,
            preview_relation_selected: 0,
            preview_relation_len: 0,
            sort: SortKey::Memory,
            direction: SortDirection::Descending,
            kind_filter: None,
            technical_view: false,
            search_filter: None,
        }
    }
}

/// Состояние Time Machine.
#[derive(Clone, Debug)]
pub struct TimelineState {
    pub story_selected: usize,
    pub story_len: usize,
    pub time_cursor: Option<Timestamp>,
    pub zoom_ms: u64,
    pub mark_a: Option<Timestamp>,
    pub mark_b: Option<Timestamp>,
    pub diff_text: Option<String>,
    /// Raw event stream — secondary subview, открывается из палитры (§183).
    pub raw_events: bool,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            story_selected: 0,
            story_len: 0,
            time_cursor: None,
            zoom_ms: 5 * 60 * 1_000,
            mark_a: None,
            mark_b: None,
            diff_text: None,
            raw_events: false,
        }
    }
}

/// Строка отношения Inspector.
///
/// Тип живёт в [`crate::investigate`]: список звеньев и его отображение
/// обязаны совпадать, поэтому отдельной структуры у экранов нет.
pub use crate::investigate::Step as RelationTarget;

/// Contextual Inspector session (разделы 168, 174, 175, 196).
#[derive(Clone, Debug)]
pub struct InspectorSession {
    pub origin: Screen,
    pub origin_pane: Pane,
    /// Уникальный путь canonical identities. Повторная сущность не push'ится.
    pub path: Vec<EntityKey>,
    pub current: EntityKey,
    pub relation_selected: usize,
    pub relation_len: usize,
    /// `Enter` действует на боковые переходы, а не на цепочку вниз.
    pub side_active: bool,
}

impl InspectorSession {
    fn new(origin: Screen, origin_pane: Pane, current: EntityKey) -> Self {
        Self {
            origin,
            origin_pane,
            path: vec![current.clone()],
            current,
            relation_selected: 0,
            relation_len: 0,
            side_active: false,
        }
    }

    /// Переход по графу без циклического роста history (раздел 174).
    pub fn follow(&mut self, target: EntityKey) {
        if let Some(position) = self.path.iter().position(|key| *key == target) {
            self.path.truncate(position + 1);
        } else {
            self.path.push(target.clone());
        }
        self.current = target;
        self.relation_selected = 0;
    }

    /// Боковой переход: новое расследование с нового корня.
    ///
    /// Путь не продолжается, потому что смежный объект не находится глубже:
    /// склеивать «спустился» и «прыгнул» в одну цепочку значит врать про
    /// причинную связь между звеньями.
    pub fn restart(&mut self, target: EntityKey) {
        self.path = vec![target.clone()];
        self.current = target;
        self.relation_selected = 0;
        self.side_active = false;
    }

    /// Назад по уникальному relation path.
    fn back(&mut self) -> bool {
        if self.path.len() <= 1 {
            return false;
        }
        let _ = self.path.pop();
        if let Some(previous) = self.path.last().cloned() {
            self.current = previous;
        }
        self.relation_selected = 0;
        true
    }
}

/// Единое authoritative UI state.
#[derive(Clone, Debug)]
pub struct App {
    /// Кэш производного списка сущностей.
    ///
    /// Построение 2000 строк стоит около 9 мс: на каждую сущность считаются
    /// владелец, состояние и потоки. Без кэша эта работа выполнялась дважды на
    /// каждое нажатие клавиши (в маршрутизации ввода и в отрисовке) и заново на
    /// каждый кадр, хотя снимок и вид не менялись. Renderer обязан рисовать
    /// состояние, а не пересчитывать аналитику.
    derived_cache: RefCell<Option<(RowCacheKey, Rc<Derived>)>>,
    pub screen: Screen,
    pub overlay: Option<Overlay>,
    /// Фактически видимые и focusable panes текущего кадра.
    visible_panes: Vec<Pane>,
    pub pane: Pane,
    pub overview: OverviewState,
    pub problems: ProblemsState,
    pub entities: EntitiesState,
    pub timeline: TimelineState,
    pub inspector: Option<InspectorSession>,
    pub paused: bool,
    pub status: String,
    pub allow_actions: bool,
    /// Набор иконок категорий в пайпе (`ui.icons`).
    pub icons: pulse_core::config::IconSet,
    /// Число колонок сетки пайпа в последнем кадре.
    ///
    /// Пишет экран, читает маршрутизация ввода: только кадр знает свою
    /// ширину, а курсор обязан двигаться по видимой сетке. `Cell` вместо
    /// параметра, потому что отрисовка получает `&App` — тот же приём уже
    /// применён к такту кэша деталей ниже.
    pipe_cols: Cell<usize>,
    pub layout: crate::layout::LayoutEngine,
    /// Источник деталей процесса: файлы, порты, пользователь.
    details: Option<Arc<dyn ProcessDetailsSource>>,
    details_cache: RefCell<pulse_core::DetailsCache>,
    /// Такт, для которого кэш деталей ещё действителен.
    details_tick: Cell<Option<pulse_core::TickId>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            derived_cache: RefCell::new(None),
            screen: Screen::Overview,
            overlay: None,
            visible_panes: vec![Pane::Primary],
            pane: Pane::Primary,
            overview: OverviewState::default(),
            problems: ProblemsState::default(),
            entities: EntitiesState::default(),
            timeline: TimelineState::default(),
            inspector: None,
            paused: false,
            status: String::new(),
            allow_actions: false,
            icons: pulse_core::config::IconSet::Off,
            pipe_cols: Cell::new(1),
            layout: crate::layout::LayoutEngine::new(),
            details: None,
            details_cache: RefCell::new(pulse_core::DetailsCache::default()),
            details_tick: Cell::new(None),
        }
    }
}

impl App {
    #[must_use]
    pub fn new(allow_actions: bool) -> Self {
        Self {
            allow_actions,
            ..Self::default()
        }
    }

    /// Подключает источник деталей процесса.
    ///
    /// Детали читаются по требованию из терминального уровня цепочки, поэтому
    /// интерфейс держит не данные, а источник: без него экран показывает
    /// сущность, но не её файлы и порты.
    #[must_use]
    pub fn with_details(mut self, details: Arc<dyn ProcessDetailsSource>) -> Self {
        self.details = Some(details);
        self
    }

    /// Детали процесса из кэша, одно чтение `/proc` на такт.
    #[must_use]
    pub fn process_details(&self, snapshot: &Snapshot, pid: i32) -> Option<ProcessDetails> {
        let source = self.details.as_ref()?;
        // Смена такта обязана сбрасывать кэш: процесс мог открыть файл или
        // закрыть порт, и показывать вчерашний ответ хуже, чем не показывать.
        if self.details_tick.get() != Some(snapshot.tick) {
            self.details_cache.borrow_mut().clear();
            self.details_tick.set(Some(snapshot.tick));
        }
        Some(self.details_cache.borrow_mut().get(source.as_ref(), pid))
    }

    /// Набор иконок категорий в пайпе: приходит из настройки `ui.icons`.
    ///
    /// По умолчанию выключен: глиф Nerd Font в непатченном шрифте рисуется
    /// пустым прямоугольником, а измерить наличие глифа из терминала нельзя.
    #[must_use]
    pub const fn with_icons(mut self, icons: pulse_core::config::IconSet) -> Self {
        self.icons = icons;
        self
    }

    /// Сущность, вокруг которой строится пайп.
    ///
    /// Порядок источников отражает то, что оператор считает выбранным:
    /// открытый Inspector важнее списка, список важнее хоста.
    #[must_use]
    pub fn focus_entity(&self, snapshot: &Snapshot) -> Option<pulse_core::EntityId> {
        if let Some(session) = &self.inspector {
            if let Some(found) = snapshot
                .entities
                .iter()
                .find(|candidate| candidate.key == session.current)
            {
                return Some(found.id);
            }
        }
        let derived = self.derived(snapshot);
        let row = match self.screen {
            Screen::Entities if self.entities.technical_view => {
                derived.rows.get(self.entities.selected).map(|row| row.id)
            }
            Screen::Entities => derived
                .logical
                .get(self.entities.selected)
                .map(|logical| logical.row.id),
            _ => derived.relevant.first().map(|logical| logical.row.id),
        };
        row.or(Some(snapshot.host))
    }

    /// Детали процесса для пайпа: читаются только под раскрытую ветку.
    ///
    /// Сущность может быть сервисом или контейнером: детали живут у процесса,
    /// поэтому здесь тот же спуск к главному процессу, что и в раскладке —
    /// иначе ветка отвечала бы «не процесс» там, где ответ есть.
    #[must_use]
    pub fn pipe_details(
        &self,
        snapshot: &Snapshot,
        entity: pulse_core::EntityId,
        state: &crate::pipe::PipeState,
    ) -> Option<ProcessDetails> {
        if !state.needs_details() {
            return None;
        }
        let (_, pid) = crate::pipe::main_process(snapshot, entity)?;
        self.process_details(snapshot, pid)
    }

    /// Заголовок отражает overlay/Inspector, который реально видит оператор.
    #[must_use]
    pub const fn title(&self) -> &'static str {
        match self.overlay {
            Some(Overlay::Help) => "HELP",
            Some(Overlay::Search(_)) => "SEARCH",
            Some(Overlay::Palette { .. }) => "COMMANDS",
            Some(Overlay::Pipe(_)) => "PIPE",
            None if self.inspector.is_some() => "INSPECTOR",
            None => self.screen.title(),
        }
    }

    /// Сообщает, сколько колонок сетки нарисовал последний кадр пайпа.
    ///
    /// Вызывает экран: ширину знает только он. Без этого маршрутизация ввода
    /// двигала бы курсор по сетке, которой на экране нет.
    pub fn set_pipe_columns(&self, cols: usize) {
        self.pipe_cols.set(cols.max(1));
    }

    #[must_use]
    pub fn search_query(&self) -> Option<&str> {
        match &self.overlay {
            Some(Overlay::Search(search)) => Some(search.query.as_str()),
            _ => self.entities.search_filter.as_deref(),
        }
    }

    #[must_use]
    pub fn active_search_query(&self) -> Option<&str> {
        match &self.overlay {
            Some(Overlay::Search(search)) => Some(search.query.as_str()),
            _ => None,
        }
    }

    #[must_use]
    pub fn palette_query(&self) -> Option<&str> {
        match &self.overlay {
            Some(Overlay::Palette { query }) => Some(query.as_str()),
            _ => None,
        }
    }

    /// Renderer публикует только реально видимые focusable panes.
    ///
    /// Если resize спрятал focused pane, focus детерминированно переходит на
    /// первый оставшийся (разделы 170, 192).
    pub fn set_visible_panes(&mut self, panes: &[Pane]) {
        self.visible_panes.clear();
        for pane in panes {
            if !self.visible_panes.contains(pane) {
                self.visible_panes.push(*pane);
            }
        }
        if self.visible_panes.is_empty() {
            self.visible_panes.push(Pane::Primary);
        }
        if !self.visible_panes.contains(&self.pane) {
            if let Some(first) = self.visible_panes.first().copied() {
                self.pane = first;
            }
        }
    }

    #[must_use]
    pub fn visible_panes(&self) -> &[Pane] {
        &self.visible_panes
    }

    #[must_use]
    pub fn pane_is_visible(&self, pane: Pane) -> bool {
        self.visible_panes.contains(&pane)
    }

    /// Цикл только по видимым panes; top-level screen не меняется (§170).
    fn cycle_pane(&mut self, backwards: bool) {
        if self.visible_panes.len() <= 1 {
            return;
        }
        let current = self
            .visible_panes
            .iter()
            .position(|pane| *pane == self.pane)
            .unwrap_or(0);
        let next = if backwards {
            current
                .checked_sub(1)
                .unwrap_or_else(|| self.visible_panes.len().saturating_sub(1))
        } else {
            (current + 1) % self.visible_panes.len()
        };
        if let Some(pane) = self.visible_panes.get(next).copied() {
            self.pane = pane;
        }
    }

    /// Точный переход top-level; повторный shortcut — no-op (§169).
    fn navigate(&mut self, destination: Screen, snapshot: &Snapshot) {
        if self.screen == destination && self.inspector.is_none() {
            return;
        }
        self.inspector = None;
        self.overlay = None;
        self.screen = destination;
        self.pane = match destination {
            Screen::Timeline if !snapshot.events.is_empty() => Pane::Story,
            _ => Pane::Primary,
        };
        self.visible_panes.clear();
        self.visible_panes.push(self.pane);
    }

    fn active_selection(&self) -> usize {
        if let Some(inspector) = &self.inspector {
            return inspector.relation_selected;
        }
        match self.screen {
            Screen::Overview if self.pane == Pane::Inspector => {
                self.overview.preview_relation_selected
            }
            Screen::Overview => self.overview.selected,
            Screen::Problems => self.problems.selected,
            Screen::Entities if self.pane == Pane::Inspector => {
                self.entities.preview_relation_selected
            }
            Screen::Entities => self.entities.selected,
            Screen::Timeline => self.timeline.story_selected,
        }
    }

    fn set_active_selection(&mut self, value: usize) {
        if let Some(inspector) = &mut self.inspector {
            inspector.relation_selected = value.min(inspector.relation_len.saturating_sub(1));
            return;
        }
        match self.screen {
            Screen::Overview if self.pane == Pane::Inspector => {
                self.overview.preview_relation_selected =
                    value.min(self.overview.preview_relation_len.saturating_sub(1));
            }
            Screen::Overview => {
                self.overview.selected = value.min(self.overview.list_len.saturating_sub(1));
            }
            Screen::Problems => {
                self.problems.selected = value.min(self.problems.list_len.saturating_sub(1));
            }
            Screen::Entities if self.pane == Pane::Inspector => {
                self.entities.preview_relation_selected =
                    value.min(self.entities.preview_relation_len.saturating_sub(1));
            }
            Screen::Entities => {
                self.entities.selected = value.min(self.entities.list_len.saturating_sub(1));
            }
            Screen::Timeline => {
                self.timeline.story_selected = value.min(self.timeline.story_len.saturating_sub(1));
            }
        }
    }

    fn active_len(&self) -> usize {
        if let Some(inspector) = &self.inspector {
            return inspector.relation_len;
        }
        match self.screen {
            Screen::Overview if self.pane == Pane::Inspector => self.overview.preview_relation_len,
            Screen::Overview => self.overview.list_len,
            Screen::Problems => self.problems.list_len,
            Screen::Entities if self.pane == Pane::Inspector => self.entities.preview_relation_len,
            Screen::Entities => self.entities.list_len,
            Screen::Timeline => self.timeline.story_len,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.active_len();
        if len == 0 {
            self.set_active_selection(0);
            return;
        }
        let max = len.saturating_sub(1);
        let current = self.active_selection().min(max) as isize;
        self.set_active_selection((current + delta).clamp(0, max as isize) as usize);
    }

    /// Включает secondary Raw Events subview на Timeline.
    fn raw_events_on(&mut self) {
        self.timeline.raw_events = true;
        self.timeline.story_selected = 0;
        self.inspector = None;
        self.screen = Screen::Timeline;
        self.pane = Pane::Story;
        self.status = "raw events (secondary view)".to_string();
    }

    /// Открывает contextual Inspector из concrete EntityKey.
    pub fn open_inspector(&mut self, key: EntityKey) {
        self.overlay = None;
        self.inspector = Some(InspectorSession::new(self.screen, self.pane, key));
        self.pane = Pane::Primary;
        self.visible_panes.clear();
        self.visible_panes.push(Pane::Primary);
    }

    /// Текущая сущность Inspector по canonical key.
    pub fn inspector_entity<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a pulse_core::Entity> {
        let key = &self.inspector.as_ref()?.current;
        snapshot.entities.iter().find(|entity| &entity.key == key)
    }

    /// Видимые отношения текущей сущности. Renderer и Enter используют этот
    /// же список, поэтому background target невозможен.
    pub fn inspector_relations(&self, snapshot: &Snapshot) -> Vec<RelationTarget> {
        let Some(entity) = self.inspector_entity(snapshot) else {
            return Vec::new();
        };
        relation_targets_for(snapshot, entity.id)
    }

    /// Боковые переходы текущей сущности: владелец и соседи по ресурсу.
    ///
    /// Отдельный список, а не часть цепочки: `Enter` по нему начинает новое
    /// расследование. Renderer и router читают именно этот метод.
    pub fn inspector_side_steps(&self, snapshot: &Snapshot) -> Vec<RelationTarget> {
        let Some(entity) = self.inspector_entity(snapshot) else {
            return Vec::new();
        };
        crate::investigate::side_steps(snapshot, entity.id)
    }

    fn inspector_back(&mut self) {
        let should_exit = self
            .inspector
            .as_mut()
            .is_none_or(|inspector| !inspector.back());
        if !should_exit {
            return;
        }
        if let Some(session) = self.inspector.take() {
            self.screen = session.origin;
            self.pane = session.origin_pane;
            self.visible_panes.clear();
            self.visible_panes.push(self.pane);
        }
    }

    /// Единственный input router: modal → Inspector/focused pane → global.
    pub fn dispatch(&mut self, key: KeyEvent, snapshot: &Snapshot) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
        {
            return Action::Quit;
        }

        if self.overlay.is_some() {
            return self.dispatch_overlay(key, snapshot);
        }
        if self.inspector.is_some() {
            if let Some(action) = self.dispatch_inspector(key, snapshot) {
                return action;
            }
        } else if let Some(action) = self.dispatch_focused_pane(key, snapshot) {
            return action;
        }
        self.dispatch_global(key, snapshot)
    }

    fn dispatch_overlay(&mut self, key: KeyEvent, snapshot: &Snapshot) -> Action {
        let Some(overlay) = self.overlay.take() else {
            return Action::None;
        };
        match overlay {
            Overlay::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1)) {
                    self.overlay = None;
                } else {
                    self.overlay = Some(Overlay::Help);
                }
            }
            Overlay::Pipe(mut state) => {
                // Число колонок берётся из последнего кадра: курсор обязан
                // двигаться по той сетке, которую оператор видит, иначе
                // «вниз» снова уедет вправо.
                let cols = self.pipe_cols.get();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('g') => self.status.clear(),
                    KeyCode::Down | KeyCode::Char('j') => {
                        state.move_by(0, 1, cols);
                        self.overlay = Some(Overlay::Pipe(state));
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.move_by(0, -1, cols);
                        self.overlay = Some(Overlay::Pipe(state));
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        state.move_by(1, 0, cols);
                        self.overlay = Some(Overlay::Pipe(state));
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        state.move_by(-1, 0, cols);
                        self.overlay = Some(Overlay::Pipe(state));
                    }
                    // Чтение источника происходит здесь и только здесь:
                    // свёрнутая ветка не стоит ни одного syscall.
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        let _ = state.toggle();
                        self.overlay = Some(Overlay::Pipe(state));
                    }
                    KeyCode::Char('v') => {
                        state.boxes = !state.boxes;
                        self.status = if state.boxes {
                            "pipe: вид древом".to_string()
                        } else {
                            "pipe: вид отступами".to_string()
                        };
                        self.overlay = Some(Overlay::Pipe(state));
                    }
                    // Дефект с живого прогона: `q` в пайпе не работал, потому
                    // что overlay съедал все неизвестные клавиши, и выйти
                    // можно было только через Esc. Выход обязан работать
                    // с любого экрана.
                    KeyCode::Char('q') => return Action::Quit,
                    _ => self.overlay = Some(Overlay::Pipe(state)),
                }
            }
            Overlay::Palette { mut query } => match key.code {
                KeyCode::Esc => self.status.clear(),
                KeyCode::Enter => {
                    // Единственная реализованная команда: raw events как
                    // secondary Timeline subview (§183). Остальные честно
                    // сообщают, что их нет в этой сборке.
                    let command = query.trim().to_ascii_lowercase();
                    match command.as_str() {
                        "raw events" | "raw" => {
                            self.raw_events_on();
                        }
                        "story" | "events" => {
                            self.timeline.raw_events = false;
                            self.screen = Screen::Timeline;
                            self.status = "story view".to_string();
                        }
                        _ => {
                            self.status =
                                format!("command \"{command}\" is not available in this build");
                        }
                    }
                }
                KeyCode::Backspace => {
                    let _ = query.pop();
                    self.overlay = Some(Overlay::Palette { query });
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    self.overlay = Some(Overlay::Palette { query });
                }
                _ => self.overlay = Some(Overlay::Palette { query }),
            },
            Overlay::Search(mut search) => match key.code {
                KeyCode::Esc => {
                    self.screen = search.previous_screen;
                    self.pane = search.previous_pane;
                    self.entities.search_filter = search.previous_filter;
                    // Live filtering may temporarily shrink list_len. Esc must
                    // restore the exact origin selection, not clamp it against
                    // modal results (v0.9 §177, §178).
                    if let Some(inspector) = &mut self.inspector {
                        inspector.relation_selected = search.previous_selection;
                    } else {
                        match search.previous_screen {
                            Screen::Overview => self.overview.selected = search.previous_selection,
                            Screen::Problems => self.problems.selected = search.previous_selection,
                            Screen::Entities => self.entities.selected = search.previous_selection,
                            Screen::Timeline => {
                                self.timeline.story_selected = search.previous_selection;
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    self.entities.search_filter =
                        (!search.query.is_empty()).then_some(search.query);
                    self.screen = Screen::Entities;
                    self.inspector = None;
                    self.pane = Pane::Primary;
                    self.entities.selected = 0;
                    self.status = "search filter applied".to_string();
                    // Первое совпадение становится selected после render.
                    let _ = snapshot;
                }
                KeyCode::Backspace => {
                    let _ = search.query.pop();
                    self.overlay = Some(Overlay::Search(search));
                }
                KeyCode::Char(c) => {
                    search.query.push(c);
                    self.overlay = Some(Overlay::Search(search));
                }
                // Underlying shortcuts/Tab не получают событие (§177).
                _ => self.overlay = Some(Overlay::Search(search)),
            },
        }
        Action::None
    }

    fn dispatch_inspector(&mut self, key: KeyEvent, snapshot: &Snapshot) -> Option<Action> {
        match key.code {
            KeyCode::Esc => {
                self.inspector_back();
                Some(Action::None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                Some(Action::None)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                Some(Action::None)
            }
            KeyCode::Enter => {
                let side = self
                    .inspector
                    .as_ref()
                    .is_some_and(|inspector| inspector.side_active);
                let targets = if side {
                    self.inspector_side_steps(snapshot)
                } else {
                    self.inspector_relations(snapshot)
                };
                let selected = self
                    .inspector
                    .as_ref()
                    .map_or(0, |inspector| inspector.relation_selected);
                if let Some(target) = targets.get(selected) {
                    if let Some(inspector) = &mut self.inspector {
                        if side {
                            // Боковой переход — не продолжение цепочки: он
                            // начинает новое расследование с нового корня,
                            // иначе путь смешал бы «спустился» и «прыгнул».
                            inspector.restart(target.key.clone());
                        } else {
                            inspector.follow(target.key.clone());
                        }
                    }
                } else {
                    self.status = "nothing to open".to_string();
                }
                Some(Action::None)
            }
            // Tab переключает, какой список получает `Enter`: цепочка вниз или
            // боковые переходы. Один ключ вместо второго набора клавиш.
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(inspector) = &mut self.inspector {
                    inspector.side_active = !inspector.side_active;
                    inspector.relation_selected = 0;
                }
                Some(Action::None)
            }
            _ => None,
        }
    }

    fn dispatch_focused_pane(&mut self, key: KeyEvent, snapshot: &Snapshot) -> Option<Action> {
        match key.code {
            KeyCode::Tab => {
                self.cycle_pane(false);
                return Some(Action::None);
            }
            KeyCode::BackTab => {
                self.cycle_pane(true);
                return Some(Action::None);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                return Some(Action::None);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                return Some(Action::None);
            }
            KeyCode::PageDown => {
                self.move_selection(10);
                return Some(Action::None);
            }
            KeyCode::PageUp => {
                self.move_selection(-10);
                return Some(Action::None);
            }
            KeyCode::Home => {
                self.set_active_selection(0);
                return Some(Action::None);
            }
            KeyCode::End => {
                self.set_active_selection(self.active_len().saturating_sub(1));
                return Some(Action::None);
            }
            KeyCode::Enter => {
                self.open_focused(snapshot);
                return Some(Action::None);
            }
            _ => {}
        }

        // Timeline pane-local actions.
        if self.screen == Screen::Timeline {
            match key.code {
                KeyCode::Left => {
                    let current = self.timeline.time_cursor.unwrap_or(snapshot.at);
                    self.timeline.time_cursor = Some(current.saturating_sub_millis(1_000));
                    return Some(Action::None);
                }
                KeyCode::Right => {
                    let current = self.timeline.time_cursor.unwrap_or(snapshot.at);
                    let next = current.saturating_add_millis(1_000);
                    self.timeline.time_cursor = (next < snapshot.at).then_some(next);
                    return Some(Action::None);
                }
                KeyCode::Char('F') => {
                    self.timeline.time_cursor = None;
                    return Some(Action::None);
                }
                KeyCode::Char('+') => {
                    self.timeline.zoom_ms = self.timeline.zoom_ms.saturating_div(2).max(30_000);
                    return Some(Action::None);
                }
                KeyCode::Char('-') => {
                    self.timeline.zoom_ms = self
                        .timeline
                        .zoom_ms
                        .saturating_mul(2)
                        .min(24 * 60 * 60 * 1_000);
                    return Some(Action::None);
                }
                _ => {}
            }
        }
        None
    }

    fn dispatch_global(&mut self, key: KeyEvent, snapshot: &Snapshot) -> Action {
        match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('1') => {
                self.navigate(Screen::Overview, snapshot);
                Action::None
            }
            KeyCode::Char('2') | KeyCode::Char('P') => {
                self.navigate(Screen::Problems, snapshot);
                Action::None
            }
            KeyCode::Char('3') | KeyCode::Char('E') | KeyCode::Char('e') => {
                self.navigate(Screen::Entities, snapshot);
                Action::None
            }
            KeyCode::Char('4') | KeyCode::Char('T') => {
                self.navigate(Screen::Timeline, snapshot);
                Action::None
            }
            KeyCode::Char('/') => {
                self.overlay = Some(Overlay::Search(SearchOverlay {
                    query: String::new(),
                    previous_screen: self.screen,
                    previous_pane: self.pane,
                    previous_selection: self.active_selection(),
                    previous_filter: self.entities.search_filter.clone(),
                }));
                Action::None
            }
            KeyCode::Char(':') => {
                self.overlay = Some(Overlay::Palette {
                    query: String::new(),
                });
                Action::None
            }
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.overlay = Some(Overlay::Help);
                Action::None
            }
            KeyCode::Char('g') => {
                // Пайп открывается на той сущности, которую оператор уже
                // выбрал: расследование продолжается, а не начинается заново.
                self.overlay = Some(Overlay::Pipe(crate::pipe::PipeState::default()));
                self.status = "pipe: → раскрыть, ← свернуть, v вид, Esc выход".to_string();
                Action::None
            }
            KeyCode::Char('m') if self.screen == Screen::Entities => {
                self.entities.technical_view = !self.entities.technical_view;
                self.entities.selected = 0;
                Action::None
            }
            KeyCode::Char('s') if self.screen == Screen::Entities => {
                self.entities.sort = self.entities.sort.next();
                self.entities.direction = match self.entities.sort {
                    SortKey::Name => SortDirection::Ascending,
                    _ => SortDirection::Descending,
                };
                Action::None
            }
            KeyCode::Char('f') if self.screen == Screen::Entities => {
                self.entities.kind_filter = match self.entities.kind_filter {
                    None => Some(EntityKind::Process),
                    Some(EntityKind::Process) => Some(EntityKind::Container),
                    Some(EntityKind::Container) => Some(EntityKind::Unit),
                    Some(EntityKind::Unit) => Some(EntityKind::Cgroup),
                    Some(_) => None,
                };
                self.entities.selected = 0;
                Action::None
            }
            KeyCode::Char('p') => {
                self.paused = !self.paused;
                self.status = if self.paused {
                    "paused display; collection continues".to_string()
                } else {
                    String::new()
                };
                Action::None
            }
            KeyCode::Char('A') => {
                self.timeline.mark_a = Some(snapshot.at);
                Action::None
            }
            KeyCode::Char('B') => {
                self.timeline.mark_b = Some(snapshot.at);
                Action::None
            }
            KeyCode::Char('D') | KeyCode::Char('d') => {
                if self.timeline.mark_a.is_some() && self.timeline.mark_b.is_some() {
                    Action::RunDiff
                } else {
                    self.status = "set markers A and B first".to_string();
                    Action::None
                }
            }
            KeyCode::Esc => Action::None,
            _ => Action::None,
        }
    }

    /// Enter действует только на focused visible pane (§172).
    fn open_focused(&mut self, snapshot: &Snapshot) {
        if !self.pane_is_visible(self.pane) {
            self.status = "nothing to open".to_string();
            return;
        }
        match (self.screen, self.pane) {
            (Screen::Overview, Pane::Primary) => {
                if let Some(row) = overview_rows(snapshot, self).get(self.overview.selected) {
                    if let Some(entity) = snapshot.entity(row.row.id) {
                        self.open_inspector(entity.key.clone());
                    }
                }
            }
            (Screen::Overview, Pane::Inspector) => {
                if let Some(key) = preview_relation_key(
                    snapshot,
                    overview_rows(snapshot, self).get(self.overview.selected),
                    self.overview.preview_relation_selected,
                ) {
                    self.open_inspector(key);
                }
            }
            (Screen::Problems, Pane::Primary) => {
                if let Some(problem) = snapshot.problems.get(self.problems.selected) {
                    if let Some(entity) = snapshot.entity(problem.id.entity) {
                        self.open_inspector(entity.key.clone());
                    }
                }
            }
            (Screen::Entities, Pane::Primary) => {
                if let Some(row) = entity_logical_rows(snapshot, self).get(self.entities.selected) {
                    if let Some(entity) = snapshot.entity(row.row.id) {
                        self.open_inspector(entity.key.clone());
                    }
                }
            }
            (Screen::Entities, Pane::Inspector) => {
                if let Some(key) = preview_relation_key(
                    snapshot,
                    entity_logical_rows(snapshot, self).get(self.entities.selected),
                    self.entities.preview_relation_selected,
                ) {
                    self.open_inspector(key);
                }
            }
            (Screen::Timeline, Pane::Story) => {
                if let Some(event) = story_events(snapshot).get(self.timeline.story_selected) {
                    if let Some(entity_id) = event.entity {
                        if let Some(entity) = snapshot.entity(entity_id) {
                            self.open_inspector(entity.key.clone());
                        }
                    } else if self.pane_is_visible(Pane::Inspector) {
                        self.pane = Pane::Inspector;
                    }
                }
            }
            // TIME RAIL/Details не имеют неявной background selection.
            _ => self.status = "nothing to open".to_string(),
        }
    }
}

/// Логические строки Overview — тот же источник, что renderer.
fn overview_rows(snapshot: &Snapshot, app: &App) -> Vec<LogicalRow> {
    app.derived(snapshot).relevant.clone()
}

/// Ключ кэша производного списка: всё, от чего зависит его содержимое.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RowCacheKey {
    tick: u64,
    kind_filter: Option<pulse_core::EntityKind>,
    sort: SortKey,
    direction: SortDirection,
    technical_view: bool,
    query: Option<String>,
}

/// Производный вид одного снимка: всё, что считается из него для экранов.
#[derive(Clone, Debug)]
pub struct Derived {
    /// Технические строки: одна сущность — одна строка.
    pub rows: Vec<EntityRow>,
    /// Логические строки для экрана сущностей с учётом режима вида.
    pub logical: Vec<LogicalRow>,
    /// Значимые логические строки для Overview.
    pub relevant: Vec<LogicalRow>,
}

impl App {
    /// Производный вид с кэшированием по снимку и состоянию вида.
    ///
    /// Считается один раз на такт: и маршрутизация ввода, и отрисовка читают
    /// один и тот же результат. До кэша каждое нажатие клавиши перестраивало
    /// две тысячи строк дважды, и это ощущалось как торможение интерфейса.
    #[must_use]
    pub fn derived(&self, snapshot: &Snapshot) -> Rc<Derived> {
        let key = RowCacheKey {
            tick: snapshot.tick.0,
            kind_filter: self.entities.kind_filter,
            sort: self.entities.sort,
            direction: self.entities.direction,
            technical_view: self.entities.technical_view,
            query: self.search_query().map(str::to_owned),
        };
        if let Some((cached_key, derived)) = self.derived_cache.borrow().as_ref() {
            if *cached_key == key {
                return Rc::clone(derived);
            }
        }

        let rows = entity_rows(snapshot, self);
        let logical = if self.entities.technical_view {
            rows.iter()
                .map(|row| LogicalRow {
                    row: row.clone(),
                    processes: 0,
                    abnormal: 0,
                    technical: 0,
                    technical_id: None,
                    reasons: Vec::new(),
                    members: vec![row.id],
                })
                .collect()
        } else {
            fold(snapshot, &rows)
        };
        let relevant = relevant(snapshot, &rows, RELEVANT_LIMIT);
        let derived = Rc::new(Derived {
            rows,
            logical,
            relevant,
        });
        *self.derived_cache.borrow_mut() = Some((key, Rc::clone(&derived)));
        derived
    }
}

/// Максимум значимых логических строк Overview (§160).
pub const RELEVANT_LIMIT: usize = 12;

/// Логический/технический список Entities — тот же источник, что renderer./// Логический/технический список Entities — тот же источник, что renderer.
fn entity_logical_rows(snapshot: &Snapshot, app: &App) -> Vec<LogicalRow> {
    app.derived(snapshot).logical.clone()
}

fn preview_relation_key(
    snapshot: &Snapshot,
    row: Option<&LogicalRow>,
    selected: usize,
) -> Option<EntityKey> {
    logical_relation_targets(snapshot, row?)
        .get(selected)
        .map(|target| target.key.clone())
}

/// Relations of a folded object excluding links between its own technical
/// members. Renderer and Enter share this exact list (v0.9 §192, §195).
pub fn logical_relation_targets(snapshot: &Snapshot, row: &LogicalRow) -> Vec<RelationTarget> {
    let member_keys: Vec<&EntityKey> = row
        .members
        .iter()
        .filter_map(|id| snapshot.entity(*id).map(|entity| &entity.key))
        .collect();
    let mut out: Vec<RelationTarget> = Vec::new();
    for member in &row.members {
        for target in relation_targets_for(snapshot, *member) {
            if member_keys.contains(&&target.key)
                || out.iter().any(|existing| existing.key == target.key)
            {
                continue;
            }
            out.push(target);
        }
    }
    out
}

/// Полный сырой поток наблюдений: secondary subview (§183).
///
/// Здесь и только здесь виден kernel worker churn: эксперт должен иметь доступ
/// к сырым данным, но они не являются Story.
pub fn raw_events(snapshot: &Snapshot) -> Vec<&pulse_core::Event> {
    snapshot.events.iter().rev().collect()
}

/// Значимые события Story.
///
/// Классификация и группировка выполнены при сборке снимка
/// ([`pulse_core::semantic`]): экран не решает, что значимо, иначе Story и
/// Recent Changes расходятся между собой.
pub fn story_events(snapshot: &Snapshot) -> Vec<&pulse_core::MeaningfulEvent> {
    snapshot.meaningful.iter().collect()
}

/// Звенья цепочки расследования для сущности.
///
/// Единственный источник — [`crate::investigate`]: renderer и `Enter`
/// обязаны видеть один список, иначе появляется невидимый target.
pub fn relation_targets_for(snapshot: &Snapshot, id: pulse_core::EntityId) -> Vec<RelationTarget> {
    crate::investigate::drill_steps(snapshot, id)
}
