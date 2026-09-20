#!/bin/sh
# Ставит последний релиз Pulse одним файлом.
#
# Причина существования: инструмент диагностики нужен ровно там, где ставить
# ничего нельзя - на чужом сервере, в инциденте, без пакетного менеджера и
# без прав root. Сборка из исходников требует Rust, а значит в этот момент
# недоступна. Скрипт скачивает статический бинарник, сверяет контрольную
# сумму и кладёт его в каталог, куда есть права.
#
# Переменные окружения:
#   PULSE_REPO         - репозиторий (по умолчанию Stolyarovmn/Pulse)
#   PULSE_INSTALL_DIR  - куда ставить (по умолчанию /usr/local/bin, иначе ~/.local/bin)
#   PULSE_RELEASE_BASE - базовый URL артефактов; позволяет проверить скрипт
#                        на локальном каталоге через file://
set -eu

REPO="${PULSE_REPO:-Stolyarovmn/Pulse}"
ASSET="pulse-linux-x86_64"
BASE="${PULSE_RELEASE_BASE:-https://github.com/${REPO}/releases/latest/download}"

die() {
    echo "pulse: $1" >&2
    exit 1
}

os="$(uname -s)"
[ "$os" = "Linux" ] || die "поддерживается только Linux, обнаружено: $os"

arch="$(uname -m)"
case "$arch" in
    x86_64 | amd64) : ;;
    *) die "поддерживается только x86_64, обнаружено: $arch. Соберите из исходников: cargo build --release -p pulse-cli" ;;
esac

# cgroup v2 - не украшение, а источник данных о владельце ресурса. Сказать об
# этом при установке честнее, чем дать пользователю пустые экраны.
if [ ! -f /sys/fs/cgroup/cgroup.controllers ]; then
    echo "pulse: предупреждение: cgroup v2 не смонтирована, часть сущностей и правил работать не будет" >&2
fi

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
else
    die "нужен curl или wget"
fi

if command -v sha256sum >/dev/null 2>&1; then
    checksum() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
    checksum() { shasum -a 256 "$1" | awk '{print $1}'; }
else
    checksum() { echo ""; }
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "pulse: загрузка ${BASE}/${ASSET}"
fetch "${BASE}/${ASSET}" "${tmp}/${ASSET}" || die "не удалось скачать ${BASE}/${ASSET}"

# Отсутствие SHA256SUMS - отказ, а не предупреждение: бинарник, который
# нечем сверить, ставить в PATH нельзя.
fetch "${BASE}/SHA256SUMS" "${tmp}/SHA256SUMS" || die "не удалось скачать SHA256SUMS"

expected="$(awk -v a="$ASSET" '$2 == a || $2 == "*" a {print $1}' "${tmp}/SHA256SUMS")"
[ -n "$expected" ] || die "в SHA256SUMS нет записи для ${ASSET}"

actual="$(checksum "${tmp}/${ASSET}")"
if [ -z "$actual" ]; then
    die "нечем проверить контрольную сумму: нет sha256sum и shasum"
fi
[ "$actual" = "$expected" ] || die "контрольная сумма не совпала: ожидалось ${expected}, получено ${actual}"
echo "pulse: контрольная сумма совпала"

chmod +x "${tmp}/${ASSET}"

# Работоспособность проверяется до установки: скачанный файл может быть
# собран не для этой системы, и узнать об этом лучше до записи в PATH.
"${tmp}/${ASSET}" --version >/dev/null || die "скачанный бинарник не запускается на этой системе"

if [ -n "${PULSE_INSTALL_DIR:-}" ]; then
    dir="$PULSE_INSTALL_DIR"
elif [ -w /usr/local/bin ] 2>/dev/null; then
    dir="/usr/local/bin"
else
    dir="${HOME}/.local/bin"
fi

mkdir -p "$dir" || die "не удалось создать каталог ${dir}"
install -m 0755 "${tmp}/${ASSET}" "${dir}/pulse" || die "не удалось записать ${dir}/pulse"

echo "pulse: установлен в ${dir}/pulse ($("${dir}/pulse" --version))"

case ":${PATH}:" in
    *":${dir}:"*) echo "pulse: запустите: pulse run" ;;
    *) echo "pulse: каталог ${dir} не в PATH; запустите: ${dir}/pulse run" ;;
esac
