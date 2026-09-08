#!/usr/bin/env bash
# Полная проверка качества одной командой: формат, линт, тесты, релизная сборка.
#
# Причина существования: эта тройка (fmt --check → clippy -D warnings → test)
# повторяется после каждой правки и должна совпадать с тем, что делает CI
# (.github/workflows/ci.yml). Расхождение локальной проверки и CI — источник
# «у меня работало».
#
# Использование:
#   bash scripts/wsl-verify.sh            # fmt-check + clippy + тесты
#   bash scripts/wsl-verify.sh --fix      # сначала отформатировать, потом проверить
#   bash scripts/wsl-verify.sh --release  # плюс релизная сборка и scorecard
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_sh="$repo_root/scripts/wsl-cargo.sh"

fix=0
release=0
for arg in "$@"; do
    case "$arg" in
        --fix) fix=1 ;;
        --release) release=1 ;;
        *) echo "неизвестный аргумент: $arg (ожидается --fix | --release)" >&2; exit 2 ;;
    esac
done

step() { printf '\n=== %s\n' "$1"; }

if [ "$fix" -eq 1 ]; then
    step "cargo fmt --all"
    bash "$cargo_sh" fmt --all
fi

step "cargo fmt --all -- --check"
bash "$cargo_sh" fmt --all -- --check

# Ниже всё с `--locked` и в том же порядке, что в .github/workflows/ci.yml:
# локальная проверка обязана совпадать с обязательным CI, иначе расхождение
# lockfile или порядка шагов даёт «у меня работало».
step "cargo check --workspace --all-targets --locked"
bash "$cargo_sh" check --workspace --all-targets --locked

step "cargo clippy --workspace --all-targets --locked -- -D warnings"
bash "$cargo_sh" clippy --workspace --all-targets --locked -- -D warnings

step "cargo test --workspace --locked"
bash "$cargo_sh" test --workspace --locked

# Потеря терминала под работающим интерфейсом. Тест на pty её не
# воспроизводит: процесс выходит сам. Живой дефект «100% ядра и игнор kill»
# даёт только смерть сервера tmux, поэтому проверка внешняя и требует
# собранного бинарника.
if command -v tmux >/dev/null 2>&1 && [ -x "$repo_root/dist/pulse" ]; then
    step "repro-terminal-loss dist/pulse"
    bash "$repo_root/scripts/repro-terminal-loss.sh" dist/pulse
fi

if [ "$release" -eq 1 ]; then
    step "cargo build --release --locked -p pulse-cli"
    bash "$cargo_sh" build --release --locked -p pulse-cli

    binary="${CARGO_TARGET_DIR:-$HOME/.cache/pulse-target}/release/pulse"
    step "$binary check"
    "$binary" check
    step "$binary scorecard --seconds 5"
    "$binary" scorecard --seconds 5
fi

printf '\nвсе проверки пройдены\n'
