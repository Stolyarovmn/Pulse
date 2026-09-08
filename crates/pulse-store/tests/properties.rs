//! Property-тесты действующих инвариантов `pulse-store`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use proptest::prelude::*;
use pulse_core::entity::EntityId;
use pulse_core::metric::ids;
use pulse_core::sample::SeriesKey;
use pulse_core::time::Timestamp;
use pulse_store::Hot;

proptest! {
    /// PT-04: нечисловые значения не имеют права попадать в историю.
    ///
    /// `NaN` и бесконечности отравляют любую последующую статистику: среднее,
    /// отклонение и сравнение с порогом перестают иметь смысл. Поэтому серия
    /// от такого значения не должна даже создаваться.
    #[test]
    fn non_finite_values_never_enter_history(
        index in 0_u32..64,
        which in 0_u8..3,
    ) {
        let value = match which {
            0 => f64::NAN,
            1 => f64::INFINITY,
            _ => f64::NEG_INFINITY,
        };
        let mut hot = Hot::new(16, 128);
        let key = SeriesKey::new(EntityId::new(index, 1), ids::HOST_CPU_UTIL);
        let seq = hot.begin_tick(Timestamp::from_millis(1_000));
        hot.push(key, seq, value);

        prop_assert_eq!(hot.series_count(), 0, "серия не имеет права создаваться");
        prop_assert_eq!(hot.samples_stored(), 0, "значение не имеет права храниться");
        prop_assert!(hot.last(key).is_none(), "значение не имеет права читаться");
    }

    /// Конечные значения, наоборот, обязаны сохраняться и читаться обратно.
    #[test]
    fn finite_values_round_trip(value in -1e9_f64..1e9_f64) {
        let mut hot = Hot::new(16, 128);
        let key = SeriesKey::new(EntityId::new(1, 1), ids::HOST_CPU_UTIL);
        let seq = hot.begin_tick(Timestamp::from_millis(1_000));
        hot.push(key, seq, value);

        prop_assert_eq!(hot.series_count(), 1);
        prop_assert_eq!(hot.last(key), Some(value));
    }
}
