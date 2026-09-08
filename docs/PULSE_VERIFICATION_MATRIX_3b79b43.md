# Pulse — Verification Matrix

**Audit baseline:** `3b79b43db0f73e9bce819cbe6be90915f26bb165`  
**Purpose:** превратить статический аудит Pulse в исполняемые, воспроизводимые контракты.  
**Policy:** сначала пишется regression test, который воспроизводит найденный дефект на baseline; затем исправление; PR считается закрывающим finding только когда тест остаётся в постоянном наборе.

## Статусы

- **RED** — по статическому анализу текущий код должен нарушить этот контракт.
- **PARTLY COVERED / COVERED** — часть контракта уже тестируется; матрица фиксирует требуемый полный вариант.
- **UNKNOWN** — нужен реальный запуск.
- **NEW / NEW CONTRACT / DESIGN** — тест определяет новый инвариант; текущая модель может потребовать архитектурного изменения.

## Приоритеты и gates

- **P0 / PR** — блокирует merge после появления соответствующего исправления.
- **P0 / nightly** — обязательный Linux/privileged gate до release; не должен быть flaky.
- **P1** — обязательный до 1.0.
- **P2** — hardening/portability.
- **release** — блокирует выпуск артефакта.

## Рекомендуемая test topology

```text
crates/pulse-core/tests/
  entity_model.rs
  config_contracts.rs
  redaction_properties.rs

crates/pulse-collect/tests/
  fixture_contracts.rs
  bounded_work.rs
  linux_oracles.rs          # #[ignore] / Linux-only where needed

crates/pulse-store/tests/
  temporal_contracts.rs
  history_properties.rs
  memory_contracts.rs

crates/pulse-engine/tests/
  analyzer_state_machine.rs
  diff_properties.rs

crates/pulse-export/tests/
  openmetrics_contracts.rs
  http_contracts.rs

crates/pulse-tui/tests/
  render_contracts.rs
  navigation_properties.rs

tests/linux/
  netns-process-ports.sh
  process-churn.sh
  cgroup-churn.sh
  netif-churn.sh
  permission-degradation.sh
  soak-memory.sh

fuzz/
  fuzz_targets/*.rs
```

Для unit tests уже есть хорошая точка внедрения: `FsSource`/`FixtureFs`. Добавить `CountingFs`/`ScriptedFs`, которые считают `read`, `read_dir`, `inode`, `read_link` и умеют менять ответ после N-го вызова. Это позволит детерминированно воспроизводить race и work-budget defects без реального `/proc`.

## Минимальные новые test dependencies

```toml
[workspace.dependencies]
proptest = "1"
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

Подключать их как `dev-dependencies` только в нужных crates. Для fuzz — отдельный стандартный `cargo fuzz init`; fuzz targets не входят в обычный dependency graph release-бинарника.

---

## 0. Базовый CI и контракт сборки

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| VER-001 | P0 | PR | static/CI | workspace | Запуск exact MSRV: `cargo +1.85 test --workspace --locked` и `cargo +1.85 clippy --workspace --all-targets -- -D warnings`. | Обе команды завершаются 0; Cargo.lock совместим с rust-version=1.85. | PULSE-001 | RED/UNKNOWN |
| VER-002 | P0 | PR | static/CI | workspace | `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --all-features`. | Нет format drift; все targets/features компилируются. | quality gate | UNKNOWN |
| VER-003 | P0 | PR | static/CI | workspace | Проверка `unsafe`, `todo!`, `unimplemented!`, production `unwrap/expect/panic` политиками workspace. | Новый production violation ломает CI. | CONTRACTS | PARTIAL |
| VER-004 | P1 | PR | contract | metric registry | Для каждого `MetricDesc` существует producer либо явная аннотация `DerivedOnly/NeverProduced`. | Нет ghost metrics без явного статуса. | PULSE-077 | RED |
| VER-005 | P1 | release | supply-chain | `cargo tree -d`, `cargo deny check`, advisory scan, лицензии. | Нет запрещённых sources/licenses/advisories; дубликаты обоснованы. | supply-chain | NOT IMPLEMENTED |

## 1. Freshness / completeness / качество наблюдения

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| OBS-001 | P0 | PR | unit+property | pulse-store/runtime | Серия была в tick N; в N+1 collector не публикует её. | Latest/Snapshot не может выдать старое значение как текущее без `age/freshness` или `unknown`. | PULSE-048 | RED |
| OBS-002 | P0 | PR | unit | HostCollector | После успешного tick убрать `/proc/stat` из FixtureFs. | Collector возвращает/репортит partial failure; CPU не выглядит свежим. | PULSE-084 | RED |
| OBS-003 | P0 | PR | unit | HostCollector | Одновременно убрать `/proc/stat`, `/proc/meminfo`, PSI и misc files. | `collector_errors`/CollectorError отражают потерю критического источника; health degraded. | PULSE-084 | RED |
| OBS-004 | P0 | PR | unit | rules | Метрика отсутствует в одном/нескольких tick во время открытой проблемы. | Missing observation не засчитывается как `clear` и не обновляет evidence ложным 0. | root #3 | NEW CONTRACT |
| OBS-005 | P1 | PR | unit | collectors | Первый tick для rate/throughput/await. | Производная метрика отсутствует (`None/Unknown`), а не 0. | unknown-vs-zero | PARTLY COVERED |
| OBS-006 | P1 | PR | property | snapshot | Случайные последовательности observed/missing/recovered. | `freshness` монотонно соответствует последнему реальному observation; recovery сбрасывает age. | root #1 | NEW CONTRACT |
| OBS-007 | P1 | PR | integration | export/TUI | Сделать один критический collector unavailable. | TUI и `/healthz` показывают degraded/partial, а не healthy. | root #1 | NEW CONTRACT |
| OBS-008 | P1 | nightly | chaos | live Linux | Во время работы временно сделать источник недоступным/permission denied. | Нет stale-as-current; есть явное событие деградации и восстановления. | root #1 | UNKNOWN |

## 2. Temporal Entity Graph

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| GRAPH-001 | P0 | PR | unit | pulse-store/entities | Entity name A в t1, rename B в t2; запрос `entities_at(t1)`. | Возвращается имя A, а не последнее B. | temporal model | RED |
| GRAPH-002 | P0 | PR | unit | pulse-store/entities | Entity parent=P1 в t1, P2 в t2. | `entities_at(t1)` возвращает P1, `entities_at(t2)` — P2. | temporal model | RED |
| GRAPH-003 | P0 | PR | unit | pulse-store/entities | Label `disks=sda` в A и `disks=sdb` в B. | Исторический diff видит соответствующее значение в каждом моменте. | temporal model/resource evidence | RED |
| GRAPH-004 | P0 | PR | property/model | EntityGraph | Случайная последовательность create/touch/miss/delete/recreate. | Модель и граф согласны по alive interval, generation и событиям. | root #2 | NEW |
| GRAPH-005 | P1 | PR | unit | EntityGraph | Одна пропущенная инвентаризация. | Grace tick не создаёт Deleted. | graph grace | PARTLY COVERED |
| GRAPH-006 | P1 | PR | unit | EntityGraph | Две/достаточно пропущенных инвентаризации. | Deleted создаётся ровно один раз. | graph lifecycle | UNKNOWN |
| GRAPH-007 | P0 | PR | unit | process identity | Один PID с двумя `start_ticks`. | Получаются разные EntityId; старая ссылка не указывает на новый процесс. | identity | PARTLY COVERED |
| GRAPH-008 | P1 | PR | unit | cgroup identity | Удалить cgroup и создать новую с повторно выданным inode после удаления старого key. | Новая сущность получает новый generation/EntityId; old ID не оживает. | identity | UNKNOWN |
| GRAPH-009 | P1 | PR | unit | restart detection | Логическая entity исчезла/вернулась внутри и вне restart window. | Restarted только внутри окна и ровно один раз. | restart semantics | UNKNOWN |
| GRAPH-010 | P1 | PR | property | diff | Одинаковый набор записей вставлять в разном HashMap order. | Diff и evidence ordering byte-for-byte детерминированны. | determinism | NEW |
| GRAPH-011 | P1 | PR | unit | baseline | Первый inventory существующего хоста. | Не создаются ложные Created events. | baseline semantics | PARTLY COVERED |

## 3. Unknown / zero / unlimited семантика

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| SEM-001 | P0 | PR | unit | core domain | `cpu.max=max`, `memory.max=max`, `pids.max=max`. | Domain представляет `Unlimited`, а не перегруженный sentinel `0`. | root #3 | RED/DESIGN |
| SEM-002 | P0 | PR | unit | rules | Unlimited memory/cpu/pids + high current value. | Правила, требующие лимита, не трактуют 0 как реальный limit. | root #3 | UNKNOWN |
| SEM-003 | P0 | PR | unit | TUI rows | Missing CPU/MEM/IO sample. | UI печатает `—/unknown`, не уверенный `0`. | root #3 | LIKELY RED |
| SEM-004 | P0 | PR | unit | OpenMetrics | Missing current sample. | Exporter опускает sample/публикует quality metric, но не синтезирует 0. | root #3 | UNKNOWN |
| SEM-005 | P1 | PR | property | formatting | Случайные missing/zero/nonzero/unlimited values. | Форматтеры различают четыре состояния. | root #3 | NEW |
| SEM-006 | P1 | PR | unit | rules | Metric disappears while below clear threshold. | Отсутствие ≠ clear; hysteresis streak не двигается. | root #3/#5 | NEW |

## 4. Identity, process details и actions

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| ID-001 | P0 | PR | unit | actions | `(pid,start_ticks)` не совпадает. | Signal не отправляется; Reused error. | PID reuse | COVERED |
| ID-002 | P0 | PR | integration | actions | Процесс умирает после проверки identity и PID быстро переиспользуется. | После remediation сигнал адресуется pidfd/immutable handle, а не новому PID. | TOCTOU identity | NEW/REQUIRES DESIGN |
| ID-003 | P0 | PR | unit | ProcDetails | Передать ожидаемый `start_ticks`; заменить `/proc/<pid>/stat` между стадиями чтения. | Нельзя смешать status/exe/fd от двух разных процессов с одним PID. | root #4 | RED/DESIGN |
| ID-004 | P0 | PR | unit | ProcDetails | Процесс исчезает во время FD scan. | Result явно Gone/Partial, а не набор случайно смешанных пустых полей. | root #4 | NEW |
| ID-005 | P0 | nightly | live Linux netns | Процесс слушает порт внутри отдельного network namespace. | ProcDetails находит порт через `/proc/<pid>/net/*`. | PULSE-086 | RED |
| ID-006 | P1 | PR | unit | ProcDetails permissions | FixtureFs возвращает PermissionDenied на fd. | `restricted=true`, это не отображается как `0 files`. | permissions | COVERED |
| ID-007 | P1 | PR | unit | redaction | exe/cwd/fd target содержат ESC/OSC/control chars. | ProcessDetails содержит только terminal-safe строки. | security boundary | PARTLY COVERED |
| ID-008 | P1 | PR | unit | process owner relation | Process cgroup path меняется между reads. | Не создаётся relation к owner, не соответствующему identity этого process sample. | root #4 | NEW |

## 5. Rule engine и hysteresis

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| RULE-001 | P0 | PR | property/state-machine | Hysteresis | Генерировать последовательности enter/clear/neutral. | Переход Open происходит ровно на `enter_after_ticks`; Close — ровно на `clear_after_ticks`. | root #5 | PARTLY COVERED |
| RULE-002 | P0 | PR | unit | Analyzer | Open problem остаётся open, evidence меняется каждый tick. | Materialized Problem содержит последнее доказательство и корректный `last_seen/streak`. | root #5 | UNKNOWN |
| RULE-003 | P0 | PR | unit | Analyzer | Rule отсутствует в evaluation из-за missing source. | State не интерпретирует отсутствие hit как доказанный clear. | root #5 | LIKELY RED |
| RULE-004 | P1 | PR | unit | Analyzer | Severity warn→crit→warn при открытой problem. | Severity transitions соответствуют явной политике и не создают duplicate open. | root #5 | NEW |
| RULE-005 | P1 | PR | unit | OomRule | Counter reset/recreate without positive delta. | Нет ложного OOM. | counter reset | UNKNOWN |
| RULE-006 | P1 | PR | unit | disk latency | Большой await, но Δops=0/нет traffic. | Rule gated, проблема не открывается. | rule gate | PARTLY COVERED |
| RULE-007 | P1 | PR | unit | memory pressure | Memory current присутствует, limit unlimited. | Limit-pressure rule не активируется от sentinel. | root #3/#5 | UNKNOWN |
| RULE-008 | P1 | PR | unit | Analyzer GC | Entity исчезла и её state удалён. | Нет orphan hysteresis state; lifecycle problem закрывается/исчезает по контракту. | state GC | UNKNOWN |
| RULE-009 | P1 | property | RuleHit evidence | Случайные finite threshold/value pairs. | Evidence всегда содержит фактическое значение, threshold и корректную сторону сравнения. | evidence-first | NEW |
| RULE-010 | P1 | PR | unit | events | 100 ticks одной открытой problem. | Ровно 1 ProblemOpened и 0 повторных open до close. | event semantics | UNKNOWN |

## 6. History: hot/warm/rate/value_at

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| HIST-001 | P0 | PR | unit | History::series/window | Запрос окна пересекает warm и hot; hot содержит хотя бы одну точку. | Результат покрывает всё окно, а не только hot tail. | root #6 | RED |
| HIST-002 | P0 | PR | unit | History::rate | Counter хранится в warm buckets. | Rate выводится из first/last/delta semantics counter aggregate, не из bucket means. | root #6 | LIKELY RED |
| HIST-003 | P0 | PR | property | counter rate | Случайные монотонные counters + resets. | Rate = сумма положительных increments / реальный elapsed; никогда отрицательный. | counter semantics | PARTLY COVERED |
| HIST-004 | P1 | PR | unit | History::window | Окно целиком warm. | `approximate=true`; min/max/first/last/count корректны агрегатам. | warm semantics | PARTLY COVERED |
| HIST-005 | P0 | PR | unit | History::value_at | Warm bucket содержит observations до и после requested `at`. | Ни одно будущее observation не возвращается. | time travel | PARTLY COVERED |
| HIST-006 | P1 | PR | unit | staleness | Серия пропала > configured freshness window. | `value_at/latest` не делает старое значение текущим. | PULSE-048 | RED/DESIGN |
| HIST-007 | P1 | property | retention | Случайный churn series/entities/events при малых limits. | Все collections остаются bounded и индексы согласованы. | boundedness | NEW |
| HIST-008 | P1 | PR | unit | max_series | Достигнуть max_series, затем дать старым series выйти из hot. | Новые series снова допускаются после освобождения slots. | resource limit | UNKNOWN |
| HIST-009 | P0 | nightly | live memory | Большие entity labels/events + churn при `max_bytes=8MiB`. | Фактический RSS/history allocation соответствует объявленному бюджету в заданной tolerance; нет многократного превышения. | PULSE-082 | RED |
| HIST-010 | P1 | PR | unit | eviction+diff | Вытеснить dead entity/event нужные A/B diff. | Diff явно сообщает insufficient history, а не делает уверенный неполный вывод. | completeness | NEW |
| HIST-011 | P1 | PR | property | wall clock | Подать timestamps с backward step. | History либо rejects/normalizes non-monotonic wall time, либо использует tick axis; ordering не ломается. | clock correctness | NEW |
| HIST-012 | P1 | PR | unit | warm boundary | Observation ровно на границе bucket и query. | Нет double-count/skip между соседними buckets. | bucket boundary | UNKNOWN |

## 7. Work budgets и scalability

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| BUD-001 | P0 | PR | unit/instrumented | FsSource/ProcessCollector | CountingFs содержит 100k `/proc` entries, `max_processes=4096`. | Enumeration/read work ограничен O(limit), а не сначала materialize 100k. | PULSE-080 | RED |
| BUD-002 | P0 | PR | unit/instrumented | CgroupCollector | 100k synthetic cgroup children, `max_cgroups=N`. | `read_dir`/inode/read calls ограничены бюджетом; traversal прекращается рано. | PULSE-080 | RED |
| BUD-003 | P0 | PR | unit/instrumented | ProcDetails | 100k fd entries при `file_limit=12`. | Есть отдельный scan budget; syscall/read_link count bounded, не только output len. | PULSE-071 | RED |
| BUD-004 | P1 | PR | unit | net table | Socket table > cap, последняя строка обрезана. | Parser игнорирует partial line; не создаёт bogus port. | bounded input | UNKNOWN |
| BUD-005 | P1 | PR | unit | exporter max_series | Snapshot больше export max_series. | Ровно budget series + явный dropped counter; deterministic selection. | cardinality | UNKNOWN |
| BUD-006 | P0 | PR | unit | rate limiter | `rate_limit_per_minute=0`. | Config rejected; server не стартует в permanently-429 state. | PULSE-081 | RED |
| BUD-007 | P1 | nightly | perf | 10k processes | 30–60s run на dedicated host. | CPU/RSS/tick p95 не превышают зафиксированный budget. | performance | UNKNOWN |
| BUD-008 | P1 | nightly | perf | 4k/10k cgroups | Synthetic/live cgroup tree. | Tick p95 scales approximately linearly до budget и не уходит в quadratic behavior. | performance | UNKNOWN |
| BUD-009 | P1 | nightly | soak | 24h | Стабильный workload + churn. | RSS выходит на plateau; series/entity maps не растут монотонно. | leak/boundedness | UNKNOWN |

## 8. Runtime self-observability и concurrency

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| SELF-001 | P0 | PR | unit/integration | runtime tick | Искусственно задержать History::ingest/Snapshot::build. | `tick_duration_ms` включает collect+analyze+store+snapshot/publication. | PULSE-078 | RED |
| SELF-002 | P0 | PR | unit/integration | runtime scheduler | Collect=0.7 interval, store=0.5 interval. | `ticks_skipped` увеличивается, поскольку полный tick > interval. | PULSE-078 | RED |
| SELF-003 | P0 | PR | concurrency | TUI/history | Renderer блокируется 500ms при history trend request; writer пытается ingest. | Writer не ждёт terminal.draw; history lock отпущен до render. | PULSE-079 | RED |
| SELF-004 | P1 | PR | concurrency | HTTP/ArcSwap | Клиент держит медленный scrape/response. | Collector tick cadence не блокируется HTTP consumer. | runtime isolation | LIKELY GREEN |
| SELF-005 | P1 | PR | unit | RwLock poison | Паника reader/writer в controlled test. | Поведение соответствует policy: данные не выдаются как trusted если invariant мог быть нарушен. | reliability | UNKNOWN |
| SELF-006 | P0 | nightly | live | SIGTERM during collection | Послать SIGTERM в середине тяжёлого `/proc` scan. | Процесс выходит bounded time; terminal/server cleanup выполнен. | shutdown | UNKNOWN |
| SELF-007 | P0 | nightly | live TTY | terminal loss | Убить tmux/server/pty во время TUI. | Нет 100% spin; process exits or reconnect policy bounded. | terminal-loss | PARTLY SCRIPTED |
| SELF-008 | P1 | nightly | perf | self metrics oracle | Внешний `/proc/<pulse>/stat` и internal agent metrics. | Ошибка internal CPU/tick/RSS не выходит за заданную tolerance. | self-observability | UNKNOWN |

## 9. Config и security contracts

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| CFG-001 | P0 | PR | unit | Config::validate | Неизвестное поле TOML. | Parse/validation fails с понятным path. | config | LIKELY GREEN |
| CFG-002 | P0 | PR | unit | Config::validate | `export.rate_limit_per_minute=0`. | Rejected. | PULSE-081 | RED |
| CFG-003 | P0 | PR | unit | Config::validate | `export.max_series=0`. | Rejected или explicit documented disable semantics; никакого silent empty exporter. | PULSE-081 | RED |
| CFG-004 | P0 | PR | unit | Config::validate | `store.max_series=0`. | Rejected или explicit disable; history не silently dies. | PULSE-081 | RED |
| CFG-005 | P1 | property | Config | Генерировать boundary values для всех usize/u64 limits. | Каждое поле имеет однозначный min/max/zero contract; validate согласован с constructors. | PULSE-081 | NEW |
| CFG-006 | P1 | PR | unit | Config round-trip | Default + edge valid configs serialize/deserialize. | Semantics сохраняются; hidden coercions отсутствуют. | config | NEW |
| SEC-001 | P0 | PR | unit+property | redaction | `redact_high_entropy=true`, argv содержит случайный 32–64 byte token-like string. | High-entropy secret реально скрыт; false не включает эвристику. | PULSE-083 | RED |
| SEC-002 | P0 | PR | unit+property | Labels/EntitySpec | Label value содержит ESC, OSC 8, BEL, CR, tabs и oversized UTF-8. | Trusted graph не содержит terminal control sequences; truncation на char boundary. | PULSE-087 | RED |
| SEC-003 | P0 | PR | fuzz | sanitize_display/redact | Произвольные bytes/Unicode/control strings. | Output terminal-safe, bounded, valid UTF-8; never panic. | security boundary | NEW |
| SEC-004 | P0 | PR | unit | export bind/auth | Non-loopback bind without token. | Config validation fails. | network exposure | LIKELY GREEN |
| SEC-005 | P1 | PR | unit | token auth | Short/whitespace/incorrect/correct bearer tokens. | Only exact valid token accepted; no content-based early-return contract regression. | auth | COVERED |
| SEC-006 | P1 | PR | unit | OpenMetrics escaping | Entity/label contains quote, slash, newline/control input after sanitize. | Output parseable; labels cannot inject new sample lines. | export security | NEW |
| SEC-007 | P1 | PR | integration | actions permission | `allow_actions=false`, TUI/command tries signal path. | Action unavailable before kernel call; default cannot mutate host. | least privilege | UNKNOWN |
| SEC-008 | P1 | release | security | Security docs vs effective config/code. | Каждый advertised security control имеет executable regression test ID. | documentation integrity | NEW |

## 10. Linux kernel semantic oracle

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| LIN-001 | P0 | nightly | live oracle | SelfCollector/statm | Сравнить `AGENT_RSS` с `rss_pages * getconf PAGESIZE`. | Используется реальный page size, не hardcoded 4096. | Linux portability | LIKELY RED |
| LIN-002 | P0 | PR | property/parser | parse_proc_stat | `comm` содержит spaces, `)`, `(`, Unicode bytes; numeric tail valid. | pid/start/utime/stime извлекаются корректно или input safely rejected. | procfs parser | NEW |
| LIN-003 | P1 | nightly | live oracle | host CPU | Сравнить Pulse CPU delta с независимым `/proc/stat` oracle на 30 ticks. | Расхождение в tolerance; guest accounting не double-counted. | Linux semantics | UNKNOWN |
| LIN-004 | P1 | nightly | live oracle | memory | Сверить total/available/used со `/proc/meminfo`. | used=total-available; missing MemAvailable handled explicitly. | Linux semantics | UNKNOWN |
| LIN-005 | P1 | PR | unit/parser | PSI | Kernel-format `some/full avg10/60/300 total=µs`. | avg percent преобразован в ratio 0..1; total в seconds. | PSI | UNKNOWN |
| LIN-006 | P1 | nightly | live oracle | diskstats | Generate fio/dd workload, compare bytes/ops/await/util с `/proc/diskstats`/iostat window. | Формулы и 512-byte sectors совпадают в tolerance. | disk semantics | PARTLY COVERED |
| LIN-007 | P1 | nightly | live oracle | net/dev | Generate controlled traffic. | RX/TX deltas Pulse совпадают с target interface counters; interface churn не даёт negative spikes. | network semantics | UNKNOWN |
| LIN-008 | P0 | nightly | live netns | ProcDetails ports | Separate netns + TCP/UDP listeners. | Target PID ports видны независимо от Pulse netns. | PULSE-086 | RED |
| LIN-009 | P1 | nightly | live cgroup | cpu.max | Create cgroup with `200000 100000` and `max 100000`. | 2.0 cores и Unlimited различаются корректно. | cgroup v2 | UNKNOWN |
| LIN-010 | P1 | nightly | live cgroup | memory.max/current/events | Set finite/unlimited limits, induce high/max/OOM where safe. | Metrics map to kernel files with correct hierarchy semantics. | cgroup v2 | UNKNOWN |
| LIN-011 | P1 | nightly | live cgroup | pids.max | Set finite/max; create tasks. | Current/limit/utilization match kernel; unlimited not 0. | cgroup v2 | UNKNOWN |
| LIN-012 | P1 | nightly | live cgroup | PSI cgroup | Controlled contention. | Pulse PSI matches cgroup `*.pressure` semantics. | cgroup PSI | UNKNOWN |
| LIN-013 | P1 | PR | fixtures | owner parsing | systemd/docker/containerd/crio/podman/cgroupfs path corpus. | Owner/runtime/pod UID classification deterministic; malformed IDs rejected. | container identity | PARTLY COVERED |
| LIN-014 | P1 | nightly | live | hidepid/permissions | Run unprivileged with restricted `/proc`. | Collector degrades honestly; no panic; restricted != empty. | least privilege | UNKNOWN |
| LIN-015 | P2 | nightly | live WSL/container | sysfs absent | No `/sys/dev/block`, disk graph labels available. | cgroup io→disk fallback still resolves correctly. | portability | PARTLY COVERED |

## 11. TUI correctness и terminal safety

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| TUI-001 | P0 | PR | TestBackend | all screens | Render widths 1..240, heights 1..80 over representative snapshots. | Never panic/index OOB; no line exceeds area width. | TUI robustness | NEW |
| TUI-002 | P1 | property | layout | Random resize sequence + screen/navigation state. | Selected indexes/clamps always valid; no invisible actionable item. | TUI state | NEW |
| TUI-003 | P0 | PR | unit | investigation chain | Generate graph with cycles/resource-sharing. | Repeated Enter strictly descends; resources never become transit nodes; Esc reverses path. | navigation invariant | PARTLY COVERED |
| TUI-004 | P1 | PR | snapshot/golden | Overview/Problems/Entities/Timeline/Inspector | Fixed semantic snapshots render normalized text golden. | Visual contract drift becomes explicit review diff. | UX regression | NEW |
| TUI-005 | P0 | PR | security | render strings | Malicious names/details/events with terminal controls. | Rendered buffer contains no raw terminal control bytes. | PULSE-087 | NEW |
| TUI-006 | P1 | PR | unit | unknown formatting | Missing metric vs actual zero. | Different visible representation. | root #3 | NEW |
| TUI-007 | P1 | PR | unit | config wiring | Toggle `ui.show_self_metrics`. | TUI output/state changes according to documented option. | PULSE-085 | RED |
| TUI-008 | P0 | nightly | PTY | normal quit/SIGTERM/terminal loss. | Alternate screen/raw mode restored when restoration is possible; no stuck terminal/spin. | terminal safety | PARTLY SCRIPTED |
| TUI-009 | P1 | property | Unicode width | Wide CJK, emoji, combining marks, invalid-lossy names. | No mid-codepoint truncation/panic; visual width bounded. | rendering | NEW |

## 12. Exporter / OpenMetrics / health

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| EXP-001 | P0 | PR | unit | OpenMetrics | Render every descriptor kind. | HELP/TYPE/name/type/suffix contracts consistent; counters `_total`. | OpenMetrics | PARTLY COVERED |
| EXP-002 | P0 | PR | unit | escaping | Quotes/backslashes/newline-like label values. | Output passes parser/strict grammar and cannot inject metrics. | security/export | NEW |
| EXP-003 | P1 | PR | unit | process policy | Default config + process metrics present in snapshot. | Process series absent unless explicit opt-in. | privacy/cardinality | PARTLY COVERED |
| EXP-004 | P0 | PR | unit/integration | healthz | Critical collector stale/unavailable. | `/healthz` reflects freshness/completeness, not merely HTTP thread liveness. | root #1 | NEW |
| EXP-005 | P0 | PR | integration | auth+rate | External bind with token, valid/invalid requests, rate exhaustion/refill. | 401/200/429 semantics deterministic; rate=0 invalid config. | PULSE-081 | PARTLY COVERED |
| EXP-006 | P1 | PR | concurrency | scrapes | 20 concurrent/slow clients. | Snapshot serving remains bounded; collector not blocked. | runtime isolation | UNKNOWN |
| EXP-007 | P1 | PR | unit | cardinality | More entities than export budget. | Selection deterministic and dropped counter accurate. | cardinality | UNKNOWN |
| EXP-008 | P1 | release | external validator | metrics endpoint | Pipe sample into `promtool check metrics` or equivalent parser. | Strict validator accepts output. | OpenMetrics | NEW |

## 13. Fuzz targets

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| FZ-001 | P1 | nightly | cargo-fuzz | parse_proc_stat | Arbitrary bytes. | No panic/OOM; accepted result satisfies field invariants. | parser safety | NEW |
| FZ-002 | P1 | nightly | cargo-fuzz | parse_pressure | Arbitrary bytes. | No panic; finite ratios/totals only. | parser safety | NEW |
| FZ-003 | P1 | nightly | cargo-fuzz | parse_diskstats/net_dev | Arbitrary text lines. | No panic; accepted counters finite/nonnegative. | parser safety | NEW |
| FZ-004 | P1 | nightly | cargo-fuzz | parse_io_stat/cpu.max/limit | Arbitrary cgroup files. | No panic; max/unlimited represented explicitly after remediation. | parser safety | NEW |
| FZ-005 | P1 | nightly | cargo-fuzz | owner_from_segment/pod_uid | Arbitrary path segments. | No panic, bounded output, no absurd identity acceptance. | identity parser | NEW |
| FZ-006 | P0 | nightly | cargo-fuzz | sanitize_display/redact_argv | Arbitrary bytes/Unicode argv. | No control injection; output bounded; no secret-pattern bypass from trivial separators. | security | NEW |
| FZ-007 | P1 | nightly | cargo-fuzz | parse_net_sockets | Arbitrary `/proc/*/net/*`. | No panic; ports 0..65535; no malformed line accepted inconsistently. | PULSE-086 vicinity | NEW |
| FZ-008 | P1 | nightly | cargo-fuzz | TUI truncate/wrap/elide | Arbitrary Unicode + widths 0..512. | No panic; valid UTF-8; width contract. | TUI | NEW |

## 14. Live chaos / soak / performance

| ID | Pri | Gate | Layer | Target | Scenario | Pass condition | Finding/root | Baseline expectation |
|---|---|---|---|---|---|---|---|---|
| CHAOS-001 | P0 | nightly | live chaos | process churn | Fork/exit thousands of short processes while collecting. | No PID identity collapse, stale process, unbounded series growth or panic. | identity/boundedness | UNKNOWN |
| CHAOS-002 | P0 | nightly | live chaos | cgroup churn | Create/delete nested cgroups at high rate. | No inode identity bleed; max_cgroups budget enforced; graph converges. | graph/cgroup | UNKNOWN |
| CHAOS-003 | P1 | nightly | live chaos | net iface churn | Create/delete veth pairs repeatedly. | No negative throughput; previous maps bounded; recreated name gets clean baseline. | network | UNKNOWN |
| CHAOS-004 | P1 | nightly | live chaos | mount churn | Mount/unmount tmpfs/loop devices. | Filesystem/disk entities converge; no stale utilization. | filesystem | UNKNOWN |
| CHAOS-005 | P1 | nightly | live chaos | permission changes | Change visibility/ownership where feasible. | Partial data explicitly degraded; no false zero/healthy. | freshness/security | UNKNOWN |
| CHAOS-006 | P0 | nightly | soak | memory ceiling | High churn + long names/events for 1–6h. | RSS/history bytes plateau; eviction makes progress; no spin. | PULSE-082 | UNKNOWN |
| CHAOS-007 | P1 | nightly | perf | collector cost | Run `measure-cost.sh` at 1000/500/200ms. | Budget curves recorded; regression threshold relative to baseline. | performance | SCRIPT EXISTS |
| CHAOS-008 | P1 | release | portable | musl artifact | Build `x86_64-unknown-linux-musl`, run on older/newer distros. | Standalone binary starts/checks; no glibc dependency. | release portability | SCRIPT EXISTS |


## Первые 15 tests, которые надо написать до любых следующих исправлений

Это **red-first queue**. Именно он превращает наиболее опасные findings из текста аудита в executable specification.

1. `OBS-001` — stale sample не может выглядеть current.
2. `OBS-002` — HostCollector обязан репортить потерю `/proc/stat`.
3. `GRAPH-001` — historical rename.
4. `GRAPH-002` — historical reparent.
5. `HIST-001` — окно, пересекающее warm + hot.
6. `HIST-002` — counter rate из warm layer.
7. `RULE-003` — missing observation не является clear.
8. `ID-003` — ProcessDetails привязаны к `(pid,start_ticks)`.
9. `ID-005/LIN-008` — socket lookup в target network namespace.
10. `BUD-001` — `/proc` enumeration реально bounded.
11. `BUD-003` — FD work budget.
12. `SELF-001/002` — полный tick duration/skipped.
13. `SELF-003` — slow TUI не блокирует History writer.
14. `CFG-002/003/004` — нулевые operational limits.
15. `SEC-001/002` — high-entropy redaction и sanitation labels.

До исправления каждый из этих тестов должен **локально доказанно падать по ожидаемой причине**. После исправления — проходить и оставаться в suite.

## Property tests: конкретные модели

### EntityGraph reference model

В test-only модели хранить:

```rust
struct ModelEntity {
    key: EntityKey,
    id: EntityId,
    name_versions: Vec<(Timestamp, String)>,
    parent_versions: Vec<(Timestamp, Option<EntityId>)>,
    alive_intervals: Vec<(Timestamp, Option<Timestamp>)>,
}
```

`proptest` генерирует операции:

```text
Create(key,name,parent)
Touch(key)
Rename(key,name)
Reparent(key,parent)
Miss(key)
AdvanceTick(ms)
RecreateLogical(key/logical)
```

После каждого tick сравнивать graph/history с reference model. Это одним набором тестов ловит ABA, grace ticks, historical metadata, duplicate lifecycle events и non-determinism.

### Hysteresis reference model

Генерировать:

```text
Enter
Clear
Neutral/Missing
```

и случайные `enter_after_ticks`, `clear_after_ticks`. Reference machine должна быть независимой от production `Hysteresis`. Главный инвариант: **Missing не эквивалентен Clear**.

### Counter/history model

Генерировать `(timestamp,value)` с:
- normal increments;
- reset;
- missing tick;
- hot/warm boundary;
- bucket boundary.

Oracle для rate:

```text
sum(max(0, v[i] - v[i-1])) / elapsed_observed_seconds
```

Но после перехода в warm слой counter aggregate обязан хранить достаточно информации для того же результата; использование bucket mean как counter point тест должен запрещать.

## Fuzz policy

PR не должен ждать долгий fuzz. Разделить режимы:

```text
PR smoke:
  10 s × critical target

nightly:
  5 min × target

release candidate:
  corpus regression + 30 min critical security/parser targets
```

Каждый crash/minimized input автоматически добавлять в `fuzz/corpus/<target>/` и в обычный deterministic regression test, если input соответствует реальному Linux format.

## Live Linux test recipes

### Target network namespace (`ID-005 / LIN-008`)

```bash
sudo ip netns add pulse-test
sudo ip netns exec pulse-test sh -c 'python3 -m http.server 18080 --bind 0.0.0.0 >/tmp/pulse-netns.log 2>&1 & echo $! >/tmp/pulse-netns.pid'
PID=$(cat /tmp/pulse-netns.pid)
```

Test code вызывает `ProcDetails::details(PID)` и требует TCP port `18080`. Отдельно assert:

```bash
grep -q ':46A0' /proc/$PID/net/tcp*
```

После remediation источник socket tables должен строиться от `/proc/<pid>/net`, не `/proc/net`.

### Process churn (`CHAOS-001`)

В течение 60–300 секунд создавать короткоживущие процессы с ограниченным пулом PID (на отдельной VM можно снизить `kernel.pid_max`). Проверять:
- одинаковый PID с разным `start_ticks` не склеивается;
- old EntityId не получает metrics нового процесса;
- series map после churn возвращается к plateau;
- collector tick не падает.

### Cgroup churn (`CHAOS-002`)

Под отдельным subtree `/sys/fs/cgroup/pulse-test/` создавать/удалять nested cgroups, двигать туда test processes, менять `cpu.max`, `memory.max`, `pids.max`. Никогда не трогать production cgroups хоста CI.

### Memory soak (`CHAOS-006`)

Подаётся synthetic workload с длинными entity names/labels/events. Снимать:
- internal `history_peak_bytes`;
- `/proc/$PID/status VmRSS`;
- число hot/warm series;
- evictions/no_progress.

Pass condition должен задаваться **по фактической памяти**, а не только `approx_bytes`.

## CI lanes после внедрения

### `ci-fast.yml` — каждый PR

```text
fmt
check --all-targets --all-features
clippy -D warnings
unit/integration deterministic tests
proptest (фиксированный cases budget, например 256)
OpenMetrics grammar tests
config/security contracts
```

Целевой budget: < 10 минут.

### `ci-linux.yml` — каждый PR или обязательный protected check

```text
ubuntu-22.04
ubuntu-24.04

live non-privileged proc/sys/cgroup oracle tests
portable build smoke
```

### `ci-privileged-nightly.yml`

```text
network namespaces
cgroup churn
veth churn
terminal-loss
process churn
fuzz
memory soak short
```

### `ci-perf-nightly.yml` — только dedicated/self-hosted runner

Нельзя ставить жёсткие CPU/RSS thresholds на шумном GitHub shared runner. Dedicated host хранит baseline JSON:

```json
{
  "tick_p95_ms": 0,
  "cpu_percent_one_core": 0,
  "rss_mib": 0,
  "export_p95_ms": 0
}
```

Regression gate сравнивает относительное изменение и абсолютный safety ceiling.

### `release.yml`

```text
all required checks
cargo deny/advisories/licenses
musl portable build
OpenMetrics validator
fuzz corpus regression
SBOM
artifact checksums
provenance/attestation
```

## Release-blocking invariants

Pulse нельзя считать готовым к 1.0, пока не выполняются все следующие утверждения:

1. **Ни одно старое наблюдение не представляется свежим без явной freshness semantics.**
2. **A/B history сохраняет историческое имя, parent и диагностически значимые labels.**
3. **Unknown, zero и unlimited — разные domain states.**
4. **Любое действие/детали процесса привязаны к immutable process identity, а не только PID.**
5. **Missing telemetry не является подтверждением clear-condition.**
6. **Окна истории корректны при пересечении hot/warm; counters имеют counter semantics в обоих слоях.**
7. **Resource budget ограничивает выполненную работу, а не только объём возвращённого результата.**
8. **Self-observability измеряет полный pipeline Pulse.**
9. **Slow TUI/HTTP consumer не блокирует collection writer.**
10. **Каждая security/config option имеет executable test и не может быть silent no-op.**
11. **Linux-specific значения подтверждены live oracle tests, а не только fixtures.**
12. **TUI не может вывести недоверенные terminal control sequences.**

## Порядок PR

### PR-01 — Test infrastructure + CI baseline
`proptest`, `CountingFs`, новые CI lanes, исправление MSRV/lock mismatch, `VER-*`.

### PR-02 — Observation quality/freshness
`OBS-*`, quality state для samples/collectors/snapshot/health.

### PR-03 — Temporal entity history
Versioned metadata/lifecycle representation, `GRAPH-*`.

### PR-04 — History semantics
Hot+warm composition, counter aggregates/rate, staleness, `HIST-*`.

### PR-05 — Analyzer/hysteresis
Missing/neutral semantics, problem materialization, `RULE-*`.

### PR-06 — Process identity/details/actions
Identity-aware details, pidfd/action safety, target netns sockets, `ID-*`, `LIN-008`.

### PR-07 — Bounded work
`FsSource` bounded iteration/visitor, process/cgroup/fd scan budgets, `BUD-*`.

### PR-08 — Config + security contracts
Validation, entropy redaction, sanitation-at-boundary, `CFG-*`, `SEC-*`.

### PR-09 — Runtime isolation/self-observability
Full tick timing, lock-free render inputs, shutdown behavior, `SELF-*`.

### PR-10 — Linux semantic oracle
Page size, proc/cgroup/disk/net live comparisons, `LIN-*`.

### PR-11 — Exporter/OpenMetrics
Health quality, grammar, escaping, cardinality, `EXP-*`.

### PR-12 — TUI contract suite
TestBackend/property/golden/PTY suite, `TUI-*`.

### PR-13 — Fuzz/chaos/perf/release hardening
`FZ-*`, `CHAOS-*`, supply-chain/release gates.

## Definition of Done для каждого audit finding

Finding считается закрытым только если одновременно есть:

```text
[ ] regression test reproduces old defect
[ ] fix makes it pass
[ ] neighboring property/invariant test where appropriate
[ ] test is assigned to a CI gate
[ ] docs/contract updated if semantics changed
[ ] no silent fallback that changes meaning
[ ] finding ID is mentioned in test/PR description
```

Иначе это исправление конкретного примера, а не устранение класса дефектов.
