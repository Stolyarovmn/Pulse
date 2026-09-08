//! Property-тесты уже действующих инвариантов `pulse-core`.
//!
//! Эти проверки обязаны быть зелёными: они закрепляют контракты, которые
//! реализация уже выполняет, и защищают их от регрессии. Дефекты аудита
//! живут отдельно, в `tests/audit_red.rs`.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use proptest::prelude::*;
use pulse_core::redact::sanitize_display;
use pulse_core::time::Timestamp;

proptest! {
    /// PT-02: санитайзер границы доверия обязан быть безопасным на любом входе.
    ///
    /// Строки приходят из `/proc`, то есть управляются локальным процессом.
    /// Терминал исполняет управляющие последовательности, поэтому ESC, BEL
    /// и переопределение направления письма не имеют права выжить.
    #[test]
    fn sanitize_display_never_panics_and_strips_controls(input in ".{0,512}") {
        let output = sanitize_display(&input);
        prop_assert!(!output.contains('\u{1b}'), "ESC остался: {output:?}");
        prop_assert!(!output.contains('\u{7}'), "BEL остался: {output:?}");
        prop_assert!(
            !output.chars().any(|c| ('\u{202a}'..='\u{202e}').contains(&c)),
            "bidi override остался: {output:?}"
        );
        prop_assert!(
            !output.chars().any(|c| c.is_control() && c != '\t'),
            "управляющий символ остался: {output:?}"
        );
    }

    /// PT-03: арифметика метки времени не имеет права заворачиваться.
    ///
    /// История сравнивает моменты; переполнение сделало бы порядок событий
    /// бессмысленным.
    #[test]
    fn timestamp_arithmetic_saturates(a in any::<u64>(), b in any::<u64>()) {
        let earlier = Timestamp::from_millis(a.min(b));
        let later = Timestamp::from_millis(a.max(b));

        let forward = later.saturating_sub(earlier);
        let backward = earlier.saturating_sub(later);
        prop_assert_eq!(backward.as_millis(), 0, "обратная разница обязана быть нулевой");
        prop_assert!(
            u64::try_from(forward.as_millis()).unwrap_or(u64::MAX) <= later.as_millis(),
            "разница не имеет права превышать саму метку"
        );

        let added = later.saturating_add_millis(u64::MAX);
        prop_assert_eq!(added.as_millis(), u64::MAX, "сложение обязано насыщаться");
        let subtracted = earlier.saturating_sub_millis(u64::MAX);
        prop_assert_eq!(subtracted.as_millis(), 0, "вычитание обязано насыщаться");
    }
}
