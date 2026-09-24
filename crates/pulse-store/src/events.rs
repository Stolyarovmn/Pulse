//! Журнал событий.
//!
//! События не агрегируются никогда: краткий OOM или перезапуск контейнера —
//! это ровно та информация, ради которой смотрят историю. Ограничивается
//! только число записей.

use std::collections::VecDeque;

use pulse_core::event::Event;
use pulse_core::time::Timestamp;

/// Кольцевой журнал событий.
#[derive(Debug)]
pub struct Events {
    events: VecDeque<Event>,
    max_events: usize,
    total: u64,
    approx_bytes: u64,
    evicted: u64,
}

impl Events {
    #[must_use]
    pub fn new(max_events: usize) -> Self {
        Events {
            events: VecDeque::with_capacity(max_events.min(1024)),
            max_events: max_events.max(16),
            total: 0,
            approx_bytes: 0,
            evicted: 0,
        }
    }

    pub fn push(&mut self, event: Event) {
        self.approx_bytes = self.approx_bytes.saturating_add(event_bytes(&event));
        self.events.push_back(event);
        self.total = self.total.saturating_add(1);
        while self.events.len() > self.max_events {
            let _ = self.pop_oldest();
        }
    }

    /// Освобождает старейшие события до достижения требуемого бюджета.
    ///
    /// Возвращает `(число, байты)`. Нужен memory ceiling: численный лимит
    /// записей не является лимитом памяти, особенно при длинных именах и
    /// деталях событий.
    pub fn evict_bytes(&mut self, requested: u64) -> (u64, u64) {
        let mut count = 0_u64;
        let mut bytes = 0_u64;
        while bytes < requested {
            let Some(freed) = self.pop_oldest() else {
                break;
            };
            count = count.saturating_add(1);
            bytes = bytes.saturating_add(freed);
        }
        (count, bytes)
    }

    fn pop_oldest(&mut self) -> Option<u64> {
        let event = self.events.pop_front()?;
        let bytes = event_bytes(&event);
        self.approx_bytes = self.approx_bytes.saturating_sub(bytes);
        self.evicted = self.evicted.saturating_add(1);
        Some(bytes)
    }

    /// События в интервале `[from, to]`, границы включительно.
    #[must_use]
    pub fn between(&self, from: Timestamp, to: Timestamp) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|event| event.at.within(from, to))
            .collect()
    }

    /// Последние `limit` событий, свежие последними.
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<&Event> {
        let skip = self.events.len().saturating_sub(limit);
        self.events.iter().skip(skip).collect()
    }

    /// Последние `raw` событий плюс последние `kept` из тех, что проходят
    /// `keep`, свежие последними, без повторов.
    ///
    /// Одного окна по счёту мало: на stage-1 шум ядра и контейнеров
    /// заполнял 200 последних событий за 10–20 секунд, и начало наблюдения
    /// с открытой проблемой уходили из Story, хотя журнал их хранил.
    /// Значимое обязано переживать шум, поэтому оно добирается отдельным
    /// счётом из той же очереди. Обход идёт от свежих и останавливается,
    /// как только оба счёта набраны.
    #[must_use]
    pub fn recent_keeping(
        &self,
        raw: usize,
        kept: usize,
        keep: impl Fn(&Event) -> bool,
    ) -> Vec<&Event> {
        let mut out: Vec<&Event> = Vec::new();
        let mut kept_count = 0;
        for (index, event) in self.events.iter().rev().enumerate() {
            let within_raw = index < raw;
            if !within_raw && kept_count >= kept {
                break;
            }
            let keeps = kept_count < kept && keep(event);
            if keeps {
                kept_count += 1;
            }
            if within_raw || keeps {
                out.push(event);
            }
        }
        out.reverse();
        out
    }

    #[must_use]
    pub fn total(&self) -> u64 {
        self.total
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub const fn approx_bytes(&self) -> u64 {
        self.approx_bytes
    }

    #[must_use]
    pub const fn evicted(&self) -> u64 {
        self.evicted
    }
}

/// Стабильная оценка удерживаемой памяти одного события.
///
/// `String::capacity` учитывает реально выделенный буфер, а не только длину.
/// Накладные расходы `VecDeque` и allocator точно измерить нельзя, поэтому
/// добавляется размер структуры и консервативный запас на элемент.
fn event_bytes(event: &Event) -> u64 {
    let fixed = std::mem::size_of::<Event>().saturating_add(32);
    let dynamic = event
        .entity_name
        .capacity()
        .saturating_add(event.detail.capacity());
    u64::try_from(fixed.saturating_add(dynamic)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_core::event::EventKind;

    fn event(at: u64) -> Event {
        Event::new(Timestamp::from_millis(at), EventKind::Created, "x")
    }

    #[test]
    fn between_includes_boundaries() {
        let mut events = Events::new(100);
        for at in [1_000, 2_000, 3_000, 4_000] {
            events.push(event(at));
        }
        let found = events.between(Timestamp::from_millis(2_000), Timestamp::from_millis(3_000));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn ring_is_bounded_but_total_keeps_counting() {
        let mut events = Events::new(16);
        for at in 0..100u64 {
            events.push(event(at * 100));
        }
        assert_eq!(events.len(), 16);
        assert_eq!(events.total(), 100);
    }

    #[test]
    fn recent_returns_freshest_last() {
        let mut events = Events::new(100);
        for at in 1..=5u64 {
            events.push(event(at * 1_000));
        }
        let recent = events.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(
            recent.last().map(|e| e.at),
            Some(Timestamp::from_millis(5_000))
        );
    }

    #[test]
    fn recent_handles_limit_larger_than_journal() {
        let mut events = Events::new(100);
        events.push(event(1_000));
        assert_eq!(events.recent(50).len(), 1);
    }

    /// Живой кадр stage-1: шум за 20 с вытеснял из окна в 200 событий начало
    /// наблюдения и открытую проблему, и Story пустел. Значимое обязано
    /// попадать в выборку, сколько бы шума ни пришло после него, без
    /// повторов и в порядке времени.
    #[test]
    fn kept_events_survive_a_flood_of_noise() {
        let mut events = Events::new(1_000);
        events.push(Event::new(
            Timestamp::from_millis(1),
            EventKind::ProblemOpened,
            "host",
        ));
        for at in 2..=500u64 {
            events.push(event(at));
        }
        let keep = |event: &Event| event.kind == EventKind::ProblemOpened;
        let found = events.recent_keeping(200, 200, keep);
        assert_eq!(found.len(), 201, "200 свежих плюс одно значимое");
        assert_eq!(
            found.first().map(|event| event.kind),
            Some(EventKind::ProblemOpened)
        );
        assert!(found.windows(2).all(|pair| match pair {
            [a, b] => a.at <= b.at,
            _ => true,
        }));
        // Значимое внутри свежего окна не удваивается.
        let fresh = events.recent_keeping(1_000, 200, keep);
        assert_eq!(fresh.len(), 500);
    }
}
