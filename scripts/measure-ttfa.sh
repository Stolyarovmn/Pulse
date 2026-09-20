#!/usr/bin/env bash
# Измеряет время до первого ответа `pulse top`.
#
# Причина существования: у соседей по нише (ps, htop) первый кадр появляется
# мгновенно, а Pulse ждёт два такта — производных величин по одному наблюдению
# не существует. Утверждение «ответ приходит примерно через два интервала»
# обязано быть замером, а не рассуждением, поэтому здесь берётся серия
# прогонов, а не один.
#
# Использование: scripts/measure-ttfa.sh [прогонов] [лимит_строк]
set -euo pipefail

runs="${1:-10}"
limit="${2:-5}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -x dist/pulse ]; then
  echo "нет dist/pulse: сначала bash scripts/build-portable.sh" >&2
  exit 1
fi

samples=()
for _ in $(seq 1 "$runs"); do
  start_ns="$(date +%s%N)"
  ./dist/pulse top --limit "$limit" >/dev/null
  end_ns="$(date +%s%N)"
  samples+=( $(( (end_ns - start_ns) / 1000000 )) )
done

printf '%s\n' "${samples[@]}" | sort -n | awk -v runs="$runs" '
  { v[NR] = $1 }
  END {
    median = (NR % 2) ? v[(NR + 1) / 2] : int((v[NR / 2] + v[NR / 2 + 1]) / 2)
    printf "runs=%d\nmin_ms=%d\nmedian_ms=%d\nmax_ms=%d\nspread_ms=%d\n", runs, v[1], median, v[NR], v[NR] - v[1]
  }'
