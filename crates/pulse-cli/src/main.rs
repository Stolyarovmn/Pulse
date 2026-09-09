//! Бинарник Pulse.
//!
//! По умолчанию запускает диагностический TUI. Все одноразовые команды
//! используют тот же реальный конвейер сбора, а не отдельные упрощённые пути.

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

mod runtime;
mod text;

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use pulse_core::time::Timestamp;
use pulse_core::Config as PulseConfig;
use pulse_engine::{diff, render_text, DiffOptions};
use tracing_subscriber::EnvFilter;

use runtime::AgentRuntime;

/// Local-first observability node с диагностическим TUI.
#[derive(Debug, Parser)]
#[command(name = "pulse", version, about)]
struct Cli {
    /// TOML-файл конфигурации. Если не задан, используются безопасные значения.
    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Демонстрационный сценарий вместо реального хоста.
    ///
    /// Нужен, потому что на здоровом хосте показать диагностику нечем:
    /// правила молчат, Timeline пуст, diff пустой. Подменяется только слой
    /// чтения файлов — коллекторы, граф, правила и история настоящие.
    #[arg(long, global = true)]
    demo: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// TUI + локальный `/metrics` (команда по умолчанию).
    Run,
    /// Агент без TUI, только сбор и HTTP-экспорт.
    Serve,
    /// Однократный текстовый снимок после двух тактов.
    Top {
        /// Максимум сущностей в таблице.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Семантическое сравнение двух моментов локальной истории.
    Diff {
        /// Момент A: `30s`, `5m` (столько назад), `now` или UNIX-ms.
        #[arg(long)]
        from: String,
        /// Момент B: `now`, относительная давность или UNIX-ms.
        #[arg(long)]
        to: String,
    },
    /// Измерить стоимость агента и размеры локального состояния.
    Scorecard {
        /// Длительность измерения; 1..=300 секунд.
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// Операции с конфигурацией.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Проверить конфигурацию и выполнить реальный такт сбора.
    Check,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Напечатать эффективную конфигурацию.
    Print,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimeSpec {
    Now,
    Ago(Duration),
    Absolute(Timestamp),
}

fn main() {
    if let Err(error) = try_main() {
        eprintln!("pulse: {error}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    // Диагностика не имеет права печататься в терминал, занятый TUI: ratatui
    // владеет каждой ячейкой экрана и не перерисовывает те, которые считает
    // неизменными. Одна строка `tracing` из потока сбора остаётся висеть
    // посреди кадра и переживает переключение экранов.
    let interactive = matches!(cli.command, None | Some(Command::Run));
    init_tracing(!interactive);
    let config = load_config(cli.config.as_deref())?;

    let demo = cli.demo;
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run_tui(&config, demo),
        Command::Serve => serve(&config, demo),
        Command::Top { limit } => top(&config, limit, demo),
        Command::Diff { from, to } => run_diff(&config, &from, &to, demo),
        Command::Scorecard { seconds } => scorecard(&config, seconds, demo),
        Command::Config {
            command: ConfigCommand::Print,
        } => {
            print!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
        Command::Check => check(&config, demo),
    }
}

/// Настраивает диагностику.
///
/// `to_terminal = false` используется в TUI-режиме: журнал уходит в файл из
/// `PULSE_LOG`, а без него отбрасывается. Терять диагностику при этом нечем:
/// состояние потолка памяти и ошибки коллекторов уже видны в самом интерфейсе
/// как счётчики и события, а не только в логе.
fn init_tracing(to_terminal: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    if to_terminal {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .try_init();
        return;
    }

    match std::env::var_os("PULSE_LOG").map(PathBuf::from) {
        Some(path) => match fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(file)
                    .compact()
                    .try_init();
            }
            Err(error) => {
                // Единственная допустимая печать до входа в alternate screen.
                eprintln!(
                    "pulse: не удалось открыть PULSE_LOG {}: {error}",
                    path.display()
                );
            }
        },
        None => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_writer(io::sink)
                .compact()
                .try_init();
        }
    }
}

/// Загружает конфигурацию с пределом размера. Неизвестные поля отклоняет
/// `#[serde(deny_unknown_fields)]` в `pulse-core`.
fn load_config(path: Option<&Path>) -> Result<PulseConfig, Box<dyn Error>> {
    const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
    let config = if let Some(path) = path {
        // Путь обязан быть в тексте ошибки: `No such file or directory
        // (os error 2)` без имени файла не отличает опечатку в `--config`
        // от удалённого файла, и оператор ищет причину в бинарнике.
        let metadata = fs::metadata(path)
            .map_err(|error| format!("конфигурация {}: {error}", path.display()))?;
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(format!(
                "конфигурация {} больше допустимых {} байт",
                path.display(),
                MAX_CONFIG_BYTES
            )
            .into());
        }
        let text = fs::read_to_string(path)
            .map_err(|error| format!("конфигурация {}: {error}", path.display()))?;
        toml::from_str(&text)?
    } else {
        PulseConfig::default()
    };
    config.validate()?;
    Ok(config)
}

/// Источник файловой системы: реальный хост или демонстрационный сценарий.
fn source_fs(config: &PulseConfig, demo: bool) -> std::sync::Arc<dyn pulse_collect::FsSource> {
    if demo {
        let step = std::time::Duration::from_millis(config.general.interval_ms.max(1));
        std::sync::Arc::new(pulse_collect::DemoFs::new(step))
    } else {
        std::sync::Arc::new(pulse_collect::RealFs)
    }
}

fn run_tui(config: &PulseConfig, demo: bool) -> Result<(), Box<dyn Error>> {
    let fs = source_fs(config, demo);
    let runtime = AgentRuntime::start_with_fs(config, std::sync::Arc::clone(&fs))?;
    let exporter = if config.export.enabled {
        let handle = pulse_export::spawn(&config.export, runtime.source())?;
        tracing::info!(address = %handle.local_addr(), "экспортёр запущен");
        Some(handle)
    } else {
        None
    };

    // Детали процесса читаются по требованию тем же источником файловой
    // системы, что и коллекторы: одна точка правды о `proc_root`.
    let details = std::sync::Arc::new(pulse_collect::details::ProcDetails::new(
        fs,
        config.general.proc_root.clone(),
    ));
    let result = pulse_tui::run(config, runtime.source(), runtime.history(), details, demo);
    if let Some(handle) = exporter {
        handle.shutdown();
    }
    runtime.shutdown();
    result.map_err(Into::into)
}

fn serve(config: &PulseConfig, demo: bool) -> Result<(), Box<dyn Error>> {
    if !config.export.enabled {
        return Err("команда serve требует export.enabled = true".into());
    }
    // Handler обязан стоять до первого сбора и запуска exporter. Первый такт
    // на нагруженном хосте может длиться больше секунды; сигнал в это окно
    // раньше применял действие ядра по умолчанию и убивал процесс напрямую,
    // минуя exporter.shutdown/runtime.shutdown.
    let terminate = install_signal_flag()?;
    // Готовность объявляется наблюдаемо: с этого момента сигнал переводится в
    // штатное завершение, а не в действие ядра по умолчанию. Оператору строка
    // говорит, что Ctrl-C уже безопасен, ещё до первого такта.
    println!("pulse: сигналы перехвачены, идёт первый сбор");
    let runtime = AgentRuntime::start_with_fs(config, source_fs(config, demo))?;
    let _ = runtime.wait_for_tick(1, tick_timeout(config))?;
    let exporter = pulse_export::spawn(&config.export, runtime.source())?;
    println!("pulse: listening on http://{}", exporter.local_addr());
    println!("pulse: endpoints /metrics /healthz");

    // Сигнал переводится в штатное завершение: поток экспорта останавливается,
    // поток сбора присоединяется. Иначе процесс умирает посреди такта и в
    // журнале это выглядит как сбой, а не как остановка по команде.
    while !terminate.load(AtomicOrdering::Relaxed) {
        thread::park_timeout(Duration::from_millis(200));
    }
    println!("pulse: остановка по сигналу");
    exporter.shutdown();
    runtime.shutdown();
    Ok(())
}

/// Флаг завершения по внешним сигналам для долгоживущих команд.
fn install_signal_flag() -> Result<Arc<AtomicBool>, Box<dyn Error>> {
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

fn top(config: &PulseConfig, limit: usize, demo: bool) -> Result<(), Box<dyn Error>> {
    if !(1..=10_000).contains(&limit) {
        return Err("--limit должен быть в диапазоне 1..=10000".into());
    }
    let runtime = AgentRuntime::start_with_fs(config, source_fs(config, demo))?;
    let snapshot = runtime.wait_for_tick(2, tick_timeout(config))?;
    print!("{}", text::render_top(&snapshot, limit));
    runtime.shutdown();
    Ok(())
}

fn run_diff(config: &PulseConfig, from: &str, to: &str, demo: bool) -> Result<(), Box<dyn Error>> {
    let from_spec = parse_time_spec(from)?;
    let to_spec = parse_time_spec(to)?;
    let lookback = max_lookback(from_spec, to_spec);
    if lookback > Duration::from_secs(300) {
        return Err(
            "относительный интервал CLI diff ограничен 5 минутами: команда сама набирает историю в памяти с нуля, дисковая история не реализована"
                .into(),
        );
    }

    let runtime = AgentRuntime::start_with_fs(config, source_fs(config, demo))?;
    // Локальная история памяти начинается вместе с командой. Дополнительный
    // такт гарантирует точку слева от относительного маркера A.
    let fill = lookback
        .saturating_add(Duration::from_millis(config.general.interval_ms))
        .max(Duration::from_millis(
            config.general.interval_ms.saturating_mul(2),
        ));
    thread::sleep(fill);
    let snapshot = runtime.wait_for_tick(2, tick_timeout(config))?;
    let anchor = snapshot.at;
    let a = resolve_time_spec(from_spec, anchor);
    let b = resolve_time_spec(to_spec, anchor);
    if a >= b {
        return Err(format!("момент --from ({a}) должен быть раньше --to ({b})").into());
    }

    let history = runtime.history();
    let report = match history.read() {
        Ok(stored) => {
            ensure_history_contains(&stored, a, b)?;
            diff(&stored, a, b, &DiffOptions::default())
        }
        Err(poisoned) => {
            let stored = poisoned.into_inner();
            ensure_history_contains(&stored, a, b)?;
            diff(&stored, a, b, &DiffOptions::default())
        }
    };
    print!("{}", render_text(&report));
    runtime.shutdown();
    Ok(())
}

fn ensure_history_contains(
    history: &pulse_store::History,
    from: Timestamp,
    to: Timestamp,
) -> Result<(), Box<dyn Error>> {
    if from < history.oldest() || to > history.newest() {
        return Err(format!(
            "запрошен интервал {from}..{to}, доступна история {}..{}; история живёт только в памяти этого процесса, дисковая история не реализована",
            history.oldest(),
            history.newest()
        )
        .into());
    }
    Ok(())
}

fn scorecard(config: &PulseConfig, seconds: u64, demo: bool) -> Result<(), Box<dyn Error>> {
    if !(1..=300).contains(&seconds) {
        return Err("--seconds должен быть в диапазоне 1..=300".into());
    }
    let runtime = AgentRuntime::start_with_fs(config, source_fs(config, demo))?;
    let started = Instant::now();
    thread::sleep(Duration::from_secs(seconds));
    let snapshot = runtime.wait_for_tick(1, tick_timeout(config))?;
    print!("{}", text::render_scorecard(&snapshot, started.elapsed()));
    runtime.shutdown();
    Ok(())
}

fn check(config: &PulseConfig, demo: bool) -> Result<(), Box<dyn Error>> {
    if let Some(path) = &config.export.token_file {
        let _ = pulse_export::load_token(path)?;
    }
    let runtime = AgentRuntime::start_with_fs(config, source_fs(config, demo))?;
    let snapshot = runtime.wait_for_tick(1, tick_timeout(config))?;
    if snapshot.agent.collector_errors > 0 {
        return Err(format!(
            "сбор доступен частично: ошибок коллекторов {} (см. timeline/log)",
            snapshot.agent.collector_errors
        )
        .into());
    }
    // Число рёбер cgroup → disk печатается отдельно: отсутствие ошибок
    // коллектора не доказывает, что топология ядра разрешилась в зависимости.
    // Ноль допустим на простаивающем хосте, но обязан быть виден, а не молчать.
    println!(
        "ok: host={} boot={} entities={} series={} disk_deps={} tick={:.2}ms bind={}",
        snapshot.hostname,
        snapshot.boot_id,
        snapshot.entities.len(),
        snapshot.agent.series_live,
        text::disk_dependency_count(&snapshot),
        snapshot.agent.tick_duration_ms,
        config.export.bind,
    );
    runtime.shutdown();
    Ok(())
}

/// Сколько ждать нужного такта у одноразовых команд.
///
/// Четыре интервала сбора: одного мало на медленном хосте, а бесконечное
/// ожидание превратило бы `check` в вечно висящую команду.
fn tick_timeout(config: &PulseConfig) -> Duration {
    Duration::from_millis(
        config
            .general
            .interval_ms
            .saturating_mul(4)
            .clamp(2_000, 65_000),
    )
}

fn parse_time_spec(text: &str) -> Result<TimeSpec, Box<dyn Error>> {
    let value = text.trim().to_ascii_lowercase();
    if value == "now" {
        return Ok(TimeSpec::Now);
    }
    for (suffix, multiplier) in [("ms", 1_u64), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)] {
        if let Some(number) = value.strip_suffix(suffix) {
            let amount: u64 = number.parse().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("неверный момент времени: {text}"),
                )
            })?;
            return Ok(TimeSpec::Ago(Duration::from_millis(
                amount.saturating_mul(multiplier),
            )));
        }
    }
    let millis: u64 = value.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("неверный момент времени {text:?}; ожидается now, 30s, 5m или UNIX-ms"),
        )
    })?;
    Ok(TimeSpec::Absolute(Timestamp::from_millis(millis)))
}

fn resolve_time_spec(spec: TimeSpec, anchor: Timestamp) -> Timestamp {
    match spec {
        TimeSpec::Now => anchor,
        TimeSpec::Ago(duration) => {
            anchor.saturating_sub_millis(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        }
        TimeSpec::Absolute(timestamp) => timestamp,
    }
}

fn max_lookback(a: TimeSpec, b: TimeSpec) -> Duration {
    [a, b]
        .into_iter()
        .filter_map(|spec| match spec {
            TimeSpec::Ago(duration) => Some(duration),
            TimeSpec::Now | TimeSpec::Absolute(_) => None,
        })
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_to_run() {
        let cli = Cli::try_parse_from(["pulse"]).expect("CLI");
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_accepts_every_contract_command() {
        for args in [
            vec!["pulse", "run"],
            vec!["pulse", "serve"],
            vec!["pulse", "top", "--limit", "5"],
            vec!["pulse", "diff", "--from", "30s", "--to", "now"],
            vec!["pulse", "scorecard", "--seconds", "1"],
            vec!["pulse", "config", "print"],
            vec!["pulse", "check"],
        ] {
            Cli::try_parse_from(args).expect("подкоманда должна разбираться");
        }
    }

    #[test]
    fn time_specs_support_relative_and_absolute_values() {
        assert_eq!(parse_time_spec("now").expect("now"), TimeSpec::Now);
        assert_eq!(
            parse_time_spec("30s").expect("30s"),
            TimeSpec::Ago(Duration::from_secs(30))
        );
        assert_eq!(
            parse_time_spec("1700000000000").expect("unix"),
            TimeSpec::Absolute(Timestamp::from_millis(1_700_000_000_000))
        );
        assert!(parse_time_spec("yesterday").is_err());
    }

    #[test]
    fn relative_time_saturates_at_epoch() {
        let at = Timestamp::from_millis(500);
        assert_eq!(
            resolve_time_spec(TimeSpec::Ago(Duration::from_secs(1)), at),
            Timestamp::ZERO
        );
    }

    #[test]
    fn oversized_config_is_rejected_before_parsing() {
        let path =
            std::env::temp_dir().join(format!("pulse-large-config-{}.toml", std::process::id()));
        let file = fs::File::create(&path).expect("создать файл");
        file.set_len(1024 * 1024 + 1).expect("увеличить файл");
        let result = load_config(Some(&path));
        let _ = fs::remove_file(path);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_config_key_is_rejected() {
        let path =
            std::env::temp_dir().join(format!("pulse-unknown-config-{}.toml", std::process::id()));
        fs::write(&path, "[general]\nunknown = 1\n").expect("записать файл");
        let result = load_config(Some(&path));
        let _ = fs::remove_file(path);
        assert!(result.is_err());
    }

    /// Отсутствующий конфиг обязан назвать свой путь.
    ///
    /// Без пути оператор видит только `No such file or directory
    /// (os error 2)` и не может отличить опечатку в `--config` от файла,
    /// который стёрла очистка `/tmp`.
    #[test]
    fn missing_config_names_the_path() {
        let path = std::env::temp_dir().join(format!("pulse-absent-{}.toml", std::process::id()));
        let _ = fs::remove_file(&path);
        let error = load_config(Some(&path)).expect_err("отсутствующий файл — ошибка");
        let text = error.to_string();
        assert!(
            text.contains(&path.display().to_string()),
            "в тексте ошибки обязан быть путь: {text}"
        );
    }
}
