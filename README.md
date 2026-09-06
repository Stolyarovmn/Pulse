# Pulse

Pulse — local-first observability node для Linux с диагностическим TUI. Он собирает данные из `procfs`, `sysfs` и cgroup v2, строит temporal entity graph, хранит короткую локальную историю и показывает не только загрузку, но и владельца ресурса, открытые проблемы и изменения между двумя моментами.

Текущий статус: рабочий Linux MVP. Дисковая история, eBPF, OTLP, Prometheus Remote Read/Write и Kubernetes/CRI API пока не реализованы.

## Что уже работает

- сущности `host`, `cgroup`, `systemd unit`, `process`, `disk`, `network interface`, `container`, `pod`;
- устойчивая идентичность процессов `(pid, start_ticks)` и cgroup по inode;
- CPU, memory, PSI, disk, network, process и cgroup telemetry;
- восемь семейств диагностических правил с гистерезисом и явными доказательствами;
- semantic A/B diff структуры, метрик и событий без недоказанных причинных утверждений;
- четыре экрана Overview, Problems, Entities, Timeline плюс контекстный Inspector и overlay'и Help/Search/Palette;
- семантический конвейер событий: рутинный churn ядра подавляется, повторы
  сворачиваются в группы, сырой поток остаётся доступен по `: raw events`;
- жёсткий предел памяти истории с гарантией прогресса вытеснения и bounded
  завершением по `SIGTERM`/`SIGINT`;
- OpenMetrics на `/metrics`, проверка состояния на `/healthz`;
- безопасные значения по умолчанию: loopback bind, process metrics выключены для экспорта, redaction включена, действия выключены.

## Требования

- Linux с cgroup v2;
- Rust 1.85 или новее;
- терминал с UTF-8; для ограниченных терминалов доступен `ui.ascii = true`.

На Windows разработка и запуск выполняются внутри WSL2. Репозиторий может находиться на диске Windows; вспомогательные скрипты держат `target` в Linux-файловой системе.

## Сборка и проверка

Linux:

```bash
cargo build --release -p pulse-cli
cargo test --workspace
```

Windows + WSL2 Ubuntu 24.04:

```powershell
wsl -d Ubuntu-24.04 -- bash /mnt/c/Users/maxim/project/pulse/scripts/wsl-cargo.sh build --release -p pulse-cli
wsl -d Ubuntu-24.04 -- bash /mnt/c/Users/maxim/project/pulse/scripts/wsl-cargo.sh test --workspace
```

### Стоимость агента

```bash
scripts/measure-cost.sh            # сбор: CPU, RSS, серии, выход по SIGTERM
scripts/measure-cost.sh --tui 15   # render-цикл живого TUI
```

### Снимок кадра TUI

Проверять раскладку глазами по скриншоту дорого и невоспроизводимо. Кадр
снимается как текстовая сетка нужного размера:

```bash
scripts/wsl-capture.sh -c 180 -r 40           # Overview на 180x40
scripts/wsl-capture.sh -c 120 -r 30 -k 4      # Timeline
scripts/wsl-capture.sh -c 120 -r 40 -k '?'    # Help с легендой состояний
```

Сетку держит `tmux`, поэтому позиционирование курсора не теряется, а агент
гарантированно останавливается по PID панели и не занимает порт 9099.
Детерминированные буферы без запуска процесса даёт
`PULSE_DUMP_SNAPSHOTS=1 cargo test -p pulse-tui --lib dump_v09 -- --nocapture`.

Цепочка расследования проходится одной командой: экран Entities, поиск,
заданное число спусков, кадр каждого шага.

```bash
scripts/capture-inspector.sh angie 3        # unit → cgroup → process
scripts/capture-inspector.sh angie 3 -a     # кадр после каждого шага
scripts/capture-inspector.sh angie 2 -t     # проверить боковой переход
```

### Диагностика зависшего агента

Если агент не отвечает или занимает ядро, нужно знать, какой поток и на чём
стоит, а не гадать:

```bash
scripts/diag-stuck.sh <pid> 2       # состояние потоков и их тики ядра
scripts/repro-terminal-loss.sh      # воспроизводит потерю терминала под TUI
```

Второй скрипт входит в `scripts/wsl-verify.sh`: тест на pty потерю терминала
не воспроизводит, а живой дефект «100% ядра и игнор `kill`» давала только
смерть сервера tmux под работающим интерфейсом.

### Переносимый бинарник

Обычная сборка динамически линкуется с glibc узла сборки: собранный на
Ubuntu 24.04 бинарник требует `GLIBC_2.39` и на 22.04 падает с
`version 'GLIBC_2.39' not found`. Для переноса на произвольный хост есть
статическая сборка на musl - внешний toolchain не нужен:

```bash
bash scripts/build-portable.sh
```

Скрипт проверяет статическую линковку, отсутствие ссылок на glibc и прогоняет
`pulse check`, после чего кладёт результат в `dist/pulse` (около 2,7 МБ).
Перенос:

```bash
scp dist/pulse user@host:~/pulse
ssh user@host 'chmod +x ~/pulse && ~/pulse run'
```

## Запуск

```bash
# TUI и локальный OpenMetrics endpoint
cargo run -p pulse-cli -- run

# Только агент и HTTP endpoint
cargo run -p pulse-cli -- serve

# Однократный текстовый снимок
cargo run -p pulse-cli -- top --limit 20

# A/B diff: собрать короткое окно и сравнить секунду назад с текущим моментом
cargo run -p pulse-cli -- diff --from 1s --to now

# Измерить стоимость агента
cargo run -p pulse-cli -- scorecard --seconds 10

# Проверить конфигурацию и реальный такт сбора
cargo run -p pulse-cli -- check

# Получить эффективную конфигурацию
cargo run -p pulse-cli -- config print
```

По умолчанию exporter слушает только `127.0.0.1:9099`:

```bash
curl http://127.0.0.1:9099/healthz
curl http://127.0.0.1:9099/metrics
```

Для bind не на loopback требуется `export.token_file` с токеном длиной не менее 16 байт. Встроенного TLS нет: для внешней публикации нужен TLS reverse proxy или защищённый туннель.

## Управление TUI

Четыре top-level экрана; Inspector — контекстная сессия, Help и Search —
overlay'и. `Tab` никогда не меняет экран: он циклит только видимые панели
текущего экрана.

| Клавиша | Действие |
|---|---|
| `1` / `2` / `3` / `4` | Overview / Problems / Entities / Timeline |
| `P` / `E`,`e` / `T` | псевдонимы Problems / Entities / Timeline |
| `↑`/`↓`, `j`/`k` | выбор строки в focused панели |
| `Enter` | спуститься по цепочке расследования в выбранный **видимый** объект |
| `Esc` | назад по уникальному пути Inspector, затем к origin |
| `Tab` / `Shift+Tab` | вне Inspector: видимые панели; в Inspector: `CHAIN` ⇄ `RELATED` |
| `/` | видимый modal поиска |
| `:` | палитра команд (`raw events`, `story`) |
| `m` / `s` / `f` | Entities: вид, сортировка, фильтр |
| `←`/`→`, `+`/`-`, `F` | Timeline: scrub, масштаб, возврат в LIVE |
| `A`, `B`, `D` | маркеры и semantic diff |
| `p` | пауза отображения; сбор продолжается |
| `?` / `F1` | Help overlay |
| `q`, `Ctrl+C` | выход |

### Цепочка расследования

Inspector ведёт от общего к конкретному и обязан заканчиваться ответом:

```text
host → unit/pod → container → cgroup → process → user, exe, cwd, порты, файлы
```

`Enter` спускается на уровень глубже, `Esc` поднимается. Диски и интерфейсы
показаны как `RESOURCES` и не открываются: диском пользуются десятки
несвязанных сервисов, и переход в него превращал бы его в пересадочный узел —
раньше из сервиса можно было уйти в диск, из диска в чужой сервис и ходить так
бесконечно.

Прыжок в сторону (владелец, сосед по общему диску) живёт отдельным блоком
`RELATED`; `Tab` переключает, какому списку достаётся `Enter`. Такой переход
начинает новое расследование, а не удлиняет текущий путь.

На процессе цепочка заканчивается блоком `PROCESS DETAILS`: пользователь,
исполняемый файл, рабочий каталог, слушающие порты и открытые файлы. Они
читаются по требованию только для открытого процесса. Без прав на чужой
процесс блок честно сообщает `restricted: run as root to read`, а не выглядит
как «файлов нет».

В TUI-режиме диагностика не печатается в терминал: ratatui владеет экраном, и
строка журнала оставалась бы висеть посреди кадра. Журнал включается переменной
`PULSE_LOG=/path/to/pulse.log`; состояние потолка памяти и ошибки сбора видны в
самом интерфейсе и в `pulse scorecard`.

## Конфигурация

Создайте файл из эффективной конфигурации и передайте его глобальным параметром:

```bash
pulse config print > pulse.toml
pulse --config pulse.toml check
pulse --config pulse.toml run
```

Неизвестные поля, небезопасный внешний bind без token file, неверные диапазоны и сломанная гистерезисная конфигурация приводят к отказу старта.

## Архитектура и безопасность

- [Архитектура](docs/ARCHITECTURE.md)
- [Контракты крейтов](docs/CONTRACTS.md)
- [Модель угроз](docs/SECURITY.md)
- [Измеримые конкурентные критерии](docs/COMPETITIVE-CRITERIA.md)
- [Идеи и компромиссы](IDEAS.md)
- [ADR](docs/adr/)

## Ограничения MVP

- история хранится только в памяти текущего процесса;
- `pulse diff` с относительным временем сам наполняет окно и ограничен пятью минутами;
- контейнеры и pod определяются эвристически по cgroup path, без обращения к runtime API;
- exporter не реализует TLS;
- `serve` и TUI завершаются по `SIGTERM`/`SIGINT`/`SIGHUP`/`SIGQUIT`, но HTTP-соединения не дренируются: запрос, начатый в момент остановки, обрывается;
- глубина истории сокращается при достижении `store.max_bytes` (64 МиБ по умолчанию): сначала warm, затем hot, но не ниже двух тактов;
- длинное окно доступно только для 22 метрик списка `LONG_WINDOW`; остальные метрики живут только в горячем окне;
- namespace pod не определяется без Kubernetes API и остаётся `unknown`;
- наблюдаемость процессов ограничивается правами пользователя и настройками `/proc`;
- дорожки метрик Timeline рисуют форму только по реальным точкам горячего окна
  (`History::series_points`); до появления второй точки честно пишут
  `collecting history`, а не подделывают кривую;
- сама линия времени Timeline не размечает события по оси: разметка требует
  биннинга истории, поэтому события живут в State River и Story;
- блоки `FILES` и `SOCKETS` инспектора не реализованы: файловых дескрипторов и
  сокетов как сущностей в модели пока нет;
- зависимость `cgroup -> disk` строится по `/sys/dev/block/<major>:<minor>`, а без
  этого каталога (WSL2, урезанный контейнер) - по `major:minor` целых устройств из
  `/proc/diskstats`; раздел, упомянутый в `io.stat` на таком хосте, до диска не
  поднимается, и связь честно не создаётся.

## Лицензия

Apache License 2.0. См. [LICENSE](LICENSE).
