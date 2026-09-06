#!/usr/bin/env bash
# Снимает кадр живого TUI как текстовую сетку нужного размера.
#
# Причина существования: ручной захват через `script` + `pkill` убивал не только
# pulse, но и оболочку вызывающего (`pkill -f` ловит собственную командную
# строку), а снятие escape-последовательностей теряет позиционирование курсора -
# ratatui двигает курсор абсолютно, поэтому кадр слипался в одну строку.
#
# Здесь кадр берётся из tmux: эмулятор сам держит сетку, `capture-pane` отдаёт
# готовые строки, а сессия убивается по имени, а не по маске процесса.
#
# Использование:
#   scripts/wsl-capture.sh                        # Overview, 120x30
#   scripts/wsl-capture.sh -c 180 -r 40 -k 4      # Timeline на 180x40
#   scripts/wsl-capture.sh -c 200 -r 50 -s 8      # дать 8 секунд на историю
#
# Флаги:
#   -b PATH   бинарник (по умолчанию dist/pulse)
#   -c N      колонок (120)
#   -r N      строк (30)
#   -s SEC    сколько секунд собирать до снятия (4)
#   -k KEYS   строка клавиш перед снятием: `4`, `?`, `:raw events` и т.п.
#   -e        отдать кадр с escape-последовательностями (цвет)
set -euo pipefail

BIN="dist/pulse"
COLS=120
ROWS=30
WARMUP=4
KEYS=""
WITH_ESCAPES=0

while getopts "b:c:r:s:k:e" opt; do
  case "$opt" in
    b) BIN="$OPTARG" ;;
    c) COLS="$OPTARG" ;;
    r) ROWS="$OPTARG" ;;
    s) WARMUP="$OPTARG" ;;
    k) KEYS="$OPTARG" ;;
    e) WITH_ESCAPES=1 ;;
    *) echo "неизвестный флаг" >&2; exit 2 ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -x "$BIN" ]; then
  echo "нет исполняемого бинарника: $BIN" >&2
  echo "собери его: scripts/build-portable.sh" >&2
  exit 1
fi

# Порт экспортёра один на хост: чужой живой агент сделал бы снимок невозможным.
if ss -ltn 2>/dev/null | grep -q '127.0.0.1:9099'; then
  echo "порт 9099 занят: агент уже запущен, снимок невозможен" >&2
  echo "останови его перед снятием кадра" >&2
  exit 1
fi

session="pulse-capture-$$"
pane_pid=""

# Завершение по PID процесса панели, а не по маске имени: `pkill -f` ловит
# собственную командную строку и уносит вызывающую оболочку, а живой агент
# держит порт 9099 и делает следующий снимок невозможным.
cleanup() {
  if [ -n "$pane_pid" ]; then
    kill -TERM "$pane_pid" 2>/dev/null || true
    for _ in $(seq 1 20); do
      kill -0 "$pane_pid" 2>/dev/null || break
      sleep 0.2
    done
    kill -KILL "$pane_pid" 2>/dev/null || true
  fi
  tmux kill-session -t "$session" 2>/dev/null || true
}
trap cleanup EXIT

tmux new-session -d -s "$session" -x "$COLS" -y "$ROWS" "$BIN run"
pane_pid="$(tmux list-panes -t "$session" -F '#{pane_pid}' 2>/dev/null | head -1)"

# Ждём первый кадр, а не фиксированную задержку: шапка появляется сразу после
# первого такта сбора.
for _ in $(seq 1 50); do
  if tmux capture-pane -p -t "$session" 2>/dev/null | grep -q 'U L S E\|^PULSE '; then
    break
  fi
  sleep 0.2
done

sleep "$WARMUP"

if [ -n "$KEYS" ]; then
  tmux send-keys -t "$session" -l "$KEYS"
  sleep 1.5
fi

if [ "$WITH_ESCAPES" = "1" ]; then
  tmux capture-pane -p -e -t "$session"
else
  tmux capture-pane -p -t "$session"
fi

# Штатный выход даёт TUI восстановить терминал; если он не сработал, cleanup
# добьёт процесс по PID.
tmux send-keys -t "$session" -l 'q' 2>/dev/null || true
for _ in $(seq 1 15); do
  [ -n "$pane_pid" ] && kill -0 "$pane_pid" 2>/dev/null || break
  sleep 0.2
done
