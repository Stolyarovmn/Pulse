//! Таблицы с приоритетом колонок (разделы 94, 95 спецификации v0.7).
//!
//! Таблица не обрезается справа: колонки исчезают по приоритету. Иначе при
//! сужении окна первым пропадает состояние - то есть именно то, ради чего
//! оператор смотрит таблицу.
//!
//! Приоритеты из раздела 95: `STATE` и `ENTITY` - 0, `CPU`/`MEM` - 1,
//! `OWNER`/`KIND` - 2, `IO`/`NET` - 3.

use crate::theme::Capability;
use crate::ui;

/// Выравнивание значения в колонке.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Описание колонки (раздел 95).
#[derive(Clone, Copy, Debug)]
pub struct Column {
    /// Заголовок в полном виде.
    pub title: &'static str,
    /// Заголовок при сжатии: `STATE` превращается в `S`.
    pub short: &'static str,
    /// 0 - никогда не скрывается.
    pub priority: u8,
    /// Минимальная ширина, при которой колонка ещё осмысленна.
    pub min_width: u16,
    /// Желаемая ширина.
    pub preferred_width: u16,
    pub align: Align,
    /// Порядок выбывания внутри одного приоритета: больше - уходит раньше.
    ///
    /// Нужен потому, что раздел 95 даёт `KIND` и `OWNER` один приоритет, а
    /// нормативный пример раздела 94 на среднем экране оставляет владельца и
    /// убирает вид. Владелец отвечает на продуктовый вопрос «кто владеет»
    /// (раздел 123), вид сущности - нет.
    pub drop_first: u8,
}

impl Column {
    #[must_use]
    pub const fn new(
        title: &'static str,
        short: &'static str,
        priority: u8,
        min_width: u16,
        preferred_width: u16,
        align: Align,
        drop_first: u8,
    ) -> Self {
        Self {
            title,
            short,
            priority,
            min_width,
            preferred_width,
            align,
            drop_first,
        }
    }
}

/// Выбранная раскладка таблицы.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TablePlan {
    /// Индексы видимых колонок в исходном порядке.
    pub visible: Vec<usize>,
    /// Ширина каждой видимой колонки.
    pub widths: Vec<u16>,
    /// Использовать короткие заголовки.
    pub short_titles: bool,
}

impl TablePlan {
    /// Заголовок колонки с учётом сжатия.
    #[must_use]
    pub fn title(&self, column: &Column) -> &'static str {
        if self.short_titles {
            column.short
        } else {
            column.title
        }
    }
}

/// Разделитель между колонками.
const GAP: u16 = 2;

/// Планирует таблицу под доступную ширину (раздел 94).
///
/// Колонки отбрасываются с самого низкого приоритета; при равном приоритете -
/// с конца, чтобы порядок оставался предсказуемым. Колонки приоритета 0 не
/// отбрасываются никогда: если они не влезают, они всё равно рисуются в
/// минимальной ширине, а лишнее усекается многоточием.
#[must_use]
pub fn plan(columns: &[Column], available: u16) -> TablePlan {
    let mut visible: Vec<usize> = (0..columns.len()).collect();

    let min_width = |index: usize| columns.get(index).map_or(0, |c: &Column| c.min_width);
    // Колонкам приоритета 0 и 1 нужна желаемая ширина, а не минимальная: иначе
    // таблица «влезает», но имя сущности усечено до десяти символов, тогда как
    // рядом стоят нулевые IO и NET. Раздел 94 требует обратного.
    let claim = |index: usize| {
        columns.get(index).map_or(0, |c: &Column| {
            if c.priority <= 1 {
                c.preferred_width
            } else {
                c.min_width
            }
        })
    };
    let total = |set: &[usize], width: &dyn Fn(usize) -> u16| -> u16 {
        let gaps = GAP.saturating_mul(u16::try_from(set.len().saturating_sub(1)).unwrap_or(0));
        set.iter()
            .map(|index| width(*index))
            .sum::<u16>()
            .saturating_add(gaps)
    };
    let required = |set: &[usize]| -> u16 { total(set, &min_width) };
    let desired = |set: &[usize]| -> u16 { total(set, &claim) };

    let rank = |index: usize| {
        columns
            .get(index)
            .map_or((0, 0), |c: &Column| (c.priority, c.drop_first))
    };
    // Кого выбросить среди колонок не ниже указанного приоритета.
    let victim = |set: &[usize], floor: u8| -> Option<usize> {
        set.iter()
            .enumerate()
            .filter(|(_, index)| rank(**index).0 >= floor)
            .max_by_key(|(position, index)| (rank(**index), *position))
            .map(|(position, _)| position)
    };

    // Фаза 1: второстепенные колонки уступают место желаемой ширине важных.
    // Именно это даёт пример раздела 94: на среднем экране имя остаётся
    // широким, а IO, NET и вид исчезают.
    while desired(&visible) > available {
        match victim(&visible, 2) {
            Some(position) => {
                visible.remove(position);
            }
            None => break,
        }
    }

    // Фаза 2: колонки приоритета 1 уходят только если не влезают даже
    // минимальные ширины. Нагрузка важнее вида и владельца, но состояние и
    // имя не уступают ей никогда.
    while required(&visible) > available {
        match victim(&visible, 1) {
            Some(position) => {
                visible.remove(position);
            }
            None => break,
        }
    }

    let short_titles = required(&visible) + 8 > available;

    // Раздаём остаток по желаемым ширинам.
    let gaps = GAP.saturating_mul(u16::try_from(visible.len().saturating_sub(1)).unwrap_or(0));
    let mut widths: Vec<u16> = visible.iter().map(|index| min_width(*index)).collect();
    let mut spare = available
        .saturating_sub(gaps)
        .saturating_sub(widths.iter().sum::<u16>());

    // Сначала до preferred, потом остаток - первой колонке: имя длиннее прочих.
    for (slot, index) in visible.iter().enumerate() {
        if spare == 0 {
            break;
        }
        let Some(column) = columns.get(*index) else {
            continue;
        };
        let want = column.preferred_width.saturating_sub(column.min_width);
        let give = want.min(spare);
        if let Some(width) = widths.get_mut(slot) {
            *width = width.saturating_add(give);
            spare -= give;
        }
    }
    if spare > 0 {
        if let Some(first) = widths.first_mut() {
            *first = first.saturating_add(spare);
        }
    }

    TablePlan {
        visible,
        widths,
        short_titles,
    }
}

/// Готовит значение под ширину колонки: усечение и выравнивание.
#[must_use]
pub fn cell(value: &str, width: u16, align: Align, capability: Capability) -> String {
    let width = usize::from(width);
    let value = ui::truncate(value, width, capability);
    let len = value.chars().count();
    if len >= width {
        return value;
    }
    let pad = width - len;
    match align {
        Align::Left => {
            let mut out = value;
            out.extend(std::iter::repeat_n(' ', pad));
            out
        }
        Align::Right => {
            let mut out = String::with_capacity(width);
            out.extend(std::iter::repeat_n(' ', pad));
            out.push_str(&value);
            out
        }
    }
}

/// Колонки таблицы сущностей (раздел 94).
#[must_use]
pub fn entity_columns() -> Vec<Column> {
    vec![
        Column::new("NAME", "NAME", 0, 10, 28, Align::Left, 0),
        Column::new("STATE", "S", 0, 1, 5, Align::Left, 0),
        Column::new("CPU", "CPU", 1, 4, 6, Align::Right, 0),
        Column::new("MEM", "MEM", 1, 6, 9, Align::Right, 0),
        // Вид уходит раньше владельца: см. `Column::drop_first`.
        Column::new("KIND", "KIND", 2, 8, 10, Align::Left, 1),
        Column::new("OWNER", "OWNER", 2, 8, 24, Align::Left, 0),
        Column::new("IO", "IO", 3, 5, 10, Align::Right, 1),
        Column::new("NET", "NET", 3, 5, 10, Align::Right, 0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Раздел 94: при сужении первыми уходят IO и NET, последними - имя и состояние.
    #[test]
    fn columns_disappear_by_priority() {
        let columns = entity_columns();
        let names = |plan: &TablePlan| -> Vec<&'static str> {
            plan.visible.iter().map(|i| columns[*i].title).collect()
        };

        let full = plan(&columns, 120);
        assert_eq!(
            names(&full),
            vec!["NAME", "STATE", "CPU", "MEM", "KIND", "OWNER", "IO", "NET"]
        );

        // Раздел 94, Medium: остаются ENTITY, CPU, MEM, OWNER, STATE.
        let medium = plan(&columns, 70);
        assert_eq!(
            names(&medium),
            vec!["NAME", "STATE", "CPU", "MEM", "OWNER"],
            "средний экран обязан сохранить владельца и убрать вид, IO и NET"
        );

        // Раздел 94, Narrow: остаются ENTITY, CPU, MEM, S.
        let narrow = plan(&columns, 40);
        assert_eq!(
            names(&narrow),
            vec!["NAME", "STATE", "CPU", "MEM"],
            "узкий экран оставляет только имя, состояние и нагрузку"
        );

        // Раздел 94, Tiny: имя и состояние.
        let tiny = plan(&columns, 14);
        assert_eq!(names(&tiny), vec!["NAME", "STATE"], "остаются только P0");
    }

    /// Колонки приоритета 0 не исчезают даже при абсурдной ширине.
    #[test]
    fn priority_zero_columns_never_disappear() {
        let columns = entity_columns();
        for width in [1_u16, 4, 8, 12] {
            let p = plan(&columns, width);
            let titles: Vec<&str> = p.visible.iter().map(|i| columns[*i].title).collect();
            assert!(titles.contains(&"NAME"), "имя обязано остаться при {width}");
            assert!(
                titles.contains(&"STATE"),
                "состояние обязано остаться при {width}"
            );
        }
    }

    /// Сумма ширин с разделителями не превышает доступное место.
    #[test]
    fn widths_fit_available_space() {
        let columns = entity_columns();
        for width in [14_u16, 30, 50, 70, 100, 120, 160, 200] {
            let p = plan(&columns, width);
            let gaps = 2 * (p.visible.len().saturating_sub(1)) as u16;
            let total: u16 = p.widths.iter().sum::<u16>() + gaps;
            assert!(
                total <= width,
                "таблица шире доступного: {total} > {width} при {} колонках",
                p.visible.len()
            );
        }
    }

    #[test]
    fn short_titles_appear_only_when_tight() {
        let columns = entity_columns();
        assert!(!plan(&columns, 160).short_titles);
        assert!(plan(&columns, 16).short_titles);
    }

    /// Раздел 96: значение усекается, а не ломает вёрстку.
    #[test]
    fn cell_pads_and_truncates() {
        assert_eq!(cell("pulse", 8, Align::Left, Capability::Ascii), "pulse   ");
        assert_eq!(
            cell("pulse", 8, Align::Right, Capability::Ascii),
            "   pulse"
        );
        let long = cell(
            "checkout-api-production",
            10,
            Align::Left,
            Capability::Ascii,
        );
        assert_eq!(long.chars().count(), 10);
        assert!(long.ends_with("..."));
    }

    /// Ширина ячейки соблюдается и для не-ASCII значений.
    #[test]
    fn cell_width_is_exact_for_unicode() {
        let value = cell("сервис-имя-длинное", 12, Align::Left, Capability::TrueColor);
        assert_eq!(value.chars().count(), 12);
    }

    /// План детерминирован: одна ширина - одна раскладка.
    #[test]
    fn plan_is_deterministic() {
        let columns = entity_columns();
        for width in [20_u16, 44, 88, 132] {
            assert_eq!(plan(&columns, width), plan(&columns, width));
        }
    }
}
