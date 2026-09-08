//! Колонка тренда в строке таблицы: форма за окно, пик и среднее.
//!
//! Строка вида `angie 0%` отвечает только на «сколько сейчас». Она не
//! отличает «всегда ноль» от «только что упало с семидесяти», а для
//! расследования важно именно второе. История для этого уже собрана
//! (горячее кольцо `store.hot_ticks`), не хватало показа в строке.
//!
//! Честность амплитуды. Форма нормируется по пику окна, и пик печатается
//! рядом числом: без подписи одинаково выглядели бы всплеск до 0.02 ядра и
//! до четырёх ядер, а это ложь амплитудой (§186). Среднее печатается вместе
//! с пиком, потому что «пик 4 ядра при среднем 3.9» и «пик 4 при среднем
//! 0.1» — разные истории.

use pulse_core::sample::SeriesKey;
use pulse_core::snapshot::Snapshot;
use pulse_core::Timestamp;

use crate::theme::Theme;

/// Ширина окна тренда: минута назад от текущего момента кадра.
///
/// Минута выбрана не произвольно: `store.warm_bucket_ticks` равен десяти
/// тактам, поэтому минута — шесть тёплых бакетов, то есть окно остаётся
/// доступным даже после вытеснения горячего кольца.
pub const WINDOW_MS: u64 = 60_000;

/// Готовая колонка тренда для одной строки.
#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    /// Форма за окно.
    pub lane: String,
    /// Пик и среднее, уже отформатированные вызывающим.
    pub peak: f64,
    pub mean: f64,
    /// Точек в окне: ноль означает «истории нет», а не «нагрузки нет».
    pub points: usize,
}

impl Trend {
    /// Есть ли что показывать.
    ///
    /// Одна точка формы не задаёт: §186 требует честного «collecting
    /// history» вместо ровной линии-заглушки.
    #[must_use]
    pub const fn is_measured(&self) -> bool {
        self.points >= 2
    }
}

/// Собирает тренд серии за окно, оканчивающееся моментом кадра.
///
/// `history` необязателен: одноразовые команды и часть тестов работают без
/// хранилища, и в этом случае колонка обязана честно отсутствовать, а не
/// показывать плоскую линию.
#[must_use]
pub fn of_series(
    history: Option<&pulse_store::History>,
    snapshot: &Snapshot,
    key: SeriesKey,
    width: usize,
    theme: &Theme,
) -> Option<Trend> {
    let history = history?;
    let to = snapshot.at;
    let from = to.saturating_sub_millis(WINDOW_MS);
    let points = history.series_points(key, from, to);
    if points.len() < 2 {
        return Some(Trend {
            lane: String::new(),
            peak: 0.0,
            mean: 0.0,
            points: points.len(),
        });
    }

    let values: Vec<f64> = points
        .iter()
        .map(|(_, value)| *value)
        .filter(|value| value.is_finite())
        .collect();
    let peak = values.iter().copied().fold(0.0_f64, f64::max);
    let mean = if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    };

    let lane = lane_of(&points, (from, to), width, theme);
    Some(Trend {
        lane,
        peak,
        mean,
        points: points.len(),
    })
}

/// Форма ряда за окно: ровная линия для ряда без разброса, иначе шкала
/// по пику окна.
///
/// Отдельная чистая функция, потому что именно её поведение и было
/// дефектом: нормировка по собственному пику ставила постоянную нагрузку
/// в потолок шкалы, и `0.03 ядра всегда` читалось как «загружено
/// полностью». Порог в пять процентов от пика отделяет дрожание измерения
/// от настоящего изменения.
#[must_use]
pub fn lane_of(
    points: &[(Timestamp, f64)],
    window: (Timestamp, Timestamp),
    width: usize,
    theme: &Theme,
) -> String {
    let finite = || {
        points
            .iter()
            .map(|(_, value)| *value)
            .filter(|v| v.is_finite())
    };
    let peak = finite().fold(0.0_f64, f64::max);
    let floor = finite().fold(f64::MAX, f64::min);
    let spread = if peak > 0.0 {
        (peak - floor.min(peak)) / peak
    } else {
        0.0
    };
    if spread < 0.05 {
        let flat = if matches!(theme.capability, crate::theme::Capability::Ascii) {
            '='
        } else {
            '─'
        };
        return std::iter::repeat_n(flat, width).collect();
    }
    crate::format::scaled_lane(points, width, window, peak, theme)
}

/// Окно тренда для момента кадра: нужно вызывающим для подписи.
#[must_use]
pub fn window(snapshot: &Snapshot) -> (Timestamp, Timestamp) {
    let to = snapshot.at;
    (to.saturating_sub_millis(WINDOW_MS), to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Capability;

    /// Без истории колонки нет вовсе: плоская линия соврала бы о том,
    /// что измерение было.
    #[test]
    fn without_history_there_is_no_column() {
        let snapshot = Snapshot::default();
        let theme = Theme::with_capability(Capability::TrueColor);
        let key = SeriesKey::new(snapshot.host, pulse_core::metric::ids::HOST_CPU_UTIL);
        assert!(of_series(None, &snapshot, key, 12, &theme).is_none());
    }

    /// Одна точка — ещё не форма: колонка обязана считаться неизмеренной.
    #[test]
    fn single_point_is_not_a_shape() {
        let trend = Trend {
            lane: String::new(),
            peak: 1.0,
            mean: 1.0,
            points: 1,
        };
        assert!(!trend.is_measured());
        assert!(Trend { points: 2, ..trend }.is_measured());
    }

    /// Ровный ряд обязан выглядеть ровным, а не полной шкалой.
    ///
    /// Дефект с живого кадра: `db.service` держал 0.03 ядра постоянно, а
    /// нормировка по собственному пику ставила его в потолок — строка
    /// читалась как «загружено полностью».
    #[test]
    fn flat_series_is_drawn_flat_not_full() {
        let theme = Theme::with_capability(Capability::TrueColor);
        let top = *theme.glyphs.blocks.last().expect("blocks");
        let window = (Timestamp::from_millis(0), Timestamp::from_millis(9_000));
        let flat: Vec<(Timestamp, f64)> = (0..10)
            .map(|step| (Timestamp::from_millis(step * 1_000), 0.03))
            .collect();

        let lane = lane_of(&flat, window, 12, &theme);
        assert!(
            !lane.contains(top),
            "постоянная нагрузка не имеет права выглядеть полной шкалой: {lane:?}"
        );
        assert!(
            lane.chars().all(|c| c == '─'),
            "ровный ряд рисуется ровной линией: {lane:?}"
        );

        // Настоящее изменение обязано остаться формой, а не превратиться
        // в ту же ровную линию.
        let rising: Vec<(Timestamp, f64)> = (0..10_u32)
            .map(|step| {
                (
                    Timestamp::from_millis(u64::from(step) * 1_000),
                    f64::from(step) * 0.1,
                )
            })
            .collect();
        let lane = lane_of(&rising, window, 12, &theme);
        assert!(
            lane.contains(top),
            "рост обязан доходить до пика окна: {lane:?}"
        );
    }

    /// Полная связка `History → of_series` обязана сохранить решение о
    /// ровном ряде. Чистый тест `lane_of` защищает алгоритм, а этот —
    /// интеграцию с горячим кольцом и реальным окном снимка.
    #[test]
    fn flat_history_stays_flat_through_of_series() {
        use pulse_core::config::Store as StoreConfig;
        use pulse_core::sample::Sample;
        use pulse_core::time::TickId;

        let mut snapshot = Snapshot::default();
        snapshot.at = Timestamp::from_millis(10_000);
        let key = SeriesKey::new(snapshot.host, pulse_core::metric::ids::HOST_CPU_UTIL);
        let mut history = pulse_store::History::new(&StoreConfig::default());
        for tick in 1..=10_u64 {
            history.ingest(&pulse_core::graph::TickBatch {
                tick: TickId(tick),
                at: Timestamp::from_millis(tick * 1_000),
                samples: vec![Sample {
                    series: key,
                    value: 0.03,
                }],
                events: Vec::new(),
                records: Vec::new(),
                alive: vec![snapshot.host],
            });
        }

        let theme = Theme::with_capability(Capability::TrueColor);
        let trend =
            of_series(Some(&history), &snapshot, key, 12, &theme).expect("история подключена");
        assert!(trend.is_measured());
        assert_eq!(trend.peak, 0.03);
        assert!((trend.mean - 0.03).abs() < f64::EPSILON);
        assert!(
            trend.lane.chars().all(|symbol| symbol == '─'),
            "ровный ряд из History обязан остаться ровным: {:?}",
            trend.lane
        );
    }
}
