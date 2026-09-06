#!/usr/bin/env bash
# Показывает, на чём стоит или крутится процесс: состояние потоков и их
# потребление ядра за интервал.
#
# Причина существования: агент, потерявший терминал, оставался жив и держал
# порт экспортёра. Диагностика «кто крутится» через одиночные `cut` в
# командной строке Windows-хоста ломалась на кавычках, поэтому она здесь.
#
# Использование: scripts/diag-stuck.sh PID [интервал_сек]
set -euo pipefail

pid="${1:?нужен PID}"
interval="${2:-1}"

if [ ! -d "/proc/$pid" ]; then
  echo "нет процесса $pid" >&2
  exit 1
fi

echo "=== процесс $pid ==="
grep -E '^Name|^State|^Threads|^SigPnd|^SigBlk|^SigIgn|^SigCgt' "/proc/$pid/status"

declare -A before
for task in /proc/$pid/task/*/; do
  tid="$(basename "$task")"
  before[$tid]="$(awk '{print $14 + $15}' "$task/stat" 2>/dev/null || echo 0)"
done

sleep "$interval"

echo "=== потоки за ${interval}s (тики ядра) ==="
for task in /proc/$pid/task/*/; do
  tid="$(basename "$task")"
  [ -r "$task/stat" ] || continue
  name="$(awk '{print $2}' "$task/stat" | tr -d '()')"
  state="$(awk '{print $3}' "$task/stat")"
  now="$(awk '{print $14 + $15}' "$task/stat")"
  delta=$(( now - ${before[$tid]:-0} ))
  wchan="$(cat "$task/wchan" 2>/dev/null || echo '?')"
  printf '%-8s %-18s state=%s delta=%-4s wchan=%s\n' "$tid" "$name" "$state" "$delta" "$wchan"
done
