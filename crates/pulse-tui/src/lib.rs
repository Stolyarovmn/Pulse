//! `pulse-tui` — терминальный интерфейс.
//!
//! Интерфейс — часть продукта, а не оболочка над коллектором: на первом экране
//! отвечает на вопросы «что плохо», «кто владелец», «что изменилось», и только
//! потом показывает утилизацию.
//!
//! Терминал восстанавливается через страж [`TerminalGuard`]: при панике или
//! ошибке в середине кадра альтернативный экран и raw-режим обязаны быть
//! отключены, иначе пользователь остаётся в сломанном терминале.

// В тестах unwrap/expect/panic допустимы: тест обязан упасть и указать строку.
// В рабочем коде запрет остаётся в силе (см. lints workspace).
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::field_reassign_with_default
    )
)]

pub mod app;
pub mod fold;
pub mod format;
pub mod glyph;
pub mod icons;
pub mod investigate;
pub mod layout;
pub mod pipe;
pub mod rows;
pub mod screens;
pub mod state;
pub mod table;
pub mod theme;
pub mod trend;
pub mod ui;

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use pulse_core::snapshot::SnapshotSource;
use pulse_core::time::Timestamp;
use pulse_core::Config;
use pulse_engine::{diff, render_text, DiffOptions};
use pulse_store::History;

pub use app::{Action, App, Screen, SortKey};
pub use rows::{entity_rows, EntityRow};
pub use theme::{Capability, Theme};

/// Страж терминала: восстанавливает режим в любом случае.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
        let _ = io::stdout().execute(crossterm::event::DisableMouseCapture);
    }
}

/// Восстанавливает терминал из любого потока.
///
/// Те же три действия, что и в [`TerminalGuard`], но доступные сторожу:
/// `Drop` не выполняется при принудительном завершении процесса.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);
    let _ = io::stdout().execute(crossterm::event::DisableMouseCapture);
}

/// Сколько ждать штатного выхода после сигнала, прежде чем завершиться силой.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1_500);

/// Гарантирует завершение по сигналу за ограниченное время.
///
/// Наблюдаемый на живом хосте дефект: если терминал исчез (убит эмулятор,
/// оборван ssh), опрос ввода перестаёт возвращать управление и крутится в
/// пользовательском пространстве на 100% ядра. Флаг завершения при этом
/// выставлен, но цикл до его проверки больше не доходит: `kill` не действует,
/// а агент продолжает держать порт экспортёра.
///
/// Сторож ждёт сигнал, даёт циклу время выйти самому и только потом
/// восстанавливает терминал и завершает процесс. Это делает «kill завершает
/// агента» инвариантом, не зависящим от поведения библиотеки ввода.
fn spawn_shutdown_watchdog(flag: &Arc<AtomicBool>) {
    let flag = Arc::clone(flag);
    let _ = std::thread::Builder::new()
        .name("pulse-shutdown".to_string())
        .spawn(move || {
            while !flag.load(AtomicOrdering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(SHUTDOWN_GRACE);
            // Цикл всё ещё жив: штатный путь недоступен.
            restore_terminal();
            std::process::exit(0);
        });
}

/// Регистрирует флаг завершения по внешним сигналам.
///
/// В raw-режиме `ISIG` отключён, поэтому `Ctrl+C` приходит как клавиша и
/// обрабатывается интерфейсом. Но `SIGTERM` от systemd, `kill` из другой
/// вкладки или остановка контейнера убили бы процесс до срабатывания
/// [`TerminalGuard`], и терминал остался бы в raw-режиме с альтернативным
/// экраном. Флаг превращает такой сигнал в штатный выход из цикла.
fn install_signal_flag() -> io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGQUIT,
    ] {
        signal_hook::flag::register(signal, Arc::clone(&flag))?;
    }
    Ok(flag)
}
/// Запускает интерфейс. Возвращается при выходе пользователя.
pub fn run(
    config: &Config,
    snapshot: SnapshotSource,
    history: Arc<RwLock<History>>,
    details: Arc<dyn pulse_core::ProcessDetailsSource>,
    demo: bool,
) -> io::Result<()> {
    // Флаг ставится ДО перевода терминала в raw-режим: иначе сигнал в этом
    // окне оставил бы пользователя в сломанном терминале.
    let terminate = install_signal_flag()?;
    spawn_shutdown_watchdog(&terminate);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;

    let theme = Theme::new(config.ui.ascii);
    let mut app = App::new(config.security.allow_actions)
        .with_details(details)
        .with_icons(config.ui.icons)
        .with_demo(demo);
    let refresh = Duration::from_millis(config.ui.refresh_ms.clamp(50, 5_000));
    let mut current = snapshot();

    loop {
        if terminate.load(AtomicOrdering::Relaxed) {
            break;
        }
        if !app.paused {
            current = snapshot();
        }
        let history_guard = match history.read() {
            Ok(guard) => guard,
            // Паника писателя не делает данные несогласованными.
            Err(poisoned) => poisoned.into_inner(),
        };
        terminal.draw(|frame| {
            screens::render(frame, &current, &mut app, &theme, Some(&history_guard));
        })?;
        drop(history_guard);
        let last_draw = Instant::now();

        // Ждём ввод не дольше интервала перерисовки: интерфейс остаётся живым,
        // но не жжёт процессор в пустом цикле.
        let wait = refresh.saturating_sub(last_draw.elapsed());
        if !event::poll(wait.max(Duration::from_millis(10)))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                // Один физический key event проходит ровно через один router.
                // Modal/Inspector/focused pane/global routing живёт в App и
                // читает то же visible state, что renderer (v0.9 §179, §195).
                match app.dispatch(key, &current) {
                    Action::Quit => break,
                    Action::RunDiff => {
                        let (Some(a), Some(b)) = (app.timeline.mark_a, app.timeline.mark_b) else {
                            continue;
                        };
                        let text = match history.read() {
                            Ok(history) => diff_report_text(&history, a, b),
                            // Отравленный замок означает панику писателя, но
                            // данные в структуре остались согласованными.
                            Err(poisoned) => diff_report_text(&poisoned.into_inner(), a, b),
                        };
                        app.timeline.diff_text = Some(text);
                        app.screen = Screen::Timeline;
                        app.inspector = None;
                    }
                    Action::None => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    Ok(())
}

/// Формирует текст A/B-отчёта и честно предупреждает о неполном окне.
///
/// История живёт только в памяти и ограничена горизонтом, поэтому маркер может
/// оказаться за её пределами: после длинной паузы, после скачка системных
/// часов или просто на старом маркере. Молча отдать отчёт по обрезанному окну
/// нельзя — оператор примет неполный список за полный.
#[must_use]
pub fn diff_report_text(history: &History, a: Timestamp, b: Timestamp) -> String {
    let (from, to) = if a <= b { (a, b) } else { (b, a) };
    let oldest = history.oldest();
    let newest = history.newest();

    if newest == Timestamp::ZERO {
        return "история пуста: сравнивать нечего\n".to_string();
    }
    if to < oldest || from > newest {
        return format!(
            "окно {from}..{to} целиком вне локальной истории {oldest}..{newest}\nдисковая история не реализована: поставьте маркеры заново\n"
        );
    }

    let report = diff(history, from, to, &DiffOptions::default());
    let mut text = String::with_capacity(1024);
    if from < oldest || to > newest {
        text.push_str(&format!(
            "внимание: окно {from}..{to} выходит за историю {oldest}..{newest} — отчёт неполный\n\n"
        ));
    }
    text.push_str(&render_text(&report));
    text
}
#[cfg(test)]
mod tests {
    use pulse_core::config::Store as StoreConfig;
    use pulse_core::metric::ids;
    use pulse_core::{EntityGraph, TickBatch};

    /// История из `ticks` тактов по одной секунде, начиная с 2000 мс.
    fn history_with_ticks(ticks: u64) -> History {
        let mut history = History::new(&StoreConfig::default());
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        for tick in 1..=ticks {
            graph.begin_tick(Timestamp::from_millis(1_000 + tick * 1_000));
            let host = graph.host();
            graph.sample(host, ids::HOST_CPU_UTIL, 0.5);
            let batch: TickBatch = graph.end_tick();
            history.ingest(&batch);
        }
        history
    }

    #[test]
    fn diff_report_warns_when_marker_predates_local_history() {
        let history = history_with_ticks(5);
        let old = history.oldest().saturating_sub_millis(600_000);
        let text = diff_report_text(&history, old, history.newest());
        assert!(
            text.contains("выходит за историю"),
            "оператор обязан видеть, что отчёт неполный: {text}"
        );
        assert!(text.contains("entities"), "сам отчёт всё равно печатается");
    }

    #[test]
    fn diff_report_refuses_window_entirely_outside_history() {
        let history = history_with_ticks(5);
        let from = history.newest().saturating_add_millis(600_000);
        let to = from.saturating_add_millis(1_000);
        let text = diff_report_text(&history, from, to);
        assert!(text.contains("целиком вне локальной истории"), "{text}");
        assert!(!text.contains("correlated"), "нечего ранжировать");
    }

    #[test]
    fn diff_report_handles_empty_history_and_swapped_markers() {
        let empty = History::new(&StoreConfig::default());
        assert!(
            diff_report_text(&empty, Timestamp::from_millis(1), Timestamp::from_millis(2))
                .contains("история пуста")
        );

        let history = history_with_ticks(5);
        let forward = diff_report_text(&history, history.oldest(), history.newest());
        let backward = diff_report_text(&history, history.newest(), history.oldest());
        assert_eq!(forward, backward, "порядок маркеров не меняет отчёт");
    }

    use super::*;

    #[test]
    fn refresh_interval_is_clamped() {
        let mut config = Config::default();
        config.ui.refresh_ms = 1;
        let clamped = Duration::from_millis(config.ui.refresh_ms.clamp(50, 5_000));
        assert_eq!(clamped, Duration::from_millis(50));
        config.ui.refresh_ms = 100_000;
        let clamped = Duration::from_millis(config.ui.refresh_ms.clamp(50, 5_000));
        assert_eq!(clamped, Duration::from_millis(5_000));
    }

    #[test]
    fn theme_follows_configuration() {
        let mut config = Config::default();
        config.ui.ascii = true;
        let theme = Theme::new(config.ui.ascii);
        assert_eq!(theme.capability, Capability::Ascii);
    }
}
