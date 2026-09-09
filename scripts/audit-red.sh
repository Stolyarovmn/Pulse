#!/usr/bin/env bash
# Проверка RED-репродьюсеров аудита.
#
# Смысл обратный обычному прогону: эти тесты описывают контракт, который
# текущая реализация нарушает, поэтому падение — ожидаемый результат, а
# неожиданный успех означает, что дефект исправлен и тест пора переводить
# в обязательный регресс (снять `#[ignore]`).
#
# Скрипт различает три исхода:
#   FAILED  → PASS для harness: дефект воспроизведён;
#   ok      → FAIL: finding, судя по всему, исправлен;
#   не найден/не собрался → FAIL: сломана сама проверка, маскировать нельзя.
#
# Использование:
#   bash scripts/audit-red.sh            # все RED-репродьюсеры
#   bash scripts/audit-red.sh pulse-core # только один крейт
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo_sh="$repo_root/scripts/wsl-cargo.sh"
filter="${1:-}"

# Крейт и точное имя теста. Список ведётся вместе с docs/VERIFICATION_RED.md.
# Продвинутые тесты (дефект исправлен) здесь не перечисляются: они уходят
# в обязательный набор.
#
# Список пуст: все воспроизведённые дефекты аудита исправлены и продвинуты в
# регресс. Пустой прогон обязан быть успешным, иначе harness мешал бы CI.
reproducers=()
expected_red=0
unexpected_green=0
broken=0

for entry in "${reproducers[@]}"; do
    read -r crate test finding <<<"$entry"
    if [ -n "$filter" ] && [ "$filter" != "$crate" ]; then
        continue
    fi

    output="$(bash "$cargo_sh" test -p "$crate" --test audit_red "$test" --locked -- --ignored --exact 2>&1)"

    # Различать падение теста и поломку проверки. `error: test failed` —
    # нормальный итог упавшего теста, а ошибка компиляции или отсутствующий
    # тест означают, что доказательство дефекта не получено.
    if printf '%s' "$output" | grep -qE "^error\[|^error: could not compile|^error: no test target|^error: Unrecognized option"; then
        printf 'BROKEN         %-12s %-58s %s\n' "$crate" "$test" "$finding"
        printf '%s\n' "$output" | grep -E '^error' | head -3
        broken=$((broken + 1))
        continue
    fi

    if printf '%s' "$output" | grep -qE "^test $test \.\.\. FAILED"; then
        reason="$(printf '%s' "$output" | grep -A1 "panicked at" | tail -1 | cut -c1-96)"
        printf 'RED            %-12s %-58s %s\n' "$crate" "$test" "$finding"
        [ -n "$reason" ] && printf '               причина: %s\n' "$reason"
        expected_red=$((expected_red + 1))
        continue
    fi

    if printf '%s' "$output" | grep -qE "^test $test \.\.\. ok"; then
        printf 'GREEN          %-12s %-58s %s\n' "$crate" "$test" "$finding"
        printf '               finding may be fixed; promote this test to normal regression suite\n'
        unexpected_green=$((unexpected_green + 1))
        continue
    fi

    printf 'NOT RUN        %-12s %-58s %s\n' "$crate" "$test" "$finding"
    broken=$((broken + 1))
done

printf '\nвоспроизведено: %d, неожиданно зелёных: %d, сломанных проверок: %d\n' \
    "$expected_red" "$unexpected_green" "$broken"

# Пустой список — успех: все дефекты исправлены и продвинуты в регресс.
# Условие `expected_red -eq 0` при непустом списке означает другое: ни один
# репродьюсер не сработал, то есть harness перестал доказывать дефекты.
if [ "$unexpected_green" -gt 0 ] || [ "$broken" -gt 0 ]; then
    exit 1
fi
if [ "${#reproducers[@]}" -gt 0 ] && [ "$expected_red" -eq 0 ]; then
    exit 1
fi
