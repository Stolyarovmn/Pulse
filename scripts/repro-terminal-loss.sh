#!/usr/bin/env bash
# Воспроизводит потерю терминала под живым агентом и проверяет, что процесс
# завершается, а не остаётся крутиться и держать порт экспортёра.
#
# Причина существования: дефект наблюдался на живом хосте (процесс жил на 100%
# ядра и игнорировал kill), а закрытая труба и даже закрытый мастер pty его не
# воспроизводят. Здесь тот же путь, что у оператора: tmux, затем смерть сервера
# tmux под работающим интерфейсом.
#
# Ключевая тонкость: маску для pgrep нужно передавать одним аргументом,
# иначе pgrep получает два шаблона и ничего не находит - тогда проверка
# «жив ли процесс» молча превращается в проверку пустой строки и всегда
# рапортует успех.
set -euo pipefail

BIN="${1:-dist/pulse}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ ! -x "$BIN" ]; then
  echo "нет исполняемого бинарника: $BIN" >&2
  exit 1
fi

session="pulse-repro-$$"
tmux new-session -d -s "$session" -x 120 -y 30 "$BIN run"
sleep 6

pid="$(pgrep -f "$BIN run" | head -1 || true)"
if [ -z "$pid" ]; then
  echo "ВЕРДИКТ: агент не запустился - воспроизведение невозможно" >&2
  tmux kill-session -t "$session" 2>/dev/null || true
  exit 2
fi
echo "агент запущен: pid=$pid"

# Терминал исчезает целиком: сервер tmux умирает вместе с мастером pty.
tmux kill-server 2>/dev/null || true
sleep 3

if ! kill -0 "$pid" 2>/dev/null; then
  echo "ВЕРДИКТ: агент вышел сам после потери терминала"
  exit 0
fi

echo "агент жив после потери терминала, состояние: $(awk '/^State/{print $2, $3}' "/proc/$pid/status")"
kill -TERM "$pid" 2>/dev/null || true

for _ in $(seq 1 40); do
  kill -0 "$pid" 2>/dev/null || { echo "ВЕРДИКТ: агент вышел по SIGTERM"; exit 0; }
  sleep 0.25
done

echo "ВЕРДИКТ: ДЕФЕКТ - агент игнорирует SIGTERM без терминала" >&2
bash "$repo_root/scripts/diag-stuck.sh" "$pid" 1 >&2 || true
kill -KILL "$pid" 2>/dev/null || true
exit 1
