//! Агрегат — единица хранения на всех уровнях истории.
//!
//! Сырой образец хранится как *вырожденный* агрегат (`count = 1`). Благодаря
//! этому downsampling — одна и та же ассоциативная функция [`Aggregate::merge`]
//! и на границе тёплого слоя, и при будущем сжатии дисковых сегментов,
//! а не два независимых кодовых пути, которые расходятся со временем.
//!
//! `first`/`last` выбираются по времени наблюдения, а не по порядку вызовов
//! `merge`. Иначе свёртка не была бы коммутативной, и результат зависел бы от
//! порядка обхода — что немедленно ломает и тесты, и восстановление истории
//! из нескольких источников.

/// Агрегат значений серии за интервал.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aggregate {
    pub count: u32,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    pub first: f64,
    pub last: f64,
    /// Время первого наблюдения, миллисекунды.
    pub first_at: u64,
    /// Время последнего наблюдения, миллисекунды.
    pub last_at: u64,
    /// Сколько раз внутри интервала счётчик уменьшился (перезапуск источника).
    pub resets: u32,
}

impl Aggregate {
    /// Нейтральный элемент свёртки.
    #[must_use]
    pub const fn empty() -> Self {
        Aggregate {
            count: 0,
            sum: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            first: 0.0,
            last: 0.0,
            first_at: u64::MAX,
            last_at: 0,
            resets: 0,
        }
    }

    /// Вырожденный агрегат из одного наблюдения.
    #[must_use]
    pub const fn point(value: f64, at_millis: u64) -> Self {
        Aggregate {
            count: 1,
            sum: value,
            min: value,
            max: value,
            first: value,
            last: value,
            first_at: at_millis,
            last_at: at_millis,
            resets: 0,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Ассоциативная и коммутативная свёртка двух агрегатов.
    #[must_use]
    pub fn merge(&self, other: &Aggregate) -> Aggregate {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }

        let (first, first_at) = if self.first_at <= other.first_at {
            (self.first, self.first_at)
        } else {
            (other.first, other.first_at)
        };
        let (last, last_at) = if self.last_at >= other.last_at {
            (self.last, self.last_at)
        } else {
            (other.last, other.last_at)
        };

        // Стык интервалов даёт ещё один возможный сброс счётчика: значение в
        // начале более позднего интервала меньше значения в конце более раннего.
        let boundary_reset = {
            let (earlier, later) = if self.last_at <= other.first_at {
                (self, other)
            } else {
                (other, self)
            };
            u32::from(later.first < earlier.last)
        };

        Aggregate {
            count: self.count.saturating_add(other.count),
            sum: self.sum + other.sum,
            min: self.min.min(other.min),
            max: self.max.max(other.max),
            first,
            last,
            first_at,
            last_at,
            resets: self
                .resets
                .saturating_add(other.resets)
                .saturating_add(boundary_reset),
        }
    }

    /// Среднее значение; `None` для пустого агрегата.
    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        if self.count == 0 {
            None
        } else {
            Some(self.sum / f64::from(self.count))
        }
    }

    /// Приращение счётчика за интервал. Отрицательное значение невозможно:
    /// при сбросе возвращается только рост после сброса.
    #[must_use]
    pub fn delta(&self) -> f64 {
        if self.resets > 0 {
            self.last.max(0.0)
        } else {
            (self.last - self.first).max(0.0)
        }
    }
}

impl Default for Aggregate {
    fn default() -> Self {
        Aggregate::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(values: &[(f64, u64)]) -> Aggregate {
        values.iter().fold(Aggregate::empty(), |acc, (v, t)| {
            acc.merge(&Aggregate::point(*v, *t))
        })
    }

    #[test]
    fn merge_is_associative() {
        let a = Aggregate::point(1.0, 10);
        let b = Aggregate::point(5.0, 20);
        let c = Aggregate::point(3.0, 30);
        let left = a.merge(&b).merge(&c);
        let right = a.merge(&b.merge(&c));
        assert_eq!(left, right);
    }

    #[test]
    fn merge_is_commutative() {
        let a = Aggregate::point(2.0, 100);
        let b = Aggregate::point(7.0, 200);
        assert_eq!(a.merge(&b), b.merge(&a));
    }

    #[test]
    fn empty_is_neutral_element() {
        let a = Aggregate::point(4.0, 1);
        assert_eq!(a.merge(&Aggregate::empty()), a);
        assert_eq!(Aggregate::empty().merge(&a), a);
    }

    #[test]
    fn fold_matches_direct_formulas() {
        let values = [(3.0, 1), (1.0, 2), (4.0, 3), (1.0, 4), (5.0, 5)];
        let folded = agg(&values);
        assert_eq!(folded.count, 5);
        assert!((folded.sum - 14.0).abs() < 1e-12);
        assert!((folded.min - 1.0).abs() < 1e-12);
        assert!((folded.max - 5.0).abs() < 1e-12);
        assert!((folded.first - 3.0).abs() < 1e-12);
        assert!((folded.last - 5.0).abs() < 1e-12);
        assert!((folded.mean().unwrap_or_default() - 2.8).abs() < 1e-12);
    }

    #[test]
    fn counter_reset_is_detected_and_delta_stays_positive() {
        // Счётчик: 100, 110, затем перезапуск источника -> 5, 9.
        let folded = agg(&[(100.0, 1), (110.0, 2), (5.0, 3), (9.0, 4)]);
        assert!(folded.resets >= 1, "сброс счётчика должен быть замечен");
        assert!(folded.delta() >= 0.0);
    }

    #[test]
    fn delta_without_reset_is_difference() {
        let folded = agg(&[(10.0, 1), (25.0, 2)]);
        assert_eq!(folded.resets, 0);
        assert!((folded.delta() - 15.0).abs() < 1e-12);
    }
}
