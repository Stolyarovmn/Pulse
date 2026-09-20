//! Доступ отрисовки к истории: одна выборка — один короткий захват замка.
//!
//! Раньше кадр брал `history.read()` перед `terminal.draw` и держал замок до
//! конца отрисовки. Писатель — поток сбора — на это время вставал: медленный
//! терминал, широкий экран или залипшая перерисовка удлиняли такт сбора,
//! то есть потребитель данных блокировал их производителя (PULSE-079).
//!
//! Тип ниже разрывает эту связь структурно, а не соглашением: отрисовка не
//! получает ни `History`, ни guard, только право спросить точки одной серии.
//! Замок захватывается на время одной выборки и отпускается сразу.

use std::sync::RwLock;

use pulse_core::sample::SeriesKey;
use pulse_core::time::Timestamp;
use pulse_store::History;

/// Право отрисовки прочитать точки серии.
#[derive(Debug, Clone, Copy)]
pub struct SeriesReader<'a> {
    history: &'a RwLock<History>,
}

impl<'a> SeriesReader<'a> {
    #[must_use]
    pub fn new(history: &'a RwLock<History>) -> Self {
        SeriesReader { history }
    }

    /// Сырые точки серии в окне.
    ///
    /// Отравленный замок означает панику писателя, но данные внутри остались
    /// согласованными: терять из-за этого весь кадр незачем.
    #[must_use]
    pub fn points(&self, key: SeriesKey, from: Timestamp, to: Timestamp) -> Vec<(Timestamp, f64)> {
        match self.history.read() {
            Ok(history) => history.series_points(key, from, to),
            Err(poisoned) => poisoned.into_inner().series_points(key, from, to),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use pulse_core::config::Store as StoreConfig;

    /// PULSE-079: медленная отрисовка не имеет права задерживать сбор.
    ///
    /// Кадр держал read-замок всю отрисовку, поэтому `History::ingest`
    /// ожидал её окончания. Тест моделирует кадр: выборка точек, затем
    /// длинная «отрисовка». Писатель обязан пройти внутри неё.
    #[test]
    fn drawing_after_a_read_does_not_block_the_writer() {
        const DRAW_MS: u64 = 400;
        const WRITER_BUDGET_MS: u128 = 150;
        let history = Arc::new(RwLock::new(History::new(&StoreConfig::default())));
        let snapshot = pulse_core::snapshot::Snapshot::default();
        let key = SeriesKey::new(snapshot.host, pulse_core::metric::ids::HOST_CPU_UTIL);

        let for_writer = Arc::clone(&history);
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = thread::spawn(move || {
            // Ждём сигнала, что кадр уже начал «рисовать».
            rx.recv().expect("сигнал кадра");
            let started = Instant::now();
            let mut guard = for_writer.write().expect("замок писателя");
            guard.ingest(&pulse_core::graph::TickBatch::default());
            started.elapsed().as_millis()
        });

        let reader = SeriesReader::new(&history);
        let _points = reader.points(
            key,
            Timestamp::from_millis(0),
            Timestamp::from_millis(1_000),
        );
        tx.send(()).expect("кадр начался");
        thread::sleep(Duration::from_millis(DRAW_MS));

        let waited = writer.join().expect("поток писателя");
        assert!(
            waited <= WRITER_BUDGET_MS,
            "писатель обязан пройти во время отрисовки, ожидание: {waited} мс"
        );
    }

    /// Контрольный опыт: измерение обязано отличать «не блокирует» от
    /// «блокирует». Здесь воспроизведено прежнее поведение кадра — guard
    /// удерживается всю отрисовку, и писатель ждёт её конца. Без этого
    /// теста предыдущий мог бы проходить по случайности.
    #[test]
    fn holding_the_guard_across_a_draw_does_block_the_writer() {
        const DRAW_MS: u64 = 400;

        let history = Arc::new(RwLock::new(History::new(&StoreConfig::default())));
        let for_writer = Arc::clone(&history);
        let (tx, rx) = std::sync::mpsc::channel();
        let writer = thread::spawn(move || {
            rx.recv().expect("сигнал кадра");
            let started = Instant::now();
            let mut guard = for_writer.write().expect("замок писателя");
            guard.ingest(&pulse_core::graph::TickBatch::default());
            started.elapsed().as_millis()
        });

        let guard = history.read().expect("замок кадра");
        tx.send(()).expect("кадр начался");
        thread::sleep(Duration::from_millis(DRAW_MS));
        drop(guard);

        let waited = writer.join().expect("поток писателя");
        assert!(
            waited >= u128::from(DRAW_MS) / 2,
            "удержанный замок обязан задерживать писателя, ожидание: {waited} мс"
        );
    }
}
