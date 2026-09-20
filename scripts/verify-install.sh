#!/usr/bin/env bash
# Проверяет scripts/install.sh на локальном «релизе» без обращения к сети.
#
# Причина существования: установщик - единственный путь, которым продукт
# попадает на чужой сервер. Его нельзя проверять только успешным сценарием:
# молчаливая установка бинарника с неверной или отсутствующей контрольной
# суммой опаснее, чем отсутствие установщика вовсе. Локальный «релиз» через
# file:// даёт все три исхода без сети и без публикации.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ ! -x "$root/dist/pulse" ]; then
    echo "нет dist/pulse: сначала bash scripts/build-portable.sh" >&2
    exit 1
fi

rel="$(mktemp -d)"
dest="$(mktemp -d)"
trap 'rm -rf "$rel" "$dest"' EXIT

install -m 0755 "$root/dist/pulse" "$rel/pulse-linux-x86_64"
( cd "$rel" && sha256sum pulse-linux-x86_64 > SHA256SUMS )

failures=0
check() {
    if [ "$1" = "ok" ]; then
        echo "ok: $2"
    else
        echo "FAIL: $2" >&2
        failures=$((failures + 1))
    fi
}

echo "=== 1. целый релиз устанавливается"
if PULSE_RELEASE_BASE="file://$rel" PULSE_INSTALL_DIR="$dest/good" sh "$root/scripts/install.sh" >/dev/null 2>&1 \
    && [ -x "$dest/good/pulse" ]; then
    check ok "бинарник установлен и исполняем"
else
    check fail "целый релиз обязан устанавливаться"
fi

echo "=== 2. подменённый бинарник отвергается"
printf '%s  pulse-linux-x86_64\n' "$(printf '0%.0s' $(seq 1 64))" > "$rel/SHA256SUMS"
PULSE_RELEASE_BASE="file://$rel" PULSE_INSTALL_DIR="$dest/bad" sh "$root/scripts/install.sh" >/dev/null 2>&1
if [ ! -e "$dest/bad/pulse" ]; then
    check ok "несовпадение суммы прерывает установку"
else
    check fail "бинарник установлен при неверной контрольной сумме"
fi

echo "=== 3. релиз без SHA256SUMS отвергается"
rm -f "$rel/SHA256SUMS"
PULSE_RELEASE_BASE="file://$rel" PULSE_INSTALL_DIR="$dest/nosum" sh "$root/scripts/install.sh" >/dev/null 2>&1
if [ ! -e "$dest/nosum/pulse" ]; then
    check ok "отсутствие контрольных сумм прерывает установку"
else
    check fail "бинарник установлен без контрольной суммы"
fi

if [ "$failures" -ne 0 ]; then
    echo "провалов: $failures" >&2
    exit 1
fi
echo "установщик проверен: 3 из 3"
