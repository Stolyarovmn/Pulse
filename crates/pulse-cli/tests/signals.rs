//! Интеграционная проверка bounded shutdown настоящего бинарника.
//!
//! P0 production-инцидент: после history ceiling процесс потреблял целое ядро
//! и не завершался по SIGTERM. Memory-manager stress покрыт в pulse-store;
//! здесь отдельно проверяется весь signal → flag → worker join → exit путь.

#![cfg(target_os = "linux")]
// Интеграционный тест: падение по `expect`/`panic` здесь и есть способ сообщить
// о нарушенном инварианте, а не дефект стиля.
#![allow(clippy::expect_used, clippy::panic, clippy::ptr_arg)]

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn config_file() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("pulse-signal-{}-{nonce}.toml", std::process::id()));
    fs::write(
        &path,
        r#"
[general]
interval_ms = 100

[store]
hot_ticks = 128
warm_buckets = 64
warm_bucket_ticks = 10
max_events = 8192
max_entity_records = 32768
max_series = 200000
max_bytes = 1024

[export]
enabled = true
bind = "127.0.0.1:0"
"#,
    )
    .expect("write signal-test config");
    path
}

fn spawn_serve(config: &PathBuf) -> Child {
    Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["--config", config.to_str().expect("utf8 path"), "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pulse serve")
}

fn signal_and_wait(signal: &str) {
    let config = config_file();
    let mut child = spawn_serve(&config);
    thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "pulse exited before signal"
    );

    let sent = Command::new("/bin/kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("invoke kill");
    assert!(sent.success(), "kill {signal} failed");

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait after signal") {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(2) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&config);
            panic!("pulse did not exit within 2s after {signal}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let _ = fs::remove_file(config);
    assert!(status.success(), "pulse exit after {signal}: {status}");
}

/// TUI на настоящем терминале обязан завершаться по SIGTERM.
///
/// Остальные тесты этого файла проверяют режим `serve`, где интерфейса нет.
/// Здесь агент работает именно как TUI на pty: цикл отрисовки, raw-режим,
/// альтернативный экран.
///
/// Полную потерю терминала (умер эмулятор, оборван ssh) этот тест НЕ
/// воспроизводит: ни закрытая труба, ни закрытый мастер pty не дают того
/// состояния - процесс выходит сам. Живой дефект «100% ядра и игнор kill»
/// воспроизводится только смертью сервера tmux под работающим интерфейсом и
/// проверяется скриптом `scripts/repro-terminal-loss.sh`.
#[test]
fn tui_on_pty_exits_on_sigterm() {
    use std::os::fd::AsRawFd;

    let config = config_file();

    // Мастер pty остаётся у теста, подчинённый уходит процессу.
    let master =
        rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .expect("openpt");
    rustix::pty::grantpt(&master).expect("grantpt");
    rustix::pty::unlockpt(&master).expect("unlockpt");
    let slave_name = rustix::pty::ptsname(&master, Vec::new()).expect("ptsname");
    let slave_path = String::from_utf8_lossy(slave_name.as_bytes()).into_owned();
    let slave = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&slave_path)
        .expect("open pty slave");

    let stdin = slave.try_clone().expect("clone slave");
    let stdout = slave.try_clone().expect("clone slave");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["--config", config.to_str().expect("utf8 path"), "run"])
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pulse run on pty");

    // Даём интерфейсу выйти на цикл: он обязан отрисовать первый кадр.
    thread::sleep(Duration::from_millis(900));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "pulse завершился до сигнала"
    );

    // Терминал исчезает: закрываем и мастер, и свою копию подчинённого.
    let _ = master.as_raw_fd();
    drop(master);
    drop(slave);
    thread::sleep(Duration::from_millis(400));

    let sent = Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("invoke kill");
    assert!(sent.success(), "kill -TERM failed");

    let started = Instant::now();
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if started.elapsed() >= Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&config);
            panic!("TUI без терминала не завершился по SIGTERM за 5s");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_file(config);
}

#[test]
fn sigterm_exits_within_two_seconds() {
    signal_and_wait("-TERM");
}

#[test]
fn sigint_exits_within_two_seconds() {
    signal_and_wait("-INT");
}

/// Сигнал сразу после объявленной готовности обязан быть штатным завершением.
///
/// Живой дефект: handler ставился после первого такта и запуска exporter, а
/// первый такт на нагруженном хосте длится сотни миллисекунд. Сигнал в это
/// окно применял действие ядра по умолчанию, и процесс умирал с
/// `signal: 15 (SIGTERM)` вместо кода 0, минуя оба shutdown.
///
/// Синхронизация по строке готовности, а не по задержке: длительность окна
/// зависит от машины, и любой `sleep` давал бы либо ложное падение, либо
/// пропуск дефекта. До этой строки контракта нет - между `exec` и
/// регистрацией handler окно физически неизбежно.
#[test]
fn signal_right_after_readiness_is_still_graceful() {
    use std::io::{BufRead, BufReader};

    let config = config_file();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["--config", config.to_str().expect("utf8 path"), "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pulse serve");

    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read readiness line");
    assert!(
        line.contains("сигналы перехвачены"),
        "первой строкой обязана быть готовность к сигналам: {line:?}"
    );

    // Exporter ещё не поднят: строка про listening печатается после первого
    // такта, то есть сигнал попадает ровно в проблемное окно.
    let sent = Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("invoke kill");
    assert!(sent.success(), "kill -TERM failed");

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait after signal") {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&config);
            panic!("pulse не завершился за 5s после сигнала на старте");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let _ = fs::remove_file(&config);
    assert!(
        status.success(),
        "сигнал сразу после готовности обязан быть штатным завершением, получено {status}"
    );
}
