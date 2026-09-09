//! `check` обязан различать неполноту и отказ сбора.
//!
//! Усечение по настроенному бюджету — законное состояние: на большой ноде
//! дефолтный `max_cgroups` не покрывает всё дерево cgroup. Если бы `check`
//! отвечал отказом, оператор получал бы красный результат, ничего не сломав,
//! и перестал бы доверять команде. Обратная крайность так же плоха: молчать
//! нельзя, потому что часть хоста не наблюдается.

#![cfg(target_os = "linux")]
// Интеграционный тест: падение по `expect` здесь и есть способ сообщить о
// нарушенном инварианте.
#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn config_file(body: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("pulse-trunc-{}-{nonce}.toml", std::process::id()));
    fs::write(&path, body).expect("write config");
    path
}

fn run_check(config: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["--config", config.to_str().expect("utf8 path"), "check"])
        .output()
        .expect("run pulse check");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

/// Хосту нужна настоящая cgroup v2 с несколькими каталогами: иначе усечение
/// не наступит и тест проверял бы не то, что заявляет.
fn cgroup_tree_is_deep_enough() -> bool {
    let Ok(entries) = fs::read_dir("/sys/fs/cgroup") else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .count()
        > 2
}

#[test]
fn truncated_collection_is_reported_without_failing_check() {
    if !cgroup_tree_is_deep_enough() {
        eprintln!("нет cgroup v2 с несколькими каталогами: проверка неприменима");
        return;
    }
    let config =
        config_file("[general]\nmax_cgroups = 2\ninterval_ms = 100\n[export]\nenabled = false\n");
    let (ok, text) = run_check(&config);
    let _ = fs::remove_file(&config);

    assert!(
        ok,
        "усечение по бюджету не является отказом: check обязан пройти\n{text}"
    );
    assert!(
        text.contains("сбор неполон"),
        "неполнота обязана быть названа оператору\n{text}"
    );
    assert!(
        text.contains("ok: host="),
        "итоговая строка обязана остаться\n{text}"
    );
}

#[test]
fn missing_kernel_sources_still_fail_check() {
    let config = config_file(
        "[general]\nproc_root = \"/nonexistent-pulse\"\ncgroup_root = \"/nonexistent-pulse\"\ninterval_ms = 100\n[export]\nenabled = false\n",
    );
    let (ok, text) = run_check(&config);
    let _ = fs::remove_file(&config);

    assert!(
        !ok,
        "отказ подсистемы обязан оставаться отказом, а не предупреждением\n{text}"
    );
    assert!(
        text.contains("ошибок коллекторов"),
        "отказ обязан быть назван отказом\n{text}"
    );
}
