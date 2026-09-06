#!/usr/bin/env bash
# Проходит цепочку расследования в живом TUI и отдаёт кадр каждого шага.
#
# Причина существования: проверка Inspector'а вручную - это каждый раз одна и
# та же последовательность (tmux, экран Entities, поиск по имени, N нажатий
# Enter, capture-pane). Она набиралась заново в каждой сессии, ошибалась в
# кавычках и оставляла живые агенты, державшие порт 9099.
#
# Использование:
#   scripts/capture-inspector.sh angie 3          # поиск «angie», три спуска
#   scripts/capture-inspector.sh pulse 1 -w 10    # дольше ждать первый кадр
#   scripts/capture-inspector.sh angie 2 -t       # переключить на RELATED
#
# Флаги:
#   -b PATH   бинарник (dist/pulse)
#   -c N      колонок (150)
#   -r N      строк (44)
#   -w SEC    ждать первый такт (8)
#   -t        нажать Tab перед последним Enter: проверка бокового перехода
#   -a        печатать кадр после каждого шага, а не только итоговый
set -euo pipefail

BIN="dist/pulse"
COLS=150
ROWS=44
WARMUP=8
USE_TAB=0
ALL_STEPS=0

query="${1:?нужно имя для поиска, например angie}"
depth="${2:-1}"
shift 2 || true

while getopts "b:c:r:w:ta" opt; do
  case "$opt" in
    b) BIN="$OPTARG" ;;
    c) COLS="$OPTARG" ;;
    r) ROWS="$OPTARG" ;;
    w) WARMUP="$OPTARG" ;;
    t) USE_TAB=1 ;;
    a) ALL_STEPS=1 ;;
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

# Порт экспортёра один на хост: чужой живой агент не даст запустить свой.
if ss -ltn 2>/dev/null | grep -q '127.0.0.1:9099'; then
  echo "порт 9099 занят: агент уже запущен" >&2
  exit 1
fi

session="pulse-inspect-$$"
pane_pid=""

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

for _ in $(seq 1 50); do
  if tmux capture-pane -p -t "$session" 2>/dev/null | grep -q 'U L S E'; then
    break
  fi
  sleep 0.2
done
sleep "$WARMUP"

# Экран Entities и поиск: цепочка всегда начинается с конкретного объекта.
tmux send-keys -t "$session" -l "3"
sleep 1
tmux send-keys -t "$session" -l "/$query"
sleep 1
tmux send-keys -t "$session" Enter
sleep 2

for step in $(seq 1 "$depth"); do
  if [ "$USE_TAB" = "1" ] && [ "$step" = "$depth" ]; then
    tmux send-keys -t "$session" Tab
    sleep 1
  fi
  tmux send-keys -t "$session" Enter
  sleep 2
  if [ "$ALL_STEPS" = "1" ]; then
    printf '\n=== шаг %s ===\n' "$step"
    tmux capture-pane -p -t "$session"
  fi
done

if [ "$ALL_STEPS" != "1" ]; then
  tmux capture-pane -p -t "$session"
fi
