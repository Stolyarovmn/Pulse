#!/usr/bin/env bash
# Проверяет, что фактическая память агента соответствует заявленному бюджету.
#
# Причина существования: `store.max_bytes` проверялся только unit-тестами
# вытеснения. Это доказывает арифметику структуры, но не отвечает на вопрос
# оператора «сколько агент займёт на моём хосте» (PULSE-082). Здесь бюджет
# намеренно мал, такт част, а процессы включены: если лестница вытеснения не
# работает, пик истории уедет за бюджет на живом ядре, а не в фикстуре.
#
# Использование: scripts/measure-memory-bound.sh [секунды] [бюджет_байт]
set -euo pipefail

SECONDS_TO_RUN="${1:-30}"
MAX_BYTES="${2:-4194304}"
# Накладные расходы процесса: аллокатор, стеки потоков, граф сущностей, буферы
# ratatui. Бюджет `store.max_bytes` ограничивает историю, а не весь RSS.
RSS_OVERHEAD_BYTES=$((48 * 1024 * 1024))

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -x dist/pulse ]; then
    echo "нет dist/pulse: сначала bash scripts/build-portable.sh" >&2
    exit 1
fi

config="$(mktemp)"
trap 'rm -f "$config"' EXIT
cat >"$config" <<EOF
[general]
interval_ms = 200
collect_processes = true

[store]
max_bytes = ${MAX_BYTES}
hot_ticks = 100000

[export]
bind = "127.0.0.1:9097"
EOF

report="$(./dist/pulse --config "$config" scorecard --seconds "$SECONDS_TO_RUN")"
echo "$report"

value() {
    echo "$report" | awk -v key="$1:" '$1 == key {print $2}'
}

peak="$(value history_peak_bytes)"
rss="$(value agent_rss_bytes)"
no_progress="$(value history_eviction_no_progress)"
ticks="$(value ticks)"
limit=$((MAX_BYTES + RSS_OVERHEAD_BYTES))

status=0
if [ "${ticks:-0}" -lt 5 ]; then
    echo "FAIL: собрано всего ${ticks} тактов, замер недостоверен" >&2
    status=1
fi

# Контракт двухсоставный. Пока вытеснение справляется, пик обязан держаться
# в бюджете. Если бюджет меньше минимально возможного окна (нижняя граница —
# два такта), соблюсти его нельзя, и тогда обязателен явный признак:
# `history_eviction_no_progress` растёт, и та же величина уходит в `/metrics`.
# Молчаливое превышение запрещено в обоих случаях.
if [ "${no_progress:-0}" -eq 0 ]; then
    if [ "${peak:-0}" -gt "$MAX_BYTES" ]; then
        echo "FAIL: пик истории ${peak} превысил бюджет ${MAX_BYTES} молча" >&2
        status=1
    else
        echo "ok: пик истории ${peak} в пределах бюджета ${MAX_BYTES}"
    fi
else
    hard_limit=$((MAX_BYTES * 2))
    echo "note: бюджет недостижим, вытеснение не дало прогресса ${no_progress} раз — агент это заявляет"
    if [ "${peak:-0}" -gt "$hard_limit" ]; then
        echo "FAIL: пик истории ${peak} вдвое превысил бюджет ${MAX_BYTES}: нижняя граница окна не удерживает рост" >&2
        status=1
    else
        echo "ok: превышение ограничено минимальным окном (${peak} <= ${hard_limit})"
    fi
fi
if [ "${rss:-0}" -gt "$limit" ]; then
    echo "FAIL: RSS ${rss} превысил бюджет истории плюс накладные ${limit}" >&2
    status=1
else
    echo "ok: RSS ${rss} в пределах ${limit}"
fi

exit "$status"
