# Контракты между крейтами Pulse

Документ фиксирует границы ответственности и публичные API. `pulse-core` реализован
и стабилен; остальные крейты обязаны реализовать ровно указанные ниже сигнатуры,
чтобы их можно было писать параллельно.

## Правила для всех крейтов

- `unsafe` запрещён (`unsafe_code = "deny"` на уровне workspace). Системные вызовы —
  только через `rustix` (safe API), не через `libc`.
- Никаких `unwrap()` / `expect()` / `panic!()` в рабочем коде; в тестах допустимо.
- Данные из `/proc` и `/sys` — недоверенный вход. Битый формат, отсутствие файла,
  non-UTF8, гигабайтная строка: skip + debug-лог. Никогда паника, никогда обрыв такта.
- Ограничение объёма чтения: не более 64 KiB на файл, лимиты на число элементов.
- `/proc/<pid>/environ` не читается никогда.
- Любая строка из ядра проходит `pulse_core::sanitize_display` до попадания в граф или вывод.
- Комментарии и док-строки — на русском, объясняют «зачем», а не «что».
- Тесты в конце файла в `mod tests`, имя теста описывает инвариант.

Проверка сборки одного крейта (отдельный target-каталог, параллельно не конфликтует):

```
wsl -d Ubuntu-24.04 -- bash /mnt/c/Users/maxim/project/pulse/scripts/wsl-test.sh <crate> [test|check|clippy]
```

## pulse-core (общий контракт)

Заголовок раньше гласил «менять нельзя»: это была заморозка на время
параллельной работы нескольких агентов над одним файлом, а не запрет навсегда.
Действующее правило: `pulse-core` — единственный источник домена, расширяется
осознанно, и любое изменение публичного типа обязано пройти по всем
потребителям в том же изменении (без shim-слоёв и алиасов).

Читать исходники: `crates/pulse-core/src/`. Краткая карта:

| Модуль | Содержимое |
|---|---|
| `time` | `Timestamp` (u64 мс, Copy+Ord), `TickId`, `format_duration` |
| `entity` | `EntityId` (индекс+поколение), `EntityKind`, `Runtime`, `EntityKey`, `Labels`, `Entity`, `EntitySpec`, `EntityRecord` |
| `metric` | `MetricId(u16)`, `MetricKind`, `Unit`, `MetricScope`, `ExportPolicy`, `MetricDesc`, `DESCRIPTORS`, `describe`, `scope_metrics`, `ids::*` |
| `sample` | `SeriesKey { entity, metric }`, `Sample` |
| `relation` | `RelationKind`, `Relation`, `.is_resource_dependency()` |
| `event` | `EventKind`, `Event` (builder) |
| `problem` | `Severity`, `Evidence`, `RuleId`, `ProblemId`, `Problem`, `Hysteresis` |
| `redact` | `sanitize_display`, `parse_cmdline`, `redact_argv`, `RedactMode` |
| `graph` | `EntityGraph`, `CollectCtx`, `Collector`, `CollectError`, `TickBatch`, `GraphStats` |
| `snapshot` | `LatestValues`, `AgentStats`, `Snapshot`, `SnapshotSource` |
| `config` | `Config` и секции `General`, `Security`, `Store`, `Export`, `Rules`, `Ui` |

Важные инварианты `pulse-core`:

- `EntityKey::Process { pid, start_ticks }` — идентичность процесса, защищённая от
  переиспользования PID; `EntityKey::Cgroup { cgroup_id }` — inode каталога, а не путь;
  `EntityKey::Unit { name }` — логическая сущность, переживающая перезапуск.
- `EntityGraph::upsert` санитизирует имя, эмитит `Created` / `MetadataChanged` /
  `Reparented`, а при возврате той же `logical` в окне 30 с — `Restarted`.
- `EntityGraph::end_tick` объявляет исчезнувшие сущности удалёнными (фора 1 такт) и
  возвращает `TickBatch { samples, events, records, alive }`.
- `graph.sample` отбрасывает не-finite значения и образцы неизвестных сущностей.
- Метрики уровня процесса имеют `ExportPolicy::OptIn` — по умолчанию не экспортируются.
- Реестр метрик закрыт: новые `MetricId` не добавляются, используются существующие `ids::*`.

## pulse-collect

```rust
pub trait FsSource: Send + Sync + std::fmt::Debug {
    fn read(&self, path: &std::path::Path, cap: usize) -> std::io::Result<Vec<u8>>;
    fn read_dir(&self, path: &std::path::Path) -> std::io::Result<Vec<std::ffi::OsString>>;
    fn read_link(&self, path: &std::path::Path) -> std::io::Result<std::path::PathBuf>;
    fn inode(&self, path: &std::path::Path) -> std::io::Result<u64>;
}
pub struct RealFs;
pub struct FixtureFs { /* in-memory дерево для тестов */ }
impl FixtureFs {
    pub fn new() -> Self;
    pub fn file(self, path: &str, content: &str) -> Self;
    pub fn bytes(self, path: &str, content: &[u8]) -> Self;
    pub fn link(self, path: &str, target: &str) -> Self;
    pub fn inode(self, path: &str, ino: u64) -> Self;
    pub fn remove(&mut self, path: &str);
}
pub fn boot_id(fs: &dyn FsSource, proc_root: &std::path::Path) -> String;
pub fn hostname(fs: &dyn FsSource, proc_root: &std::path::Path) -> String;
pub fn clock_ticks_per_second() -> u64;
pub fn build_collectors(cfg: &pulse_core::Config, fs: std::sync::Arc<dyn FsSource>)
    -> Vec<Box<dyn pulse_core::Collector>>;

pub mod actions {
    pub enum Signal { Term, Kill, Hup, Int }
    pub struct ActionError; // реализует Display
    /// Посылает сигнал только если (pid, start_ticks) совпадает с текущим состоянием ядра.
    pub fn signal_process(fs: &dyn FsSource, proc_root: &std::path::Path,
                          pid: i32, start_ticks: u64, sig: Signal) -> Result<(), ActionError>;
}
```

## pulse-store

```rust
pub struct WindowStats {
    pub count: usize, pub min: f64, pub max: f64, pub mean: f64,
    pub stddev: f64, pub first: f64, pub last: f64,
    /// true, если статистика восстановлена из агрегатов тёплого слоя.
    pub approximate: bool,
}
pub struct History { /* ... */ }
impl History {
    pub fn new(cfg: &pulse_core::config::Store) -> Self;
    pub fn ingest(&mut self, batch: &pulse_core::TickBatch);
    pub fn latest(&self) -> pulse_core::LatestValues;
    pub fn series(&self, key: pulse_core::SeriesKey, from: Timestamp, to: Timestamp) -> Vec<(Timestamp, f64)>;
    pub fn rate(&self, key: pulse_core::SeriesKey, from: Timestamp, to: Timestamp) -> Option<f64>;
    pub fn window(&self, key: pulse_core::SeriesKey, from: Timestamp, to: Timestamp) -> Option<WindowStats>;
    pub fn value_at(&self, key: pulse_core::SeriesKey, at: Timestamp) -> Option<f64>;
    pub fn entities_at(&self, at: Timestamp) -> Vec<&pulse_core::EntityRecord>;
    pub fn entity_record(&self, id: pulse_core::EntityId) -> Option<&pulse_core::EntityRecord>;
    pub fn events_between(&self, from: Timestamp, to: Timestamp) -> Vec<&pulse_core::Event>;
    pub fn recent_events(&self, limit: usize) -> Vec<&pulse_core::Event>;
    pub fn oldest(&self) -> Timestamp;
    pub fn newest(&self) -> Timestamp;
    pub fn ticks(&self) -> Vec<Timestamp>;
    pub fn series_count(&self) -> usize;
    pub fn dropped_series(&self) -> u64;
    pub fn samples_stored(&self) -> u64;
    pub fn events_total(&self) -> u64;
    pub fn approx_bytes(&self) -> u64;
    pub fn evicted_buckets(&self) -> u64;
    pub fn evicted_hot_ticks(&self) -> u64;
}
```

Обязательные свойства:

1. Counter-серии хранятся сырыми; `rate` считает скорость на чтении и обрабатывает
   сброс счётчика суммированием положительных приращений, а не `last - first`.
2. `window().stddev` — алгоритм Уэлфорда, не сумма квадратов. Если окно
   обслужено тёплым слоем, `approximate = true`: `stddev` посчитан по средним
   бакетов и занижен, поэтому потребитель обязан не делить на него (см.
   `pulse-engine`, ветка относительного изменения).
3. `value_at` — последнее значение строго не позже `at`, с лимитом давности.
   Тёплый бакет отдаётся только если его последнее наблюдение не в будущем
   относительно `at`.
4. Жёсткий предел `cfg.max_series`; при переполнении новые серии не создаются,
   растёт `dropped_series()`.
5. Тёплый слой хранит агрегаты только метрик из списка `pulse_core::metric::LONG_WINDOW` (22 метрики) и только для сущностей, не являющихся `Process`; остальные метрики живут только в горячем кольце. Указанный список также используется в pulse-engine как набор сравниваемых метрик (`WATCHED`), чтобы ретеншн и diff оставались согласованными.
6. Записи `EntityRecord` живут после смерти сущности — именно они дают A/B diff
   возможность сказать «что исчезло».
7. Предел памяти `store.max_bytes` (по умолчанию 64 МиБ, `0` отключает проверку).
   При превышении сначала сокращается тёплый слой, затем горячее окно, но не
   ниже двух тактов; журналы сущностей/событий и свежие значения сохраняются.
   Счётчики доступны через `History::evicted_buckets()` и
   `History::evicted_hot_ticks()`, при первом вытеснении пишется warning.

## pulse-engine

```rust
pub struct Analyzer { /* ... */ }
impl Analyzer {
    pub fn new(rules: pulse_core::config::Rules) -> Self;
    pub fn evaluate(&mut self, graph: &pulse_core::EntityGraph,
                    latest: &pulse_core::LatestValues,
                    history: &pulse_store::History,
                    now: Timestamp) -> Vec<pulse_core::Problem>;
    pub fn take_events(&mut self) -> Vec<pulse_core::Event>;
}

pub struct DiffOptions { pub baseline_secs: u64, pub max_evidence: usize, pub min_score: f64 }
// Default: baseline_secs = 30, max_evidence = 5, min_score = 0.2

pub enum ChangeKind { Created, Deleted, Restarted, Reparented, MetadataChanged }
pub struct StructuralChange {
    pub kind: ChangeKind, pub entity: pulse_core::EntityId,
    pub entity_kind: pulse_core::EntityKind, pub name: String, pub detail: String,
}
pub struct MetricChange {
    pub entity: pulse_core::EntityId, pub name: String, pub metric: pulse_core::MetricId,
    pub before: f64, pub after: f64, pub delta: f64, pub z: f64,
}
pub struct ScoredEvidence { pub score: f64, pub text: String, pub at: Option<Timestamp> }
pub struct DiffReport {
    pub a: Timestamp, pub b: Timestamp,
    pub structural: Vec<StructuralChange>, pub metrics: Vec<MetricChange>,
    pub events: Vec<pulse_core::Event>, pub evidence: Vec<ScoredEvidence>,
}
pub fn diff(history: &pulse_store::History, a: Timestamp, b: Timestamp, opts: &DiffOptions) -> DiffReport;
pub fn render_text(report: &DiffReport) -> String;
```

Формула ранжирования доказательств (реализовать буквально, каждая часть — отдельная
чистая функция с тестами на границах):

```
score     = 0.35*magnitude + 0.25*temporal + 0.25*graph + 0.15*resource
magnitude = min(|z|, 4.0) / 4.0
temporal  = max(0, 1 - |t_anchor - t_evidence| / 120s); событие позже anchor -> 0
graph     = 1/(1+hops) по цепочкам parent из EntityRecord; hops > 4 -> 0
resource  = 1.0 только при ПОДТВЕРЖДЁННОЙ зависимости:
            потребитель ссылается на устройство меткой `disks` (её ставит
            pulse-collect по io.stat, то есть по факту трафика), либо два
            потребителя делят одного родителя-cgroup; иначе 0.0.
            Вид сущности сам по себе зависимости не доказывает: балл «диск против
            любого потребителя» добавлял бы одинаковую константу всем парам и
            ничего не различал. Сигнатура: resource_score(&EntityRecord, &EntityRecord).
            Структурные изменения ранжируются с resource = 0: у исчезнувшей
            сущности метки могут быть уже недоступны.
```

Тексты доказательств строго корреляционные: «вместе с этим изменилось …».
Слова «вызвал», «причина», «caused» запрещены — причинность не доказана.

## pulse-export

```rust
pub struct RenderStats { pub series: usize, pub dropped: u64, pub bytes: usize }
pub fn render_openmetrics(snapshot: &pulse_core::Snapshot,
                          cfg: &pulse_core::config::Export) -> (String, RenderStats);
pub struct ExportRuntimeStats { pub requests: u64, pub rejected: u64, pub series: usize, pub dropped: u64 }
pub struct ExportHandle { /* ... */ }
impl ExportHandle {
    pub fn shutdown(self);
    pub fn stats(&self) -> ExportRuntimeStats;
    pub fn local_addr(&self) -> std::net::SocketAddr;
}
pub fn spawn(cfg: &pulse_core::config::Export, source: pulse_core::SnapshotSource)
    -> std::io::Result<ExportHandle>;
```

Аутентификация — единственная реализация в модуле `auth`, сервер обязан
использовать её, а не собственные копии:

```rust
pub const MIN_TOKEN_BYTES: usize = 16;
pub fn load_token(path: &std::path::Path) -> std::io::Result<Vec<u8>>;
pub fn parse_bearer(header: &str) -> Option<&str>;
pub fn constant_time_equal(a: &[u8], b: &[u8]) -> bool;
```

`load_token` обрезает только краевые пробелы и **отклоняет** внутренние: такой
токен нельзя передать в заголовке, и агент иначе поднялся бы с секретом, который
никакой клиент предъявить не может.

Обязательные свойства безопасности:

- bind по умолчанию только loopback; не-loopback без токена — отказ старта;
- токен из файла, минимум 16 байт, сравнение за константное время, проверка до
  сборки ответа; ответ 401 с `WWW-Authenticate: Bearer`;
- маршруты: `GET /metrics`, `GET /healthz` (без токена), `GET /`; остальное 404;
  не-GET/HEAD — 405; тело > 8 KiB — 413; слишком длинный URL — 414;
- rate-limit по адресу с ограничением размера таблицы адресов;
- доступность: соединения обрабатываются параллельно с лимитом `MAX_INFLIGHT = 8`,
  при переполнении — немедленный 503 без чтения тела; чтение заголовка ограничено
  не только таймаутом сокета, но и общим бюджетом `REQUEST_BUDGET = 10 с`, а путь
  отказа 429 — бюджетом 1 с. Без этого один медленный клиент занимал бы сервер
  произвольно долго, не предъявив токена;
- метка `cmdline` и любые аргументы процессов никогда не попадают в вывод;
- значения метк экранируются по OpenMetrics и проходят `sanitize_display`;
- ответ завершается строкой `# EOF`.

## pulse-tui

```rust
pub fn run(config: &pulse_core::Config,
           snapshot: pulse_core::SnapshotSource,
           history: std::sync::Arc<std::sync::RwLock<pulse_store::History>>) -> std::io::Result<()>;
```

```rust
/// Текст A/B-отчёта с честным предупреждением о неполном окне.
pub fn diff_report_text(history: &pulse_store::History,
                        a: Timestamp, b: Timestamp) -> String;
```

Экраны: Overview, Problems, Entities, Inspector, Timeline/Diff, Help.
Клавиши: `↑↓/jk` выбор, `Enter` drill-down, `Esc` назад, `Tab` следующий экран,
`1..5` экраны, `/` поиск, `:` палитра команд, `T` timeline, `A`/`B` маркеры,
`D` diff, `p` пауза, `s` сортировка, `f` фильтр вида, `?` справка, `q`/`Ctrl+C` выход.
Адаптивная вёрстка: `<90` — одна панель; `90..139` — master|inspector;
`>=140` — master|inspector плюс полоса timeline снизу.
Действия, меняющие систему, показываются только при `security.allow_actions = true`.
Терминал восстанавливается всегда: страж `Drop` покрывает штатный выход и панику,
а `SIGTERM`/`SIGINT`/`SIGHUP`/`SIGQUIT` переводятся в атомарный флаг завершения,
зарегистрированный до включения raw-режима.

## pulse-cli

Собирает конвейер: коллекторы → граф → `TickBatch` → `History` → `Analyzer` →
`Snapshot` (публикуется через `ArcSwap`) → TUI и экспортёр.
Подкоманды: `run` (по умолчанию, TUI), `serve` (без TUI, только `/metrics`),
`top` (однократный текстовый снимок), `diff --from --to`, `scorecard`,
`config print`, `check`.
`serve` завершается по тем же сигналам: экспортёр останавливается, поток сбора
присоединяется, код возврата — успех.

## pulse-tui: композиция и представление (v0.8)

- `layout::OverviewComposition` - композиция главного экрана по ширине и высоте:
  `Stacked` (<130), `StateBesideAttention` (130-159), `WithSelected` (160+).
  Больше места означает больше контекста, а не больше колонок таблицы.
- `layout::bounded(BlockKind, available) -> (width, spare)` - предел полезной
  ширины блока. Остаток отдаётся соседней панели или остаётся пустым.
- `fold::fold` сворачивает технические сущности в логический объект: unit,
  его cgroup и его процессы дают одну строку `unit+12p`. Модель данных не
  меняется - меняется представление. `App::technical_view` возвращает полный
  инвентарь.
- `fold::relevant` отбирает значимые объекты для главного экрана. Полный
  инвентарь живёт на экране Entities.
- Единица `BytesPerSecond` в реестре метрик отделяет скорость от накопленного
  объёма. Суффикс `/s` ставится только для неё. Накопительный счётчик
  показывается через `format::counter_total` с явной подписью `total`.
- Утверждения о спокойствии ограничены длиной наблюдения:
  `observed nominal for N`. Аптайм хоста подписан `up` и никогда не подменяет
  длину наблюдения.

## pulse-tui: единое UI state и dispatch (v0.9)

- `app::App` — единственный authoritative state: `screen` (4 top-level),
  `overlay` (`Search`/`Palette`/`Help`), `visible_panes` (публикует renderer),
  `pane`, per-screen `OverviewState`/`ProblemsState`/`EntitiesState`/
  `TimelineState`, `Option<InspectorSession>`.
- `App::dispatch` — единственный вход событий. Порядок: overlay → Inspector →
  focused visible pane → global. Один key event = максимум одно действие.
- `App::set_visible_panes` вызывается рендерером каждый кадр; если resize скрыл
  focused pane, фокус детерминированно переходит на первый видимый. Скрытая
  selection не может получить `Enter`.
- `InspectorSession` хранит `path: Vec<EntityKey>`; переход по связи к сущности,
  уже присутствующей в пути, усекает путь вместо push (cycle-safe). `Esc` идёт
  по уникальному пути, затем возвращает точный origin screen/pane/selection.
- Inspector и Help не являются top-level screens: Inspector — contextual
  session, Help/Search/Palette — overlays с полным владением вводом.
- Overview: `fold::relevant` возвращает только объекты с объяснимой причиной
  (`RelevanceReason`); при отсутствии причин renderer называет блок
  `KEY ENTITIES`. Selected/Context pane обязателен при ширине >=140 и высоте
  >=18. `layout::bounded` держит max useful width, остаток — намеренная пустота.
- Timeline: rail + State River + metric lanes + Story + Snapshot/Details.
  Lanes рисуют спарклайн только при >=2 реальных точках из
  `History::series_points`, иначе honest `collecting history`. Raw stream —
  secondary subview через палитру `: raw events`.
- Footer — hotkey legend `[1]Overview …`, не Tab bar; Tab остаётся pane-local.

## pulse-store: memory ceiling — жёсткая граница

`History::enforce_memory_ceiling` вызывается один раз за такт после `ingest`.
Порядок вытеснения фиксирован и каждый шаг обязан дать прогресс в байтах:

```text
warm buckets → hot ticks (не ниже 2) → сырые события → мёртвые записи сущностей
```

- Каждый цикл ограничен сверху: `depth` и `hot.capacity()` делятся на два, то
  есть шагов не больше `log2(n)`; события и мёртвые записи отдаются одним
  проходом на нужное число байт.
- Если шаг не уменьшил `approx_bytes()`, цикл немедленно прекращается. Retry
  запрещён: именно повторная попытка без прогресса даёт CPU spin.
- Живые записи сущностей и последние значения серий не вытесняются никогда:
  без них снимок перестаёт объяснять сам себя. Поэтому абсурдно малый бюджет
  честно превышается, и это отражается счётчиком `eviction_no_progress`, а не
  бесконечным циклом.
- `approx_bytes()` для hot/warm считается по инкрементальным счётчикам
  удерживаемых точек и бакетов, а не обходом всех серий: обход был O(N) на
  каждой проверке потолка.
- Диагностика: `evicted_buckets`, `evicted_hot_ticks`, `evicted_events`,
  `evicted_entity_records`, `eviction_iterations`, `eviction_no_progress`,
  `peak_bytes`. Предупреждение о достижении потолка печатается один раз на
  переход, а не на каждый такт.

## pulse-core: семантический конвейер событий

Единственное место, где решается «что значимо» — `pulse_core::semantic`.

```text
RAW → significance() → drop Tier0 → group by identity → budget → MEANINGFUL
```

- `Significance::Noise` (Tier 0): рутинный churn ядра и метаданных. Не попадает
  ни в Recent Changes, ни в Story, ни в relevance. Определяется парой
  «вид события + вид сущности» плюс `is_kernel_thread` по стабильным префиксам
  ядра Linux.
- `Significance::Operational` (Tier 1): появление, исчезновение, переименование
  сервиса, контейнера, pod, диска, интерфейса.
- `Significance::Diagnostic` (Tier 2): OOM, открытие и закрытие проблемы,
  ошибка коллектора, перезапуск логической сущности, начало наблюдения.
- Повторы сворачиваются в `MeaningfulEvent` с `count`, `at` (первое) и
  `last_at` (последнее). Сервисы и pod не смешиваются между собой; контейнеры и
  процессы схлопываются по виду.
- Бюджет: `STORY_BUDGET = 20`, `RECENT_CHANGES_BUDGET = 8`. Сортировка ставит
  диагностику выше операционных изменений, поэтому OOM не вытесняется десятком
  безобидных фактов.
- `Snapshot` несёт и сырой журнал (`events`), и результат (`meaningful`,
  `suppressed_noise`, `grouped_events`, `over_budget`). Экраны читают
  `meaningful`; `: raw events` читает `events`.

## pulse-tui: relevance как взвешенная сумма

`fold::scored_reasons` возвращает причины с весами; `WHY` показывает
сильнейшую, ранжирование использует сумму.

```text
problem 100000 | critical 80000 | warning 40000
changed  6000  | cpu  ядра×1000 (≤60000) | memory  MiB/16 (≤20000)
io ≥300 | net ≥200 | system 1500
```

Фоновый поток ядра без проблемы и без ненормального состояния не получает ни
одной причины и потому не попадает в Relevant Entities: он остаётся в полном
инвентаре и в technical view.

Имя логического объекта: если оно выглядит как непрозрачный идентификатор
(`is_opaque_id`), показывается имя главного по памяти процесса, а сам
идентификатор сохраняется в `LogicalRow::technical_id` и печатается строкой
`ID` в панели выбранного.

## Диагностика не печатается в терминал TUI

`pulse run` устанавливает подписчика `tracing`, который пишет в файл из
`PULSE_LOG` либо отбрасывает записи. Запись в stderr запрещена: ratatui владеет
каждой ячейкой экрана и не перерисовывает те, которые считает неизменными, —
одна строка журнала из потока сбора остаётся висеть посреди кадра и переживает
переключение экранов. Остальные команды (`serve`, `top`, `diff`, `scorecard`,
`check`, `config print`) пишут в stderr как раньше.

Информация при этом не теряется: состояние потолка памяти видно в блоке
`ATTENTION` (`history retention reduced …` / `history at memory ceiling …`),
ошибки коллекторов — событиями `collector_error`, а числа — в `pulse scorecard`.

## pulse-tui: производный вид считается один раз на такт

`App::derived(snapshot)` возвращает `Rc<Derived>` с тремя готовыми списками:
`rows` (технические строки), `logical` (свёртка с учётом режима вида) и
`relevant` (значимые для Overview). Кэш инвалидируется ключом
`(tick, kind_filter, sort, direction, technical_view, query)`.

Причина: построение строки требует вычисления владельца, состояния и потоков, и
на хосте с двумя тысячами сущностей полный список стоил около 9 мс. Без кэша он
строился дважды на каждое нажатие клавиши (маршрутизация ввода и отрисовка) и
заново на каждый кадр при неизменных данных.

Индекс связей `Snapshot::relations_index` обязателен для этого пути:
`relations_of` без него сканировал весь вектор связей на каждый вызов, то есть
давал квадратичную стоимость кадра.

Измерено на 1955 сущностях, release: кадр **32.3 мс → 0.81 мс**.

## Цепочка расследования (pulse-tui/investigate.rs)

Граф связен, расследование - нет. Оператор идёт от общего к конкретному и
обязан приходить к ответу, а не возвращаться на тот же уровень.

Уровни: `host(0) → workload: unit|pod(1) → container(2) → cgroup(3) → process(4)`.
Диск и интерфейс - `Resource`: они вне вертикали.

Правила шага вниз (`Enter`, список `CHAIN`):

| Откуда | Куда | Разрешено | Почему |
|---|---|---|---|
| любой | строго глубже по связи | да | это и есть спуск |
| любой | тот же уровень по дереву `parent` | да | вложенные cgroup - дерево, оно ациклично |
| любой | тот же уровень по связи | нет | ровно это давало круги сервис → диск → сервис |
| любой | ресурс | нет | диском пользуются десятки объектов; он лист, не пересадка |
| ресурс | куда-либо | нет | иначе диск становится узлом между несвязанными сервисами |

Подпись звена - **вид цели**, а не направление связи. Причина в реальном
графе: коллектор делает `unit` ребёнком своей cgroup (`EntitySpec::parent`),
хотя семантически unit владеет ею. Направленная подпись давала на живом хосте
шаг «owner angie.service» из `angie.service` - оператор видел круг вместо
спуска.

`RELATED` (`Tab` переключает, кому достаётся `Enter`) - боковые переходы:
владелец не глубже текущего объекта и соседи по общему ресурсу. Переход по
ним начинает **новое** расследование (`InspectorSession::restart`), потому что
смежный объект не лежит глубже: склеивать «спустился» и «прыгнул» в один путь
значит врать о причинной связи.

Свёртка однотипных: сначала собираются уникальные объекты, потом сворачиваются.
Обратный порядок давал неверный счёт - свёрнутая строка хранит ключ лишь
первого объекта, и на живом хосте 14 процессов показывались как «5 × process».
Счётная форма `N × kind`, а не суффикс `s`: вид приходит из реестра и не
обязан быть склоняемым словом.

## Детали процесса (pulse-core/details.rs, pulse-collect/details.rs)

Терминальный уровень цепочки: `user`, `exe`, `cwd`, слушающие порты, открытые
файлы. Домен и контракт `ProcessDetailsSource` живут в `pulse-core`, чтение
`/proc` - в `pulse-collect`, интерфейс зависит от контракта, а не от файловой
системы.

Инварианты:

- Чтение **по требованию**, только для открытого процесса. Обход
  `/proc/<pid>/fd` для двух тысяч процессов - десятки тысяч syscall на такт,
  то есть агент сам становится проблемой.
- `DetailsCache` сбрасывается при смене такта: одно чтение на процесс на такт,
  иначе нажатие клавиши стоило бы новый обход `/proc`.
- Санитизация в источнике: имя файла и путь задаёт сам процесс и может
  содержать управляющие последовательности терминала.
- `restricted` отличает отказ ядра в правах от отсутствия файлов. Пустой блок
  без пометки читался бы как «процесс не держит ни файлов, ни портов».
- Таблицы сокетов читаются с явным лимитом 4 МиБ: 64 КиБ кончаются на четырёх
  сотнях соединений, и слушающий сокет ушёл бы за обрезку.
- Детали не попадают ни в экспорт, ни в историю: они существуют только на
  экране Inspector.

## Ограниченное завершение TUI (pulse-tui/lib.rs)

Наблюдённый на живом хосте дефект: после смерти эмулятора терминала (умер
сервер tmux, оборван ssh) главный поток крутился на 100% ядра внутри опроса
ввода, не доходил до проверки флага завершения, игнорировал `kill` и продолжал
держать порт экспортёра.

`spawn_shutdown_watchdog` ждёт флаг сигнала, даёт циклу 1.5 с выйти самому,
затем восстанавливает терминал и завершает процесс. «kill завершает агента» -
инвариант, не зависящий от поведения библиотеки ввода.

Ни закрытая труба, ни закрытый мастер pty этот дефект не воспроизводят:
процесс выходит сам. Живая репродукция - `scripts/repro-terminal-loss.sh`
(смерть сервера tmux под работающим интерфейсом), она включена в
`scripts/wsl-verify.sh`.

## Файловые системы и скорость сети (pulse-collect)

- Заполненность считается по формуле `df`: `used / (used + available)`, где
  `used = total - free`. Отклонение даёт расхождение в разы на ext4 с
  пятипроцентным резервом root: агент показывал 6% там, где `df` показывает 2%.
- Процент округляется вверх, как в `df`: оператор сравнивает числа в одной
  консоли.
- Служебные монтирования (overlay, snap, внутренности WSL) отфильтрованы, но
  настоящие смонтированные диски - нет: `/mnt/data` на 94% обязан быть виден.
- Скорость сети и диска по хосту публикуется отдельными сериями агрегата
  (`HOST_NET_*_THROUGHPUT`, `HOST_DISK_*_THROUGHPUT`). Раньше Overview читал
  `NETIF_*` на хосте - серии, которой там нет, - и месяцами печатал уверенный
  ноль вместо измерения.
