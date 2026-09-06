#!/usr/bin/env bash
# Снимает траекторию памяти работающего агента через его собственный /metrics.
#
# Причина существования: «легковесность» проверяется не одним замером на старте,
# а выходом на плато. Кольцо истории заполняется минутами, поэтому одиночный
# scorecard показывает заниженный RSS.
#
# Использование: bash scripts/mem-trace.sh [число_замеров] [пауза_секунд] [адрес]
set -euo pipefail

samples="${1:-6}"
pause="${2:-30}"
addr="${3:-127.0.0.1:9099}"

printf '%8s %10s %12s %12s %10s\n' "tick" "rss_MiB" "store_MiB" "series" "sec"
start="$(date +%s)"

for _ in $(seq 1 "$samples"); do
    body="$(curl -s --max-time 5 "http://$addr/metrics" || true)"
    if [ -z "$body" ]; then
        echo "нет ответа от http://$addr/metrics" >&2
        exit 1
    fi
    field() { printf '%s\n' "$body" | awk -v k="$1" '$1 == k { print $2; exit }'; }

    ticks="$(field pulse_agent_ticks_total)"
    rss="$(field pulse_agent_resident_memory_bytes)"
    store="$(field pulse_agent_store_bytes)"
    series="$(field pulse_agent_series_live)"
    now="$(date +%s)"

    printf '%8.0f %10.1f %12.1f %12.0f %10d\n' \
        "${ticks:-0}" \
        "$(awk -v v="${rss:-0}" 'BEGIN { print v / 1048576 }')" \
        "$(awk -v v="${store:-0}" 'BEGIN { print v / 1048576 }')" \
        "${series:-0}" \
        "$((now - start))"

    sleep "$pause"
done
