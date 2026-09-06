#!/usr/bin/env bash
# Собирает переносимый статический бинарник и кладёт его в dist/pulse.
#
# Причина существования: обычная сборка динамически линкуется с glibc узла
# сборки. Бинарник, собранный на Ubuntu 24.04, требует GLIBC_2.39 и на 22.04
# падает с `version 'GLIBC_2.39' not found`. README при этом обещает один
# переносимый standalone-бинарник, поэтому поставляется musl-сборка.
#
# Внешний musl-toolchain не нужен: зависимости чисто на Rust, а Rust везёт
# самодостаточную musl-libc для этого таргета.
set -euo pipefail

TARGET="x86_64-unknown-linux-musl"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="$ROOT/scripts/wsl-cargo.sh"

if [ ! -x "$CARGO" ]; then
    echo "не найден $CARGO" >&2
    exit 1
fi

echo "=== rustup target add $TARGET"
"${CARGO_HOME:-$HOME/.cargo}/bin/rustup" target add "$TARGET" >/dev/null

echo "=== cargo build --release --target $TARGET"
"$CARGO" build --release --locked --target "$TARGET" -p pulse-cli

BIN="${CARGO_TARGET_DIR:-$HOME/.cache/pulse-target}/$TARGET/release/pulse"
if [ ! -f "$BIN" ]; then
    echo "бинарник не найден: $BIN" >&2
    exit 1
fi

mkdir -p "$ROOT/dist"
install -m 0755 "$BIN" "$ROOT/dist/pulse"

echo "=== проверка переносимости"

# Статическая линковка обязательна: иначе обещание «один бинарник» ложно.
if ldd "$ROOT/dist/pulse" 2>&1 | grep -q "statically linked"; then
    echo "ok: статически связан"
else
    echo "бинарник не статический:" >&2
    ldd "$ROOT/dist/pulse" >&2
    exit 1
fi

# Ни одной ссылки на версии glibc.
#
# `grep -c` возвращает код 1 при нуле совпадений, а под `pipefail` это увело бы
# скрипт в ветку ошибки на успешном результате - поэтому код подавляется.
GLIBC_REFS="$(grep -ac "GLIBC_" "$ROOT/dist/pulse" || true)"
if [ "$GLIBC_REFS" = "0" ]; then
    echo "ok: ссылок на glibc нет"
else
    echo "в бинарнике осталось ссылок на glibc: $GLIBC_REFS" >&2
    exit 1
fi

echo "=== смоук"
"$ROOT/dist/pulse" check

SIZE="$(du -h "$ROOT/dist/pulse" | cut -f1)"
echo
echo "готово: dist/pulse ($SIZE), таргет $TARGET"
echo "перенос на любой Linux x86_64:"
echo "  scp dist/pulse user@host:~/pulse"
echo "  ssh user@host 'chmod +x ~/pulse && ~/pulse run'"
