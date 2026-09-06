//! Время в Pulse: настенные метки для истории, номер такта для сбора.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Настенная метка времени в миллисекундах от UNIX-эпохи.
///
/// Хранится как `u64`: сравнение и арифметика без потерь точности, в отличие от `f64`.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(u64);

impl Timestamp {
    pub const ZERO: Timestamp = Timestamp(0);
    pub const MAX: Timestamp = Timestamp(u64::MAX);

    /// Текущее настенное время. При часах до эпохи возвращает `ZERO`.
    #[must_use]
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Timestamp(millis)
    }

    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Timestamp(millis)
    }

    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1000.0
    }

    /// Разница `self - earlier`, никогда не отрицательная.
    #[must_use]
    pub const fn saturating_sub(self, earlier: Timestamp) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }

    #[must_use]
    pub const fn saturating_add_millis(self, millis: u64) -> Self {
        Timestamp(self.0.saturating_add(millis))
    }

    #[must_use]
    pub const fn saturating_sub_millis(self, millis: u64) -> Self {
        Timestamp(self.0.saturating_sub(millis))
    }

    /// Содержится ли метка в отрезке `[from, to]` (границы включительно).
    #[must_use]
    pub const fn within(self, from: Timestamp, to: Timestamp) -> bool {
        self.0 >= from.0 && self.0 <= to.0
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self.0)
    }
}

impl fmt::Display for Timestamp {
    /// `HH:MM:SS` в UTC — формат для журналов и TUI-таймлайна.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.0 / 1000;
        let secs = total % 60;
        let mins = (total / 60) % 60;
        let hours = (total / 3600) % 24;
        write!(f, "{hours:02}:{mins:02}:{secs:02}")
    }
}

/// Номер такта сбора. Монотонно растёт от старта агента.
#[derive(
    Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize,
)]
pub struct TickId(pub u64);

impl TickId {
    pub const FIRST: TickId = TickId(0);

    #[must_use]
    pub const fn next(self) -> Self {
        TickId(self.0.wrapping_add(1))
    }
}

impl fmt::Display for TickId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Форматирование интервала для UI: `42s`, `7m12s`, `3h04m`, `12d3h`.
#[must_use]
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else if secs < 86_400 {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d{}h", secs / 86_400, (secs % 86_400) / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_sub_never_underflows() {
        let a = Timestamp::from_millis(1_000);
        let b = Timestamp::from_millis(5_000);
        assert_eq!(a.saturating_sub(b), Duration::ZERO);
        assert_eq!(b.saturating_sub(a), Duration::from_secs(4));
    }

    #[test]
    fn within_is_inclusive() {
        let t = Timestamp::from_millis(100);
        assert!(t.within(Timestamp::from_millis(100), Timestamp::from_millis(100)));
        assert!(!t.within(Timestamp::from_millis(101), Timestamp::from_millis(200)));
    }

    #[test]
    fn duration_formatting_covers_all_ranges() {
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
        assert_eq!(format_duration(Duration::from_secs(432)), "7m12s");
        assert_eq!(format_duration(Duration::from_secs(11_040)), "3h04m");
        assert_eq!(format_duration(Duration::from_secs(1_036_800)), "12d0h");
    }
}
