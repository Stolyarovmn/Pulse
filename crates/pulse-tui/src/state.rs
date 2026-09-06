//! Алфавит состояния и State Glyph главного экрана.
//!
//! Две независимые оси, которые нельзя смешивать:
//!
//! * **тип символа** отвечает, насколько всё серьёзно (`○ ● ◉ ◇ ◆ ▲ ×`);
//! * **положение символа** в фигуре отвечает, где именно проблема.
//!
//! Поэтому запрещено кодировать подсистему символом («`○` — это CPU»): символ
//! всегда означает только класс состояния. Подсистема определяется сектором
//! фигуры, и оператор читает диагноз до чтения подписей.
//!
//! Смысл сохраняется в монохроме: круглые символы — система управляема, ромбы —
//! выход из штатного режима, треугольник — требуется внимание сейчас, крест —
//! функция потеряна. Цвет только усиливает уже читаемую форму.

use pulse_core::entity::EntityKind;
use pulse_core::metric::ids;
use pulse_core::problem::Severity;
use pulse_core::snapshot::Snapshot;

use crate::theme::{Capability, Theme};

/// Класс состояния. Порядок объявления задаёт «хуже»: сравнение и `max` дают
/// худший класс без отдельной таблицы приоритетов.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateClass {
    /// Сигнала нет: фон фигуры.
    Inactive,
    /// Штатное состояние.
    Normal,
    /// Заметная активность.
    Active,
    /// Сильная нагрузка, но система ещё управляема.
    Saturated,
    /// Отклонение от штатного режима.
    Degraded,
    /// Существенное нарушение.
    Warning,
    /// Критическое состояние, требующее внимания сейчас.
    Critical,
    /// Функция или объект потеряны.
    Failed,
}

impl StateClass {
    /// Символ класса. В ASCII-режиме — только ASCII, иначе старый терминал
    /// покажет вопросительные знаки вместо диагноза.
    #[must_use]
    pub const fn symbol(self, capability: Capability) -> char {
        if matches!(capability, Capability::Ascii) {
            return match self {
                StateClass::Inactive => '.',
                StateClass::Normal => 'o',
                StateClass::Active => 'O',
                StateClass::Saturated => '0',
                StateClass::Degraded => '<',
                StateClass::Warning => '*',
                StateClass::Critical => '^',
                StateClass::Failed => 'x',
            };
        }
        match self {
            StateClass::Inactive => '·',
            StateClass::Normal => '○',
            StateClass::Active => '●',
            StateClass::Saturated => '◉',
            StateClass::Degraded => '◇',
            StateClass::Warning => '◆',
            StateClass::Critical => '▲',
            StateClass::Failed => '×',
        }
    }

    /// Подпись класса для легенды (раздел 120).
    ///
    /// Текст обязан объяснять символ, а не дублировать его: оператор читает
    /// легенду один раз и дальше пользуется формой.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            StateClass::Inactive => "background: no contribution",
            StateClass::Normal => "normal edge / reference",
            StateClass::Active => "normal active mass",
            StateClass::Saturated => "saturated but controlled",
            StateClass::Degraded => "degraded",
            StateClass::Warning => "warning",
            StateClass::Critical => "critical",
            StateClass::Failed => "lost / failed function",
        }
    }

    /// Вышла ли система из штатного режима: ромб и хуже.
    #[must_use]
    pub const fn is_abnormal(self) -> bool {
        matches!(
            self,
            StateClass::Degraded | StateClass::Warning | StateClass::Critical | StateClass::Failed
        )
    }

    /// Класс из серьёзности открытой проблемы.
    #[must_use]
    pub const fn from_severity(severity: Severity) -> Self {
        match severity {
            Severity::Info => StateClass::Degraded,
            Severity::Warn => StateClass::Warning,
            Severity::Crit => StateClass::Critical,
        }
    }

    /// Класс из доли использования ресурса.
    ///
    /// Пороги те же, что у правил (0.85 / 0.95), поэтому картинка и список
    /// проблем не могут противоречить друг другу.
    #[must_use]
    pub fn from_ratio(value: f64) -> Self {
        if !value.is_finite() {
            return StateClass::Inactive;
        }
        if value >= 0.95 {
            StateClass::Warning
        } else if value >= 0.85 {
            StateClass::Degraded
        } else if value >= 0.60 {
            StateClass::Saturated
        } else if value >= 0.15 {
            StateClass::Active
        } else if value > 0.0 {
            StateClass::Normal
        } else {
            StateClass::Inactive
        }
    }

    /// Цвет класса. Цвет усиливает форму, а не заменяет её.
    #[must_use]
    pub fn style(self, theme: &Theme) -> ratatui::style::Style {
        match self {
            // Фон поля обязан быть виден, но не должен спорить с массой.
            StateClass::Inactive => theme.dim(),
            // Контур - система отсчёта, а не активность.
            StateClass::Normal => theme.dim(),
            // Штатная активная масса: спокойный цвет, а не белый акцент.
            StateClass::Active => theme.nominal(),
            // Насыщение требует внимания, но проблемой ещё не является.
            StateClass::Saturated | StateClass::Degraded => theme.severity(Severity::Info),
            StateClass::Warning => theme.severity(Severity::Warn),
            StateClass::Critical | StateClass::Failed => theme.severity(Severity::Crit),
        }
    }
}

/// Сектор фигуры: отвечает на вопрос «где», а не «насколько плохо».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sector {
    /// Верх фигуры.
    Cpu,
    /// Правая часть фигуры.
    Memory,
    /// Низ фигуры.
    Io,
    /// Левая часть фигуры.
    Net,
    /// Ядро фигуры: общее состояние системы.
    Core,
}

impl Sector {
    /// Подпись сектора для легенды.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Sector::Cpu => "cpu",
            Sector::Memory => "mem",
            Sector::Io => "io",
            Sector::Net => "net",
            Sector::Core => "system",
        }
    }

    /// Сектор проблемы по идентификатору правила.
    ///
    /// Привязка к правилам, а не к произвольным подстрокам имени сущности:
    /// правило знает, какой ресурс он измеряет.
    #[must_use]
    pub fn for_rule(rule: &str) -> Self {
        match rule {
            "psi.cpu" | "cgroup.throttle" => Sector::Cpu,
            "memory.pressure" | "memory.oom" | "swap.pressure" | "psi.memory" => Sector::Memory,
            "disk.latency" | "psi.io" => Sector::Io,
            // Правил сети пока нет; неизвестное правило относим к ядру, чтобы
            // проблема не потерялась молча в неверном секторе.
            _ => Sector::Core,
        }
    }
}

/// Форма фигуры: для каждой строки — позиции ячеек в полуклетках.
///
/// Значение `x` умножается на 2 и даёт индекс символа в строке. Форма
/// намеренно фиксирована: здоровое состояние обязано выглядеть одинаково на
/// любом хосте, иначе «узнаваемая форма» перестаёт быть узнаваемой.
const SHAPE: [&[u8]; 8] = [
    &[4, 6],
    &[3, 5, 7],
    &[2, 4, 6, 8],
    &[1, 3, 5, 7, 9],
    &[1, 3, 5, 7, 9],
    &[2, 4, 6, 8],
    &[3, 5, 7],
    &[4, 6],
];

/// Ширина строки фигуры в символах.
const SHAPE_WIDTH: usize = 20;

/// Состояние подсистем и вердикт для главного экрана.
#[derive(Clone, Copy, Debug)]
pub struct StateGlyph {
    cpu: StateClass,
    memory: StateClass,
    io: StateClass,
    net: StateClass,
    core: StateClass,
    /// Нагрузка секторов 0..1 в том же порядке, что и `Sector`.
    ///
    /// §120 определяет `·` как «no active state contribution», а `●` как
    /// «normal active mass». Значит масса фигуры обязана расти с нагрузкой:
    /// на простаивающем хосте залитый диск утверждал бы активность, которой
    /// нет. Класс отвечает за severity, нагрузка - за площадь.
    load: SectorLoad,
}

/// Нагрузка подсистем 0..1.
#[derive(Clone, Copy, Debug, Default)]
struct SectorLoad {
    cpu: f64,
    memory: f64,
    io: f64,
    net: f64,
    core: f64,
}

impl SectorLoad {
    fn get(self, sector: Sector) -> f64 {
        let value = match sector {
            Sector::Cpu => self.cpu,
            Sector::Memory => self.memory,
            Sector::Io => self.io,
            Sector::Net => self.net,
            Sector::Core => self.core,
        };
        value.clamp(0.0, 1.0)
    }
}

impl StateGlyph {
    /// Считает состояние подсистем из снимка.
    ///
    /// Берётся худшее из двух источников: открытых проблем этого сектора и
    /// текущей утилизации. Первое даёт объяснимость, второе — реакцию до того,
    /// как правило подтвердит проблему гистерезисом.
    #[must_use]
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        let host = snapshot.host;
        let value = |metric| snapshot.value(host, metric);

        let net_load = Self::net_load(snapshot);
        let load = SectorLoad {
            cpu: Self::load_of(&[
                value(ids::HOST_CPU_UTIL),
                value(ids::HOST_PSI_CPU_SOME_AVG10).map(|v| v * 4.0),
            ]),
            memory: Self::load_of(&[
                value(ids::HOST_MEM_UTIL),
                value(ids::HOST_PSI_MEM_FULL_AVG10).map(|v| v * 8.0),
            ]),
            io: Self::load_of(&[
                value(ids::HOST_PSI_IO_FULL_AVG10).map(|v| v * 8.0),
                value(ids::HOST_CPU_IOWAIT).map(|v| v * 4.0),
            ]),
            net: net_load,
            core: 0.0,
        };

        let mut glyph = StateGlyph {
            cpu: Self::from_metrics(&[
                value(ids::HOST_CPU_UTIL),
                value(ids::HOST_PSI_CPU_SOME_AVG10).map(|v| v * 4.0),
            ]),
            memory: Self::from_metrics(&[
                value(ids::HOST_MEM_UTIL),
                value(ids::HOST_PSI_MEM_FULL_AVG10).map(|v| v * 8.0),
            ]),
            io: Self::from_metrics(&[value(ids::HOST_PSI_IO_FULL_AVG10).map(|v| v * 8.0)]),
            net: Self::from_metrics(&[]),
            core: StateClass::Normal,
            load,
        };
        // Ядро всегда несёт минимальную живую массу: процесс наблюдения
        // работает, и центр фигуры не должен быть пустым.
        glyph.load.core = glyph
            .load
            .cpu
            .max(glyph.load.memory)
            .max(glyph.load.io)
            .max(glyph.load.net)
            .max(0.12);

        for problem in &snapshot.problems {
            let class = StateClass::from_severity(problem.severity);
            let slot = match Sector::for_rule(problem.id.rule.0) {
                Sector::Cpu => &mut glyph.cpu,
                Sector::Memory => &mut glyph.memory,
                Sector::Io => &mut glyph.io,
                Sector::Net => &mut glyph.net,
                Sector::Core => &mut glyph.core,
            };
            *slot = (*slot).max(class);
        }

        // Ядро не может выглядеть спокойнее, чем худший из секторов: иначе центр
        // фигуры противоречит её краю.
        glyph.core = glyph
            .core
            .max(glyph.cpu.min(StateClass::Saturated))
            .max(glyph.worst_sector().min(StateClass::Saturated));
        glyph
    }

    /// Худший класс среди подсистем.
    #[must_use]
    pub fn worst_sector(&self) -> StateClass {
        self.cpu.max(self.memory).max(self.io).max(self.net)
    }

    /// Итоговый класс всей системы.
    #[must_use]
    pub fn overall(&self) -> StateClass {
        self.worst_sector().max(self.core)
    }

    /// Нагрузка сектора 0..1 для отрисовки массы.
    #[must_use]
    pub fn sector_load(&self, sector: Sector) -> f64 {
        self.load.get(sector)
    }

    /// Нагрузка из набора необязательных долей: худшая известная.
    fn load_of(values: &[Option<f64>]) -> f64 {
        values
            .iter()
            .filter_map(|value| *value)
            .filter(|value| value.is_finite())
            .fold(0.0_f64, f64::max)
            .clamp(0.0, 1.0)
    }

    /// Нагрузка сети: доля от условного гигабита на интерфейс.
    ///
    /// Абсолютного предела у сети в снимке нет, поэтому масштаб задан явной
    /// константой. Это оценка площади, а не утверждение о пропускной
    /// способности: severity сети живёт в классе, а не здесь.
    fn net_load(snapshot: &Snapshot) -> f64 {
        const SCALE: f64 = 125_000_000.0;
        let mut worst = 0.0_f64;
        for entity in &snapshot.entities {
            if entity.kind != EntityKind::NetIf {
                continue;
            }
            let rx = snapshot
                .value(entity.id, ids::NETIF_RX_THROUGHPUT)
                .unwrap_or(0.0);
            let tx = snapshot
                .value(entity.id, ids::NETIF_TX_THROUGHPUT)
                .unwrap_or(0.0);
            let total = rx + tx;
            if total.is_finite() {
                worst = worst.max(total / SCALE);
            }
        }
        worst.clamp(0.0, 1.0)
    }

    /// Состояние конкретного сектора.
    #[must_use]
    pub fn sector(&self, sector: Sector) -> StateClass {
        match sector {
            Sector::Cpu => self.cpu,
            Sector::Memory => self.memory,
            Sector::Io => self.io,
            Sector::Net => self.net,
            Sector::Core => self.core,
        }
    }

    /// Вердикт одной строкой: то, что оператор читает после формы.
    #[must_use]
    pub fn verdict(&self) -> &'static str {
        match self.overall() {
            StateClass::Failed => "FUNCTION LOST",
            StateClass::Critical => "CRITICAL",
            StateClass::Warning => "DEGRADED",
            StateClass::Degraded => "ATTENTION",
            StateClass::Saturated => "BUSY",
            _ => "SYSTEM NOMINAL",
        }
    }

    /// Класс из набора необязательных долей: худшая известная.
    fn from_metrics(values: &[Option<f64>]) -> StateClass {
        values
            .iter()
            .filter_map(|value| value.map(StateClass::from_ratio))
            .max()
            .unwrap_or(StateClass::Inactive)
    }

    /// Строки фигуры в полном размере: восемь строк по 20 символов.
    #[must_use]
    pub fn rows(&self, capability: Capability) -> Vec<String> {
        SHAPE
            .iter()
            .enumerate()
            .map(|(row, cells)| {
                let mut line = vec![' '; SHAPE_WIDTH];
                for x in cells.iter().copied() {
                    let class = self.cell_class(row, x);
                    if let Some(slot) = line.get_mut(usize::from(x) * 2) {
                        *slot = class.symbol(capability);
                    }
                }
                line.into_iter().collect()
            })
            .collect()
    }

    /// Компактная фигура для узкого терминала: три строки.
    ///
    /// Форма другая, но правило то же: положение отвечает за подсистему.
    #[must_use]
    pub fn compact_rows(&self, capability: Capability) -> Vec<String> {
        let sym = |class: StateClass| class.symbol(capability);
        vec![
            format!("  {}  ", sym(self.cpu)),
            format!("{} {} {}", sym(self.net), sym(self.core), sym(self.memory)),
            format!("  {}  ", sym(self.io)),
        ]
    }

    /// Класс ячейки фигуры по её положению.
    ///
    /// Сектор определяется углом от центра, а не идентичностью символа. Ячейки
    /// ядра показывают общее состояние, но не ярче худшего сектора.
    fn cell_class(&self, row: usize, x: u8) -> StateClass {
        let dx = f64::from(x) - 5.0;
        // Строки идут вдвое реже, чем полуклетки по горизонтали.
        let dy = (row as f64) * 2.0 - 7.0;
        let sector = if dx.abs() <= 1.0 && dy.abs() <= 1.5 {
            Sector::Core
        } else if dx > dy.abs() {
            Sector::Memory
        } else if -dx > dy.abs() {
            Sector::Net
        } else if dy < 0.0 {
            Sector::Cpu
        } else {
            Sector::Io
        };
        let class = self.sector(sector);
        // Край фигуры остаётся контуром: пока сектор в норме, он показывает
        // спокойное состояние, а не пустоту.
        if class == StateClass::Inactive {
            StateClass::Normal
        } else {
            class
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::entity::{EntityKey, EntitySpec};
    use pulse_core::problem::{Evidence, Problem, ProblemId, RuleId};
    use pulse_core::snapshot::{AgentStats, LatestValues};
    use pulse_core::time::Timestamp;
    use pulse_core::{EntityGraph, SeriesKey};

    /// Снимок хоста с заданными долями и списком проблем.
    fn snapshot(values: &[(pulse_core::MetricId, f64)], problems: Vec<Problem>) -> Snapshot {
        let mut graph = EntityGraph::new("boot", "host-1", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let _ = graph.upsert(
            EntitySpec::new(
                EntityKey::Unit {
                    name: "web.service".into(),
                },
                "web.service",
            )
            .parent(host),
        );
        let mut latest = LatestValues::new();
        for (metric, value) in values {
            latest.set(SeriesKey::new(host, *metric), *value);
        }
        let _ = graph.end_tick();
        Snapshot::build(
            &graph,
            latest,
            problems,
            Vec::new(),
            AgentStats::default(),
            "host-1",
            "boot",
        )
    }

    fn problem(rule: &'static str, severity: Severity, entity: pulse_core::EntityId) -> Problem {
        Problem {
            id: ProblemId {
                rule: RuleId(rule),
                entity,
            },
            severity,
            entity_name: "web.service".to_string(),
            title: "проблема".to_string(),
            summary: "описание".to_string(),
            evidence: vec![Evidence::new("значение", "1")],
            since: Timestamp::from_millis(1_000),
            last_seen: Timestamp::from_millis(2_000),
            streak: 3,
        }
    }

    #[test]
    fn healthy_system_has_only_round_symbols() {
        let snap = snapshot(
            &[
                (ids::HOST_CPU_UTIL, 0.03),
                (ids::HOST_MEM_UTIL, 0.08),
                (ids::HOST_PSI_IO_FULL_AVG10, 0.0),
            ],
            Vec::new(),
        );
        let glyph = StateGlyph::from_snapshot(&snap);
        assert_eq!(glyph.verdict(), "SYSTEM NOMINAL");

        let drawn: String = glyph.rows(Capability::TrueColor).join("");
        for abnormal in ['◇', '◆', '▲', '×'] {
            assert!(
                !drawn.contains(abnormal),
                "здоровая система не должна показывать {abnormal}"
            );
        }
        assert!(drawn.contains('○'), "контур обязан быть виден: {drawn}");
    }

    #[test]
    fn memory_problem_marks_right_side_and_leaves_left_healthy() {
        let entity = pulse_core::EntityId::new(1, 1);
        let snap = snapshot(
            &[(ids::HOST_CPU_UTIL, 0.05), (ids::HOST_MEM_UTIL, 0.9)],
            vec![problem("memory.pressure", Severity::Crit, entity)],
        );
        let glyph = StateGlyph::from_snapshot(&snap);
        assert_eq!(glyph.sector(Sector::Memory), StateClass::Critical);
        assert_eq!(glyph.verdict(), "CRITICAL");

        let rows = glyph.rows(Capability::TrueColor);
        // Строка через центр: слева контур в норме, справа критическое состояние.
        let middle = rows.get(3).cloned().unwrap_or_default();
        let chars: Vec<char> = middle.chars().collect();
        let left = chars.get(2).copied().unwrap_or(' ');
        let right = chars.get(18).copied().unwrap_or(' ');
        assert_eq!(left, '○', "сеть в норме — левый край круглый: {middle}");
        assert_eq!(
            right, '▲',
            "память критична — правый край треугольник: {middle}"
        );
    }

    #[test]
    fn cpu_and_io_problems_land_in_different_sectors() {
        let entity = pulse_core::EntityId::new(1, 1);
        let cpu = StateGlyph::from_snapshot(&snapshot(
            &[],
            vec![problem("psi.cpu", Severity::Warn, entity)],
        ));
        assert_eq!(cpu.sector(Sector::Cpu), StateClass::Warning);
        assert_eq!(cpu.sector(Sector::Io), StateClass::Inactive);

        let io = StateGlyph::from_snapshot(&snapshot(
            &[],
            vec![problem("disk.latency", Severity::Warn, entity)],
        ));
        assert_eq!(io.sector(Sector::Io), StateClass::Warning);
        assert_eq!(io.sector(Sector::Cpu), StateClass::Inactive);
    }

    #[test]
    fn unknown_rule_is_never_lost_silently() {
        assert_eq!(Sector::for_rule("brand.new.rule"), Sector::Core);
        let entity = pulse_core::EntityId::new(1, 1);
        let glyph = StateGlyph::from_snapshot(&snapshot(
            &[],
            vec![problem("brand.new.rule", Severity::Crit, entity)],
        ));
        assert_eq!(glyph.overall(), StateClass::Critical);
    }

    #[test]
    fn severity_is_readable_without_colour() {
        // Каждый класс обязан иметь собственную форму: монохромный терминал
        // различает состояние по символу, а не по цвету.
        let classes = [
            StateClass::Inactive,
            StateClass::Normal,
            StateClass::Active,
            StateClass::Saturated,
            StateClass::Degraded,
            StateClass::Warning,
            StateClass::Critical,
            StateClass::Failed,
        ];
        let unicode: std::collections::HashSet<char> = classes
            .iter()
            .map(|class| class.symbol(Capability::TrueColor))
            .collect();
        assert_eq!(unicode.len(), classes.len());

        let ascii: std::collections::HashSet<char> = classes
            .iter()
            .map(|class| class.symbol(Capability::Ascii))
            .collect();
        assert_eq!(ascii.len(), classes.len());
        assert!(ascii.iter().all(char::is_ascii));
    }

    #[test]
    fn ratio_thresholds_match_rule_thresholds() {
        assert_eq!(StateClass::from_ratio(0.10), StateClass::Normal);
        assert_eq!(StateClass::from_ratio(0.30), StateClass::Active);
        assert_eq!(StateClass::from_ratio(0.70), StateClass::Saturated);
        assert_eq!(StateClass::from_ratio(0.86), StateClass::Degraded);
        assert_eq!(StateClass::from_ratio(0.96), StateClass::Warning);
        assert_eq!(StateClass::from_ratio(f64::NAN), StateClass::Inactive);
    }

    #[test]
    fn shape_is_stable_and_fits_the_budget() {
        let snap = snapshot(&[(ids::HOST_CPU_UTIL, 0.02)], Vec::new());
        let glyph = StateGlyph::from_snapshot(&snap);
        let rows = glyph.rows(Capability::TrueColor);
        assert_eq!(rows.len(), 8, "бюджет главного экрана: 7–12 строк на глиф");
        for row in &rows {
            assert_eq!(row.chars().count(), SHAPE_WIDTH);
        }
        assert_eq!(glyph.compact_rows(Capability::TrueColor).len(), 3);
    }

    #[test]
    fn compact_rows_keep_sector_positions() {
        let entity = pulse_core::EntityId::new(1, 1);
        let glyph = StateGlyph::from_snapshot(&snapshot(
            &[],
            vec![problem("memory.oom", Severity::Crit, entity)],
        ));
        let rows = glyph.compact_rows(Capability::TrueColor);
        let middle = rows.get(1).cloned().unwrap_or_default();
        assert!(
            middle.ends_with('▲'),
            "в компактной форме память тоже справа: {middle}"
        );
    }
}
