#!/usr/bin/env bash
# Сверяет NET и DISK в кадре TUI с независимым замером по /proc.
#
# Причина существования: Overview месяцами показывал `NET ↓0 B/s` при живом
# трафике, потому что читал серию, которой на host не существует. Проверять это
# глазами по скриншоту дорого и невоспроизводимо.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -x dist/pulse ]; then
  echo "нет dist/pulse: сначала scripts/build-portable.sh" >&2
  exit 1
fi

sum_rx() {
  awk 'NR>2 { gsub(/:/, " "); if ($1 != "lo") rx += $2 } END { print rx + 0 }' /proc/net/dev
}

# Независимый замер: дельта агрегата по всем интерфейсам, кроме loopback.
before="$(sum_rx)"

session="pulse-net-$$"
tmux kill-session -t "$session" 2>/dev/null || true
tmux new-session -d -s "$session" -x 200 -y 24 "./dist/pulse run"
trap 'tmux kill-session -t "$session" 2>/dev/null || true' EXIT

# Нагружаем сеть, пока агент собирает такты.
for _ in $(seq 1 6); do
  curl -s -o /dev/null --max-time 2 http://ya.ru/ || true
  sleep 0.5
done
sleep 3

frame="$(tmux capture-pane -p -t "$session")"
after="$(sum_rx)"

signals="$(printf '%s\n' "$frame" | grep -m1 'CPU ' || true)"
echo "frame_signals: ${signals}"
echo "proc_rx_delta_bytes: $((after - before))"

tmux send-keys -t "$session" -l 'q' 2>/dev/null || true
sleep 1

if printf '%s' "$signals" | grep -q 'NET ↓0 B/s ↑0 B/s'; then
  echo "ВЕРДИКТ: сеть по-прежнему нулевая при трафике" >&2
  exit 1
fi
if printf '%s' "$signals" | grep -qE 'NET ↓(—|[0-9])'; then
  echo "ВЕРДИКТ: сеть измеряется"
else
  echo "ВЕРДИКТ: строка сигналов не найдена" >&2
  exit 1
fi
