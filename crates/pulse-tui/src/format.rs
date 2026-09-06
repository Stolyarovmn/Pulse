//! Форматирование величин и микрографики.
//!
//! Единицы берутся из реестра метрик, а не угадываются на месте вызова: иначе
//! одна и та же величина рано или поздно будет показана в разных экранах в
//! разных единицах.

use pulse_core::metric::{describe, MetricId, Unit};
use pulse_core::time::Timestamp;

use crate::theme::Theme;

/// Форматирует значение метрики согласно её единице измерения.
#[must_use]
pub fn metric_value(metric: MetricId, value: f64) -> String {
    match describe(metric).map(|d| d.unit) {
        Some(Unit::Ratio) => percent(value),
        Some(Unit::Bytes) => bytes(value),
        Some(Unit::BytesPerSecond) => rate(value),
        Some(Unit::Seconds) => format!("{value:.1} s"),
        Some(Unit::Milliseconds) => millis(value),
        Some(Unit::Microseconds) => format!("{:.1} ms", value / 1000.0),
        Some(Unit::Cores) => cores(value),
        Some(Unit::Count) => count(value),
        _ => format!("{value:.2}"),
    }
}

/// Заполненность файловой системы: округление вверх, как в `df`.
///
/// Оператор сравнивает число с выводом `df` в той же консоли. `df` округляет
/// вверх, поэтому 1.46% там показаны как `2%`; обычное округление дало бы `1%`
/// и выглядело бы как расхождение измерений.
#[must_use]
pub fn fs_percent(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_string();
    }
    let scaled = (value * 100.0).clamp(0.0, 100.0);
    format!("{}%", scaled.ceil() as u32)
}

/// Доля 0..1 в проценты.
#[must_use]
pub fn percent(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_string();
    }
    format!("{:.0}%", (value * 100.0).clamp(0.0, 1000.0))
}

/// Байты в человекочитаемом виде.
#[must_use]
pub fn bytes(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "—".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut scaled = value;
    let mut unit = 0usize;
    while scaled >= 1024.0 && unit + 1 < UNITS.len() {
        scaled /= 1024.0;
        unit += 1;
    }
    let suffix = UNITS.get(unit).copied().unwrap_or("B");
    // Байты — целые; для крупных единиц один знак после запятой, но начиная с
    // трёх цифр дробная часть уже не несёт информации и только шумит.
    if unit == 0 || scaled >= 100.0 {
        format!("{scaled:.0} {suffix}")
    } else {
        format!("{scaled:.1} {suffix}")
    }
}

/// Скорость в байтах за секунду.
///
/// Суффикс `/s` ставится только здесь и только для величин, объявленных в
/// реестре как `BytesPerSecond` (раздел 154). Накопительный счётчик через эту
/// функцию не проходит: он форматируется как объём и подписывается «total».
#[must_use]
pub fn rate(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "—".to_string();
    }
    format!("{}/s", bytes(value))
}

/// Направленная пара скоростей: приём и передача (раздел 155).
///
/// Стрелки читаются без подписей, но в ASCII-режиме их нет, поэтому
/// используются буквенные метки `RX`/`TX`.
#[must_use]
pub fn rx_tx(rx: Option<f64>, tx: Option<f64>, ascii: bool) -> String {
    // `None` — измерения нет (первый такт, серия не публикуется). Печатать
    // `0 B/s` в этом случае значит утверждать отсутствие трафика.
    if ascii {
        format!("RX {} TX {}", optional_rate(rx), optional_rate(tx))
    } else {
        format!("↓{} ↑{}", optional_rate(rx), optional_rate(tx))
    }
}

/// Дисковый поток хоста той же формой, что сетевой.
#[must_use]
pub fn disk_rw(read: Option<f64>, write: Option<f64>, ascii: bool) -> String {
    if ascii {
        format!("R {} W {}", optional_rate(read), optional_rate(write))
    } else {
        format!("↓{} ↑{}", optional_rate(read), optional_rate(write))
    }
}

/// Скорость или честный прочерк, если измерения нет.
#[must_use]
pub fn optional_rate(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), rate)
}

/// Накопительный объём с явной подписью (раздел 154).
///
/// Существует, чтобы счётчик нельзя было случайно показать как скорость:
/// подпись «total» является частью значения, а не украшением.
#[must_use]
pub fn counter_total(read: f64, write: f64) -> String {
    format!("total R {} W {}", bytes(read), bytes(write))
}

/// Миллисекунды.
#[must_use]
pub fn millis(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_string();
    }
    if value >= 1_000.0 {
        format!("{:.1} s", value / 1000.0)
    } else if value >= 10.0 {
        format!("{value:.0} ms")
    } else {
        format!("{value:.1} ms")
    }
}

/// Ядра CPU.
#[must_use]
pub fn cores(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_string();
    }
    // Суффикс обязателен: `CPU 15` двусмысленно (15 процентов? 15 ядер?).
    // Выбрана одна форма на весь продукт — ядра с суффиксом `c` (§Resource
    // semantics). Проценты остаются у host-метрик, где знаменатель очевиден.
    if value >= 10.0 {
        format!("{value:.1}c")
    } else {
        format!("{value:.2}c")
    }
}

/// Целочисленные счётчики.
#[must_use]
pub fn count(value: f64) -> String {
    if !value.is_finite() {
        return "—".to_string();
    }
    if value >= 10_000.0 {
        format!("{:.1}k", value / 1000.0)
    } else {
        format!("{value:.0}")
    }
}

/// Усечение строки по числу символов с многоточием.
///
/// Считаются символы, а не байты: иначе строка с кириллицей обрезалась бы
/// посередине символа и ломала вывод.
#[must_use]
pub fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Микрографик из блочных символов.
#[must_use]
pub fn sparkline(points: &[(Timestamp, f64)], width: usize, theme: &Theme) -> String {
    if width == 0 {
        return String::new();
    }
    let values: Vec<f64> = points
        .iter()
        .map(|(_, v)| *v)
        .filter(|v| v.is_finite())
        .collect();
    if values.is_empty() {
        return " ".repeat(width);
    }

    // Берём последние `width` значений: график всегда «прижат» к настоящему.
    let start = values.len().saturating_sub(width);
    let tail = values.get(start..).unwrap_or(&values);
    let min = tail.iter().copied().fold(f64::INFINITY, f64::min);
    let max = tail.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = max - min;
    let blocks = theme.glyphs.blocks;
    let last_index = blocks.len().saturating_sub(1);

    let mut out = String::with_capacity(width);
    for value in tail {
        let level = if span <= f64::EPSILON {
            // Постоянный ряд рисуется ровной линией по середине шкалы,
            // а не «в пол»: иначе стабильная нагрузка выглядит как ноль.
            last_index / 2
        } else {
            let normalized = (value - min) / span;
            ((normalized * last_index as f64).round() as usize).min(last_index)
        };
        out.push(blocks.get(level).copied().unwrap_or('_'));
    }
    // Дополняем слева, чтобы график был выровнен по правому краю.
    let missing = width.saturating_sub(out.chars().count());
    if missing > 0 {
        let mut padded = " ".repeat(missing);
        padded.push_str(&out);
        return padded;
    }
    out
}

/// Дорожка метрики-доли по абсолютной шкале 0..1 с разрядкой.
///
/// Отличие от [`sparkline`] принципиальное. `sparkline` нормирует ряд по его
/// собственному min-max, поэтому дрожание 0.00-0.04 на простаивающем хосте
/// растягивается на всю высоту и три дорожки сливаются в одно светлое полотно.
/// §183 показывает простой как ряд `▁ ▁ ▁ ▁` у самого пола: для доли шкала
/// известна заранее, и врать амплитудой нельзя.
///
/// Разрядка между ячейками - требование §183 («state cells have spacing when
/// width permits»): без неё блоки склеиваются в сплошную заливку.
#[must_use]
pub fn ratio_lane(points: &[(Timestamp, f64)], width: usize, theme: &Theme) -> String {
    if width == 0 {
        return String::new();
    }
    let blocks = theme.glyphs.blocks;
    let last_index = blocks.len().saturating_sub(1);
    // Разрядка стоит две колонки на ячейку; при нехватке места идём плотно.
    let spaced = width >= 16;
    let cells = if spaced { width / 2 } else { width };

    let values: Vec<f64> = points
        .iter()
        .map(|(_, v)| *v)
        .filter(|v| v.is_finite())
        .collect();
    let start = values.len().saturating_sub(cells);
    let tail = values.get(start..).unwrap_or(&values);

    let mut out = String::with_capacity(width);
    for value in tail {
        if spaced && !out.is_empty() {
            out.push(' ');
        }
        let level = ((value.clamp(0.0, 1.0) * last_index as f64).round() as usize).min(last_index);
        out.push(blocks.get(level).copied().unwrap_or('_'));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Capability;
    use pulse_core::metric::ids;

    fn theme() -> Theme {
        Theme::with_capability(Capability::TrueColor)
    }

    fn series(values: &[f64]) -> Vec<(Timestamp, f64)> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (Timestamp::from_millis(i as u64 * 1000), *v))
            .collect()
    }

    #[test]
    fn bytes_scale_through_units() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(2048.0), "2.0 KiB");
        assert_eq!(bytes(5.0 * 1024.0 * 1024.0), "5.0 MiB");
        assert_eq!(bytes(300.0 * 1024.0 * 1024.0), "300 MiB");
        assert_eq!(bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GiB");
    }

    #[test]
    fn non_finite_values_render_as_dash() {
        assert_eq!(bytes(f64::NAN), "—");
        assert_eq!(percent(f64::INFINITY), "—");
        assert_eq!(millis(f64::NAN), "—");
        assert_eq!(cores(f64::NAN), "—");
    }

    #[test]
    fn percent_and_cores_and_millis_formats() {
        assert_eq!(percent(0.421), "42%");
        assert_eq!(cores(0.5), "0.50c");
        assert_eq!(cores(12.0), "12.0c");
        assert_eq!(millis(3.15), "3.1 ms");
        assert_eq!(millis(42.0), "42 ms");
        assert_eq!(millis(2500.0), "2.5 s");
    }

    #[test]
    fn metric_value_uses_registry_units() {
        assert_eq!(metric_value(ids::HOST_CPU_UTIL, 0.5), "50%");
        assert_eq!(metric_value(ids::HOST_MEM_TOTAL, 1024.0), "1.0 KiB");
        assert_eq!(metric_value(ids::DISK_AWAIT, 12.0), "12 ms");
        assert_eq!(metric_value(ids::CG_CPU_CORES, 1.5), "1.50c");
        assert_eq!(metric_value(ids::DISK_READ_THROUGHPUT, 1024.0), "1.0 KiB/s");
        assert_eq!(metric_value(ids::DISK_READ_BYTES, 1024.0), "1.0 KiB");
        assert_eq!(
            counter_total(9.0 * 1024.0, 2.0 * 1024.0),
            "total R 9.0 KiB W 2.0 KiB"
        );
        assert_eq!(
            rx_tx(Some(207.0 * 1024.0), Some(7.2 * 1024.0), false),
            "↓207 KiB/s ↑7.2 KiB/s"
        );
        assert_eq!(
            rx_tx(Some(207.0 * 1024.0), Some(7.2 * 1024.0), true),
            "RX 207 KiB/s TX 7.2 KiB/s"
        );
        // Отсутствие измерения обязано читаться как отсутствие, а не как ноль.
        assert_eq!(rx_tx(None, None, false), "↓— ↑—");
        assert_eq!(
            disk_rw(Some(4.0 * 1024.0 * 1024.0), None, false),
            "↓4.0 MiB/s ↑—"
        );
    }

    #[test]
    fn fs_percent_matches_df_rounding() {
        // 14 GiB занято из 957 доступных: `df` печатает 2%.
        assert_eq!(fs_percent(14.0 / 957.0), "2%");
        assert_eq!(fs_percent(0.0), "0%");
        assert_eq!(fs_percent(0.931), "94%");
        assert_eq!(fs_percent(1.0), "100%");
        assert_eq!(fs_percent(f64::NAN), "—");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("nginx", 10), "nginx");
        assert_eq!(truncate("payment-api", 5), "paym…");
        // Кириллица: 5 символов, 10 байт.
        assert_eq!(truncate("сервис", 4).chars().count(), 4);
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn ratio_lane_uses_absolute_scale_not_own_minmax() {
        let t = theme();
        // Дрожание простоя обязано остаться у пола, а не растянуться на всю
        // высоту: иначе три дорожки сливаются в светлое полотно (§183).
        let idle = ratio_lane(&series(&[0.00, 0.04, 0.01, 0.03, 0.02]), 40, &t);
        let cells: Vec<char> = idle.chars().filter(|c| *c != ' ').collect();
        assert!(!cells.is_empty(), "дорожка обязана быть нарисована");
        let floor = t.glyphs.blocks[0];
        assert!(
            cells.iter().all(|c| *c == floor),
            "простой обязан идти по полу шкалы: {idle:?}"
        );

        // Тот же ряд через относительную нормировку даёт разброс - это и есть
        // прежний дефект, поэтому сравнение зафиксировано тестом.
        let relative = sparkline(&series(&[0.00, 0.04, 0.01, 0.03, 0.02]), 5, &t);
        assert!(
            relative.chars().filter(|c| *c != ' ').any(|c| c != floor),
            "относительная шкала обязана давать разброс: {relative:?}"
        );
    }

    #[test]
    fn ratio_lane_is_spaced_when_width_permits() {
        let t = theme();
        let wide = ratio_lane(&series(&[0.1, 0.5, 0.9]), 40, &t);
        assert!(
            wide.contains(' '),
            "широкая дорожка обязана иметь разрядку: {wide:?}"
        );
        let narrow = ratio_lane(&series(&[0.1, 0.5, 0.9]), 6, &t);
        assert!(
            !narrow.contains(' '),
            "узкая дорожка идёт плотно: {narrow:?}"
        );
    }

    #[test]
    fn ratio_lane_reaches_top_only_at_full_load() {
        let t = theme();
        let top = *t.glyphs.blocks.last().expect("blocks");
        let full = ratio_lane(&series(&[1.0, 1.0]), 8, &t);
        assert!(
            full.chars().all(|c| c == top),
            "полная нагрузка обязана быть на потолке: {full:?}"
        );
        let half = ratio_lane(&series(&[0.5]), 8, &t);
        assert!(
            half.chars().all(|c| c != top),
            "половина нагрузки не обязана касаться потолка: {half:?}"
        );
    }

    #[test]
    fn sparkline_handles_degenerate_series() {
        let t = theme();
        assert_eq!(sparkline(&[], 8, &t).chars().count(), 8);
        assert_eq!(sparkline(&series(&[5.0]), 8, &t).chars().count(), 8);
        let flat = sparkline(&series(&[3.0, 3.0, 3.0]), 3, &t);
        assert_eq!(flat.chars().count(), 3);
        let unique: std::collections::HashSet<char> = flat.chars().collect();
        assert_eq!(unique.len(), 1, "постоянный ряд — ровная линия");
    }

    #[test]
    fn sparkline_ignores_nan_and_keeps_width() {
        let t = theme();
        let with_nan = series(&[1.0, f64::NAN, 5.0, 2.0]);
        let line = sparkline(&with_nan, 6, &t);
        assert_eq!(line.chars().count(), 6);
    }

    #[test]
    fn sparkline_shows_last_values_when_series_is_long() {
        let t = theme();
        let long: Vec<f64> = (0..100).map(f64::from).collect();
        let line = sparkline(&series(&long), 10, &t);
        assert_eq!(line.chars().count(), 10);
        // Последнее значение — максимум, значит последний символ самый высокий.
        let last = line.chars().last().unwrap_or(' ');
        assert_eq!(last, '█');
    }

    #[test]
    fn sparkline_respects_ascii_mode() {
        let ascii = Theme::with_capability(Capability::Ascii);
        let line = sparkline(&series(&[1.0, 2.0, 3.0]), 3, &ascii);
        assert!(line.is_ascii(), "в ASCII-режиме недопустим Unicode: {line}");
    }
}
