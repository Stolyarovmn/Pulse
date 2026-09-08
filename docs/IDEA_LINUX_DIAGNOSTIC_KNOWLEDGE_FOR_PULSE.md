# Идея: Linux Diagnostic Knowledge для Pulse

> Статус: design idea / research + implementation roadmap  
> Проект: [Stolyarovmn/Pulse](https://github.com/Stolyarovmn/Pulse)  
> Срез репозитория, по которому сформулирована идея: `main` @ `3b79b43db0f73e9bce819cbe6be90915f26bb165` от 2026-09-07  
> Цель документа: передать агенту задачу на исследование, проектирование и поэтапное внедрение систематизированной Linux-диагностики без LLM-зависимости и без недоказанных причинных утверждений.

---

## 1. Краткая формулировка идеи

Pulse уже умеет собирать Linux-телеметрию, строить `EntityGraph`, хранить короткую историю, находить проблемы детерминированными правилами и прикладывать к каждой проблеме `Evidence`.

Следующий шаг — **не писать десятки новых эвристик с нуля**, а систематически извлечь накопленные Linux/SRE-эвристики из зрелых открытых источников, нормализовать их под модель Pulse и построить поверх существующего `Problem` второй уровень: **объяснимую корреляционную диагностику**.

Идея состоит из двух частей:

1. **Linux Knowledge Catalogue** — каталог проверяемых эвристик вида «какой симптом считается проблемой, какие сигналы нужны, какие дополнительные проверки полезны, что противоречит гипотезе, что измерить дальше».
2. **Diagnostic Correlator** — детерминированный слой Pulse, который связывает уже обнаруженные проблемы, события, историю и отношения `EntityGraph` в более содержательные диагнозы, не выдавая предположение за факт.

Основные внешние источники знаний:

- Performance Co-Pilot `pmie` / `pmieconf` — готовая база inference/performance rules;
- Netdata stock health rules — большой набор production alert-эвристик;
- Prometheus Node Exporter Mixin — современные host-level alert rules, включая trend/prediction;
- Brendan Gregg USE Method / Linux USE checklist — систематическая матрица `utilization / saturation / errors`;
- Linux kernel documentation — каноническая семантика PSI, cgroup v2, VM, block IO, scheduler и т. п.;
- BCC/eBPF/perf tooling — не основной ruleset, а источник стратегии «что измерить следующим» после обнаружения симптома;
- Falco rules — отдельно, как будущий security-набор, не смешивать с performance/reliability диагностикой.

Ключевая мысль: **внешние проекты являются источниками инженерных знаний, а не runtime-зависимостями Pulse**.

---

## 2. Почему это хорошо подходит текущему Pulse

На текущем `main` Pulse уже имеет нужную основу:

- `procfs`, `sysfs`, cgroup v2 collectors;
- сущности `host`, `cgroup`, `systemd unit`, `process`, `disk`, `network interface`, `container`, `pod`;
- устойчивую идентичность процессов `(pid, start_ticks)` и cgroup по inode;
- CPU, memory, PSI, disk, network, process и cgroup telemetry;
- `EntityGraph`;
- историю и temporal queries;
- semantic A/B diff;
- детерминированные диагностические правила;
- `Evidence`;
- общий `Analyzer` с гистерезисом и событиями открытия/закрытия проблем;
- Inspector с переходом от host/unit/container/cgroup к process и process details.

См. текущий README:

- <https://github.com/Stolyarovmn/Pulse/blob/main/README.md>

Текущая реализация правил:

- <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-engine/src/rules.rs>
- <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-engine/src/analyzer.rs>
- <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-core/src/problem.rs>

В `rules.rs` уже закреплён правильный принцип: правила детерминированы, мини-DSL сознательно не вводится, каждое правило само обязано формировать доказательства. Эту идею нужно **сохранить**, а не заменить универсальным YAML-интерпретатором.

---

## 3. Что Pulse делает сейчас, а чего не хватает

Сейчас основной поток примерно такой:

```text
procfs / sysfs / cgroup v2
          │
          ▼
      collectors
          │
          ▼
 EntityGraph + LatestValues + History
          │
          ▼
        Rule
          │
          ▼
       RuleHit
          │
          ▼
       Analyzer
       hysteresis
          │
          ▼
        Problem
       + Evidence
```

Существующие правила уже покрывают, среди прочего:

```text
psi.cpu
psi.memory
psi.io
cgroup.throttle
memory.pressure
disk.latency
fd.pressure
swap.pressure
memory.oom
```

Это хороший **signal/problem layer**. Он отвечает на вопрос:

> Что сейчас измеримо ненормально?

Например:

```text
memory.pressure
  memory utilization = 97%
  limit = 512 MiB
```

или:

```text
cgroup.throttle
  throttled periods = 34%
  CPU limit = 2 cores
```

Но оператору нужен ещё один уровень:

> Как связаны эти факты? Что из них является причиной, следствием или просто сопутствующим сигналом? Какие данные подтверждают гипотезу? Какие данные ей противоречат? Чего не хватает для уверенного вывода?

Именно этот уровень предлагается добавить.

---

## 4. Не превращать Pulse в «умный алертинг»

Важно не свести задачу к импорту сотен порогов.

Плохая цель:

```text
RAM > 90% → warning
loadavg > N → warning
await > X → warning
```

Хорошая цель:

```text
symptom
  + temporal behaviour
  + related entity state
  + resource limit/configuration
  + supporting evidence
  + contradicting evidence
  + topology / ownership
  → explainable diagnosis
```

Пример:

```text
memory.oom on api.service
        │
        ├─ memory.current ≈ memory.max     ✓
        ├─ memory utilization 97%          ✓
        ├─ Memory PSI increased first      ✓
        ├─ host has plenty of free memory  ✓ context
        └─ oom_kill +1                     ✓
                 │
                 ▼
        cgroup memory limit exhausted
        confidence: confirmed/strong
```

Это намного полезнее, чем три независимых красных строки.

---

## 5. Основные источники Linux-эвристик

### 5.1. Performance Co-Pilot: `pmie` / `pmieconf`

Главный кандидат на источник №1.

Performance Co-Pilot содержит Performance Metrics Inference Engine (`pmie`) и поставляемые правила `pmieconf` для автоматической интерпретации performance-метрик.

Upstream:

- <https://github.com/performancecopilot/pcp>
- <https://github.com/performancecopilot/pcp/tree/main/src/pmieconf>
- `pmie` man page: <https://man7.org/linux/man-pages/man1/pmie.1.html>
- `pmieconf` man page: <https://man7.org/linux/man-pages/man5/pmieconf.5.html>

В актуальном дереве `src/pmieconf/` присутствуют категории:

```text
cpu/
memory/
filesys/
network/
percpu/
perdisk/
pernetif/
entropy/
...
```

Примеры из `memory/`:

```text
exhausted
oom_kill
swap_low
```

Примеры из `cpu/`:

```text
context_switch
excess_fpe
load_average
low_util
syscall
system
util
```

Почему PCP особенно интересен для Pulse:

- это не просто список metric thresholds;
- правила рассчитаны на временные ряды;
- есть inference-идея, близкая к задуманной архитектуре Pulse;
- правила организованы по системным ресурсам;
- многие проверки можно адаптировать к уже имеющимся `MetricId`.

Задача агента: **не копировать выражения механически**, а извлечь из каждого правила его инженерный смысл, предпосылки и необходимые сигналы.

---

### 5.2. Netdata stock health rules

Netdata содержит большую штатную базу health/alert definitions.

Upstream:

- <https://github.com/netdata/netdata>
- health rules: <https://github.com/netdata/netdata/tree/master/src/health/health.d>
- health reference: <https://github.com/netdata/netdata/blob/master/src/health/REFERENCE.md>
- documentation: <https://learn.netdata.cloud/docs/alerts-&-notifications>

Ценность:

- production-tested набор условий;
- много subsystem-specific rules;
- окна времени;
- предупреждения vs critical;
- часть правил учитывает предыдущие состояния;
- есть filesystem/systemd/network/TCP и другие области.

Использовать как источник для:

```text
metric(s) + window + state → symptom/problem
```

Но не считать каждое Netdata-правило автоматически root cause.

---

### 5.3. Prometheus Node Exporter Mixin

Upstream:

- <https://github.com/prometheus/node_exporter/tree/master/docs/node-mixin>
- alerts: <https://github.com/prometheus/node_exporter/blob/master/docs/node-mixin/alerts/alerts.libsonnet>
- config: <https://github.com/prometheus/node_exporter/blob/master/docs/node-mixin/config.libsonnet>

Особенно интересны правила, которые используют не только текущее значение, но и производные/тренды.

Например, класс `FilesystemSpaceFillingUp` использует прогноз по истории (`predict_linear`) и тем самым отвечает не только на вопрос «сколько места осталось», но и «закончится ли место при текущей скорости роста».

Для Pulse с его собственной history это важный класс эвристик:

```text
current value
+
rate/slope
+
prediction horizon
→ future exhaustion
```

Похожие идеи применимы к:

- filesystem space;
- inode exhaustion;
- FD exhaustion;
- memory growth;
- process growth;
- queue growth;
- network error growth.

---

### 5.4. Brendan Gregg USE Method

USE = для каждого ресурса проверять:

```text
Utilization
Saturation
Errors
```

Linux checklist:

- <https://www.brendangregg.com/USEmethod/use-linux.html>

Это не готовый machine-readable ruleset, но это отличный **coverage framework**.

Для Pulse нужно построить таблицу:

| Resource | Utilization | Saturation | Errors | Pulse coverage |
|---|---|---|---|---|
| CPU | ? | ? | ? | audit |
| Memory | ? | ? | ? | audit |
| Block device | ? | ? | ? | audit |
| Filesystem | ? | ? | ? | audit |
| Network interface | ? | ? | ? | audit |
| cgroup CPU | ? | ? | ? | audit |
| cgroup memory | ? | ? | ? | audit |
| process | ? | ? | ? | audit |
| FD | ? | ? | ? | audit |

Не заполнять таблицу догадками. Агент должен для каждого пункта указать конкретные существующие `MetricId` либо точный gap.

USE нужен не столько для новых правил, сколько чтобы увидеть, **какие классы наблюдаемости Pulse вообще не покрывает**.

---

### 5.5. Linux kernel documentation

Нельзя переносить семантику PSI, cgroup memory events, VM counters, scheduler counters и block statistics только из сторонних alert rules.

Финальная трактовка каждого Linux-факта должна проверяться по канонической документации ядра.

Базовые источники:

- Kernel docs: <https://docs.kernel.org/>
- PSI: <https://docs.kernel.org/accounting/psi.html>
- cgroup v2: <https://docs.kernel.org/admin-guide/cgroup-v2.html>
- procfs: <https://docs.kernel.org/filesystems/proc.html>
- block statistics: <https://docs.kernel.org/admin-guide/iostats.html>
- VM sysctl / memory docs: <https://docs.kernel.org/admin-guide/sysctl/vm.html>

Правило для агента:

> Сторонний проект может подсказать эвристику, но смысл сырой Linux-метрики должен подтверждаться kernel documentation либо другим первичным источником.

---

### 5.6. BCC / eBPF / perf — как библиотека «следующего измерения»

Это не основной словарь проблем и не нужно тащить eBPF в первую итерацию.

BCC:

- <https://github.com/iovisor/bcc>

Brendan Gregg Linux performance tools:

- <https://www.brendangregg.com/linuxperf.html>

Их роль в будущем:

```text
Pulse обнаружил symptom
        │
        ▼
какого доказательства не хватает?
        │
        ▼
какой cheap/on-demand probe включить?
```

Например:

```text
disk latency high
    ↓
нужно определить contributor
    ↓
on-demand block IO tracing
    ↓
pid / cgroup / file
```

Важный принцип: тяжёлые probes должны включаться **по требованию** и на ограниченное время, а не становиться постоянной стоимостью агента.

---

### 5.7. Falco — отдельная будущая security-база

Falco rules:

- <https://github.com/falcosecurity/rules>

Не включать Falco rules в первую performance/reliability базу. Это другая семантика: syscall/runtime security behaviour.

В будущем это может стать отдельной осью:

```text
PERFORMANCE
RELIABILITY
SECURITY
```

Но не смешивать security finding с resource bottleneck в одном rulespace.

---

## 6. Предлагаемая модель знаний

Не начинать с runtime DSL.

Первоначально Linux Knowledge Catalogue может быть Markdown/CSV/JSON **для исследования и аудита**, но исполняемые правила Pulse должны оставаться Rust-кодом.

Для каждой эвристики в каталоге хранить как минимум:

```text
ID
Domain
Title
What it detects
Source project
Source URL
Source rule/file
Original semantics
Required signals
Optional supporting signals
Contradicting signals
Temporal requirement
Scope
Applicable entities
Next checks
Pulse metrics already available
Missing metrics
Collector required
Estimated runtime cost
False-positive risks
Can assert cause?
Confidence ceiling
Implementation status
Tests required
```

Пример записи:

```yaml
id: memory.cgroup_limit_exhausted

domain: memory
scope: cgroup

symptom:
  - problem: memory.oom

required_evidence:
  - metric: cgroup.memory.events.oom_kill
    relation: delta > 0

supporting_evidence:
  - memory.current approximately memory.max
  - cgroup memory PSI elevated before OOM

context:
  - host memory available

contradicting_evidence:
  - cgroup memory.current far below memory.max

next_checks:
  - largest processes in cgroup
  - recent memory growth
  - parent cgroup limits

result:
  statement: "cgroup reached its memory limit"
  maximum_confidence: confirmed
```

Это лишь schema example. Имена метрик должны быть сопоставлены с фактическими `MetricId` Pulse после аудита.

---

## 7. Разделить Signal, Problem, Diagnosis и Incident Story

Предлагается формально различать четыре уровня.

### 7.1. Signal

Сырой наблюдаемый факт:

```text
Memory PSI full avg10 = 38%
oom_kill +1
disk await = 84ms
process RSS grew by 400MiB
```

### 7.2. Problem

Результат существующего Rule/Analyzer:

```text
memory.pressure
memory.oom
psi.io
disk.latency
cgroup.throttle
```

Это уже есть в Pulse и должно остаться самостоятельным уровнем.

### 7.3. Diagnosis

Корреляция нескольких доказательств:

```text
api.service исчерпал memory.max
postgresql.service является доминирующим источником disk contention
service ограничен CPU quota, а не host CPU capacity
host испытывает memory reclaim pressure
```

### 7.4. Incident Story

Временная последовательность доказанных изменений:

```text
14:02:13 memory crossed 90%
14:02:19 memory PSI started rising
14:02:27 memory reached cgroup limit
14:02:28 oom_kill +1
14:02:29 process disappeared
14:02:30 systemd restarted service
```

Story не обязана утверждать причинность между соседними событиями. Она обязана показать последовательность; причинные утверждения даёт только Diagnosis при достаточных доказательствах.

---

## 8. Предлагаемый `Diagnosis` как отдельная сущность

Не перегружать текущий `Problem`.

Концептуально:

```rust
pub struct Diagnosis {
    pub id: DiagnosisId,
    pub subject: EntityId,

    pub title: String,
    pub statement: String,
    pub confidence: Confidence,

    pub supporting: Vec<DiagnosticEvidence>,
    pub contradicting: Vec<DiagnosticEvidence>,
    pub context: Vec<DiagnosticEvidence>,
    pub missing: Vec<MissingEvidence>,

    pub related_problems: Vec<ProblemId>,
    pub related_events: Vec<EventId>,
    pub related_entities: Vec<EntityId>,

    pub started_at: Timestamp,
    pub updated_at: Timestamp,
}
```

Это proposal, не обязательный API.

### Confidence

Не использовать «магические» проценты вроде `87%`, если нет калиброванной статистической модели.

Предпочтительно:

```rust
pub enum Confidence {
    Possible,
    Strong,
    Confirmed,
}
```

Семантика должна быть формальной.

Например:

- `Possible` — есть symptom и одно поддерживающее доказательство, но остаются конкурирующие объяснения;
- `Strong` — выполнены все required evidence + несколько independent supporting signals;
- `Confirmed` — есть прямой kernel/runtime факт, однозначно подтверждающий утверждение в пределах его формулировки.

Важно: `Confirmed` допускается только для **узкой формулировки**, которую действительно подтверждают данные.

Например:

```text
Confirmed: "cgroup reported an OOM kill"
```

не означает:

```text
Confirmed: "application has a memory leak"
```

Второе требует совершенно других доказательств.

---

## 9. Более богатая evidence model

Текущий `Evidence` хорошо подходит пороговым правилам:

```rust
Evidence {
    label,
    value,
    threshold,
}
```

Для корреляционной диагностики лучше ввести отдельный тип, не ломая текущий.

Концептуально:

```rust
pub enum EvidenceRole {
    Supports,
    Contradicts,
    Context,
}

pub enum FactKind {
    Metric,
    Event,
    State,
    Relation,
    Change,
    Config,
    Trace,
}

pub struct DiagnosticEvidence {
    pub role: EvidenceRole,
    pub kind: FactKind,
    pub entity: EntityId,
    pub observed_at: Timestamp,
    pub description: String,
    pub source: EvidenceSource,
}
```

Нужно избегать превращения evidence в произвольные строки, если факт может быть типизирован.

UI может отображать так:

```text
WHY?

✓ metric    memory.current = 497 MiB
✓ config    memory.max = 512 MiB
✓ event     oom_kill +1
✓ temporal  OOM followed limit saturation
· context   host has 21 GiB available
✗ metric    swap activity is low
```

`✗` означает не «ошибка», а свидетельство против конкретной гипотезы.

---

## 10. Почему `EntityGraph` даёт Pulse преимущество

Большинство alert systems останавливаются на:

```text
sda latency high
```

Pulse может потенциально пройти ownership/topology chain:

```text
host
 ├─ disk sda
 └─ unit postgresql.service
      └─ cgroup
           └─ postgres pid 18291
```

Если доступны attribution-сигналы, можно перейти от:

```text
disk is slow
```

к:

```text
sda is saturated
postgresql.service is the dominant observable contributor
```

Но формулировка должна зависеть от реально доступной attribution data.

Нельзя написать «PostgreSQL вызвал задержку», если Pulse знает лишь, что PostgreSQL и диск одновременно были активны. В таком случае корректно:

```text
postgresql.service is the dominant observed I/O contributor during the saturation window
```

а не:

```text
postgresql caused the disk latency
```

Это принципиальный guardrail проекта.

---

## 11. Примеры Linux-диагнозов, которые стоит исследовать

Это **не утверждение, что текущий Pulse уже имеет все нужные метрики**. Это начальный backlog для gap-analysis.

### CPU

```text
cpu.host_saturation
cpu.run_queue_pressure
cpu.quota_bottleneck
cpu.scheduler_contention
cpu.high_system_time
cpu.context_switch_storm
cpu.single_thread_bottleneck
cpu.steal_pressure          # если среда виртуализирована и метрика доступна
```

### Memory

```text
memory.host_pressure
memory.cgroup_limit_exhausted
memory.reclaim_pressure
memory.swap_pressure
memory.swap_thrashing
memory.oom_host
memory.oom_cgroup
memory.major_fault_pressure
memory.possible_unbounded_growth
```

### Disk / block IO

```text
io.device_saturation
io.high_service_latency
io.queue_pressure
io.write_heavy_contention
io.read_heavy_contention
io.single_dominant_contributor
io.device_error_condition
```

### Filesystems

```text
fs.space_low
fs.space_filling_up
fs.inodes_low
fs.inodes_filling_up
fs.read_only
```

### Network

```text
net.interface_saturation
net.rx_errors
net.tx_errors
net.drops
net.retransmission_pressure
net.connection_pressure
net.listen_backlog_pressure
```

### Processes / services

```text
process.runaway_cpu
process.memory_growth
process.fd_exhaustion
process.churn
process.restart_loop
process.zombie_growth
service.restart_loop
service.failed
service.resource_limit_hit
```

### Host/kernel

```text
host.fd_exhaustion
host.pid_exhaustion
host.clock_or_time_anomaly
host.entropy_low            # только если всё ещё релевантно современному ядру/сценарию
host.kernel_error_rate
```

Каждый кандидат сначала проходит source validation и availability audit.

---

## 12. Пример: CPU quota bottleneck

Плохая диагностика:

```text
CPU usage high
```

Более полезная:

```text
cgroup CPU quota is constraining the workload
```

Возможный evidence chain:

```text
cgroup.throttle is open
        │
        ├─ CPU quota exists                         required
        ├─ throttled ratio high                    required/supporting
        ├─ cgroup CPU usage near quota             supporting
        ├─ cgroup CPU PSI elevated                 supporting
        └─ host CPU has spare capacity             contradicts host saturation
```

Если host CPU также полностью saturated, диагноз должен быть осторожнее:

```text
CPU throttling is present, but host-level saturation is also present;
quota is not proven to be the only bottleneck.
```

Именно поддержка **contradicting evidence** отличает RCA от обычного алертинга.

---

## 13. Пример: cgroup memory limit exhaustion

```text
memory.oom
   +
memory.current ≈ memory.max
   +
pre-OOM memory PSI / pressure
   +
host-wide memory not exhausted
   ↓
strong/confirmed narrow conclusion:
"the cgroup hit its configured memory boundary and reported an OOM kill"
```

Не делать следующий необоснованный скачок:

```text
"application has a memory leak"
```

Для leak нужны тренд, workload context, expected steady state и/или profile-level evidence.

---

## 14. Пример: filesystem filling trend

Использовать идею Node Exporter Mixin:

```text
free space now
+
space consumption slope over history
+
prediction horizon
+
filesystem writable state
→ likely future exhaustion
```

Pulse уже имеет историю, поэтому можно сделать нативно, без PromQL.

UI:

```text
WARN  /var is filling

12.4 GiB free (18%)
consumption trend: +1.9 GiB/hour
projected exhaustion: ~6h 30m

Evidence
✓ filesystem is writable
✓ free space declining consistently
✓ prediction crosses zero inside configured horizon
```

Prediction — математический прогноз, а не причинное утверждение. Нужно показывать окно и модель/метод расчёта.

---

## 15. Архитектура после добавления correlation layer

```text
                           COLLECTORS
                               │
                               ▼
             ┌────────────────────────────────┐
             │ EntityGraph                   │
             │ LatestValues                  │
             │ History                       │
             │ Events                        │
             │ Semantic Diff                 │
             └───────────────┬────────────────┘
                             │
                   LEVEL 1   ▼
                   ┌──────────────────┐
                   │ Signal Rules     │
                   │ existing engine  │
                   └────────┬─────────┘
                            │
                         Problems
                         Evidence
                            │
                   LEVEL 2   ▼
                ┌─────────────────────┐
                │ Diagnostic          │
                │ Correlator          │
                └─────────┬───────────┘
                          │
                       Diagnosis
                          │
                          ▼
                    Incident Story
```

Возможная структура crate:

```text
crates/pulse-engine/src/
├── analyzer.rs
├── rules.rs
├── diff.rs
└── diagnostics/
    ├── mod.rs
    ├── context.rs
    ├── correlator.rs
    ├── cpu.rs
    ├── memory.rs
    ├── io.rs
    ├── filesystem.rs
    ├── network.rs
    ├── process.rs
    └── systemd.rs
```

Это proposal. Перед созданием файлов агент должен проверить, не лучше ли текущая crate boundary допускает другой layout.

---

## 16. Не вводить общий YAML DSL на первом этапе

Текущая позиция `rules.rs` правильная: universal mini-DSL создаст новый слой, который придётся отдельно валидировать, отлаживать и версионировать.

Не делать сразу:

```yaml
when:
  any:
    - metric: foo
      gt: 0.95
  then:
    cause: bar
```

Вместо этого:

```rust
struct CgroupMemoryLimitDiagnosis;
struct CpuQuotaDiagnosis;
struct DiskSaturationDiagnosis;
```

с небольшими переиспользуемыми primitives:

```text
problem(...)
metric(...)
rate(...)
delta(...)
trend(...)
changed(...)
ancestor(...)
descendant(...)
related(...)
before(...)
after(...)
during(...)
```

Knowledge Catalogue при этом может оставаться data/documentation layer, а runtime behaviour — Rust.

DSL можно рассматривать позже только если накопится достаточное количество повторяющихся patterns и будет доказано, что он реально уменьшает сложность.

---

## 17. План исследования для агента

### Этап A. Зафиксировать текущие возможности Pulse

Агент должен проанализировать текущий `main`, минимум:

```text
crates/pulse-core/src/metric.rs
crates/pulse-core/src/entity.rs
crates/pulse-core/src/relation.rs
crates/pulse-core/src/event.rs
crates/pulse-core/src/problem.rs
crates/pulse-core/src/semantic.rs
crates/pulse-engine/src/rules.rs
crates/pulse-engine/src/analyzer.rs
crates/pulse-engine/src/diff.rs
crates/pulse-collect/**
crates/pulse-store/**
```

Результат — machine-readable или Markdown inventory:

```text
MetricId
source file (/proc, /sys, cgroup...)
entity kinds
instant/counter/gauge
history available?
derived metric?
cost
collection interval
permissions
```

Нельзя строить gap-analysis по README בלבד — README может быть неполным.

### Этап B. Собрать upstream rule catalogue

Обязательные источники первой волны:

1. PCP `src/pmieconf/`;
2. Netdata `src/health/health.d/`;
3. Node Exporter Mixin alerts/config;
4. USE Linux checklist.

Для каждой найденной эвристики сохранить:

```text
source
exact URL/path
what it checks
metrics used
window
threshold/default
semantic meaning
resource/domain
```

Не ограничиваться названиями файлов.

### Этап C. Нормализация

Свести дублирующие правила разных проектов в одну концепцию.

Например:

```text
PCP disk busy rule
Netdata disk utilization rule
USE disk utilization/saturation
Node mixin disk IO saturation
```

могут оказаться несколькими вариантами одного family:

```text
io.device_saturation
```

При этом сохранить provenance — откуда взят каждый элемент логики.

### Этап D. Gap-analysis Pulse

Для каждой нормализованной эвристики:

```text
READY        все данные уже есть
PARTIAL      часть данных есть
CHEAP GAP    не хватает дешёвого /proc /sys / cgroup чтения
EXPENSIVE    нужен eBPF/perf/tracepoint/privilege
NOT USEFUL   низкая ценность / устаревшая / слишком специфичная
UNCERTAIN    смысл или переносимость не подтверждены
```

Не присваивать READY только потому, что похожая метрика существует. Проверять units/semantics/window/source.

### Этап E. Приоритизация

Приоритет вычислять не «на глаз», а по нескольким осям:

```text
operator value
frequency in real incidents
false-positive risk
runtime cost
permissions required
existing Pulse coverage
explainability
ability to attribute to entity
```

Можно использовать простой ordinal score, но нужно документировать формулу.

### Этап F. Первая реализация

Реализовать небольшой вертикальный slice, например:

```text
CPU quota bottleneck
cgroup memory limit exhaustion
host memory pressure
swap/reclaim pressure
disk saturation
filesystem filling trend
FD exhaustion
service restart loop
```

Фактический список выбрать после gap-analysis, а не фиксировать заранее.

---

## 18. Что должен содержать итоговый research artifact

Предлагаемый файл:

```text
docs/linux-diagnostic-knowledge.md
```

или отдельные:

```text
docs/diagnostics/SOURCES.md
docs/diagnostics/CATALOGUE.md
docs/diagnostics/GAPS.md
docs/diagnostics/DESIGN.md
```

Если каталог становится большим, второй вариант предпочтительнее.

Для каждого правила обязательно приводить provenance:

```text
Source: Performance Co-Pilot
File: src/pmieconf/memory/exhausted
URL: https://github.com/performancecopilot/pcp/...
Reviewed against: Linux kernel docs ...
```

Никаких «известно, что...» без проверяемого источника.

---

## 19. Тестовая стратегия

Каждый диагностический вывод должен иметь детерминированные тесты.

### 19.1. Positive case

Все необходимые доказательства присутствуют → диагноз появляется.

### 19.2. Missing required evidence

Нет ключевого факта → диагноз не появляется либо остаётся `Possible`.

### 19.3. Contradicting evidence

Есть сильное противоречие → confidence понижается или диагноз снимается.

### 19.4. Temporal ordering

Одинаковые значения в разном порядке не должны автоматически приводить к одинаковому причинному утверждению.

Пример:

```text
A: PSI rise → memory limit → OOM
B: OOM event → later unrelated PSI rise
```

Это разные evidence sequences.

### 19.5. Entity isolation

Проблемы разных cgroup/process не должны случайно коррелироваться только потому, что произошли одновременно.

### 19.6. Disappearing entities

Процесс может завершиться между ticks. Диагностика должна сохранять достаточный snapshot identity/evidence, не переиспользовать PID нового процесса.

### 19.7. No-causality regression tests

Должны быть тесты, которые специально запрещают сильную формулировку при недостаточных данных.

Например:

```text
high disk latency + active postgres
```

не должно автоматически давать:

```text
"postgres caused disk latency"
```

---

## 20. Demo/scenario tests

Текущий Pulse уже имеет честный `--demo`, который подменяет нижний слой чтения, но дальше использует реальный pipeline.

Эту же архитектуру использовать для diagnosis scenarios.

Добавить сценарии, которые физически создают последовательность метрик/событий, а не рисуют готовый `Diagnosis`.

Пример:

```text
scenario: cgroup_memory_limit

phase 1: normal
phase 2: memory.current grows
phase 3: memory PSI grows
phase 4: oom_kill counter increments
phase 5: process disappears/restarts
phase 6: recovery
```

Тест проверяет:

```text
Problem(memory.pressure) opens
Problem(memory.oom) opens
Diagnosis(cgroup.memory_limit_exhausted) appears
confidence reaches expected level
incident story ordering is correct
problem/diagnosis eventually clear or become historical
```

---

## 21. UI-принцип

Не делать отдельный «AI explanation panel».

Диагностика должна быть частью существующего расследования.

Пример в TUI:

```text
CRIT  api.service

DIAGNOSIS
  cgroup memory limit exhausted              CONFIRMED

WHY
  ✓ memory.current      497 MiB
  ✓ memory.max          512 MiB
  ✓ memory utilization  97%
  ✓ memory PSI full     38%
  ✓ oom_kill            +1
  · host available      21 GiB

SEQUENCE
  14:02:13  memory crossed 90%
  14:02:19  memory PSI started rising
  14:02:27  memory reached limit
  14:02:28  OOM kill

INSPECT NEXT
  → processes in cgroup
  → memory growth history
  → parent cgroup
```

`INSPECT NEXT` должен быть actionable navigation, если соответствующие сущности/данные доступны.

---

## 22. «Что измерить следующим» как отдельная концепция

Это важное развитие после первого correlation layer.

Для каждой диагностики можно иметь `next_checks`, например:

```text
suspected IO contention
  → inspect top cgroup IO contributors
  → if attribution unavailable: short block tracing session

suspected scheduler contention
  → inspect run queue
  → inspect per-process CPU
  → optional sched latency tracing

suspected memory growth
  → compare process RSS/PSS trend
  → inspect cgroup descendants
  → optional alloc/profile tooling later
```

Это не означает автоматически запускать привилегированные probes.

Модель:

```text
cheap always-on evidence
        ↓
correlator
        ↓
missing evidence identified
        ↓
user-triggered / bounded probe
        ↓
new evidence
        ↓
updated diagnosis
```

Это хорошо сочетается с local-first философией Pulse.

---

## 23. Производительность и стоимость

Любая новая метрика/эвристика должна оцениваться с точки зрения стоимости агента.

Для каждого collector gap фиксировать:

```text
read frequency
syscalls/tick
bytes parsed/tick
cardinality
history cardinality
privileges
expected CPU cost
expected memory cost
```

Использовать существующие:

```bash
scripts/measure-cost.sh
pulse scorecard
```

Тяжёлые источники не включать always-on без измерения.

Особенно осторожно:

```text
/proc/<pid> loops over all processes
fd enumeration
open files
socket correlation
perf events
eBPF maps
stack traces
```

Pulse уже использует on-demand process details — этот принцип стоит сохранять.

---

## 24. Лицензирование и provenance

Перед переносом логики из upstream проектов агент должен проверить лицензии каждого источника.

Нужно различать:

1. изучение идеи/эвристики и самостоятельную реализацию;
2. буквальное копирование rule code/text;
3. перенос порогов/default values;
4. копирование документации/описаний.

В repository documentation хранить источник и ссылку даже если код написан заново.

Не копировать большие фрагменты текста/кода без проверки совместимости лицензий.

---

## 25. Non-goals первой версии

Не делать в первой итерации:

- LLM как обязательную часть диагностики;
- универсальный rule DSL;
- Kubernetes rule database;
- автоматический запуск тяжёлого eBPF tracing для каждого alert;
- сотни импортированных правил без coverage/gap анализа;
- «AI confidence 93%» без статистической калибровки;
- причинные утверждения только по temporal proximity;
- service-specific knowledge вроде PostgreSQL/MySQL/JVM до завершения базовой Linux taxonomy;
- security rules Falco в том же namespace с performance/reliability.

---

## 26. Definition of Done для исследовательской стадии

Research stage считается завершённой, когда есть:

1. Полный inventory текущих Pulse metrics/collectors, используемых для диагностики.
2. Список просмотренных upstream источников с commit/tag/date.
3. Нормализованный Linux diagnostic catalogue.
4. Для каждой записи — точный provenance URL.
5. Для каждой записи — Pulse coverage: `READY/PARTIAL/CHEAP GAP/EXPENSIVE/...`.
6. USE coverage matrix.
7. Список дубликатов/эквивалентных правил разных upstream.
8. Список сомнительных/устаревших эвристик, которые **не** следует переносить, с объяснением.
9. Приоритизированный shortlist первой реализации.
10. Архитектурное предложение `Diagnosis`/correlator с минимальными изменениями существующих crates.
11. План тестов для каждого выбранного diagnosis family.
12. Измеримая оценка стоимости недостающих collectors.

Важно: не заявлять заранее «найдено 80 правил» или «35 уже покрыты». Количества должны быть результатом фактического аудита.

---

## 27. Definition of Done для первой implementation stage

Первая реализация считается успешной, если:

1. Существующий `Problem` API не превращён в свалку RCA-полей.
2. Добавлен отдельный correlation/diagnosis concept.
3. Реализовано несколько high-value Linux diagnoses из разных domains.
4. Каждый вывод полностью детерминирован.
5. Каждый diagnosis показывает supporting evidence.
6. При применимости показывается contradicting/context evidence.
7. Нет числовой «уверенности», не имеющей строгой семантики.
8. Есть unit tests на positive/negative/contradicting/temporal cases.
9. Есть demo scenario минимум для одного многошагового incident.
10. TUI позволяет перейти от diagnosis к участвующим entities/problems/history.
11. `cargo test --workspace` проходит.
12. `clippy`/project lint policy проходит.
13. Стоимость агента до/после измерена существующим scorecard/measure-cost tooling.
14. Документация объясняет происхождение каждой адаптированной эвристики.

---

## 28. Возможная последовательность PR

Не делать всё одним огромным PR.

Предпочтительно:

```text
PR 1  Research: metric inventory + upstream catalogue + gap matrix
PR 2  Core: Diagnosis/Evidence data model, без реальных диагнозов
PR 3  Engine: DiagnosticContext + correlator skeleton
PR 4  Memory diagnoses + tests + demo
PR 5  CPU diagnoses + tests
PR 6  IO/filesystem diagnoses + trend/prediction
PR 7  Network/process/systemd diagnoses
PR 8  TUI: WHY / SEQUENCE / INSPECT NEXT
PR 9  optional on-demand probes design
```

Фактическую разбивку скорректировать после изучения зависимостей.

---

## 29. Главный критерий качества

Pulse не должен быть инструментом, который «звучит умно».

Он должен быть инструментом, у которого любой вывод можно раскрыть до наблюдаемых фактов:

```text
VERDICT
   ↓
EVIDENCE
   ↓
METRIC / EVENT / STATE / RELATION
   ↓
Linux source (/proc, /sys, cgroup, kernel event...)
```

Если такой цепочки нет — Pulse должен либо:

- не показывать диагноз;
- понизить его до `Possible`;
- явно указать недостающие доказательства;
- предложить следующий безопасный способ измерения.

Это соответствует уже заложенному в проекте принципу не делать недоказанных причинных утверждений.

---

## 30. Короткое задание агенту

> Исследуй существующие Linux performance/reliability knowledge bases и спроектируй их применение к Pulse. Не копируй Kubernetes rules и не вводи LLM. Начни с PCP `pmieconf`, Netdata stock health rules, Prometheus Node Exporter Mixin и Brendan Gregg USE Linux checklist. Проверь смысл сырых метрик по Linux kernel documentation. Построй фактический inventory текущих `MetricId`/collectors Pulse, затем нормализованный каталог Linux-эвристик и gap-analysis `READY / PARTIAL / CHEAP GAP / EXPENSIVE / NOT USEFUL / UNCERTAIN`. Не придумывай количество правил заранее. Для каждой эвристики укажи точный upstream source URL, требуемые сигналы, temporal semantics, supporting/contradicting evidence, false-positive risks и стоимость недостающих данных. После аудита предложи минимальный `Diagnosis`/correlator layer поверх существующих `Problem + Evidence + History + EntityGraph`, сохранив детерминированность, объяснимость и запрет на недоказанную причинность. Не вводи общий YAML DSL на первой итерации. Подготовь поэтапный implementation plan и набор детерминированных тестов.

---

## 31. Источники

### Pulse

- Repository: <https://github.com/Stolyarovmn/Pulse>
- README: <https://github.com/Stolyarovmn/Pulse/blob/main/README.md>
- Current rules: <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-engine/src/rules.rs>
- Analyzer: <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-engine/src/analyzer.rs>
- Problem/Evidence model: <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-core/src/problem.rs>
- Metrics: <https://github.com/Stolyarovmn/Pulse/blob/main/crates/pulse-core/src/metric.rs>

### External Linux knowledge sources

- Performance Co-Pilot: <https://github.com/performancecopilot/pcp>
- PCP pmieconf rules: <https://github.com/performancecopilot/pcp/tree/main/src/pmieconf>
- pmie: <https://man7.org/linux/man-pages/man1/pmie.1.html>
- pmieconf: <https://man7.org/linux/man-pages/man5/pmieconf.5.html>
- Netdata: <https://github.com/netdata/netdata>
- Netdata health rules: <https://github.com/netdata/netdata/tree/master/src/health/health.d>
- Node Exporter Mixin: <https://github.com/prometheus/node_exporter/tree/master/docs/node-mixin>
- USE Linux checklist: <https://www.brendangregg.com/USEmethod/use-linux.html>
- Linux kernel documentation: <https://docs.kernel.org/>
- PSI: <https://docs.kernel.org/accounting/psi.html>
- cgroup v2: <https://docs.kernel.org/admin-guide/cgroup-v2.html>
- procfs: <https://docs.kernel.org/filesystems/proc.html>
- block IO statistics: <https://docs.kernel.org/admin-guide/iostats.html>
- BCC: <https://github.com/iovisor/bcc>
- Linux performance tools: <https://www.brendangregg.com/linuxperf.html>
- Falco rules: <https://github.com/falcosecurity/rules>

