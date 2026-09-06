//! State Glyph спецификации v0.7 (разделы 98, 99, 119-122).
//!
//! Две независимые оси, которые нельзя смешивать:
//!
//! * **символ** отвечает, насколько всё серьёзно (`· ○ ● ◉ ◇ ◆ ▲ ×`);
//! * **положение** отвечает, где именно проблема (раздел 122): верх - compute,
//!   право - memory/capacity, низ - IO/storage, лево - network, центр - система.
//!
//! Поле фиксировано: Large всегда занимает 9 колонок на 7 строк, и каждая
//! позиция рендерится - фоновая как `·`, а не как пробел (раздел 119). Это даёт
//! стабильную геометрию, на которой локальная деформация видна сразу.
//!
//! Силуэты взяты из спецификации таблицами, а не выведены одной формулой:
//! у Large верхняя строка целиком контурная (`· · · ○ ○ ○ · · ·`), а у Medium
//! центр верхней строки - масса (`· ○ ● ○ ·`). Это разные фигуры, и попытка
//! свести их к общему правилу расходится с нормативными mockups.

use crate::layout::GlyphPreset;
use crate::state::{Sector, StateClass, StateGlyph};
use crate::theme::Capability;

/// Роль ячейки в силуэте.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    /// Фон поля: рендерится как `·`.
    Background,
    /// Контур фигуры.
    Edge,
    /// Внутренняя масса фигуры.
    Mass,
}

use Cell::{Background as B, Edge as E, Mass as M};

/// Ширина большого поля в ячейках (раздел 119).
pub const LARGE_COLS: usize = 9;
/// Высота большого поля в ячейках.
pub const LARGE_ROWS: usize = 7;

/// Reference healthy silhouette для Large (раздел 119).
const LARGE: [[Cell; LARGE_COLS]; LARGE_ROWS] = [
    [B, B, B, E, E, E, B, B, B],
    [B, B, E, M, M, M, E, B, B],
    [B, E, M, M, M, M, M, E, B],
    [E, M, M, M, M, M, M, M, E],
    [B, E, M, M, M, M, M, E, B],
    [B, B, E, M, M, M, E, B, B],
    [B, B, B, E, E, E, B, B, B],
];

/// Силуэт Medium (раздел 98).
const MEDIUM: [[Cell; 5]; 3] = [[B, E, M, E, B], [E, M, M, M, E], [B, E, M, E, B]];

/// Отрисованная фигура: сетка классов состояния.
///
/// Хранится как логическая сетка, а не как готовая строка (раздел 7):
/// одна и та же сетка рендерится в любом режиме терминала.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphSurface {
    rows: Vec<Vec<StateClass>>,
}

impl GlyphSurface {
    /// Строит поверхность под выбранный пресет.
    #[must_use]
    pub fn build(glyph: &StateGlyph, preset: GlyphPreset) -> Self {
        let rows = match preset {
            GlyphPreset::Hidden => Vec::new(),
            GlyphPreset::Minimal => vec![vec![glyph.overall()]],
            GlyphPreset::Compact => vec![Self::compact_row(glyph)],
            // Эрозия края - деталь большого поля: на 5x3 сектор занимает
            // одну ячейку, и её потеря стёрла бы severity целиком (раздел 98).
            GlyphPreset::Medium => Self::from_shape(glyph, &MEDIUM, 2, 1, false),
            GlyphPreset::Large => Self::from_shape(glyph, &LARGE, 4, 3, true),
        };
        Self { rows }
    }

    /// Число строк поверхности.
    #[must_use]
    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// Классы построчно: нужны для раскраски по ячейкам.
    #[must_use]
    pub fn cells(&self) -> &[Vec<StateClass>] {
        &self.rows
    }

    /// Текстовые строки: ячейки через пробел, как в mockups спецификации.
    ///
    /// `Compact` идёт без разрядки - это одна плотная полоса из раздела 98.
    #[must_use]
    pub fn lines(&self, capability: Capability, preset: GlyphPreset) -> Vec<String> {
        let dense = matches!(preset, GlyphPreset::Compact | GlyphPreset::Minimal);
        self.rows
            .iter()
            .map(|row| {
                let mut line = String::with_capacity(row.len() * 2);
                for (index, class) in row.iter().enumerate() {
                    if index > 0 && !dense {
                        line.push(' ');
                    }
                    line.push(class.symbol(capability));
                }
                line
            })
            .collect()
    }

    /// Плотная полоса из пяти ячеек (раздел 98).
    ///
    /// В одну строку вертикальная ось складывается на внутреннюю пару:
    /// слева-снаружи сеть, слева-внутри compute, центр - система,
    /// справа-внутри storage, справа-снаружи память.
    fn compact_row(glyph: &StateGlyph) -> Vec<StateClass> {
        vec![
            Self::visible(glyph.sector(Sector::Net)),
            Self::visible(glyph.sector(Sector::Cpu)),
            Self::visible(glyph.sector(Sector::Core)),
            Self::visible(glyph.sector(Sector::Io)),
            Self::visible(glyph.sector(Sector::Memory)),
        ]
    }

    /// Заполняет силуэт классами секторов.
    fn from_shape<const W: usize>(
        glyph: &StateGlyph,
        shape: &[[Cell; W]],
        cx: i32,
        cy: i32,
        erode: bool,
    ) -> Vec<Vec<StateClass>> {
        // Деформация: сектор в состоянии Warning и хуже съедает свою крайнюю
        // ячейку - силуэт теряет массу именно с той стороны, где проблема.
        // Это воспроизводит нормативные примеры разделов 121.2-121.4.
        let eroded = if erode {
            Self::eroded_edge(glyph, shape, cx, cy)
        } else {
            None
        };

        // Глубина ячейки: 0 в центре, 1 у контура. Масса заливается от центра
        // наружу пропорционально нагрузке, поэтому силуэт «дышит», а поле
        // остаётся фиксированным 9x7 - фон рисуется `·`, а не пробелом.
        let max_dx = f64::from(u16::try_from(W).unwrap_or(1))
            .mul_add(0.5, -0.5)
            .max(1.0);
        let max_dy = f64::from(u16::try_from(shape.len()).unwrap_or(1))
            .mul_add(0.5, -0.5)
            .max(1.0);

        shape
            .iter()
            .enumerate()
            .map(|(row, cells)| {
                cells
                    .iter()
                    .enumerate()
                    .map(|(col, cell)| {
                        if eroded == Some((row, col)) {
                            return StateClass::Inactive;
                        }
                        match cell {
                            Cell::Background => StateClass::Inactive,
                            Cell::Edge | Cell::Mass => {
                                let dx = col as i32 - cx;
                                let dy = row as i32 - cy;
                                let sector = Self::sector_at(dx, dy);
                                let nx = f64::from(dx) / max_dx;
                                let ny = f64::from(dy) / max_dy;
                                let depth = nx.hypot(ny).min(1.0);
                                Self::cell_class(
                                    glyph.sector(sector),
                                    sector,
                                    *cell,
                                    glyph.sector_load(sector),
                                    depth,
                                )
                            }
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// Класс ячейки по состоянию её сектора, роли в силуэте и нагрузке.
    ///
    /// Правило из разделов 120 и 121:
    ///
    /// * отклонение (`◇ ◆ ▲ ×`) показывается и на контуре, и в массе - именно
    ///   так возникает видимая деформация нужной стороны;
    /// * контур в штатном режиме это `○`: он всегда виден, потому что задаёт
    ///   систему отсчёта («normal edge/reference»);
    /// * `◉` появляется только в ядре: в примере «Busy but controlled»
    ///   насыщение видно в центре, а масса секторов остаётся `●`.
    fn cell_class(
        sector_class: StateClass,
        sector: Sector,
        cell: Cell,
        load: f64,
        depth: f64,
    ) -> StateClass {
        if sector_class.is_abnormal() {
            return sector_class;
        }
        match cell {
            Cell::Background => StateClass::Inactive,
            Cell::Edge => StateClass::Normal,
            Cell::Mass => {
                // Площадь массы пропорциональна нагрузке, поэтому радиус берётся
                // как корень: иначе между контуром и ядром появляется кольцевой
                // разрыв. `FULL` - нагрузка, при которой диск залит целиком;
                // она ниже порога насыщения (0.60), чтобы нормативный силуэт
                // §121 «Healthy» был достижим именно классом `●`.
                const FULL: f64 = 0.45;
                let radius = (load / FULL).sqrt().min(1.0);
                if depth > radius {
                    return StateClass::Inactive;
                }
                if sector == Sector::Core && sector_class == StateClass::Saturated {
                    StateClass::Saturated
                } else {
                    StateClass::Active
                }
            }
        }
    }

    /// Какая крайняя ячейка съедена деформацией.
    ///
    /// Берётся худший сектор; при равенстве порядок обхода фиксирован, чтобы
    /// картинка не мигала между кадрами при одинаковых данных.
    fn eroded_edge<const W: usize>(
        glyph: &StateGlyph,
        shape: &[[Cell; W]],
        cx: i32,
        cy: i32,
    ) -> Option<(usize, usize)> {
        let sectors = [Sector::Memory, Sector::Cpu, Sector::Io, Sector::Net];
        let worst = sectors
            .into_iter()
            .filter(|sector| glyph.sector(*sector) >= StateClass::Warning)
            .max_by_key(|sector| glyph.sector(*sector))?;

        // Самая удалённая от центра ячейка контура в направлении сектора.
        shape
            .iter()
            .enumerate()
            .flat_map(|(row, cells)| {
                cells
                    .iter()
                    .enumerate()
                    .filter_map(move |(col, cell)| (*cell == Cell::Edge).then_some((row, col)))
            })
            .filter(|(row, col)| Self::sector_at(*col as i32 - cx, *row as i32 - cy) == worst)
            .max_by_key(|(row, col)| {
                let dx = (*col as i32 - cx).abs();
                let dy = (*row as i32 - cy).abs();
                // Горизонталь и вертикаль в терминале имеют разный шаг:
                // строка выше колонки примерно вдвое.
                dx + dy * 2
            })
    }

    /// Сектор по смещению от центра (раздел 122).
    fn sector_at(dx: i32, dy: i32) -> Sector {
        // Символ клетки вдвое уже, чем высок, поэтому вертикаль весит больше.
        let fx = f64::from(dx);
        let fy = f64::from(dy) * 2.0;
        if fx.abs() <= 1.0 && fy.abs() <= 1.0 {
            return Sector::Core;
        }
        if fx > fy.abs() {
            Sector::Memory
        } else if -fx > fy.abs() {
            Sector::Net
        } else if fy < 0.0 {
            Sector::Cpu
        } else {
            Sector::Io
        }
    }

    /// Сектор без сигнала показывается штатным, а не пустым: отсутствие данных
    /// о сети не должно выглядеть как дыра в фигуре.
    fn visible(class: StateClass) -> StateClass {
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
    use pulse_core::graph::EntityGraph;
    use pulse_core::metric::ids;
    use pulse_core::problem::{Evidence, Problem, ProblemId, RuleId, Severity};
    use pulse_core::sample::SeriesKey;
    use pulse_core::snapshot::{AgentStats, LatestValues, Snapshot};
    use pulse_core::time::Timestamp;

    /// Нагруженный, но здоровый хост: без проблем, с реальной активной массой.
    ///
    /// Именно этому состоянию соответствует нормативный силуэт §121 «Healthy»:
    /// `●` определён в §120 как «normal active mass», поэтому полный диск
    /// означает работающую систему, а не простой.
    fn loaded_healthy_snapshot() -> Snapshot {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let mut latest = LatestValues::new();
        latest.set(SeriesKey::new(host, ids::HOST_CPU_UTIL), 0.55);
        latest.set(SeriesKey::new(host, ids::HOST_MEM_UTIL), 0.55);
        latest.set(SeriesKey::new(host, ids::HOST_PSI_IO_FULL_AVG10), 0.07);
        // Сеть - такой же сектор фигуры: без трафика её масса пуста, и полный
        // диск §121 недостижим.
        let netif = graph.upsert(
            EntitySpec::new(
                EntityKey::NetIf {
                    name: "eth0".into(),
                },
                "eth0",
            )
            .parent(host),
        );
        latest.set(
            SeriesKey::new(netif, ids::NETIF_RX_THROUGHPUT),
            70_000_000.0,
        );
        latest.set(
            SeriesKey::new(netif, ids::NETIF_TX_THROUGHPUT),
            70_000_000.0,
        );
        let _ = graph.end_tick();
        Snapshot::build(
            &graph,
            latest,
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        )
    }

    /// Здоровый простаивающий хост: без проблем и с низкой утилизацией.
    fn healthy_snapshot() -> Snapshot {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let mut latest = LatestValues::new();
        latest.set(SeriesKey::new(host, ids::HOST_CPU_UTIL), 0.01);
        latest.set(SeriesKey::new(host, ids::HOST_MEM_UTIL), 0.05);
        latest.set(SeriesKey::new(host, ids::HOST_PSI_IO_FULL_AVG10), 0.0);
        let _ = graph.end_tick();
        Snapshot::build(
            &graph,
            latest,
            vec![],
            vec![],
            AgentStats::default(),
            "host",
            "boot",
        )
    }

    /// Тот же хост с одной проблемой заданного правила.
    fn snapshot_with(rule: &'static str, severity: Severity) -> Snapshot {
        problem_on(healthy_snapshot(), rule, severity)
    }

    /// Нагруженный хост с одной проблемой: масса есть, деформация видна.
    fn loaded_snapshot_with(rule: &'static str, severity: Severity) -> Snapshot {
        problem_on(loaded_healthy_snapshot(), rule, severity)
    }

    fn problem_on(mut snap: Snapshot, rule: &'static str, severity: Severity) -> Snapshot {
        let host = snap.host;
        snap.problems.push(Problem {
            id: ProblemId {
                rule: RuleId(rule),
                entity: host,
            },
            severity,
            entity_name: "host".to_string(),
            title: "проблема".to_string(),
            summary: "измеренное отклонение".to_string(),
            evidence: vec![Evidence::new("метрика", "38%")],
            since: Timestamp::from_millis(1_000),
            last_seen: Timestamp::from_millis(2_000),
            streak: 5,
        });
        snap
    }

    /// Раздел 119: reference healthy silhouette воспроизводится буквально.
    #[test]
    fn large_healthy_matches_reference_silhouette() {
        let glyph = StateGlyph::from_snapshot(&loaded_healthy_snapshot());
        let surface = GlyphSurface::build(&glyph, GlyphPreset::Large);
        let lines = surface.lines(Capability::TrueColor, GlyphPreset::Large);
        assert_eq!(
            lines,
            vec![
                "· · · ○ ○ ○ · · ·".to_string(),
                "· · ○ ● ● ● ○ · ·".to_string(),
                "· ○ ● ● ● ● ● ○ ·".to_string(),
                "○ ● ● ● ● ● ● ● ○".to_string(),
                "· ○ ● ● ● ● ● ○ ·".to_string(),
                "· · ○ ● ● ● ○ · ·".to_string(),
                "· · · ○ ○ ○ · · ·".to_string(),
            ],
            "силуэт обязан совпадать с нормативным полем 9x7"
        );
    }

    /// Раздел 98: силуэт Medium.
    #[test]
    fn medium_healthy_matches_reference_silhouette() {
        let glyph = StateGlyph::from_snapshot(&loaded_healthy_snapshot());
        let lines = GlyphSurface::build(&glyph, GlyphPreset::Medium)
            .lines(Capability::TrueColor, GlyphPreset::Medium);
        assert_eq!(
            lines,
            vec![
                "· ○ ● ○ ·".to_string(),
                "○ ● ● ● ○".to_string(),
                "· ○ ● ○ ·".to_string(),
            ]
        );
    }

    /// Раздел 119: фон обязан рендериться, а не заменяться пробелами.
    #[test]
    fn idle_host_shows_contour_without_mass() {
        // §120: `·` это «no active state contribution», `●` - «normal active
        // mass». На простаивающем хосте залитый диск утверждал бы активность,
        // которой нет, поэтому масса сжимается до ядра, а контур остаётся.
        let glyph = StateGlyph::from_snapshot(&healthy_snapshot());
        let surface = GlyphSurface::build(&glyph, GlyphPreset::Large);
        let cells: Vec<StateClass> = surface.cells().iter().flatten().copied().collect();
        let mass = cells
            .iter()
            .filter(|class| **class == StateClass::Active)
            .count();
        let contour = cells
            .iter()
            .filter(|class| **class == StateClass::Normal)
            .count();
        let loaded = StateGlyph::from_snapshot(&loaded_healthy_snapshot());
        let loaded_mass = GlyphSurface::build(&loaded, GlyphPreset::Large)
            .cells()
            .iter()
            .flatten()
            .filter(|class| **class == StateClass::Active)
            .count();

        assert!(contour > 0, "контур обязан остаться видимым: {cells:?}");
        assert!(
            mass < loaded_mass,
            "масса простоя обязана быть меньше массы под нагрузкой: {mass} против {loaded_mass}"
        );
        assert!(
            mass * 3 < loaded_mass,
            "на простое залито не должно быть больше трети: {mass} против {loaded_mass}"
        );
        assert!(
            cells.iter().all(|class| !class.is_abnormal()),
            "простой не является отклонением: {cells:?}"
        );
    }

    #[test]
    fn mass_grows_monotonically_with_load() {
        let mass_at = |util: f64| {
            let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
            graph.begin_tick(Timestamp::from_millis(2_000));
            let host = graph.host();
            let mut latest = LatestValues::new();
            latest.set(SeriesKey::new(host, ids::HOST_CPU_UTIL), util);
            let _ = graph.end_tick();
            let snap = Snapshot::build(
                &graph,
                latest,
                vec![],
                vec![],
                AgentStats::default(),
                "host",
                "boot",
            );
            let glyph = StateGlyph::from_snapshot(&snap);
            GlyphSurface::build(&glyph, GlyphPreset::Large)
                .cells()
                .iter()
                .flatten()
                .filter(|class| **class != StateClass::Inactive && **class != StateClass::Normal)
                .count()
        };

        let series: Vec<usize> = [0.0, 0.1, 0.2, 0.3, 0.45]
            .iter()
            .map(|u| mass_at(*u))
            .collect();
        assert!(
            series.windows(2).all(|w| w[1] >= w[0]),
            "масса обязана расти монотонно с нагрузкой: {series:?}"
        );
        assert!(
            series.last() > series.first(),
            "рост обязан быть заметным: {series:?}"
        );
    }

    #[test]
    fn background_is_rendered_not_blank() {
        let glyph = StateGlyph::from_snapshot(&healthy_snapshot());
        for preset in [GlyphPreset::Large, GlyphPreset::Medium] {
            for line in GlyphSurface::build(&glyph, preset).lines(Capability::TrueColor, preset) {
                assert!(
                    !line.contains("  "),
                    "в поле не должно быть пустых позиций: {line:?}"
                );
                assert!(
                    !line.starts_with(' '),
                    "поле начинается с пустоты: {line:?}"
                );
            }
        }
    }

    /// Раздел 119: поле ровно 9 на 7.
    #[test]
    fn large_field_is_exactly_nine_by_seven() {
        let glyph = StateGlyph::from_snapshot(&healthy_snapshot());
        let surface = GlyphSurface::build(&glyph, GlyphPreset::Large);
        assert_eq!(surface.height(), LARGE_ROWS);
        for row in surface.cells() {
            assert_eq!(row.len(), LARGE_COLS);
        }
    }

    /// Раздел 122: проблема памяти деформирует правую часть, левая остаётся
    /// штатной. Это и есть смысл «положение отвечает, где».
    #[test]
    fn memory_problem_deforms_right_side_only() {
        let glyph = StateGlyph::from_snapshot(&snapshot_with("memory.pressure", Severity::Crit));
        let surface = GlyphSurface::build(&glyph, GlyphPreset::Large);
        let middle = &surface.cells()[3];
        let left = middle[1];
        let right = middle[7];
        assert!(
            right.is_abnormal(),
            "правая часть обязана показать проблему: {right:?}"
        );
        assert!(
            !left.is_abnormal(),
            "левая часть не должна тревожить без причины: {left:?}"
        );
    }

    /// Раздел 122: compute деформирует верх, а не право.
    #[test]
    fn cpu_problem_deforms_top_side() {
        let glyph = StateGlyph::from_snapshot(&snapshot_with("psi.cpu", Severity::Crit));
        let surface = GlyphSurface::build(&glyph, GlyphPreset::Large);
        let top = surface.cells()[1][4];
        let right = surface.cells()[3][7];
        assert!(top.is_abnormal(), "верх обязан реагировать на compute");
        assert!(!right.is_abnormal(), "память не при чём");
    }

    /// Раздел 121: `×` появляется только там, где функция потеряна,
    /// и никогда - в здоровой фигуре.
    #[test]
    fn failed_symbol_never_appears_in_healthy_glyph() {
        let glyph = StateGlyph::from_snapshot(&healthy_snapshot());
        let lines = GlyphSurface::build(&glyph, GlyphPreset::Large)
            .lines(Capability::TrueColor, GlyphPreset::Large)
            .join("");
        for forbidden in ['×', '▲', '◆', '◇'] {
            assert!(
                !lines.contains(forbidden),
                "здоровая фигура не должна содержать {forbidden}"
            );
        }
    }

    /// Раздел 2.1: смысл сохраняется в монохроме и в ASCII.
    #[test]
    fn ascii_mode_keeps_field_and_severity() {
        let glyph = StateGlyph::from_snapshot(&snapshot_with("memory.pressure", Severity::Crit));
        let lines = GlyphSurface::build(&glyph, GlyphPreset::Large)
            .lines(Capability::Ascii, GlyphPreset::Large);
        let joined = lines.join("");
        assert!(joined.is_ascii(), "в ASCII-режиме не должно быть Unicode");
        assert!(
            joined.contains('^'),
            "критическое состояние должно остаться"
        );
        assert!(joined.contains('.'), "фон должен остаться видимым");
    }

    /// Раздел 98: при уменьшении пресета severity не теряется.
    #[test]
    fn severity_survives_every_preset() {
        let glyph = StateGlyph::from_snapshot(&snapshot_with("memory.pressure", Severity::Crit));
        for preset in [
            GlyphPreset::Large,
            GlyphPreset::Medium,
            GlyphPreset::Compact,
            GlyphPreset::Minimal,
        ] {
            let joined = GlyphSurface::build(&glyph, preset)
                .lines(Capability::TrueColor, preset)
                .join("");
            assert!(
                joined.contains('▲'),
                "пресет {preset:?} потерял критическое состояние: {joined:?}"
            );
        }
    }

    #[test]
    fn hidden_preset_draws_nothing() {
        let glyph = StateGlyph::from_snapshot(&healthy_snapshot());
        let surface = GlyphSurface::build(&glyph, GlyphPreset::Hidden);
        assert_eq!(surface.height(), 0);
        assert!(surface
            .lines(Capability::TrueColor, GlyphPreset::Hidden)
            .is_empty());
    }

    /// Деформация детерминирована: одинаковые данные дают одинаковую фигуру.
    #[test]
    fn surface_is_deterministic() {
        let snap = snapshot_with("memory.pressure", Severity::Crit);
        let glyph = StateGlyph::from_snapshot(&snap);
        let first = GlyphSurface::build(&glyph, GlyphPreset::Large);
        let second = GlyphSurface::build(&glyph, GlyphPreset::Large);
        assert_eq!(first, second);
    }

    /// Раздел 5: фигура остаётся фигурой - деформация не разрушает силуэт
    /// целиком, иначе оператор потеряет узнаваемую форму.
    #[test]
    fn erosion_removes_at_most_one_edge_cell() {
        // Сравнение имеет смысл только при равной массе: эрозия отвечает за
        // потерю крайней ячейки, а не за площадь заливки.
        let healthy = StateGlyph::from_snapshot(&loaded_healthy_snapshot());
        let baseline = GlyphSurface::build(&healthy, GlyphPreset::Large);
        let alive = |surface: &GlyphSurface| {
            surface
                .cells()
                .iter()
                .flatten()
                .filter(|class| **class != StateClass::Inactive)
                .count()
        };
        let critical =
            StateGlyph::from_snapshot(&loaded_snapshot_with("memory.pressure", Severity::Crit));
        let deformed = GlyphSurface::build(&critical, GlyphPreset::Large);
        assert_eq!(
            alive(&baseline) - alive(&deformed),
            1,
            "деформация обязана съедать ровно одну крайнюю ячейку"
        );
    }
}
