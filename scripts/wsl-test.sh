#!/usr/bin/env bash
# Тесты одного крейта в WSL с отдельным target-каталогом.
#
# Отдельный target на крейт нужен, чтобы параллельно работающие агенты не стояли
# в очереди на блокировке общего каталога сборки.
#
# Использование из Windows:
#   wsl -d Ubuntu-24.04 -- bash /mnt/c/Users/maxim/project/pulse/scripts/wsl-test.sh pulse-store
#   wsl -d Ubuntu-24.04 -- bash /mnt/c/Users/maxim/project/pulse/scripts/wsl-test.sh pulse-store clippy
set -euo pipefail

crate="${1:?укажите крейт, например pulse-store}"
mode="${2:-test}"

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$HOME/.cache/pulse-t-${crate}"
export CARGO_TERM_COLOR=never

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

case "$mode" in
    test)
        cargo test -p "$crate"
        ;;
    check)
        cargo check -p "$crate" --all-targets
        ;;
    clippy)
        cargo clippy -p "$crate" --all-targets -- -D warnings
        ;;
    *)
        echo "неизвестный режим: $mode (ожидается test|check|clippy)" >&2
        exit 2
        ;;
esac
