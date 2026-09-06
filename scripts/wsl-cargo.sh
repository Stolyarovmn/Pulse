#!/usr/bin/env bash
# Запуск cargo внутри WSL из Windows-хоста.
#
# Причина существования: репозиторий лежит на диске Windows (/mnt/c/...), а сборка
# должна идти Linux-тулчейном. Кроме того, target-каталог держим в файловой системе
# Linux — на 9p-монтировании сборка в разы медленнее.
#
# Использование:
#   bash scripts/wsl-cargo.sh test -p pulse-core
#   bash scripts/wsl-cargo.sh clippy --all-targets -- -D warnings
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/pulse-target}"
export CARGO_TERM_COLOR=never

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

exec cargo "$@"
