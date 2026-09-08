# PR-01 — Verification baseline и RED-test infrastructure для Pulse

Репозиторий: `Stolyarovmn/Pulse`  
Audit baseline: `3b79b43db0f73e9bce819cbe6be90915f26bb165`  
Рабочая ветка: создай отдельную ветку, например `audit/pr01-verification-baseline`.  
Не коммить напрямую в `main`.

## Цель

Сделать первую инфраструктурную PR из аудита Pulse:

1. восстановить зелёный, воспроизводимый CI на заявленном MSRV;
2. добавить test-support для детерминированного моделирования `/proc`, `/sys`, cgroup и races;
3. добавить property-test infrastructure;
4. зафиксировать первые подтверждённые audit findings как **RED reproducers**, не исправляя сами дефекты;
5. синхронизировать локальную verification-команду и GitHub CI;
6. документировать политику: finding закрывается только после promotion RED test → обычный regression test.

Это **не PR с исправлениями продукта**. Нельзя по пути переписывать temporal model, rules, history, exporter, TUI или collector semantics.

---

# 1. Жёсткие ограничения scope

## Разрешено

- `Cargo.toml` / `Cargo.lock` — только чтобы:
  - сохранить заявленный MSRV Rust 1.85;
  - добавить dev/test dependencies;
  - убрать текущую несовместимость lockfile с MSRV минимальным dependency-resolution изменением.
- `.github/workflows/ci.yml` — verification commands и deterministic toolchain.
- `scripts/wsl-verify.sh` — привести локальную проверку к CI.
- test-only код под `#[cfg(test)]`.
- новые unit/property tests.
- новые scripts/docs для RED reproducers.
- behavior-neutral test seams, только если они не попадают в production code path.

## Запрещено

- повышать `rust-version`, чтобы обойти текущий MSRV defect;
- чинить PULSE findings в production logic;
- менять runtime semantics;
- менять форматы Snapshot/Event/Entity ради удобства тестов;
- менять rule thresholds;
- менять exporter output;
- менять TUI UX;
- делать broad dependency upgrade;
- отключать lint/test, чтобы получить green;
- превращать failing reproducer в “assert current broken behavior”;
- удалять/ослаблять существующие проверки.

Если конкретный finding нельзя воспроизвести без изменения production API — пометь его `BLOCKED_BY_DESIGN` в документе RED tests и не исправляй API в этой PR.

---

# 2. Сначала восстанови CI/MSRV baseline

Сейчас workspace декларирует:

```toml
rust-version = "1.85"
```

и CI использует Rust 1.85. Текущий frozen lockfile ранее падал из-за транзитивных packages, требующих Rust 1.88.

## Требование

**Сохранить MSRV 1.85.**

Сначала воспроизведи dependency chain:

```bash
rustup toolchain install 1.85.1 --profile minimal --component rustfmt --component clippy

cargo +1.85.1 tree
cargo +1.85.1 tree -i darling
cargo +1.85.1 tree -i instability
```

Фактические package names/version могут отличаться после разрешения lockfile — не угадывай. Найди точную цепочку.

Затем минимально зафиксируй совместимые transitive versions или ближайшую совместимую direct dependency. Не делай широкого `cargo update`.

После исправления baseline обязательно:

```bash
cargo +1.85.1 fmt --all -- --check
cargo +1.85.1 check --workspace --all-targets --locked
cargo +1.85.1 clippy --workspace --all-targets --locked -- -D warnings
cargo +1.85.1 test --workspace --locked
cargo +1.85.1 build --release --locked -p pulse-cli
```

Все пять команд должны завершаться `0`.

Если сохранение MSRV 1.85 невозможно без существенного downgrade с изменением API/behavior — остановись и отчитай точную dependency chain. Не повышай MSRV самостоятельно.

---

# 3. Сделай CI deterministic

Обнови `.github/workflows/ci.yml`.

Минимальный required pipeline:

```text
Formatting
Check
Clippy
Tests
Release build
```

Использовать exact toolchain `1.85.1`.

Команды:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked -p pulse-cli
```

Добавь:

```yaml
env:
  PROPTEST_CASES: 256
```

Не добавляй пока privileged/live Linux jobs в эту PR.

Приведи `scripts/wsl-verify.sh` к тем же `--locked` semantics. Локальная проверка и CI не должны расходиться.

---

# 4. Test-support для FsSource

Существующий `FixtureFs` уже умеет:

- in-memory files;
- links;
- inode;
- statfs;
- `PermissionDenied`;
- `vanishing()`.

Не переписывай его.

Добавь test-only support, предпочтительно:

```text
crates/pulse-collect/src/test_support.rs
```

и в `lib.rs`:

```rust
#[cfg(test)]
pub(crate) mod test_support;
```

## 4.1 CountingFs

Нужен wrapper над `Arc<dyn FsSource>`:

```rust
#[derive(Debug, Default, Clone)]
pub(crate) struct FsCallCounts {
    pub read: u64,
    pub read_dir: u64,
    pub read_link: u64,
    pub inode: u64,
    pub statfs: u64,
}

#[derive(Debug)]
pub(crate) struct CountingFs {
    inner: Arc<dyn FsSource>,
    ...
}
```

Требования:

- thread-safe;
- `FsSource`;
- считает каждый вызов каждого метода;
- умеет вернуть snapshot counters;
- умеет reset;
- по возможности ведёт bounded call log:
  - operation;
  - normalized path;
  - cap для `read`.
- сам call log тоже должен иметь test-only hard bound, например 100_000 entries, чтобы тестовая утилита не была unbounded.

Это будет использоваться позднее для PULSE-071/PULSE-080.

## 4.2 ScriptedFs

Нужен deterministic source, способный менять ответ между вызовами одного path.

Минимальный API по смыслу:

```rust
enum ScriptedRead {
    Bytes(Vec<u8>),
    NotFound,
    PermissionDenied,
    Io(ErrorKind),
}
```

Пример желаемого теста:

```rust
ScriptedFs::new(base)
    .read_sequence(
        "/proc/123/stat",
        [
            ok(old_stat),
            ok(old_stat),
            ok(new_stat),
        ],
    )
```

После исчерпания sequence поведение должно быть явно определено:
- либо repeat last;
- либо fallback в base.

Выбери один вариант и задокументируй. Не делай nondeterministic behavior.

Нужны scripted варианты хотя бы для:
- `read`;
- `read_link`;
- `read_dir`.

Это позволит позже моделировать PID reuse, disappearance и partial collection без sleeps/races.

---

# 5. Property-test infrastructure

Добавь `proptest = "1"` как workspace dependency и только нужные `dev-dependencies`.

В этой PR достаточно property tests для уже существующих инвариантов, которые должны быть GREEN.

Минимум:

### PT-01 EntityId round-trip

```text
raw -> EntityId::from_u64(raw) -> as_u64() == raw
```

### PT-02 sanitize_display

Для произвольной Unicode String:

- function never panics;
- result valid UTF-8;
- result не содержит ESC;
- result не содержит C0/C1 controls, кроме тех случаев, которые контракт явно разрешает;
- result не содержит bidi override/isolate chars, которые sanitizer обещает удалять;
- output bounded действующим лимитом.

### PT-03 Timestamp arithmetic

Для произвольных `u64`:
- saturating add/sub не wrap;
- `within(a,b)` корректен для `a <= b`.

### PT-04 Hot non-finite rejection

Для `NaN/+Inf/-Inf`:
- series не создаётся;
- retained_points не растёт.

Не добавляй property tests, требующие новой production semantics, в required GREEN suite.

---

# 6. RED reproducer policy

Новые тесты, которые **правильно описывают желаемый контракт, но падают на frozen baseline**, должны:

```rust
#[test]
#[ignore = "audit RED: PULSE-xxx — promote when remediation lands"]
fn audit_red_...() {
    ...
}
```

Они обязаны:
- компилироваться обычным `cargo test`;
- не выполняться required CI;
- при ручном запуске падать **именно по ожидаемой причине**;
- не утверждать broken behavior как правильный;
- содержать finding ID в имени/comment.

Создай:

```text
docs/VERIFICATION_RED.md
scripts/audit-red.sh
```

## `docs/VERIFICATION_RED.md`

Таблица:

```text
test name
finding ID
source crate
expected failure on 3b79b43
observed failure
promotion PR
status = RED | BLOCKED_BY_DESIGN | PROMOTED
```

## `scripts/audit-red.sh`

Скрипт запускает каждый RED test отдельно:

```bash
cargo test -p <crate> <exact-test-name> --locked -- --ignored --exact
```

Логика скрипта:

- test FAILED ожидаемым assert/panic → PASS для audit-red harness;
- test unexpectedly PASSED → script FAILS с сообщением:
  `finding may be fixed; promote this test to normal regression suite`;
- test не найден / compile error / infrastructure error → script FAILS;
- не маскировать неожиданную ошибку как “expected failure”.

Для expected panic PULSE-043 желательно проверять exit/result и характер failure, а не только любой non-zero.

---

# 7. Первые RED reproducers

Реализуй столько из этого списка, сколько можно сделать без production redesign. Минимум **10 реально воспроизводимых**.

## RED-01 — PULSE-043
### `Labels::set` Unicode boundary panic

Расположение:
`pulse-core`, рядом с `Labels`.

Сконструировать String >512 bytes так, чтобы byte offset 512 попадал внутрь multi-byte UTF-8 char.

Желаемый contract:

```text
Labels::set никогда не panic;
result valid UTF-8;
value <= MAX_VALUE_LEN bytes;
truncation happens on char boundary.
```

На frozen baseline ожидается panic из `String::truncate(512)`.

---

## RED-02 — PULSE-048
### stale History::latest

Расположение:
`pulse-store`.

1. tick N: sample `CG_MEM_UTIL = 0.9`;
2. следующие ticks entity остаётся alive, metric больше не sample;
3. вызвать `History::latest()`.

Желаемый contract:

```text
старое значение не возвращается как current без freshness metadata.
```

Если будущая модель будет хранить `Fresh/Stale`, test promotion можно адаптировать, но сейчас test должен фиксировать semantic defect.

---

## RED-03 — PULSE-044
### historical rename

1. entity имеет name `old` at A;
2. позже та же EntityId становится `new`;
3. `entities_at(A)`.

Желаемый result: `old`.

Frozen baseline ожидаемо возвращает latest metadata.

---

## RED-04 — PULSE-044
### historical reparent

1. same EntityId parent=P1 at A;
2. reparent to P2;
3. `entities_at(A)` и `entities_at(B)`.

Желаемо:
- A → P1;
- B → P2.

---

## RED-05 — PULSE-035 / HIST-001
### query crossing warm + hot

Заполнить history достаточно долго, чтобы начало requested window было warm, конец hot.

Желаемо:
- `History::series/window` покрывает весь requested interval;
- наличие hot points не должно выкидывать warm prefix.

Frozen baseline возвращает только hot points, если они есть.

---

## RED-06 — PULSE-039 / HIST-002
### warm counter rate

Counter sequence с неравномерным ростом и/или reset внутри warm bucket.

Желаемый oracle:

```text
sum(positive counter increments) / real observed elapsed
```

Не использовать bucket mean как counter point.

Frozen baseline должен расходиться с oracle.

---

## RED-07 — PULSE-045
### open problem remains visible in hysteresis neutral zone

Sequence:

```text
warn threshold exceeded enough ticks -> ProblemOpened
then value enters clear..warn dead zone
```

Желаемо:
- problem остаётся materialized/open;
- нет ProblemClosed;
- повторный enter не создаёт второй ProblemOpened.

Frozen baseline ожидаемо теряет problem из returned Vec while internal state stays open.

---

## RED-08 — PULSE-023
### reject `crit < warn`

Сконструировать Rules config:

```text
clear < crit < warn
```

Желаемо: `Config::validate()` rejects.

Frozen baseline пропускает.

---

## RED-09 — PULSE-081
### reject unusable zero operational limits

Отдельные cases:

```text
export.rate_limit_per_minute = 0
export.max_series = 0
store.max_series = 0
```

Желаемо: validation rejects либо существует явно документированная disable semantics.

Для этой PR зафиксировать желаемый contract как **reject**.

---

## RED-10 — PULSE-083
### `redact_high_entropy=true` реально что-то делает

Сконструировать argv с token-like random/high-entropy string, который не ловится текущим key-based masking.

Сравнить:

```text
redact_high_entropy=false
redact_high_entropy=true
```

Желаемо: true скрывает дополнительный secret.

Если текущий API вообще не передаёт flag в `redact_argv`, reproducer может быть на уровне ProcessCollector config → resulting `cmdline` label.

Не меняй production API ради теста. Если невозможно — `BLOCKED_BY_DESIGN`.

---

## RED-11 — PULSE-084
### HostCollector reports missing critical source

Fixture:
- `/proc/stat` отсутствует;
- остальные минимальные files присутствуют.

Желаемо:
- collection result сообщает partial/critical failure через существующий error/event mechanism;
- отсутствие `/proc/stat` не является silent `Ok`.

На baseline ожидается silent success.

---

## RED-12 — PULSE-087
### Labels trust boundary removes terminal controls

В `Labels::set` положить:

```text
"safe\x1b]8;;https://evil\aLINK\x1b]8;;\a"
```

и C0/C1/bidi cases.

Желаемо:
- trusted Label value terminal-safe.

Baseline ожидаемо хранит control sequences.

Этот test может пересекаться с RED-01; держать отдельным, потому что один проверяет UTF-8 truncation safety, второй trust-boundary sanitation.

---

## RED-13 — PULSE-059
### Problem events preserve entity identity

Открыть rule problem для известной entity.

Желаемо:

```rust
event.entity == Some(target)
event.entity_kind == Some(target_kind)
event.rule == Some(rule)
```

Baseline ProblemOpened/ProblemClosed теряют entity identity.

---

## RED-14 — PULSE-070
### process export keeps incarnation identity

Создать два Process Entity с:

```text
pid = 123
start_ticks = 100
pid = 123
start_ticks = 200
```

Отрендерить opt-in process metrics.

Желаемо: две разные OpenMetrics label identities.

Baseline, где exporter оставляет только `pid`, должен создавать одинаковый label identity.

---

## RED-15 — PULSE-038
### `History::oldest()` reflects full retained history

Набить history так, чтобы warm horizon был старше hot.

Желаемо:

```text
oldest <= oldest warm retained observation/bucket
```

Baseline возвращает только hot oldest.

---

# 8. RED tests, которые НЕ надо насильно делать в PR-01

Оставить `BLOCKED_BY_DESIGN`/`LIVE_REQUIRED`, если нет чистого reproducer без production redesign:

- PULSE-086 target network namespace ports;
- pidfd TOCTOU between validation and `kill`;
- true bounded `read_dir()` work — текущий `FsSource` materializes Vec before caller can stop;
- slow TUI read-lock blocks writer, если нужен runtime seam;
- real page-size != 4096 — обычный GitHub x86_64 может иметь 4096 и не воспроизвести defect;
- terminal loss / raw mode cleanup.

Для них только подготовить test IDs/placeholders в `docs/VERIFICATION_RED.md`, **не делать fake tests**.

---

# 9. Никаких “expected broken behavior” tests

Неправильно:

```rust
assert_eq!(history.latest().get(...), Some(0.9)); // фиксирует баг как контракт
```

Правильно:

```rust
#[ignore = "audit RED: PULSE-048"]
fn audit_red_stale_value_is_not_current() {
    ...
    assert!(latest.get(...).is_none());
}
```

Test должен описывать будущее правильное поведение и поэтому быть RED на baseline.

---

# 10. Проверка RED harness

После реализации выполнить каждый reproducer вручную.

Для каждого записать в `docs/VERIFICATION_RED.md`:

```text
PULSE-043
command:
cargo +1.85.1 test -p pulse-core audit_red_labels_unicode_boundary --locked -- --ignored --exact

baseline:
FAIL

failure:
thread panicked at String::truncate: byte index 512 is not a char boundary

status:
RED
```

Не придумывать output. Записывать фактический.

Если test неожиданно проходит:
- не ломать его искусственно;
- проверить finding;
- отметить `UNEXPECTED_GREEN`;
- приложить объяснение.

---

# 11. Acceptance criteria PR-01

PR готова только если:

```text
[ ] rust-version остаётся 1.85
[ ] CI toolchain pinned to 1.85.1
[ ] Cargo.lock совместим с 1.85.1
[ ] fmt passes
[ ] check --locked passes
[ ] clippy --locked -D warnings passes
[ ] normal cargo test --workspace --locked passes
[ ] release build --locked passes
[ ] proptest infrastructure работает
[ ] CountingFs работает и покрыт unit test
[ ] ScriptedFs работает и покрыт unit test
[ ] >=10 audit RED reproducers компилируются и реально падают на baseline
[ ] normal CI их не запускает
[ ] scripts/audit-red.sh проверяет expected-red корректно
[ ] docs/VERIFICATION_RED.md содержит фактические results
[ ] production behavior findings не исправлялись
[ ] нет broad dependency upgrade
```

---

# 12. Рекомендуемые commits

Сделай небольшими логическими commits:

```text
test: restore Rust 1.85 verification baseline
test: add FsSource counting and scripted test support
test: add property-test infrastructure
test: capture initial audit findings as RED reproducers
ci: align locked verification commands
docs: document audit RED promotion workflow
```

Если dependency/MSRV resolution требует отдельного объяснения — отдельный commit.

---

# 13. Что написать в PR description

Заголовок:

```text
test: establish audit verification baseline
```

Описание:

```text
## Scope
Infrastructure only. No production finding remediation.

## Baseline
3b79b43db0f73e9bce819cbe6be90915f26bb165

## Added
- deterministic Rust 1.85.1 verification
- CountingFs / ScriptedFs
- proptest support
- ignored RED reproducers
- audit-red runner
- RED promotion policy

## RED findings captured
<таблица finding → test → observed failure>

## MSRV resolution
<точная dependency chain и что было pinned/downgraded>

## Verification
<команды и реальные результаты>

## Explicitly not fixed
<список findings, которые остаются RED>

## Follow-up
PR-02 Observation quality/freshness
```

---

# 14. Финальный отчёт агенту

После выполнения не отвечай просто “готово”.

Верни:

1. branch name;
2. commit SHA(s);
3. полный список изменённых файлов;
4. точную dependency/MSRV проблему и resolution;
5. результаты пяти required commands;
6. таблицу RED reproducers:
   - finding;
   - test name;
   - actual baseline failure;
   - status;
7. список `BLOCKED_BY_DESIGN/LIVE_REQUIRED`;
8. подтверждение, что production behavior не менялось;
9. PR URL, если PR был создан.

Если required CI не green — PR не считать завершённой.
