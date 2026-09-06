//! Лимиты: rate limiter по IP, бюджет ответов.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;

/// Максимальный мгновенный всплеск запросов с одного адреса.
///
/// Пополнение задаётся `per_minute`, но ёмкость ведра ограничена отдельно:
/// иначе при `per_minute = 600` один клиент мог бы выпустить 600 запросов
/// подряд и заставить агент 600 раз собрать полный ответ. Диагностическому
/// эндпоинту всплеск больше десятка запросов не нужен ни при какой настройке.
pub const MAX_BURST: u32 = 10;

/// Простой token bucket на IP: не более `per_minute` запросов за окно в минуту.
pub struct RateLimiter {
    per_minute: u32,
    max_entries: usize,
    entries: HashMap<IpAddr, Entry>,
}

struct Entry {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Создаёт лимитер: `per_minute` запросов в минуту, максимум `max_entries`
    /// уникальных IP (при переполнении вытесняется самое старое пополнение).
    pub fn new(per_minute: u32, max_entries: usize) -> Self {
        Self {
            per_minute,
            max_entries,
            entries: HashMap::new(),
        }
    }

    /// Попытка снять токен. Если лимит исчерпан — `false`.
    pub fn try_acquire(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let refill_rate = self.per_minute as f64 / 60.0;
        // Ёмкость ведра: минута запросов, но не больше разрешённого всплеска.
        let capacity = f64::from(self.per_minute.min(MAX_BURST));

        // Вытеснение выполняется ДО взятия изменяемой ссылки на запись:
        // иначе один и тот же `self` был бы занят двумя мутирующими заимствованиями.
        let is_new = !self.entries.contains_key(&ip);
        if is_new && self.entries.len() >= self.max_entries {
            self.evict_oldest();
        }

        let entry = self.entries.entry(ip).or_insert(Entry {
            tokens: capacity,
            last_refill: now,
        });

        let elapsed = now.duration_since(entry.last_refill).as_secs_f64();
        entry.tokens = (entry.tokens + refill_rate * elapsed).min(capacity);
        entry.last_refill = now;

        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest_ip) = self
            .entries
            .iter()
            .min_by_key(|(_, e)| e.last_refill)
            .map(|(ip, _)| *ip)
        {
            self.entries.remove(&oldest_ip);
        }
    }

    /// Текущее число отслеживаемых IP.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Пустой ли лимитер.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Бюджет на один ответ: ограничение объёма и числа строк.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_bytes: usize,
    pub max_lines: usize,
}

impl Budget {
    /// Создаёт бюджет, округляя объём вниз до кратности 4096.
    pub fn new(max_bytes: usize, max_lines: usize) -> Self {
        let rounded = (max_bytes / 4096) * 4096;
        Self {
            max_bytes: rounded.max(4096),
            max_lines,
        }
    }

    /// Бюджет «без ограничений» для тестов.
    pub fn unlimited() -> Self {
        Self {
            max_bytes: usize::MAX,
            max_lines: usize::MAX,
        }
    }

    /// Сколько байт ещё можно добавить в тело при текущей длине.
    pub fn remaining_bytes(&self, current_len: usize) -> usize {
        self.max_bytes.saturating_sub(current_len)
    }

    /// Сколько строк ещё можно добавить.
    pub fn remaining_lines(&self, current_lines: usize) -> usize {
        self.max_lines.saturating_sub(current_lines)
    }

    /// Проверяет, влезает ли новая строка (с учётом символа `\n`) в бюджет.
    pub fn can_push_line(&self, body_len: usize, line_len: usize, line_count: usize) -> bool {
        if line_count >= self.max_lines {
            return false;
        }
        // +1 на перевод строки, если строка не первая.
        let needed = line_len + 1;
        body_len.saturating_add(needed) <= self.max_bytes
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_lines: 100_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::time::Duration;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn rate_limiter_allows_burst() {
        let mut rl = RateLimiter::new(5, 10);
        for _ in 0..5 {
            assert!(rl.try_acquire(ip("1.1.1.1")));
        }
        assert!(!rl.try_acquire(ip("1.1.1.1")));
        assert!(rl.try_acquire(ip("2.2.2.2")));
    }

    #[test]
    fn rate_limiter_is_per_ip() {
        let mut rl = RateLimiter::new(1, 10);
        assert!(rl.try_acquire(ip("1.1.1.1")));
        assert!(!rl.try_acquire(ip("1.1.1.1")));
        assert!(rl.try_acquire(ip("2.2.2.2")));
    }

    #[test]
    fn rate_limiter_refills_after_time() {
        let mut rl = RateLimiter::new(600, 10);
        // Начальный пул = 10 токенов (600/60).
        for _ in 0..10 {
            assert!(rl.try_acquire(ip("3.3.3.3")));
        }
        assert!(!rl.try_acquire(ip("3.3.3.3")));
        std::thread::sleep(Duration::from_millis(200));
        assert!(rl.try_acquire(ip("3.3.3.3")));
    }

    #[test]
    fn rate_limiter_evicts_when_full() {
        let mut rl = RateLimiter::new(1000, 2);
        assert!(rl.try_acquire(ip("1.1.1.1")));
        assert!(rl.try_acquire(ip("2.2.2.2")));
        // Новый IP заставляет вытеснить старейший.
        assert!(rl.try_acquire(ip("3.3.3.3")));
        assert_eq!(rl.len(), 2);
    }

    #[test]
    fn budget_rounds_bytes_down() {
        let b = Budget::new(8192, 100);
        assert_eq!(b.max_bytes, 8192);
        assert_eq!(b.max_lines, 100);
    }

    #[test]
    fn budget_ensures_minimum() {
        let b = Budget::new(10, 100);
        assert_eq!(b.max_bytes, 4096);
    }

    #[test]
    fn budget_remaining_bytes() {
        let b = Budget::new(8192, 100);
        assert_eq!(b.remaining_bytes(2048), 6144);
        assert_eq!(b.remaining_bytes(8192), 0);
        assert_eq!(b.remaining_bytes(9000), 0);
    }

    #[test]
    fn budget_remaining_lines() {
        let b = Budget::new(8192, 100);
        assert_eq!(b.remaining_lines(50), 50);
        assert_eq!(b.remaining_lines(100), 0);
    }

    #[test]
    fn budget_can_push_line() {
        let b = Budget::new(8192, 2);
        assert!(b.can_push_line(0, 10, 0));
        assert!(b.can_push_line(11, 10, 1));
        assert!(!b.can_push_line(11, 10, 2));
        assert!(!b.can_push_line(8182, 10, 0)); // 8182 + 11 = 8193 > 8192
    }
}
