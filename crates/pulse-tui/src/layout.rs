//! Layout engine спецификации v0.7 (разделы 87-116).
//!
//! Экран описывается не координатами, а политикой: класс ширины, класс высоты,
//! приоритет блоков и уровень сжатия. Один и тот же размер терминала при том же
//! экране и фокусе обязан давать один и тот же план - это проверяется тестом.
//!
//! Отдельная забота - дрожание на границе (раздел 109). Без гистерезиса окно,
//! которое пользователь тянет мышью, скачет между режимами на каждом пикселе.

use crate::app::Screen;

/// Класс ширины терминала (раздел 90).
///
/// Границы взяты из спецификации буквально и не «улучшены»: XL >= 160,
/// Wide 120-159, Medium 90-119, Narrow 60-89, Tiny < 60.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WidthClass {
    Tiny,
    Narrow,
    Medium,
    Wide,
    Xl,
}

impl WidthClass {
    /// Класс без учёта предыдущего состояния.
    #[must_use]
    pub fn from_width(width: u16) -> Self {
        match width {
            0..=59 => Self::Tiny,
            60..=89 => Self::Narrow,
            90..=119 => Self::Medium,
            120..=159 => Self::Wide,
            _ => Self::Xl,
        }
    }

    /// Нижняя граница входа в класс.
    #[must_use]
    const fn enter_at(self) -> u16 {
        match self {
            Self::Tiny => 0,
            Self::Narrow => 60,
            Self::Medium => 90,
            Self::Wide => 120,
            Self::Xl => 160,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Narrow => "narrow",
            Self::Medium => "medium",
            Self::Wide => "wide",
            Self::Xl => "xl",
        }
    }
}

/// Класс высоты терминала (раздел 91).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HeightClass {
    VeryShort,
    Short,
    Normal,
    Tall,
}

impl HeightClass {
    #[must_use]
    pub fn from_height(height: u16) -> Self {
        match height {
            0..=15 => Self::VeryShort,
            16..=23 => Self::Short,
            24..=39 => Self::Normal,
            _ => Self::Tall,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VeryShort => "very-short",
            Self::Short => "short",
            Self::Normal => "normal",
            Self::Tall => "tall",
        }
    }
}

/// Минимальный поддерживаемый терминал (раздел 110).
pub const MIN_WIDTH: u16 = 50;
/// Минимальная поддерживаемая высота.
pub const MIN_HEIGHT: u16 = 12;

/// Ширина гистерезиса на границе класса (раздел 109).
///
/// Спецификация даёт пример «вход в Wide при >= 122, выход при <= 116», то есть
/// зона шириной 6 колонок вокруг границы 120. Обобщаю это правило на все
/// границы: удержание требует, чтобы ширина упала ниже `enter - HYSTERESIS`.
pub const HYSTERESIS: u16 = 4;

/// Уровень сжатия содержимого блока (раздел 93).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Compression {
    /// Полные названия и значения.
    Full,
    /// Сокращённые заголовки, значения целиком.
    Compact,
    /// Только ключевые числа.
    Terse,
    /// Только состояние.
    IconOnly,
}

/// Приоритет блока (раздел 92).
///
/// `P0` не скрывается никогда: это режим, состояние, критические проблемы и
/// выбранная сущность. Именно поэтому «terminal too small» - отдельный экран,
/// а не обрезанный обычный: если P0 не помещается, показывать нечего.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

/// Пресет State Glyph (разделы 98, 99).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GlyphPreset {
    Hidden,
    Minimal,
    Compact,
    Medium,
    Large,
}

impl GlyphPreset {
    /// Сколько строк занимает пресет.
    #[must_use]
    pub const fn rows(self) -> u16 {
        match self {
            Self::Hidden => 0,
            Self::Minimal | Self::Compact => 1,
            Self::Medium => 3,
            Self::Large => 7,
        }
    }

    /// Сколько колонок занимает пресет при рендере через один пробел.
    #[must_use]
    pub const fn cols(self) -> u16 {
        match self {
            Self::Hidden => 0,
            Self::Minimal => 1,
            // 5 ячеек подряд без разрядки.
            Self::Compact => 5,
            // 5 ячеек через пробел.
            Self::Medium => 9,
            // 9 ячеек через пробел.
            Self::Large => 17,
        }
    }
}

/// Пресет шапки (раздел 106).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderPreset {
    Wide,
    Medium,
    Narrow,
    Tiny,
}

/// Пресет футера (раздел 105).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterPreset {
    Wide,
    Medium,
    Narrow,
    Tiny,
}

/// Панель рабочей области.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    /// Главный список или основное содержимое экрана.
    Primary,
    /// Инспектор / детали выбранного объекта.
    Inspector,
    /// Журнал событий (Story) на Timeline и в расследовании.
    Story,
}

impl Pane {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Inspector => "inspector",
            Self::Story => "story",
        }
    }
}

/// Вход layout engine (раздел 112).
#[derive(Clone, Copy, Debug)]
pub struct LayoutContext {
    pub width: u16,
    pub height: u16,
    pub screen: Screen,
    pub focus: Pane,
    /// Есть ли открытая проблема: критическое состояние поднимает приоритет
    /// блока внимания и запрещает скрывать состояние.
    pub has_problem: bool,
    /// Сколько строк содержимого реально есть у второстепенного блока.
    /// Раздел 108: блок из двух строк не имеет права занимать 40% экрана.
    pub story_len: u16,
}

/// Результат планирования (раздел 112).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutPlan {
    pub width_class: WidthClass,
    pub height_class: HeightClass,
    /// Терминал меньше минимального - рисуется только предупреждение.
    pub too_small: bool,
    pub glyph: GlyphPreset,
    pub header: HeaderPreset,
    pub footer: FooterPreset,
    /// Показывать ли разделители с линией (раздел 134).
    pub section_rules: bool,
    /// Инспектор виден рядом с таблицей, а не как отдельная вкладка.
    pub inspector_beside: bool,
    /// Уровень сжатия текста в блоках.
    pub compression: Compression,
    /// Минимальный приоритет, который ещё показывается.
    pub keep_until: Priority,
}

impl LayoutPlan {
    /// Виден ли блок такого приоритета.
    #[must_use]
    pub fn shows(&self, priority: Priority) -> bool {
        priority <= self.keep_until
    }
}

/// Состояние движка между кадрами: нужно только для гистерезиса (раздел 109).
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutEngine {
    last_width_class: Option<WidthClass>,
}

impl LayoutEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Класс ширины с гистерезисом.
    ///
    /// Повышение класса требует обычной границы; понижение - падения ниже
    /// границы за вычетом [`HYSTERESIS`]. Так окно на 118 колонках, пришедшее
    /// из Wide, остаётся Wide, а на 115 - честно уходит в Medium.
    fn width_class(&mut self, width: u16) -> WidthClass {
        let raw = WidthClass::from_width(width);
        let class = match self.last_width_class {
            Some(previous) if raw < previous => {
                let hold = previous.enter_at().saturating_sub(HYSTERESIS);
                if width >= hold {
                    previous
                } else {
                    raw
                }
            }
            _ => raw,
        };
        self.last_width_class = Some(class);
        class
    }

    /// Планирует кадр. Детерминирован при одинаковом входе и одинаковой истории.
    pub fn plan(&mut self, ctx: LayoutContext) -> LayoutPlan {
        let width_class = self.width_class(ctx.width);
        let height_class = HeightClass::from_height(ctx.height);
        let too_small = ctx.width < MIN_WIDTH || ctx.height < MIN_HEIGHT;

        // Порядок из раздела 113: сначала обязательное, потом по приоритету.
        let keep_until = match (width_class, height_class) {
            (_, HeightClass::VeryShort) => Priority::P0,
            (WidthClass::Tiny, _) => Priority::P0,
            (WidthClass::Narrow, HeightClass::Short) => Priority::P1,
            (WidthClass::Narrow, _) | (_, HeightClass::Short) => Priority::P1,
            (WidthClass::Medium, _) => Priority::P2,
            (WidthClass::Wide | WidthClass::Xl, _) => Priority::P3,
        };

        let glyph = Self::glyph_preset(ctx, width_class, height_class);

        let header = match width_class {
            WidthClass::Xl | WidthClass::Wide => HeaderPreset::Wide,
            WidthClass::Medium => HeaderPreset::Medium,
            WidthClass::Narrow => HeaderPreset::Narrow,
            WidthClass::Tiny => HeaderPreset::Tiny,
        };

        let footer = match width_class {
            WidthClass::Xl | WidthClass::Wide => FooterPreset::Wide,
            WidthClass::Medium => FooterPreset::Medium,
            WidthClass::Narrow => FooterPreset::Narrow,
            WidthClass::Tiny => FooterPreset::Tiny,
        };

        let compression = match width_class {
            WidthClass::Xl | WidthClass::Wide => Compression::Full,
            WidthClass::Medium => Compression::Compact,
            WidthClass::Narrow => Compression::Terse,
            WidthClass::Tiny => Compression::IconOnly,
        };

        LayoutPlan {
            width_class,
            height_class,
            too_small,
            glyph,
            header,
            footer,
            // Линия рядом с заголовком требует места под сам заголовок и хвост.
            section_rules: width_class >= WidthClass::Medium,
            inspector_beside: width_class >= WidthClass::Wide && height_class >= HeightClass::Short,
            compression,
            keep_until,
        }
    }

    /// Выбор пресета glyph по доступному месту (раздел 99).
    ///
    /// `Hidden` допустим только когда состояние уже видно в шапке, поэтому
    /// он выбирается лишь при нехватке строк, а не «для экономии».
    fn glyph_preset(
        ctx: LayoutContext,
        width_class: WidthClass,
        height_class: HeightClass,
    ) -> GlyphPreset {
        // На экранах без блока состояния glyph не рисуется вообще.
        if !matches!(
            ctx.screen,
            Screen::Overview | Screen::Problems | Screen::Timeline
        ) {
            return GlyphPreset::Hidden;
        }
        match (width_class, height_class) {
            (_, HeightClass::VeryShort) => GlyphPreset::Minimal,
            (WidthClass::Tiny, _) => GlyphPreset::Minimal,
            (WidthClass::Narrow, HeightClass::Short) => GlyphPreset::Compact,
            (WidthClass::Narrow, _) => GlyphPreset::Medium,
            (_, HeightClass::Short) => GlyphPreset::Compact,
            (WidthClass::Medium, _) => GlyphPreset::Medium,
            (WidthClass::Wide | WidthClass::Xl, _) => GlyphPreset::Large,
        }
    }
}

/// Композиция главного экрана (раздел 161).
///
/// Ширина не растягивает блоки, а добавляет соседние панели: больше места
/// означает больше контекста одновременно, а не больше колонок таблицы
/// (разделы 143, 160).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverviewComposition {
    /// До ~129 колонок: блоки идут друг под другом.
    Stacked,
    /// 130-159: состояние и внимание рядом, таблица на всю ширину.
    StateBesideAttention,
    /// 160-199: плюс панель выбранной сущности справа от таблицы.
    WithSelected,
    /// 200+: место уходит в контекст, а не в растяжение.
    WithContext,
}

impl OverviewComposition {
    /// Композиция по ширине и высоте.
    ///
    /// Высота учитывается: две панели рядом требуют строк на заголовки, и на
    /// коротком терминале выгоднее оставить один блок (раздел 12 задания).
    #[must_use]
    pub fn resolve(width_class: WidthClass, height_class: HeightClass) -> Self {
        let width = match width_class {
            WidthClass::Tiny => 50,
            WidthClass::Narrow => 70,
            WidthClass::Medium => 100,
            WidthClass::Wide => 130,
            WidthClass::Xl => 180,
        };
        let height = match height_class {
            HeightClass::VeryShort => 12,
            HeightClass::Short => 18,
            HeightClass::Normal => 30,
            HeightClass::Tall => 40,
        };
        Self::resolve_dimensions(width, height)
    }

    /// Точный gate v0.9 §166: Selected обязателен при ширине >=140 и body
    /// height >=18. WidthClass недостаточно точен: его XL начинается с 160.
    #[must_use]
    pub const fn resolve_dimensions(width: u16, body_height: u16) -> Self {
        if body_height < 18 {
            return if width >= 120 {
                Self::StateBesideAttention
            } else {
                Self::Stacked
            };
        }
        if width >= 200 {
            Self::WithContext
        } else if width >= 140 {
            Self::WithSelected
        } else if width >= 120 {
            Self::StateBesideAttention
        } else {
            Self::Stacked
        }
    }

    /// Показывать ли панель выбранной сущности (разделы 145, 146).
    #[must_use]
    pub const fn shows_selected(self) -> bool {
        matches!(self, Self::WithSelected | Self::WithContext)
    }
}

/// Максимальная полезная ширина блока (раздел 144).
///
/// После этого предела дополнительное место не улучшает читаемость: длинная
/// линия-разделитель на 190 колонок под одной короткой фразой и колонка
/// `OWNER` в 60 символов - это шум, а не информация. Остаток отдаётся соседней
/// панели или остаётся намеренной пустотой.
#[must_use]
pub const fn max_useful_width(kind: BlockKind) -> u16 {
    match kind {
        BlockKind::StateBlock => 42,
        BlockKind::Attention => 76,
        BlockKind::Signals => 96,
        BlockKind::Changes => 120,
        BlockKind::EntityTable => 88,
        BlockKind::Selected => 68,
    }
}

/// Вид блока: нужен только для предела полезной ширины.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    StateBlock,
    Attention,
    Signals,
    Changes,
    EntityTable,
    Selected,
}

/// Ограничивает ширину блока полезным пределом.
///
/// Возвращает пару «ширина блока, остаток». Остаток - не потеря: вызывающий
/// решает, отдать его соседней панели или оставить пустым.
#[must_use]
pub fn bounded(kind: BlockKind, available: u16) -> (u16, u16) {
    let cap = max_useful_width(kind);
    if available <= cap {
        (available, 0)
    } else {
        (cap, available - cap)
    }
}

/// Высота второстепенного блока по его содержимому (раздел 108).
///
/// Блок получает столько строк, сколько у него есть содержимого, но не больше
/// потолка и не меньше минимума. Свободное место остаётся таблице сущностей.
#[must_use]
pub fn content_height(content_lines: u16, min: u16, max: u16) -> u16 {
    content_lines.clamp(min, max.max(min))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(width: u16, height: u16) -> LayoutContext {
        LayoutContext {
            width,
            height,
            screen: Screen::Overview,
            focus: Pane::Primary,
            has_problem: false,
            story_len: 3,
        }
    }

    #[test]
    fn width_classes_follow_spec_boundaries() {
        assert_eq!(WidthClass::from_width(59), WidthClass::Tiny);
        assert_eq!(WidthClass::from_width(60), WidthClass::Narrow);
        assert_eq!(WidthClass::from_width(89), WidthClass::Narrow);
        assert_eq!(WidthClass::from_width(90), WidthClass::Medium);
        assert_eq!(WidthClass::from_width(119), WidthClass::Medium);
        assert_eq!(WidthClass::from_width(120), WidthClass::Wide);
        assert_eq!(WidthClass::from_width(159), WidthClass::Wide);
        assert_eq!(WidthClass::from_width(160), WidthClass::Xl);
    }

    #[test]
    fn height_classes_follow_spec_boundaries() {
        assert_eq!(HeightClass::from_height(15), HeightClass::VeryShort);
        assert_eq!(HeightClass::from_height(16), HeightClass::Short);
        assert_eq!(HeightClass::from_height(23), HeightClass::Short);
        assert_eq!(HeightClass::from_height(24), HeightClass::Normal);
        assert_eq!(HeightClass::from_height(39), HeightClass::Normal);
        assert_eq!(HeightClass::from_height(40), HeightClass::Tall);
    }

    /// Раздел 109: у границы layout не должен дрожать.
    #[test]
    fn hysteresis_holds_class_when_shrinking_slightly() {
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.plan(ctx(122, 30)).width_class, WidthClass::Wide);
        // 118 < 120, но выше 120-4: остаёмся в Wide.
        assert_eq!(engine.plan(ctx(118, 30)).width_class, WidthClass::Wide);
        assert_eq!(engine.plan(ctx(116, 30)).width_class, WidthClass::Wide);
        // 115 ниже зоны удержания - честно уходим.
        assert_eq!(engine.plan(ctx(115, 30)).width_class, WidthClass::Medium);
    }

    /// Рост класса гистерезис не задерживает: расширение окна должно
    /// немедленно давать больше информации.
    #[test]
    fn hysteresis_does_not_delay_growth() {
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.plan(ctx(100, 30)).width_class, WidthClass::Medium);
        assert_eq!(engine.plan(ctx(120, 30)).width_class, WidthClass::Wide);
    }

    /// Раздел 113: одинаковый вход даёт одинаковый план.
    #[test]
    fn plan_is_deterministic() {
        let mut a = LayoutEngine::new();
        let mut b = LayoutEngine::new();
        for width in [50_u16, 60, 90, 120, 160, 200] {
            for height in [12_u16, 18, 24, 30, 40, 50] {
                assert_eq!(
                    a.plan(ctx(width, height)),
                    b.plan(ctx(width, height)),
                    "план обязан зависеть только от входа: {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn too_small_terminal_is_reported() {
        let mut engine = LayoutEngine::new();
        assert!(engine.plan(ctx(43, 9)).too_small, "43x9 меньше минимума");
        assert!(engine.plan(ctx(50, 11)).too_small, "высоты 11 не хватает");
        assert!(!engine.plan(ctx(50, 12)).too_small, "50x12 - минимум");
    }

    /// Раздел 92: критическое состояние не скрывается ни при каком размере,
    /// поэтому glyph никогда не становится `Hidden` на экране состояния.
    #[test]
    fn state_is_never_hidden_on_state_screens() {
        let mut engine = LayoutEngine::new();
        for width in [50_u16, 60, 80, 90, 120, 160, 200] {
            for height in [12_u16, 16, 24, 40] {
                let plan = engine.plan(LayoutContext {
                    has_problem: true,
                    ..ctx(width, height)
                });
                assert_ne!(
                    plan.glyph,
                    GlyphPreset::Hidden,
                    "состояние обязано быть видно: {width}x{height}"
                );
                assert!(plan.shows(Priority::P0));
            }
        }
    }

    /// Раздел 99: glyph обязан влезать в отведённое место.
    #[test]
    fn glyph_preset_fits_terminal() {
        let mut engine = LayoutEngine::new();
        for width in [50_u16, 60, 80, 90, 120, 160] {
            for height in [12_u16, 16, 24, 40] {
                let plan = engine.plan(ctx(width, height));
                assert!(
                    plan.glyph.cols() <= width,
                    "glyph шире терминала: {width}x{height}"
                );
                assert!(
                    plan.glyph.rows() + 6 <= height,
                    "glyph не оставил места под шапку и футер: {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn large_glyph_only_on_wide_terminals() {
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.plan(ctx(160, 40)).glyph, GlyphPreset::Large);
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.plan(ctx(120, 30)).glyph, GlyphPreset::Large);
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.plan(ctx(100, 30)).glyph, GlyphPreset::Medium);
        let mut engine = LayoutEngine::new();
        assert_eq!(engine.plan(ctx(55, 14)).glyph, GlyphPreset::Minimal);
    }

    /// Раздел 128: узкий терминал превращает инспектор в вкладку,
    /// а не в испорченную узкую колонку.
    #[test]
    fn inspector_becomes_tab_when_narrow() {
        let mut engine = LayoutEngine::new();
        assert!(engine.plan(ctx(160, 40)).inspector_beside);
        let mut engine = LayoutEngine::new();
        assert!(!engine.plan(ctx(100, 30)).inspector_beside);
        let mut engine = LayoutEngine::new();
        assert!(!engine.plan(ctx(70, 20)).inspector_beside);
    }

    /// Раздел 134: линия у заголовка только когда ширина позволяет.
    #[test]
    fn section_rules_disappear_on_narrow() {
        let mut engine = LayoutEngine::new();
        assert!(engine.plan(ctx(120, 30)).section_rules);
        let mut engine = LayoutEngine::new();
        assert!(!engine.plan(ctx(70, 20)).section_rules);
    }

    /// Раздел 108: короткий журнал не занимает пол-экрана.
    #[test]
    fn content_height_respects_content() {
        assert_eq!(content_height(2, 1, 8), 2);
        assert_eq!(content_height(0, 1, 8), 1);
        assert_eq!(content_height(40, 1, 8), 8);
        // Потолок ниже минимума не должен давать перевёрнутый диапазон.
        assert_eq!(content_height(5, 3, 1), 3);
    }

    /// Раздел 161: прогрессия композиции по ширине.
    #[test]
    fn overview_composition_follows_width_progression() {
        let at = |w: u16, h: u16| {
            OverviewComposition::resolve(WidthClass::from_width(w), HeightClass::from_height(h))
        };
        assert_eq!(at(100, 30), OverviewComposition::Stacked);
        assert_eq!(at(130, 30), OverviewComposition::StateBesideAttention);
        assert_eq!(at(160, 40), OverviewComposition::WithSelected);
        assert_eq!(at(200, 50), OverviewComposition::WithSelected);
        assert!(at(160, 40).shows_selected());
        assert!(!at(100, 30).shows_selected());
    }

    /// Короткий терминал не получает третью панель: строки нужнее ширины.
    #[test]
    fn selected_appears_at_minimum_v09_height() {
        let plan = OverviewComposition::resolve(WidthClass::Xl, HeightClass::Short);
        assert_eq!(plan, OverviewComposition::WithSelected);
        assert!(plan.shows_selected());
    }

    /// Раздел 144: блок не растёт бесконечно, остаток уходит соседу.
    #[test]
    fn blocks_stop_growing_after_max_useful_width() {
        let (table, spare) = bounded(BlockKind::EntityTable, 200);
        assert_eq!(table, max_useful_width(BlockKind::EntityTable));
        assert_eq!(spare, 200 - table, "остаток обязан быть отдан соседу");

        // Узкий терминал получает всю доступную ширину без остатка.
        let (table, spare) = bounded(BlockKind::EntityTable, 60);
        assert_eq!(table, 60);
        assert_eq!(spare, 0);
    }

    /// Пределы полезной ширины согласованы между собой: панель выбранного не
    /// может быть шире таблицы, ради которой она существует.
    #[test]
    fn useful_widths_are_consistent() {
        assert!(max_useful_width(BlockKind::Selected) < max_useful_width(BlockKind::EntityTable));
        assert!(max_useful_width(BlockKind::StateBlock) < max_useful_width(BlockKind::Attention));
    }

    /// Приоритеты сжимаются монотонно: чем меньше место, тем меньше видно.
    #[test]
    fn priority_budget_shrinks_monotonically() {
        let mut engine = LayoutEngine::new();
        let wide = engine.plan(ctx(160, 40)).keep_until;
        let mut engine = LayoutEngine::new();
        let medium = engine.plan(ctx(100, 30)).keep_until;
        let mut engine = LayoutEngine::new();
        let narrow = engine.plan(ctx(70, 20)).keep_until;
        let mut engine = LayoutEngine::new();
        let tiny = engine.plan(ctx(55, 13)).keep_until;
        assert!(wide >= medium, "wide показывает не меньше medium");
        assert!(medium >= narrow, "medium показывает не меньше narrow");
        assert!(narrow >= tiny, "narrow показывает не меньше tiny");
        assert_eq!(tiny, Priority::P0);
    }
}
