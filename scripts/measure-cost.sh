#!/usr/bin/env bash
# Измеряет фактическую стоимость агента на живом ядре.
#
# Причина существования: жалоба «PULSE сам жрёт ядро» проверяется только
# измерением, а не чтением кода. Скрипт печатает CPU в процентах одного ядра,
# RSS, число экспортируемых серий и время выхода по SIGTERM.
#
# Использование: scripts/measure-cost.sh [секунды] [интервал_мс]
set -euo pipefail

# Режим TUI: замеряется render-цикл, а не только сбор.
if [ "${1:-}" = "--tui" ]; then
  WINDOW="${2:-15}"
  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  cd "$repo_root"
  if [ ! -x dist/pulse ]; then
    echo "нет dist/pulse: сначала scripts/build-portable.sh" >&2
    exit 1
  fi
  session="pulse-cost-$$"
  tmux kill-session -t "$session" 2>/dev/null || true
  tmux new-session -d -s "$session" -x 180 -y 40 "./dist/pulse run"
  sleep 3
  pane_pid="$(tmux list-panes -t "$session" -F '#{pane_pid}' | head -1)"
  hz="$(getconf CLK_TCK)"
  before="$(awk '{print $14 + $15}' "/proc/${pane_pid}/stat")"
  sleep "$WINDOW"
  after="$(awk '{print $14 + $15}' "/proc/${pane_pid}/stat")"
  rss="$(awk '/VmRSS/{print $2}' "/proc/${pane_pid}/status")"
  awk -v a="$before" -v b="$after" -v hz="$hz" -v w="$WINDOW" \
    'BEGIN { printf "tui_cpu_percent_of_one_core=%.2f\n", (b - a) / hz / w * 100 }'
  echo "tui_rss_kib=${rss}"
  start_ns="$(date +%s%N)"
  tmux send-keys -t "$session" -l 'q'
  for _ in $(seq 1 60); do
    kill -0 "$pane_pid" 2>/dev/null || break
    sleep 0.05
  done
  end_ns="$(date +%s%N)"
  echo "tui_quit_ms=$(( (end_ns - start_ns) / 1000000 ))"
  tmux kill-session -t "$session" 2>/dev/null || true
  exit 0
fi

WINDOW="${1:-20}"
INTERVAL_MS="${2:-1000}"
PORT=9098

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -x dist/pulse ]; then
  echo "нет dist/pulse: сначала scripts/build-portable.sh" >&2
  exit 1
fi

config="$(mktemp)"
trap 'rm -f "$config"' EXIT
cat >"$config" <<EOF
[general]
interval_ms = ${INTERVAL_MS}

[store]
max_bytes = 8388608

[export]
enabled = true
bind = "127.0.0.1:${PORT}"
EOF

./dist/pulse --config "$config" serve >/tmp/pulse-measure.log 2>&1 &
pid=$!
sleep 2

if ! kill -0 "$pid" 2>/dev/null; then
  echo "агент не запустился:" >&2
  cat /tmp/pulse-measure.log >&2
  exit 1
fi

cpu_ticks() {
  awk '{print $14 + $15}' "/proc/$1/stat"
}

before="$(cpu_ticks "$pid")"
sleep "$WINDOW"
after="$(cpu_ticks "$pid")"
hz="$(getconf CLK_TCK)"
rss="$(awk '/VmRSS/{print $2}' "/proc/$pid/status")"
series="$(curl -s "http://127.0.0.1:${PORT}/metrics" | grep -c '^pulse_' || true)"

awk -v a="$before" -v b="$after" -v hz="$hz" -v w="$WINDOW" \
  'BEGIN { printf "cpu_percent_of_one_core=%.2f\n", (b - a) / hz / w * 100 }'
echo "rss_kib=${rss}"
echo "exported_series=${series}"
echo "interval_ms=${INTERVAL_MS}"

start_ns="$(date +%s%N)"
kill -TERM "$pid"
for _ in $(seq 1 100); do
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.05
done
end_ns="$(date +%s%N)"
if kill -0 "$pid" 2>/dev/null; then
  echo "sigterm_exit=TIMEOUT" >&2
  kill -KILL "$pid" 2>/dev/null || true
  exit 1
fi
echo "sigterm_exit_ms=$(( (end_ns - start_ns) / 1000000 ))"
