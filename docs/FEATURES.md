# FEATURES

Каталог функций Pulse с аудитом состояния. Каждая строка подтверждена кодом
(файл указан) или knowledge graph (`.understand-anything/knowledge-graph.json`,
создана в сессии от 2 сентября 2026). Статусы:

- **реализовано** — работает, покрывается тестами;
- **частично** — работает, но с зафиксированным ограничением (указано);
- **нет в v1** — функции не существует в коде; разобрано в разделе
  «Честные разрывы» и в `docs/ROADMAP.md` (раздел 3 — что стоит в очереди);
- **отложено** — явно отложено в `IDEAS.md` (раздел «Отложено»).

Метрики в каталоге приведены по реестру `pulse-core/src/metric.rs`
(нумерованные диапазоны: host 1..=49, disk 50..=69, network 70..=89,
cgroup 90..=149, process 150..=189, agent 190..=219).

---

## 1. Сбор данных (`crates/pulse-collect`)

| Функция | Где | Статус |
|---|---|---|
| Метрики хоста: CPU, память, loadavg, uptime, swap, FD-лимит | `src/host.rs` | реализовано |
| PSI (pressure stall information): cpu some, memory full, io full, окна avg10/60/300 | `src/host.rs:138-177` (отдельного `psi.rs` нет) | реализовано |
| Процессы: инвентарь по `/proc`, имя, пользователь, исполняемый файл, cwd | `src/process.rs` | реализовано |
| cgroup v2: inode-идентичность, владелец (unit/container/pod), CPU-троттлинг, память | `src/cgroup.rs` | частично — контейнеры и pod распознаются только по пути cgroup (префиксы `docker-`, `cri-containerd-`, `crio-`, `libpod-`, `kubepods`), без обращения к Docker/CRI API; namespace pod фиксируется как `unknown`. Компромисс осознанный (коммент в `cgroup.rs`: доступ к сокету рантайма требует прав и создаёт зависимость) |
| Диски: await, throughput, наличие трафика (для гейтов правил) | `src/disk.rs` | реализовано |
| Сетевые интерфейсы: пер-интерфейсные счётчики `/proc/net` — rx/tx bytes, packets, errors, drops, throughput (реестр 70..=79) | `src/net.rs:83-101` | реализовано |
| Файловые системы: mounts, заполненность по формуле `df` (root + худший значимый mount) | `src/filesystem.rs`, `src/fs.rs` | частично — см. раздел «Честные разрывы» |
| Детали процесса по требованию: инвентарь fd (fd → цель), слушающие TCP/UDP-порты, UID → имя | `src/details.rs` | реализовано; без прав экран честно сообщает `restricted` |
| Ограничители чтения: capped-чтение файлов, гонки исчезнувших точек монтирования | `src/read.rs`, `src/filesystem.rs` | реализовано |

## 2. Ядро модели (`crates/pulse-core`)

| Функция | Где | Статус |
|---|---|---|
| Граф сущностей: 10 видов (host, cgroup, unit, process, container, pod, disk, netif, …), связи (RunsIn, OwnedBy, MemberOf, …), слияние коллекторов | `src/graph.rs` | реализовано |
| Реестр метрик: стабильные нумерованные ID, тип/размерность/периодичность | `src/metric.rs` | реализовано |
| Мгновенный снимок `Snapshot` + обмен через `ArcSwap` (потребители не блокируют сбор) | `src/snapshot.rs` | реализовано |
| Проблемы с evidence (метрика, порог, значение, сущность, класс) | `src/problem.rs` | реализовано |
| Декларативные правила + 9 правил по умолчанию (psi.cpu, psi.memory, psi.io, cgroup.throttle, memory.pressure, disk.latency, fd.pressure, swap.pressure, memory.oom) | `pulse-engine/src/rules.rs:415` (`default_rules()`) — правила живут в движке, не в ядре | реализовано — см. «Честные разрывы» про fill-правило |
| Семантический конвейер: классификация событий (потоки ядра vs операционные изменения), дедупликация, группировка, информационный бюджет | `src/semantic.rs` | реализовано |
| Объяснимая релевантность: взвешенная сумма факторов, `WHY` у каждой строки | `pulse-tui/src/fold.rs` (`scored_reasons:363`, `reasons:448`, `relevance:460`) — ранжирование живёт в слое интерфейса и работает на строках `LogicalRow`, а не в ядре | реализовано |
| Конфигурация с валидацией: доли 0..1, warn > clear (иначе `ConfigError::Hysteresis`), пороги дребезга | `src/config.rs` | реализовано |
| Абстрактные источники деталей с однотактным кэшем | `src/details.rs` | реализовано |

## 3. Движок (`crates/pulse-engine`)

| Функция | Где | Статус |
|---|---|---|
| Analyzer: enter/clear streak по тикам (гистерезис дребезга: `enter_after_ticks: 3`, `clear_after_ticks: 15` по умолчанию) | `src/analyzer.rs` | реализовано |
| Runtime: конвейер collect → graph → history → analyzer → snapshot; единственный поток-писатель | `src/runtime.rs` (в `pulse-cli`) | реализовано |
| Ограниченный shutdown: SIGTERM → завершение сервиса за 2 c (регрессия P0-инцидента) | `pulse-cli/tests/signals.rs` | реализовано |

## 4. История и память (`crates/pulse-store`)

| Функция | Где | Статус |
|---|---|---|
| История метрик: hot/warm, `value_at`, `series_points` | `src/lib.rs` (тип `History`: `series_points:369`, `value_at:417`), hot-слой `src/hot.rs`, warm-слой `src/warm.rs` | реализовано |
| Жёсткий потолок памяти с гарантией прогресса: вытеснение (включая события и мёртвые записи сущностей), при невозможном лимите — выход без retry-цикла | `src/lib.rs:1148-1152` и тесты `sustained_ceiling_churn_makes_progress_and_stays_bounded`, `impossible_ceiling_exits_without_retry_loop` | реализовано |
| Исторические корзины State River (прошлые состояния на полосе) | — | отложено (`IDEAS.md`: требует window-агрегатов; сейчас честно `collecting state history`) |
| Отметки событий на линии Timeline | — | отложено (`IDEAS.md`: требует горизонтального биннинга по истории) |

## 5. Экспорт (`crates/pulse-export`)

| Функция | Где | Статус |
|---|---|---|
| OpenMetrics-экспорт `GET /metrics` (встроенный HTTP, без внешних зависимостей) | `src/metrics.rs` | реализовано |
| Prometheus remote_write | `src/remote_write.rs` | реализовано |
| OTLP egress (logs/traces) | — | отложено (`IDEAS.md`: не включён в MVP) |
| Prometheus Remote Read (чтение внешней истории) | — | отложено (`IDEAS.md`: future-адаптер) |
| Полная поддержка PromQL | — | отложено (`IDEAS.md`: future-адаптер) |

## 6. TUI (`crates/pulse-tui`)

Нормативный контракт — `pulse_tui_visual_navigation_spec_v0.9.md` (append-only,
§164–197 корректировочные).

| Функция | Где | Статус |
|---|---|---|
| Экраны: Overview, Problems, Entities, Timeline/Time Machine, Help (overlay), acceptance | `src/screens/` | реализовано |
| Единый UI state: видимое = фокус = клавиатурная цель; renderer публикует видимые панели | `src/app.rs`, `src/layout.rs` | реализовано |
| Клавиатура: `1–4` экраны, `↑↓/k j`, `Enter`, `Esc`, `Tab/Shift+Tab` (pane-local), `/` поиск, `:` палитра, `?/F1` help, `q/Ctrl+C` | `src/screens/help.rs` | реализовано |
| Контекстный Inspector: сессия с origin/путём, `A→B→C→A` = `[A]`, `Esc` возвращает точный origin | `src/screens/inspector.rs`, `src/investigate.rs` | реализовано |
| Расследование как направленный спуск: уровни сущностей, `Enter` вниз по уровню/дереву, ресурсы — листья, прыжок в сторону — отдельный список | `src/investigate.rs` | реализовано |
| Explainable relevance: `WHY` у каждой строки `RELEVANT ENTITIES`; без причин блок называется `KEY ENTITIES` | `src/screens/overview.rs`, `src/fold.rs` | реализовано |
| Логическая свёртка (`unit+14p`), `m` — technical view | `src/screens/entities.rs`, `src/fold.rs` | реализовано |
| Анти-stretch layout: `max_useful_width`, остаток ширины — намеренная пустота | `src/layout.rs` | реализовано |
| Честная история: спарклайн только при ≥2 точках, иначе `collecting history`; вердикт ≤ окна наблюдения | `src/screens/timeline.rs`, тесты | реализовано |
| Глиф состояния: 8 классов, масса = нагрузка (радиус `sqrt(load/0.45)`) | `src/state.rs` | реализовано |
| Semantic time-travel: time rail, State River, metric lanes (CPU/MEM/IO), Incident Story, A/B-метки, `D` — diff | `src/screens/timeline.rs` | частично — см. «Честные разрывы» |
| Палитра команд `:` | `src/screens/` | частично — реализованы `raw events` и `story`; остальные команды честно сообщают об отсутствии (полная палитра отложена в `IDEAS.md`) |
| Пайп расследования: `g` открывает факты о выбранной сущности (exe, user, cwd, ports, owner, limits, files, log, journal, problems, resources), `→`/`←` раскрывают и сворачивают ветку, `v` переключает боксы и отступы | `src/pipe.rs`, `src/screens/pipe.rs` | реализовано |
| Ленивость пайпа: свёрнутая ветка не делает ни одного чтения `/proc`, раскрытая читает один раз на такт через кэш деталей | `src/pipe.rs` (`PipeState::needs_details`), `src/app.rs` (`pipe_details`) | реализовано |
| Граф-режим боксами: ширина бокса фиксирована, значение переносится по словам, путь сокращается по сегментам, рамка фокуса двойная; при нехватке ширины вид меняется на отступы с подписью | `src/pipe.rs` | реализовано |
| Иконки на весь интерфейс: `ui.icons = off \| nerd \| unicode` (`true` — синоним `unicode`); один словарь на 32 смысла, покрыты шапка (режим), футер (вкладки), список сущностей (вид), категории пайпа; в ASCII-режиме иконок нет | `pulse-core/src/config.rs` (`IconSet`), `pulse-tui/src/icons.rs` (`Icon`), `pulse-tui/src/ui.rs` (`footer_line`) | реализовано |
| Пайп древом в стиле панелей omp: скруглённые рамки `╭╮╰╯`, подпись в верхней границе, тяжёлая рамка фокуса `┏━┓`, ствол слева и ветвь по верхней границе ряда | `pulse-tui/src/pipe.rs` (`Charset`, `render_boxes`) | реализовано |
| Навигация пайпа по осям экрана: `↑↓←→` двигают курсор по сетке, `⎵` раскрывает и сворачивает; число колонок сообщает кадр | `pulse-tui/src/pipe.rs` (`move_by`, `columns_for`), `pulse-tui/src/app.rs` | реализовано |
| Демонстрационный сценарий `--demo`: подменяется только `FsSource`, конвейер и правила настоящие; старт у порога деградации, чтобы первый кадр уже двигался; цикл покой → деградация → восстановление; пометка `DEMO` в шапке | `pulse-collect/src/demo.rs` (`DemoFs`), `pulse-cli/src/main.rs` (`source_fs`), `pulse-cli/tests/demo_scenario.rs` | реализовано |
| Ось времени с отметками: события ставятся в свой момент, символ берётся из серьёзности события (критика отличается от предупреждения), худшее состояние выигрывает колонку, таймкоды подписываются без наложения | `pulse-tui/src/screens/timeline.rs` (`rail_marks`, `rail_labels`) | реализовано |
| Дорожки метрик покрывают запрошенное окно бакетами, а не хвост ряда; в бакете берётся максимум, бакет без данных рисуется точкой, а не нулём | `pulse-tui/src/format.rs` (`ratio_lane`) | реализовано |
| Колонка тренда в строках таблиц: форма CPU за минуту, масштаб — пик окна, пик и среднее в панели выбранного; ровный ряд рисуется ровной линией, отсутствие истории названо `collecting`; колонка уходит первой при сужении | `pulse-tui/src/trend.rs` (`lane_of`, `of_series`), `pulse-tui/src/table.rs` (`TREND`), `pulse-tui/src/screens/overview.rs` (`trend_cell`) | реализовано |
| Тема: 3 тирса терминала (truecolor/256/16), ASCII-fallback | `src/theme.rs`, `src/capability.rs` | реализовано |
| Детерминированная приёмка (§194/§197): сценарий ввода → ожидаемая картина | `src/screens/acceptance.rs` | реализовано |
| Инфраструктура проверки аудита: `CountingFs` считает фактические чтения, `ScriptedFs` детерминированно меняет ответ по пути между вызовами, property-тесты закрепляют санитайзер, арифметику времени и отсев не-finite значений | `pulse-collect/src/test_support.rs`, `pulse-core/tests/properties.rs`, `pulse-store/tests/properties.rs` | реализовано |
| RED-репродьюсеры находок аудита: 13 дефектов зафиксированы падающими тестами под `#[ignore]`, `scripts/audit-red.sh` требует продвижения теста при исправлении дефекта | `crates/*/tests/audit_red.rs`, `scripts/audit-red.sh`, `docs/VERIFICATION_RED.md` | реализовано |
| Детерминированный обязательный CI: точный toolchain 1.85.1, все шаги с `--locked`, `PROPTEST_CASES` задан; отдельный гейт advisory и дубликатов зависимостей | `.github/workflows/ci.yml`, `.github/workflows/supply-chain.yml`, `scripts/wsl-verify.sh` | реализовано |
| Граница доверия метк: значение проходит санитизацию и усекается по границе UTF-8 не позже лимита, поэтому недоверенная строка из `/proc` не роняет агент | `pulse-core/src/entity.rs` (`Labels::set`), `pulse-core/src/redact.rs` (`sanitize_with_limit`) | реализовано |
| Валидация конфигурации отклоняет `crit < warn` и нулевые операционные лимиты, называя последствие | `pulse-core/src/config.rs` (`ConfigError::Severity`, `ConfigError::ZeroLimit`) | реализовано |
| Opt-in сокрытие безымянных токенов по энтропии: `security.redact_high_entropy` доходит до метки `cmdline`, в режиме `Off` не действует | `pulse-core/src/redact.rs` (`looks_high_entropy`), `pulse-collect/src/process.rs` | реализовано |

## 7. CLI (`crates/pulse-cli`)

| Команда | Назначение | Статус |
|---|---|---|
| `pulse` / `pulse run` | TUI + локальный `/metrics` (по умолчанию) | реализовано |
| `pulse serve` | Агент без TUI: сбор + HTTP-экспорт | реализовано |
| `pulse top [-n N]` | Однократный текстовый снимок после двух тактов | реализовано |
| `pulse diff --from --to` | Семантическое A/B-сравнение двух моментов локальной истории (без причинных утверждений) | реализовано |
| `pulse scorecard --seconds` | Измерить стоимость агента и размеры локального состояния | реализовано |
| `pulse config print` | Эффективная конфигурация | реализовано |
| `pulse check` | Проверить конфигурацию и выполнить реальный такт сбора | реализовано |

## 8. Инженерия

| Функция | Где | Статус |
|---|---|---|
| CI: `cargo fmt --check`, `clippy --deny warnings`, `test`, `verify` (10 c) | `.github/workflows/ci.yml`, `scripts/verify.sh` | реализовано |
| Сетевая проверка remote_write (реальный HTTP-сервер) | `scripts/verify-net.sh` | реализовано |
| Измерение стоимости агента (RSS/peak, CPU-цикл/такт, история на 24h) | `scripts/measure-cost.sh` | реализовано |
| Tracing-профиль памяти по тактам | `scripts/mem-trace.sh` | реализовано |
| Переносимая статическая сборка (musl) | `scripts/build-portable.sh` | реализовано |
| ADR 0001–0004, ARCHITECTURE, CONTRACTS, SECURITY, COMPETITIVE-CRITERIA | `docs/` | реализовано |

## 9. События и журналы

| Функция | Где | Статус |
|---|---|---|
| Собственный журнал событий агента: ring-буфер с потолком `max_events` (по умолчанию 8192), `recent(limit)` для TUI | `pulse-store/src/events.rs` | реализовано |
| Журнал событий участвует в жёстком пределе памяти: при достижении `max_bytes` старые записи вытесняются | `pulse-store/src/lib.rs:1148-1152` | реализовано |
| События жизненного цикла сущностей и операционные изменения, отделённые от потоков ядра | `pulse-core/src/semantic.rs` | реализовано |
| Проблемы как события с evidence (метрика, порог, значение, сущность) и моментами enter/clear | `pulse-core/src/problem.rs`, `pulse-engine/src/analyzer.rs` | реализовано |
| Timeline: `: raw events` — поток событий, `: story` — значимая история; `Enter` в панели Story открывает Inspector сущности события | `pulse-tui/src/screens/timeline.rs`, `pulse-tui/src/app.rs:985-989` | реализовано |
| Открытые файлы процесса (в том числе пути журналов вида `/var/log/...`) как деталь терминального уровня | `pulse-collect/src/details.rs` | реализовано |
| Чтение системных журналов (journald, syslog, `/var/log`) | — | нет в v1: коллектора журналов не существует, совпадения `/var/log` в коде — только тестовые фикстуры |
| Привязка проблем и метрик к строкам системных журналов | — | нет в v1: логи и трейсы отвергнуты для MVP, OTLP-egress отложен (`docs/ROADMAP.md` 3.7) |

## 10. Диски и файловые системы (что видно оператору)

| Функция | Где | Статус |
|---|---|---|
| vitals Overview: заполненность корня (`/ 94%`) и худшей значимой ФС (`worst fs`, если она хуже корня) | `pulse-tui` vitals, `pulse-collect/src/fs.rs` | реализовано |
| Формула заполненности как у `df`: `used = total − free`, знаменатель `used + available` | `pulse-collect/src/fs.rs:41-44` | реализовано |
| Метрики ФС в реестре и `/metrics`: `FS_ROOT_UTIL` (49), `FS_ROOT_TOTAL` (66), `FS_ROOT_FREE` (67), `FS_WORST_UTIL` (68) | `pulse-core/src/metric.rs` | реализовано |
| Метрики устройств: read/write bytes и ops, wait, io_time, inflight, `DISK_AWAIT` (58), util, queue, throughput (диапазон 50..=62) | `pulse-collect/src/disk.rs` | реализовано |
| Правило задержки диска `disk.latency` (`DISK_AWAIT` 20/50 ms, гейт `has_disk_traffic`) | `pulse-engine/src/rules.rs` | реализовано |
| Список точек монтирования с fstype и заполненностью (экран или экспорт per-mount) | — | нет в v1 (честный разрыв 3, `docs/ROADMAP.md` 3.2) |
| Правило на заполненность файловой системы | — | нет в v1 (честный разрыв 1, `docs/ROADMAP.md` 3.1) |

---

## Честные разрывы (не в IDEAS.md, найдены аудитом)

1. **Нет fill-правила для файловой системы.** В `default_rules()`
   (`pulse-engine/src/rules.rs:415`) девять правил, ни одно не смотрит на
   `host_filesystem_*_utilization`. При 94% занятого диска проблема
   возникает только через `psi.io` (0.10/0.30) или `disk.latency`
   (20/50 ms, гейт `has_disk_traffic`) — т.е. только при реальном давлении,
   а не при заполненности. Полная, но бездействие-по-IO ФС проблему не
   создаёт. Метрики при этом собираются и видны в vitals (`/ 94%`,
   `worst fs`) и в `/metrics`.
2. **Load average не показан в TUI.** `host_load1/5/15` собираются
   (`pulse-collect/src/host.rs`), зарегистрированы в реестре
   (`metric.rs` id 8/9/10, без нормализации на ядра) и публикуются в
   `/metrics`, но ни один экран их не рисует: vitals показывает
   CPU/MEM/PSI/IO WAIT/NET/DISK/FS, lanes Timeline — CPU/MEM/IO.
3. **Детали по mount не раскрываются.** Коллектор различает значимые
   mount (fstype allowlist, исключение `/var/lib/docker/*`, `/snap/*`),
   но наружу выходят только корневая ФС и худшая значимая (util/total/free);
   список mount с fstype в TUI/экспорте не представается.
4. **Сеть — только метрики, нет правил.** Пер-интерфейсные счётчики
   (bytes, packets, errors, drops, throughput) собираются и доступны как
   метрики сущности NetIf, в vitals показан только host-агрегат пропускной
   способности; правил по сети в `default_rules()` нет, поэтому «кто шлёт
   трафик» и «интерфейс сыпет ошибками» в v1 — вопрос расследования,
   а не проблемы. Проверок доступности (ping, DNS, TCP-connect) нет вовсе:
   Pulse — наблюдатель, а не пробник.
5. **Анимация состояния из раздела 6 спецификации не реализована.**
   Спека нормирует язык анимации glyph (пульсация, поток, рывок при
   retransmit, collapse → reform при перезапуске, вспышка события,
   замедление при ожидании) и флаг `--no-animation`. В `crates/` нет ни
   одного совпадения по `animation`: `GlyphSurface` — статическая сетка
   классов состояния (`pulse-tui/src/glyph.rs`), а тест требует, чтобы две
   сборки одного glyph были равны. Раздел 7 (логическая сетка, пресеты,
   dot-режим, ASCII-fallback, масса глифа) реализован полностью.

## Отложено (из `IDEAS.md`)

- eBPF-инструментация (опциональный collector);
- полная eBPF-корреляция (cluster-level, kernel-события ↔ K8s-объекты);
- OTLP egress;
- Prometheus Remote Read;
- полная PromQL-поддержка;
- поддержка Windows/macOS (MVP — Linux-focused);
- отметки событий на линии Timeline;
- исторические корзины State River;
- отдельный экран semantic A/B diff (CLI `diff` уже даёт отчёт);
- полная палитра команд.
