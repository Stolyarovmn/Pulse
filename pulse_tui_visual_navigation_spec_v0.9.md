# PULSE TUI — Visual & Navigation Design Spec v0.2

> Статус: зафиксированная рабочая концепция.  
> Область: визуальный язык, State Glyph, главный экран, навигация, основные экраны и подменю.  
> Timeline / History / Diff намеренно не специфицированы детально — их обсуждаем следующим этапом.

---

## 1. Миссия интерфейса

PULSE не должен восприниматься как «ещё один htop».

Первый экран должен с первой секунды отвечать на три вопроса:

1. Как чувствует себя система?
2. Есть ли что-то, что требует внимания?
3. Куда провалиться, чтобы понять причину?

При этом PULSE остаётся настоящим TUI:

- работает через SSH;
- не требует графического окружения;
- сохраняет высокую информационную плотность терминала;
- полностью управляется с клавиатуры;
- использует Unicode и TrueColor как улучшение, а не как обязательное условие;
- не имитирует веб-dashboard карточками ради карточек.

Главная визуальная особенность — **Glyph Surface**: небольшая матричная поверхность, которая в зависимости от контекста показывает состояние, событие, прогресс или диагностический контекст.

---

# 2. Символьный алфавит состояния

Базовая шкала:

```text
·  ○  ●  ◉   ◇  ◆   ▲   ×
```

| Символ | Значение | Класс состояния |
|---|---|---|
| `·` | фон / отсутствие сигнала | inactive |
| `○` | штатное состояние | normal |
| `●` | активность / заметная нагрузка | active / busy |
| `◉` | сильная нагрузка / hotspot, но система ещё контролируема | saturated |
| `◇` | отклонение от штатного режима | degraded |
| `◆` | существенное нарушение | warning |
| `▲` | критическое состояние, требующее внимания | critical |
| `×` | функция/объект потерян или недоступен | failed/down |

Ключевая визуальная семантика:

```text
круглое      = система контролируема
ромб         = система вышла из штатного режима
треугольник  = требуется внимание сейчас
крест        = функция потеряна
```

Шкала:

```text
· → ○ → ● → ◉ → ◇ → ◆ → ▲ → ×
```

Это не шкала процента загрузки. Например `◉` не означает «90% CPU». Переход `◉ → ◇` означает смену класса состояния: высокая активность стала деградацией.

---

## 2.1. Цвет — только второй канал

Смысл должен сохраняться в монохроме.

Предлагаемая палитра:

```text
·        dim gray
○        gray / soft white
●        white / cyan
◉        bright white / cyan

◇        dim amber
◆        amber

▲        red
×        red / bright white
```

Цвет нельзя использовать как единственный способ различить normal/warning/critical.

---

## 2.2. `⚠` не используем как пиксель Glyph Surface

`⚠` допустим в тексте и списках, но не как базовый элемент матрицы.

Основной critical-marker:

```text
▲
```

---

# 3. Glyph Surface

Логический размер:

```text
10 × 10
```

или близкий к нему.

Это не контейнер для 100 независимых метрик.

Главное правило:

> **Один glyph отвечает на один вопрос.**

Предварительно фиксируем режимы:

```text
HEALTH
RESOURCE
EVENT
PROGRESS
ENTITY
TRACE
DIFF        # детально вместе с Timeline
```

---

# 4. State / Health Glyph

Главный glyph стартового экрана отвечает:

> Насколько система здорова и насколько она отклоняется от нормы?

Он не заменяет числовые показатели.

## 4.1. Normal

```text
        ○   ○
      ○   ●   ○
    ○   ●   ●   ○
  ○   ●   ●   ●   ○
  ○   ●   ●   ●   ○
    ○   ●   ●   ○
      ○   ●   ○
        ○   ○
```

Свойства:

- округлый;
- симметричный;
- без угловатых элементов;
- без разрывов;
- небольшое количество `●` допустимо.

## 4.2. Busy / hot but healthy

```text
        ○   ○
      ○   ●   ○
    ○   ●   ◉   ○
  ○   ●   ◉   ●   ○
  ○   ●   ●   ●   ○
    ○   ●   ●   ○
      ○   ●   ○
        ○   ○
```

`◉` = высокая активность, но не обязательно проблема.

## 4.3. Degraded

```text
        ○   ○
      ○   ●   ◇
    ○   ●   ●   ◇
  ○   ●   ●   ●   ◆
  ○   ●   ●   ●   ◆
    ○   ●   ●   ◇
      ○   ●   ◇
        ○   ○
```

## 4.4. Warning

```text
        ○   ◇
      ○   ●   ◆
    ○   ●   ●   ◆
  ○   ●   ●   ◆   ◆
  ○   ●   ●   ◆   ◆
    ○   ●   ●   ◆
      ○   ●   ◆
        ○   ◇
```

## 4.5. Critical

```text
        ○   ◇
      ○   ◆   ▲
    ○   ●   ◆   ▲
  ○   ●   ◆   ▲   ▲
  ○   ●   ◆   ▲   ▲
    ○   ●   ◆   ▲
      ○   ◆   ▲
        ○   ◇
```

`▲` должен оставаться редким.

## 4.6. Failure / lost function

```text
        ○   ◇
      ○       ▲
    ○           ▲

          ×

    ○           ▲
      ○       ▲
        ○   ◇
```

Смысл:

```text
▲ = функция ещё существует, но состояние критическое
× = функция/объект уже потерян
```

---

# 5. Геометрия State Glyph

Основные правила:

- healthy-state имеет стабильную узнаваемую форму;
- healthy entities одного типа визуально похожи;
- локальная проблема меняет соответствующую часть формы;
- severity кодируется типом символов;
- shape и severity не смешиваются с идентичностью ресурса.

Запрещённая семантика:

```text
○ = CPU
◇ = RAM
▲ = IO
```

Правильная:

```text
геометрия / положение = где проблема
тип символа            = насколько серьёзно
```

---

# 6. Анимация

Анимация — информационный канал, а не украшение.

Предварительная семантика:

```text
спокойная слабая пульсация    healthy live state
ускоренная пульсация          rising activity
направленное движение         flow
рывок / возврат               retransmit
collapse → reform             restart
короткая вспышка              discrete event
замедление / почти freeze     wait / stall
```

Анимацию можно полностью отключить:

```text
--no-animation
```

---

# 7. Рендеринг Glyph

Внутри glyph хранится как логическая сетка, а не как заранее подготовленная Unicode-строка.

Пример модели:

```text
GlyphSurface
└── cells[y][x]
    ├── state
    ├── intensity
    ├── semantic_role
    └── animation_phase
```

Режимы рендеринга:

### Основной dot-mode

```text
·  ○  ●  ◉  ◇  ◆  ▲  ×
```

### Dense fallback

```text
▀ ▄ █
```

### Ultra-compact

Braille:

```text
⣠⣶⣄
⣿⣿⣿
⠻⣿⠟
```

---

# 8. Главный экран

Главный экран не должен начинаться с таблицы процессов.

Информационный порядок:

```text
состояние
→ проблемы
→ изменения
→ сущности
```

## 8.1. Wide layout

```text
 P U L S E      asuspc           LIVE ●          09:33:03       up 17h18m
──────────────────────────────────────────────────────────────────────────────

      STATE

        ○   ○
      ○   ●   ○                 SYSTEM NOMINAL
    ○   ●   ●   ○
  ○   ●   ●   ●   ○             0 problems
    ○   ●   ●   ○
      ○   ●   ○                 stable for 17h18m
        ○   ○

 CPU  3%       MEM  8%       PSI  0%       IO  0%       NET  200 MiB
──────────────────────────────────────────────────────────────────────────────

 WHAT NEEDS ATTENTION

 ✓ nothing requires attention

──────────────────────────────────────────────────────────────────────────────

 RECENT CHANGES

 09:29  ModemManager restarted
 09:27  configuration changed
 09:14  network interface reconnected

──────────────────────────────────────────────────────────────────────────────

 ENTITIES                     kind       CPU       MEM       STATE

 > system.slice               cgroup     0.03      613M      ○
   ModemManager.service       unit       0.02       46M      ○
   accounts-daemon.service    unit       0.00      2.7M      ○
   pulse                      process    0.02       7.2M      ○
   ...

──────────────────────────────────────────────────────────────────────────────
 ↑↓ select   Enter inspect   / search   : commands   P problems   T timeline
```

## 8.2. При наличии проблемы

Layout остаётся тем же — меняется состояние:

```text
 P U L S E      prod-api-07       LIVE ●        12:42:31      2 problems
──────────────────────────────────────────────────────────────────────────────

      STATE

        ○   ◇
      ○   ◆   ▲
    ○   ●   ◆   ▲              DEGRADED
  ○   ●   ◆   ▲   ▲
    ○   ●   ◆   ▲              MEMORY PRESSURE
      ○   ◆   ▲
        ○   ◇                  since 02:17

 CPU 12%       MEM 84%       PSI MEM 38%       IO 4%       NET 256M
──────────────────────────────────────────────────────────────────────────────

 WHAT NEEDS ATTENTION                                               2

 > ▲ Memory pressure high
     asuspc                       4m22s

   ◆ checkout-api
     latency degradation         1m13s

──────────────────────────────────────────────────────────────────────────────

 RECENT CHANGES

 12:38  memory pressure entered warning
 12:39  reclaim rate increased
 12:40  checkout-api p95 increased
```

---

# 9. Glyph не должен съедать экран

Рекомендуемый бюджет:

```text
header             1–2 строки
glyph/status       7–12 строк
attention          4–8 строк
recent changes     3–6 строк
entities           всё оставшееся пространство
footer             1 строка
```

Glyph — visual anchor, а не декоративный splash screen.

---

# 10. Responsive layouts

## Wide

```text
>= 120 columns
```

Можно использовать:

```text
[ glyph/status ] [ attention/details ]
[ entities                            ]
```

## Medium

```text
80–119 columns
```

Glyph сверху, таблица ниже.

## Compact

```text
<= 79 columns
```

Пример:

```text
PULSE asuspc LIVE ●

  ○ ● ○     NOMINAL
○ ● ● ● ○   0 problems
  ○ ● ○

CPU 3 MEM 8 PSI 0 IO 0

ENTITIES
> system.slice        ○
  modemmanager        ○
  pulse               ○

↑↓  ↵inspect  /find  :cmd
```

---

# 11. Общая модель навигации

Базовые клавиши:

```text
↑ / ↓       выбрать соседний объект
← / →       соседняя колонка / ветка / временная позиция по контексту

Enter       открыть / inspect
Esc         назад / закрыть overlay

/           Search
:           Command Palette
?           Help
q           Quit
```

## 11.1. Главное правило Enter

```text
select
  ↓
Enter
  ↓
deeper
```

Например:

```text
service
  ↓ Enter
service inspect
  ↓ Enter
container inspect
  ↓ Enter
pod inspect
```

## 11.2. Главное правило Esc

```text
Esc = один уровень назад
```

---

# 12. Верхнеуровневые режимы

```text
Overview
Problems
Entities
Inspect
Trace
Logs
Network
Storage
Containers
Kubernetes
Services
Glyph Wall
Timeline
Command Palette
Help
Settings
```

Не все режимы должны быть постоянными вкладками.

Редкие действия доступны через `:`.

---

# 13. Overview

Вход:

```text
pulse
```

или возврат Esc до корня.

Содержит:

```text
State Glyph
System strip
What needs attention
Recent changes
Relevant entities
```

Клавиши:

```text
P   Problems
E   Entities
G   Glyph Wall
T   Timeline
/   Search
:   Commands
```

---

# 14. Problems

Вход:

```text
P
```

или Enter на `WHAT NEEDS ATTENTION`.

```text
 PROBLEMS                                             3

 > ▲ Memory Pressure
     host asuspc
     since 4m22s
     impact high

   ◆ checkout-api latency
     service checkout-api
     since 1m13s

   ◇ nvme0 latency
     disk nvme0
     since 43s
```

При выделении показываются:

```text
SUMMARY
CAUSE PATH
AFFECTED ENTITIES
RECENT EVENTS
```

Enter открывает `Problem Detail`.

---

# 15. Problem Detail

```text
 MEMORY PRESSURE                                      ▲ CRITICAL

 since       4m22s
 impact      high
 confidence  92%

 WHY

 memory pressure
      ↓
 kswapd activity
      ↓
 direct reclaim
      ↓
 CPU stalls
      ↓
 service latency

 AFFECTED

 > postgres       high
   checkout-api   high
   redis          medium

 RELATED CHANGES

 12:38 swap crossed 50%
 12:39 reclaim increased
 12:40 latency increased
```

Переходы:

```text
Enter   inspect selected entity
T       timeline
C       cause / trace
L       related logs
E       related events
```

---

# 16. Entities

Вход:

```text
E
```

или выбор таблицы на Overview.

Это список **всех типов сущностей**, а не только процессов:

```text
ENTITY                         KIND          CPU      MEM      STATE

system.slice                   cgroup        0.03     613M     ○
ModemManager.service           unit          0.02      46M     ○
checkout                       container     71.2     1.8G     ◆
checkout-7d9f                  pod           68.1     1.9G     ◆
postgres                       process       82.0     3.1G     ○
```

## Filter

```text
f
```

```text
FILTER

[ ] process
[ ] systemd unit
[ ] cgroup
[ ] container
[ ] pod
[ ] workload
[ ] service
[ ] socket
[ ] disk
[ ] interface
[ ] external dependency
```

## Sort

```text
s
```

```text
SORT BY

> relevance
  health
  CPU
  memory
  IO
  network
  age
  restarts
  name
```

---

# 17. Inspect

Вход — `Enter` из любой таблицы, graph или Glyph Wall.

```text
PULSE > checkout.service > checkout > checkout-7d9f > java/18421
──────────────────────────────────────────────────────────────────

 PROCESS java                         STATE ◆

 PID          18421
 USER         checkout
 CPU          612%
 MEM          4.8G
 THREADS      184
 FD           881 / 1024

 OWNERSHIP

 host
  └─ checkout.service
      └─ cgroup
          └─ container checkout
              └─ pod checkout-7d9f
                  └─ java 18421

 RESOURCE

 CPU       612%       ◉
 throttle   22%       ◆
 MEM       4.8G       ●
 PSI       18.4%      ◆

 RELATED

 3 sockets
 2 traces
 41 logs
 4 events
```

### Context actions

```text
:
```

```text
INSPECT ACTIONS

> Open parent
  Open children
  View owner chain
  View dependents
  View dependencies

  Network connections
  Open files
  Threads
  Environment
  Cgroup limits

  Related logs
  Related traces
  Related events

  Compare with...
  Add to timeline

  Restart service...
  Kill process...
```

Destructive action всегда требует подтверждения.

---

# 18. Ownership navigation

Одна из основных фич PULSE:

```text
host
 ↓
systemd unit
 ↓
cgroup
 ↓
container
 ↓
pod
 ↓
process
```

или:

```text
process
 ↓
socket
 ↓
remote endpoint
 ↓
service
 ↓
trace/span
```

Breadcrumb всегда виден:

```text
asuspc > system.slice > checkout.service > checkout > java/18421
```

---

# 19. Trace / Cause

Вход:

```text
t
```

из Inspect/Problem либо через Command Palette.

```text
checkout-api
    │
    ▼
GET /checkout
    │
    ├── auth              4 ms
    ├── inventory        43 ms
    │
    └── postgres.query   2.81 s  ▲
             │
             ▼
       TCP retransmits   ◆
             │
             ▼
       DB timeout        ▲
```

Subviews:

```text
Trace view
Cause view
Dependencies
Dependents
Spans
Related logs
Related metrics
```

---

# 20. Logs

Вход:

```text
l
```

из Inspect или:

```text
: logs
```

```text
LOGS / checkout-api

12:42:01 INFO   request received
12:42:01 WARN   DB connection slow
12:42:03 ERROR  context deadline exceeded
```

Фильтры:

```text
level
entity
service
trace_id
time range
substring
regex
```

Переходы:

```text
Enter on trace_id → Trace
Enter on entity   → Inspect
T                 → Timeline
```

---

# 21. Network

Вход:

```text
n
```

из Inspect или через Command Palette.

Subviews:

```text
Overview
Interfaces
Connections
Listeners
Flows
Retransmits
Remote endpoints
By process
By container
By pod
```

```text
NETWORK / checkout-api

REMOTE                  RX       TX       RETRANS      STATE

10.42.8.31:5432         18M/s    4M/s     0.1%         ○
10.42.8.32:5432         11M/s    2M/s     6.8%         ◆
10.42.4.18:6379          8M/s    7M/s     0.0%         ○
```

---

# 22. Storage / IO

Вход:

```text
: storage
```

Subviews:

```text
Disks
Filesystems
IO latency
IO pressure
Queues
Top IO entities
Mounts
```

---

# 23. Services / systemd

Subviews:

```text
Units
Failed
Restarting
Dependencies
Timers
Sockets
```

Unit Inspect:

```text
status
PID(s)
cgroup
children
restart history
logs
dependencies
resource usage
```

---

# 24. Containers

Subviews:

```text
Running
Degraded
Stopped
By image
By runtime
```

Inspect показывает:

```text
container
image
runtime
cgroup
processes
network
mounts
limits
pod/workload if known
logs
events
```

---

# 25. Kubernetes

PULSE не заменяет k9s.

Задача Kubernetes view:

> дать инфраструктурный контекст локальным процессам и проблемам.

Subviews:

```text
Pods
Workloads
Namespaces
Node workloads
Problems
Dependencies
```

Не приоритет:

```text
редактирование ConfigMap
массовое управление CRD
полная замена kubectl/k9s
```

---

# 26. Glyph Wall

Вход:

```text
G
```

Цель:

> визуально найти outlier среди большого количества однотипных entities.

Compact:

```text
checkout-01   ○     checkout-02   ○     checkout-03   ○
checkout-04   ○     checkout-05   ◆     checkout-06   ○
checkout-07   ○     checkout-08   ○     checkout-09   ○
```

Expanded:

```text
┌ checkout-01 ┐  ┌ checkout-02 ┐  ┌ checkout-03 ┐
│    ○ ● ○    │  │    ○ ● ○    │  │    ○ ◆ ▲    │
│  ○ ● ● ● ○  │  │  ○ ● ● ● ○  │  │  ○ ● ◆ ▲ ○  │
│    ○ ● ○    │  │    ○ ● ○    │  │    ○ ◆ ○    │
└─────────────┘  └─────────────┘  └─────────────┘
```

Enter → Inspect.

Filters:

```text
kind
namespace
service
owner
health
label
```

---

# 27. Command Palette

Вход:

```text
:
```

```text
> ins_

  Inspect entity
  Inspect process
  Inspect container
  Inspect pod

  Open problems
  Open timeline
  Open network
  Open logs

  Filter entities
  Compare with...
```

Command Palette отвечает за discoverability редких функций.

---

# 28. Search

Вход:

```text
/
```

Глобальный, не привязан к текущему view.

```text
/payment

PROCESSES
  payment-worker

CONTAINERS
  payment-api

PODS
  payment-api-88421

SERVICES
  payment-api

SYSTEMD
  payment-worker.service

ENDPOINTS
  payment-db:5432
```

Enter открывает Inspect.

---

# 29. Contextual actions

Команды зависят от типа выбранной entity.

Process:

```text
Inspect
Parent
Children
Open files
Network
Threads
Logs
Trace
Timeline
Kill...
```

Pod:

```text
Inspect
Containers
Processes
Network
Logs
Events
Timeline
Owner
Dependencies
Restart...
```

Disk:

```text
Inspect
IO
Processes using disk
Filesystem
Mount
Timeline
SMART
```

---

# 30. Help

```text
?
```

Help всегда contextual.

Пример Inspect:

```text
INSPECT HELP

Enter       open selected relation
Esc         back
↑↓          select
/           search
:           actions
t           trace
l           logs
n           network
T           timeline
?           close help
```

---

# 31. Mouse

Мышь optional.

```text
click             select
double click      Enter-equivalent
wheel             scroll
drag              только там, где имеет смысл
```

Ни одна функция не должна требовать мышь.

---

# 32. Меню как дерево

```text
PULSE
│
├── Overview
│   ├── State Glyph
│   ├── Attention
│   ├── Recent Changes
│   └── Relevant Entities
│
├── Problems
│   └── Problem Detail
│       ├── Cause
│       ├── Affected Entities
│       ├── Events
│       ├── Logs
│       └── Timeline
│
├── Entities
│   ├── Filter
│   ├── Sort
│   └── Inspect
│       ├── Ownership
│       ├── Children
│       ├── Dependencies
│       ├── Network
│       ├── Files
│       ├── Threads
│       ├── Logs
│       ├── Trace
│       └── Timeline
│
├── Glyph Wall
│   └── Inspect
│
├── Trace / Cause
│   ├── Dependencies
│   ├── Dependents
│   ├── Spans
│   ├── Logs
│   └── Metrics
│
├── Network
│   ├── Interfaces
│   ├── Connections
│   ├── Listeners
│   ├── Flows
│   ├── Retransmits
│   └── Endpoints
│
├── Storage
│   ├── Disks
│   ├── Filesystems
│   ├── Latency
│   ├── Pressure
│   └── Queues
│
├── Services
│   ├── Units
│   ├── Failed
│   ├── Restarting
│   ├── Dependencies
│   ├── Timers
│   └── Sockets
│
├── Containers
│   ├── Running
│   ├── Degraded
│   ├── Stopped
│   └── By image/runtime
│
├── Kubernetes
│   ├── Pods
│   ├── Workloads
│   ├── Namespaces
│   ├── Node workloads
│   └── Problems
│
├── Logs
│
├── Timeline
│   └── [TO BE DESIGNED]
│
├── Command Palette
├── Help
└── Settings
```

---

# 33. Navigation graph

```text
                     ┌──────────┐
                     │ Overview │
                     └────┬─────┘
                          │
          ┌───────────────┼──────────────────┐
          │               │                  │
          ▼               ▼                  ▼
      Problems         Entities          Glyph Wall
          │               │                  │
          ▼               └────────┬─────────┘
   Problem Detail                  │
          │                        ▼
          └───────────────────► Inspect
                                   │
                   ┌───────────────┼──────────────┐
                   │               │              │
                   ▼               ▼              ▼
                 Trace           Logs          Network
                   │               │              │
                   └───────────────┼──────────────┘
                                   │
                                   ▼
                                Timeline
                           [design pending]
```

---

# 34. PULSE mental model

Не:

```text
машина
  ↓
таблица процессов
  ↓
ищи проблему
```

А:

```text
машина
  ↓
состояние
  ↓
что требует внимания
  ↓
что изменилось
  ↓
какие entities связаны
  ↓
Inspect
  ↓
Cause
```

---

# 35. Что должно цеплять с первой секунды

Не один декоративный gimmick, а связка:

1. **State Glyph** — визуальная подпись состояния.
2. **Problem-first UI** — не «CPU 84%», а «Memory pressure affects checkout-api».
3. **Entity model** — process → cgroup → unit → container → pod → service.
4. **Recent Changes** — сразу видно, что изменилось перед проблемой.
5. **Enter-to-understand** — любой объект открывается одним способом.
6. **Command Palette** — не нужно помнить сотни shortcut'ов.

---

# 36. Что пока не фиксируем

## Timeline / History / Diff

Нужно отдельно решить:

- масштаб времени;
- event lanes;
- telemetry lanes;
- live edge;
- cursors A/B;
- zoom;
- semantic diff;
- entity-specific history;
- переходы event → entity → trace → logs;
- исчезнувшие entities;
- aggregation;
- sampling;
- bookmarks;
- replay;
- историческое представление State Glyph.

Пока зафиксирован только факт существования:

```text
A ↔ B
```

---

# 37. Предварительный набор клавиш

```text
↑ ↓       select
← →       context-dependent navigation

Enter     inspect / open
Esc       back

/         global search
:         command palette
?         help

P         problems
E         entities
G         glyph wall
T         timeline

t         trace
l         logs
n         network

f         filter
s         sort

q         quit
```

Все клавиши должны быть переназначаемыми.

---

# 38. Design invariants

- PULSE не является htop clone.
- Glyph не заменяет числа.
- Glyph показывает состояние; числа объясняют его.
- Severity читается формой даже без цвета.
- Healthy entities визуально похожи.
- Abnormal entity должен выделяться до чтения текста.
- Enter всегда означает «глубже».
- Esc всегда означает «назад».
- Search глобальный.
- Command Palette делает редкие функции discoverable.
- Основной UI problem-first, а не metrics-first.
- Kubernetes — контекст, а не попытка заменить k9s.
- TUI проектируется как TUI, а не как веб-dashboard, нарисованный Unicode-рамками.
- Timeline/History — часть ядра продукта, но его UX проектируется отдельно.

---

# 39. Следующий дизайн-этап

## PULSE Timeline / Time Machine

Следующее обсуждение должно ответить:

1. Что является основной единицей timeline?
2. Какие события показываются всегда?
3. Какие metrics получают lanes?
4. Как выглядит live edge?
5. Как пользователь выбирает момент A?
6. Как выбирает B?
7. Как выглядит semantic diff?
8. Как переходить из события в Inspect?
9. Как показывать OOM/restart/deploy/config change?
10. Как показывать исчезнувшие процессы и контейнеры?
11. Как соединить Timeline, Trace и Logs?
12. Как использовать State Glyph как исторический визуальный маркер?
13. Как должен работать zoom от секунд до часов/дней?
14. Какие данные реально нужно хранить локально?

---

_End of PULSE TUI Visual & Navigation Design Spec v0.2_

---

# 40. Timeline / Time Machine — Interaction Model v0.1

> Статус: зафиксировано. Этот раздел определяет поведение времени во всём PULSE. Визуальная компоновка Timeline будет спроектирована отдельно следующей итерацией.

## 40.1. Главный принцип

Timeline не является отдельной страницей с графиками.

> **Time is global application state.**

PULSE всегда находится в одном из двух глобальных режимов:

```text
LIVE
```

или

```text
HISTORY @ <timestamp>
```

Если пользователь перемещает курсор Timeline назад, весь PULSE переходит в исторический контекст выбранного момента.

То есть следующие экраны должны отображать состояние на выбранный timestamp:

```text
Overview
Problems
Entities
Inspect
Network
Storage
Services
Containers
Kubernetes
Glyph Wall
Trace/related context where historical data exists
```

Глобальный header явно показывает режим:

```text
PULSE  asuspc  LIVE ●
```

или:

```text
PULSE  asuspc  HISTORY ◀ 13:07:18
```

Исторический режим нельзя визуально спутать с live.

## 40.2. Возврат в настоящее

Клавиша:

```text
F = Follow live
```

В LIVE курсор приклеен к правому краю шкалы времени.

После scrub назад:

```text
LIVE ● → HISTORY ◀
```

После `F`:

```text
HISTORY ◀ → LIVE ●
```

## 40.3. Четыре логических слоя истории

Timeline объединяет:

```text
TIME
 │
 ├─ telemetry     continuous/aggregated measurements
 ├─ events        discrete changes and incidents
 ├─ entities      lifecycle and graph changes
 └─ state         derived PULSE health transitions
```

### Telemetry

Примеры:

```text
CPU / CPU stress
memory / memory stress
PSI
IO / IO stress
network / retransmits
entity-specific metrics
```

### Events

Примеры:

```text
service start/restart/stop
container/pod lifecycle
deploy/config change
OOM
pressure transition
network degradation
storage problem
OTel event
```

### Entity changes

Нужно хранить:

```text
entity appeared
entity disappeared
entity restarted/replaced
owner changed
identity changed
relationship appeared/disappeared
```

### Derived state transitions

Хранить именно переходы:

```text
○ → ◇ → ◆ → ▲ → ◆ → ◇ → ○
```

а не отдельный glyph каждую секунду.

## 40.4. State lane

Timeline обязательно имеет компактную полосу состояния:

```text
STATE
○────○────○────◇────◆────▲────◆────◇────○
```

Она позволяет увидеть фазу инцидента до чтения графиков.

Полноценные State Glyph показываются только в значимых точках или при focus/inspect.

## 40.5. Scrubbing

Основная навигация:

```text
← / →   перемещение времени
```

При перемещении курсора:

- timestamp становится глобальным временем приложения;
- detail pane показывает состояние выбранного момента;
- остальные экраны после перехода открываются в этом же времени;
- live-follow отключается автоматически.

## 40.6. Zoom

Предварительные клавиши:

```text
+ / -
```

Zoom должен ощущаться непрерывным, даже если внутри используются дискретные уровни агрегации.

Примерные масштабы:

```text
30 sec
2 min
10 min
1 hour
6 hours
24 hours
7 days
```

При изменении масштаба:

- метрики агрегируются;
- события группируются;
- state transitions сохраняют смысл;
- курсор остаётся привязан к той же временной точке.

## 40.7. Event aggregation

Timeline не должен превращаться в лог.

Повторяющиеся события группируются:

```text
12:41–12:43
▲ DB connection failures ×184
```

`Enter` раскрывает группу.

## 40.8. Severity vs event type

Символ отвечает за severity:

```text
● informational
◇ degraded
◆ warning
▲ critical
× failure
```

Тип события указывается короткой подписью:

```text
● DEPLOY
◇ MEM
◆ NET
▲ OOM
```

Не заводить отдельный уникальный glyph-symbol для каждого типа события.

## 40.9. Context-aware metric lanes

По умолчанию Timeline показывает агрегированные operational-health lanes:

```text
CPU STRESS
MEM STRESS
IO STRESS
NET HEALTH
```

Если выбрана entity, добавляются/заменяются relevant lanes этой entity.

Пример для postgres:

```text
HOST CPU STRESS
HOST MEM STRESS
postgres CPU
postgres MEM
postgres IO
postgres connections
```

Raw utilization не должен вытеснять stress/pressure показатели с первого уровня.

## 40.10. A/B markers

Клавиши:

```text
A = set marker A
B = set marker B
D = semantic diff
```

Timeline визуально показывает:

```text
A                   B
▼                   ▼
●───────────────────●───────────────
```

## 40.11. Semantic Diff

Diff отвечает на вопрос:

> Что стало другим между A и B?

Он должен показывать не просто две колонки чисел, а изменения состояния, entities и событий.

Пример структуры:

```text
DIFF  13:00:00 → 13:07:18

SYSTEM
○ → ▲      nominal → critical

WHAT CHANGED
▲ Memory pressure appeared
◆ checkout-api degraded
+ 14 processes
- 3 processes
+ container checkout-831c
- container checkout-79af

NETWORK
retransmits 0.1% → 6.8%

EVENTS BETWEEN A AND B
13:02 memory pressure
13:04 reclaim increased
13:05 deployment
13:07 OOM
```

## 40.12. Entity lifecycle semantics

Semantic diff должен понимать логические replacement/restart связи.

Например:

```text
pod checkout-AAA → checkout-BBB
container a8f34  → f21c9
PID 16211        → 17321
```

Не показывать это как шесть независимых unrelated add/remove событий, если Entity Graph позволяет установить замену одной логической workload-entity.

## 40.13. Correlation without false causality

Timeline может показывать последовательность:

```text
deploy
  ↓
new container
  ↓
memory pressure
  ↓
latency
  ↓
OOM
```

Но до подтверждения причинности UI формулирует это как:

```text
RELATED CHANGES
```

а не:

```text
DEPLOY CAUSED OOM
```

Причинные выводы должны иметь отдельную evidence/confidence модель.

## 40.14. Исторические данные и идентичность

Процесс нельзя идентифицировать только PID.

Историческая entity identity должна учитывать устойчивые признаки жизненного цикла, например:

```text
host identity + pid + creation/start time
```

Контейнеры/pods/workloads также должны иметь собственные stable lifecycle IDs.

## 40.15. Логическая модель хранения

На уровне продукта фиксируем четыре слоя:

```text
Metric samples
Events
Entity graph snapshots/deltas
Derived state transitions
```

Entity Graph предпочтительно хранить как:

```text
baseline snapshot
+
deltas
```

а не как полный snapshot каждую секунду.

## 40.16. Клавиши Timeline v0.1

```text
← / →      scrub time
+ / -      zoom
Enter      inspect focused event/entity/state
A          set A
B          set B
D          diff A↔B
F          follow live / return to NOW
Esc        back
/          search
:          commands
?          help
```

## 40.17. Зафиксированная продуктовая идея

Timeline = **Time Machine**, а не history tab.

Пользователь должен иметь возможность:

```text
T
← ← ←
Enter
```

и продолжить исследование той же системы в прошлом:

```text
Entities @ timestamp
Inspect @ timestamp
Network @ timestamp
Glyph Wall @ timestamp
```

После `F` приложение возвращается в настоящее.

---

# 41. Следующий этап: визуальная компоновка Timeline

Нужно сравнить несколько принципиально разных layout-моделей и выбрать одну:

1. **Rail** — одна доминирующая временная ось + раскрывающиеся lanes.
2. **Incident Story** — события и причинная последовательность главнее графиков.
3. **Split Time Machine** — timeline слева/сверху, исторический Inspect рядом.
4. **State River** — state/pressure как основная непрерывная полоса, metrics вторичны.
5. **Event Constellation** — редкие значимые события на оси, details раскрываются по focus.

Критерии выбора:

- не выглядеть как Grafana в ASCII;
- мгновенно показывать момент начала деградации;
- удобно scrub'ить клавиатурой;
- не терять связь event ↔ entity ↔ state;
- хорошо работать на 80×24;
- хорошо масштабироваться на wide terminal;
- поддерживать A/B diff без отдельной ментальной модели;
- сохранять фирменный PULSE visual language.


---

# 42. Зафиксированный Timeline layout: State River + Story + Snapshot

После сравнения пяти вариантов основным направлением фиксируется гибрид:

```text
State River
    +
Incident Story
    +
Historical Snapshot
```

## 42.1. Основная идея

Timeline должен отвечать в таком порядке:

```text
КОГДА состояние изменилось?
        ↓
ЧТО произошло вокруг этого момента?
        ↓
КАК выглядела система в выбранный момент?
        ↓
КУДА провалиться для расследования?
```

Он не является отдельным dashboard с графиками.

Время остаётся глобальным состоянием всего приложения:

```text
LIVE
```

или:

```text
HISTORY @ timestamp
```

---

## 42.2. Основной wide-layout

```text
 P U L S E / TIME MACHINE                 HISTORY ◀ 13:07:18

 12:30                                                       NOW
 ○○○○○○○○○◇◇◇◆◆▲▲◆◇◇○○○○○○○○○○○○○○○○○○○●
                         ▲
                         │
─────────────────────────┼───────────────────────────────────────
 STORY                   │ SNAPSHOT @ 13:07
                         │
 ● deploy                │       ○   ◇
 │                       │     ○   ◆   ▲
 ● pod replaced          │   ○   ●   ◆   ▲
 │                       │     ○   ◆   ▲
 ◇ memory pressure       │       ○   ◇
 │                       │
 ◆ reclaim               │ MEMORY PRESSURE
 │                       │
 ◆ latency               │ PSI       38%
 │                       │ reclaim   2.1G/s
>▲ OOM                   │
 │                       │ postgres        ▲
 ● restart               │ checkout-api    ◆
                         │
─────────────────────────┴───────────────────────────────────────
 CPU  ▁▁▂▃▄▅▆▇██▇▄▂▁
 MEM  ▁▂▃▄▅▆▇█████▇▅▃

 ←→ scrub  ↑↓ story  Enter inspect  +/- zoom  A/B diff  F live
```

---

## 42.3. State River

State River — главный визуальный объект Timeline.

Пример:

```text
○○○○○○○◇◇◆◆▲▲◆◇◇○○○○
```

Он показывает не raw utilization, а **derived operational state**.

Смысл:

```text
○   normal
●   active/high but healthy
◉   saturated but controlled
◇   degraded
◆   warning
▲   critical
×   failed/down
```

State River должен позволять одним взглядом увидеть:

- начало деградации;
- escalation;
- критическую фазу;
- recovery;
- повторяющиеся incidents.

---

## 42.4. Incident Story

Story lane показывает только значимые изменения и события.

Пример:

```text
● deploy
│
● new pod
│
◇ memory pressure
│
◆ reclaim
│
◆ latency
│
▲ OOM
│
● restart
```

Это **не автоматически доказанная causality chain**.

По умолчанию это:

```text
temporal + contextual relation
```

Формулировки должны быть осторожными:

```text
related change
preceded by
followed by
affected
correlated
```

Слово `caused` допустимо только если причина действительно подтверждена отдельной диагностической логикой.

---

## 42.5. Historical Snapshot

При scrub выбранный timestamp становится глобальным временем PULSE.

Snapshot показывает:

```text
State Glyph
Problems
Relevant metrics
Changed entities
Selected entity context
```

Другие экраны также переходят в этот timestamp:

```text
Entities @ timestamp
Inspect @ timestamp
Network @ timestamp
Glyph Wall @ timestamp
```

---

# 43. Timeline interaction model

## Scrub

```text
← / →
```

перемещают time cursor.

## Zoom

```text
+ / -
```

изменяют временной масштаб.

## Live follow

```text
F
```

возвращает:

```text
LIVE ●
```

## History state

После ухода с live edge header явно меняется:

```text
HISTORY ◀ 13:07:18
```

Это состояние отображается глобально на всех экранах.

---

# 44. A/B semantic diff

Пользователь выбирает:

```text
A = baseline
B = comparison
```

а затем:

```text
D
```

Diff должен отвечать:

> Что стало другим?

а не просто показывать две колонки метрик.

Пример:

```text
SYSTEM

○ → ▲
nominal → critical

WHAT CHANGED

▲ Memory pressure appeared
  PSI          0.4% → 38%
  reclaim      0    → 2.1 GiB/s

◆ checkout-api degraded
  CPU          81% → 612%
  throttling    0% → 22%

+ container checkout-831c
- container checkout-79af

NETWORK
  retransmits  0.1% → 6.8%

EVENTS BETWEEN A AND B
  deploy
  memory pressure
  latency degradation
  OOM
```

Entity lifecycle должен понимать replacement/restart как более высокий semantic event, а не всегда как независимые `+ entity` и `- entity`.

---

# 45. Что фиксируем как следующий вопрос

Следующий дизайн-этап:

## State River aggregation semantics

Нужно определить:

1. Что означает один символ при текущем zoom?
2. Как агрегировать severity внутри временного bucket?
3. Как не потерять короткий critical spike?
4. Как различать краткий spike и длительную деградацию?
5. Как показывать multiple incidents внутри одного bucket?
6. Как сохранять начало/конец incident при zoom-out?
7. Как вычислять derived operational state?
8. Как отделить measured facts от heuristic/derived state?
9. Как State River связан с event density?
10. Какие данные хранятся raw, а какие pre-aggregated?


---

# 46. Timeline marker `!`

Дополнительно к шкале состояния:

```text
· ○ ● ◉ ◇ ◆ ▲ ×
```

фиксируется отдельный символ:

```text
!
```

`!` **не является состоянием severity** и не участвует в State Glyph.

Он означает:

> в этот момент произошло дискретное значимое событие.

Пример:

```text
 TIMELINE

 08:30       08:40       08:50       09:00       NOW
 ─────●─────────!──────────◇─────────!────────────●────>
      start     deploy     PSI       OOM
```

Типичные `!`:

```text
exec
exit/crash
restart
deploy
OOM
filesystem read-only
mount change
config change
port bind failure
connection timeout burst
kernel error
file removed/renamed
```

Severity события показывается:

- цветом;
- текстом;
- связанным State transition;

но сам `!` остаётся общим знаком **event punctuation**.

---

# 47. Incident Story — epistemic contract

PULSE не имеет права превращать временную последовательность в выдуманную причинность.

Incident Story строится поверх **Evidence Graph**.

Каждая связь имеет тип доказательства.

## 47.1. Классы связей

### OBSERVED

Прямо наблюдавшийся факт:

```text
! process exec
! process exit
! file opened
! socket connected
! OOM killed process
! service restarted
```

### DETERMINISTIC LINK

Однозначная техническая связь:

```text
FD 17 → inode 63107
socket inode → TCP endpoint
PID → cgroup
cgroup → systemd unit
PID → executable
mount_id → mount
trace_id → spans
container id → pod
```

### CORRELATED

Совпадение по времени/контексту, но не доказанная причина:

```text
deploy preceded memory pressure by 3m
retransmits coincided with latency increase
new container appeared before OOM
```

### HYPOTHESIS

Диагностическая гипотеза.

Пример:

```text
? deployment may have increased working set
```

Hypothesis не показывается как факт.

---

## 47.2. Правило слова `caused`

PULSE может писать:

```text
caused
```

только если причинная связь следует из семантики наблюдаемого механизма.

Допустимо:

```text
OOM killer terminated PID 18421
```

Недопустимо без дополнительного доказательства:

```text
deployment caused OOM
network retransmits caused DB timeout
```

Вместо этого:

```text
deployment occurred 6m before OOM
retransmits increased during DB timeout burst
```

---

# 48. Incident Story layout

Пример:

```text
 INCIDENT / checkout-api                              13:01 → 13:18

 13:01  ! deploy checkout:v42
         │
 13:02  ! exec /opt/app/start.sh
         │
         ├─ interpreter /bin/bash
         └─ container checkout-831c
         │
 13:03  ◇ memory pressure
         │
 13:04  ◆ direct reclaim increased
         │
 13:06  ! DB timeout burst ×184
         │
 13:07  ! OOM
         │
         ├─ PID 18421 killed
         ├─ postgres container affected
         └─ State ◆ → ▲
         │
 13:07  ! postgres restarted
         │
 13:11  ◇ pressure recovering
         │
 13:18  ○ nominal
```

Выделение события открывает справа/снизу:

```text
EVENT
RELATED ENTITIES
EVIDENCE
PRECEDING CHANGES
FOLLOWING EFFECTS
```

---

# 49. Investigation Graph

PULSE должен позволять двигаться по инфраструктуре **в обе стороны**.

Примеры:

```text
process
  → executable
  → script/interpreter
  → current working directory
  → mapped files
  → open FD
  → file
  → inode
  → mount
  → filesystem
  → block device
```

и обратно:

```text
filesystem
  → inode/file
  → holders
  → FD
  → process
  → cgroup/unit/container/pod
```

Сеть:

```text
process
  → FD
  → socket inode
  → socket
  → local port
  → remote endpoint
```

и обратно:

```text
port/listener
  → socket
  → FD
  → PID
  → process
  → service/container/pod
```

Ownership:

```text
host
  → systemd unit
  → cgroup
  → container
  → pod
  → workload
  → process
```

Telemetry:

```text
process/service
  → trace
  → span
  → log
  → event
```

---

# 50. Entity types Investigation Graph

Минимальный набор Linux entities:

```text
Host
Process
Thread
Executable
Interpreter
Script
MemoryMapping
FD
File
Inode
FileLock
Mount
Filesystem
BlockDevice
Socket
Port
Endpoint
NetworkInterface
Cgroup
SystemdUnit
Container
Pod
Workload
Service
Trace
Span
Log
Event
Incident
```

Позже:

```text
NUMA node
IRQ
Device
GPU process/context
DNS query
TLS connection
Database connection
Kernel stack
User stack
Symbol/function
Source line
```

---

# 51. Process Deep Inspect

Процесс — одна из главных точек входа.

```text
 PROCESS python3 / 18421                                   STATE ◆
────────────────────────────────────────────────────────────────────

 IDENTITY

 PID          18421
 started      13:02:14
 user         checkout

 EXECUTION

 executable   /usr/bin/python3
 script       /opt/app/worker.py
 argv         [redacted by policy]
 cwd          /opt/app

 OWNERSHIP

 checkout.service
   └─ cgroup ...
       └─ container checkout
           └─ pod checkout-7d9f

 RESOURCES

 CPU          612%
 MEM          4.8 GiB
 PSI          18.4%
 FD           881 / 1024

 RELATED

 Files        142
 Sockets       31
 Mappings      97
 Threads       84
 Locks          2
 Traces        12
 Events         7
```

---

# 52. Process → executable / script

Linux current-state sources include:

```text
/proc/<pid>/exe
/proc/<pid>/cmdline
/proc/<pid>/cwd
/proc/<pid>/maps
/proc/<pid>/map_files
```

Historical exec collection should preserve:

```text
exec timestamp
PID identity
executable path
script/file path when applicable
argv subject to sanitization policy
cwd
owner/cgroup/container context
```

For interpreted scripts:

```text
script.sh
   ↓ interpreted by
/bin/bash
```

or:

```text
worker.py
   ↓
/usr/bin/python3
```

should be modelled as two separate entities when the evidence is available.

## Important limitation

Universal Linux metadata does **not** reliably tell which Python/Bash/Java source line is executing at an arbitrary instant.

Exact current execution point belongs to:

```text
STACK / PROFILE layer
```

For native code:

```text
instruction pointer
→ memory mapping
→ ELF/shared object
→ symbol/function
```

may be available through stack profiling and symbolization.

For managed/interpreted runtimes, runtime-specific support may be required for:

```text
function
method
script line
source line
```

---

# 53. Files / FD / inode inspection

Linux allows a deterministic path:

```text
Process
  → FD
  → target
  → inode
  → mount_id
  → mount
  → filesystem
```

Example UI:

```text
 FILE DESCRIPTOR 17

 process      python3/18421
 type         file
 path         /var/log/checkout/app.log

 inode        63107
 mount_id     29
 filesystem   ext4
 mount        /var

 offset       4.28 GiB
 flags        O_WRONLY | O_APPEND

 locks        none
```

Reverse lookup:

```text
 INODE 63107

 /var/log/checkout/app.log

 HOLDERS

 > python3/18421      fd 17
   fluent-bit/2199    fd 41

 MAPPED BY

   none
```

This reverse index should be maintained by PULSE so a user can start from a file/inode rather than from a process.

---

# 54. Memory-mapped executable/files

Process mappings can be exposed as entities:

```text
PROCESS
  → mapping
  → file
  → inode
```

Example:

```text
 MAPPINGS

 RANGE                 PERM   FILE

 00400000-00452000     r-xp   /usr/bin/dbus-daemon
 7f1...                r-xp   /usr/lib/libssl.so
 7f2...                rw-p   [heap]
```

This allows navigation from a stack/profile frame back to:

```text
symbol
→ mapped object
→ inode/file
→ package/build
```

---

# 55. File locks

File locks are first-class diagnostic relations:

```text
process
  → FD
  → inode
  → lock
```

Example:

```text
 LOCK

 inode       7864554
 owner PID   2001
 type        FLOCK
 mode        WRITE
 range       0 → EOF
```

Use cases:

```text
application hangs waiting for lock
database lock file
single-instance daemon
blocked writer
```

---

# 56. Socket / port inspection

Preferred current-state model:

```text
FD
  → socket inode
  → socket
  → local endpoint
  → remote endpoint
```

Example:

```text
 SOCKET

 process       checkout/18421
 fd            22

 local         10.42.2.18:51142
 remote        10.42.8.32:5432
 state         ESTABLISHED

 rx queue      0
 tx queue      184 KiB
 retransmits   elevated
```

Reverse lookup:

```text
 PORT 8080

 LISTENERS

 > nginx/1892
   container ingress
   pod ingress-7df8
```

Historical network events:

```text
connect
accept
close
failure
```

should be captured where possible.

---

# 57. Typical diagnostic journeys

The product should explicitly support common troubleshooting paths.

## 57.1. CPU high

```text
CPU stress
 → process
 → thread
 → current/user stack
 → symbol/function
 → executable/mapped file
 → owner cgroup/unit/container/pod
```

Also inspect:

```text
CPU PSI
run queue
throttling
steal
context switches
```

---

## 57.2. High load but CPU not busy

```text
load high
 → blocked tasks
 → process/thread state
 → kernel stack / wait channel
 → IO / lock / socket / futex context
```

---

## 57.3. Memory pressure / OOM

```text
memory pressure
 → cgroup / workload
 → process
 → RSS/PSS/anon/file/shmem
 → mappings
 → swap/reclaim
 → OOM event
 → killed process
```

---

## 57.4. Disk full

```text
filesystem full
 → mount
 → files/inodes
 → top writers
 → open files
 → process holders
```

Important special investigation:

```text
unlinked/deleted file still held open
 → inode
 → holder FD
 → process
```

---

## 57.5. Inodes exhausted

```text
filesystem inode pressure
 → mount
 → inode usage
 → directories / creation activity
 → file-open/create events
 → responsible process/container
```

---

## 57.6. Slow disk / IO wait

```text
IO pressure/latency
 → block device
 → filesystem/mount
 → file/inode
 → FD
 → process
 → syscall/stack
```

---

## 57.7. Too many open files

```text
FD pressure
 → process
 → FD count vs RLIMIT_NOFILE
 → FD type histogram
 → sockets/files/pipes/epoll/inotify
 → repeated/leaking object
```

---

## 57.8. Port already in use

```text
bind failure
 → port
 → listener socket
 → FD
 → PID/process
 → unit/container/pod
```

---

## 57.9. Connection timeout

```text
timeout
 → process/span
 → socket
 → remote endpoint
 → retransmits/queues/errors
 → route/interface
 → remote service
```

If OTel context exists:

```text
timeout
 → span
 → service
 → process/container/pod
```

---

## 57.10. Service crash loop

```text
systemd/container restart loop
 → exit status/signal
 → process
 → executable/script
 → logs
 → core/stack if available
 → preceding file/network/resource events
```

---

## 57.11. Fork/process storm

```text
process count spike
 → parent
 → children
 → exec events
 → executable/script
 → owner workload
```

---

## 57.12. Hung process

```text
process not progressing
 → threads
 → state
 → kernel stack / wait reason
 → syscall
 → file/socket/lock
```

---

## 57.13. Kubernetes throttling/OOM

```text
pod problem
 → container
 → cgroup
 → limits
 → process
 → pressure/throttle/OOM
```

PULSE's value is the ability to cross the boundary from Kubernetes identity into kernel/process evidence.

---

# 58. Investigation modes inside Inspect

Every entity can expose contextual tabs/subviews:

```text
Overview
Relations
History
Resources
Files
Network
Memory
Execution
Stack
Logs
Trace
Events
Evidence
```

Only relevant tabs are displayed.

For example Disk does not show `Execution`, and Socket does not show `Memory` unless there is a meaningful view.

---

# 59. Universal relation navigation

A selected relation always uses:

```text
Enter = follow relation
Esc   = return
```

Example:

```text
Process
  Enter on FD 17
    ↓
FD
  Enter on inode
    ↓
File/Inode
  Enter on mount
    ↓
Filesystem
```

Breadcrumb:

```text
checkout/18421 > fd/17 > inode/63107 > mount/29
```

Reverse relations are normal navigation, not a special mode.

---

# 60. Evidence panel

Every derived/problem view can expose:

```text
E
```

or:

```text
: evidence
```

Example:

```text
 EVIDENCE / MEMORY PRESSURE

 OBSERVED
   memory PSI avg10 increased
   direct reclaim increased
   process RSS increased

 LINKED
   PID 18421 belongs to checkout cgroup
   cgroup belongs to checkout container

 CORRELATED
   deployment occurred 3m12s earlier

 HYPOTHESES
   none
```

This panel is important for trust.

---

# 61. Collector capability tiers

Not every relationship is available with the same privileges.

## Tier 0 — passive/unprivileged where allowed

Examples:

```text
/proc basic process information
resource metrics
own-process or accessible process metadata
basic filesystem statistics
basic netlink data where permitted
```

## Tier 1 — privileged host inspection

Adds richer:

```text
other users' /proc
FD targets
maps
kernel stacks
namespaces
systemd/cgroup relations
```

Exact access depends on:

```text
UID
ptrace restrictions
hidepid
capabilities
container namespace
LSM/security policy
```

## Tier 2 — event tracing / eBPF

Adds historical:

```text
exec
exit
open
connect/accept/close
selected failures
lifecycle
selected filesystem operations
```

This is what allows PULSE to answer questions about entities that have already disappeared.

## Tier 3 — profiling/runtime integrations

Adds:

```text
user stacks
symbols
hot functions
managed runtime frames
source-level execution when supported
```

This layer must be optional because cost, privileges and runtime support vary.

---

# 62. Privacy / sensitive data

Fields such as:

```text
argv
environment variables
file paths
URLs
database statements
trace attributes
logs
```

can contain secrets.

Default policy:

```text
collect minimum
redact known secret patterns
do not export sensitive fields by default
allow explicit opt-in
```

In particular, command arguments/environment must not be treated as harmless telemetry.

---

# 63. Current state vs historical truth

Important distinction:

## Current inspection

Can reconstruct from current kernel/proc/netlink state:

```text
current FD
current inode
current socket
current mapping
current cgroup
current executable
```

## Historical inspection

Can answer only if PULSE collected the relevant event/state before it disappeared.

Example:

```text
"What file did PID 1821 have open 40 minutes ago?"
```

requires historical FD/open/close collection or a previous snapshot.

PULSE must show:

```text
not recorded
```

instead of guessing.

---

# 64. Sources for Linux capability assumptions

Primary references used for this design:

- Linux `/proc/<pid>/fd`: https://man7.org/linux/man-pages/man5/proc_pid_fd.5.html
- Linux `/proc/<pid>/fdinfo`: https://kernel.org/doc/html/latest/filesystems/proc.html
- Linux `/proc/<pid>/exe`: https://man7.org/linux/man-pages/man5/proc_pid_exe.5.html
- Linux `/proc/<pid>/cwd`: https://man7.org/linux/man-pages/man5/proc_pid_cwd.5.html
- Linux `/proc/<pid>/maps`: https://man7.org/linux/man-pages/man5/proc_pid_maps.5.html
- Linux `/proc/<pid>/map_files`: https://man7.org/linux/man-pages/man5/proc_pid_map_files.5.html
- Linux `/proc/<pid>/mountinfo`: https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html
- Linux `/proc/<pid>/cgroup`: https://man7.org/linux/man-pages/man5/proc_pid_cgroup.5.html
- Linux socket diagnostics: https://man7.org/linux/man-pages/man7/sock_diag.7.html
- Linux `/proc/locks`: https://man7.org/linux/man-pages/man5/proc_locks.5.html
- Linux `/proc/<pid>/io`: https://man7.org/linux/man-pages/man5/proc_pid_io.5.html
- Linux `/proc/<pid>/limits`: https://man7.org/linux/man-pages/man5/proc_pid_limits.5.html
- Linux PSI: https://kernel.org/doc/html/latest/accounting/psi.html
- Inspektor Gadget `trace_exec`: https://inspektor-gadget.io/docs/main/gadgets/trace_exec/
- Inspektor Gadget `trace_open`: https://inspektor-gadget.io/docs/main/gadgets/trace_open/
- Inspektor Gadget `trace_tcp`: https://inspektor-gadget.io/docs/latest/gadgets/trace_tcp/
- OpenTelemetry process attributes: https://opentelemetry.io/docs/specs/semconv/registry/attributes/process/

---

# 65. Next design step

Next:

## Investigation UX + Incident Story screen

Need decide:

1. How Story and Investigation Graph coexist on one screen.
2. Whether selecting an event changes the relation graph.
3. How `!` events are grouped when there are thousands.
4. How Files/FD/Inode views look.
5. How current stack/profile attaches to Process Inspect.
6. How reverse lookup ("who holds this inode/port?") is surfaced.
7. Which diagnostic journeys are MVP and which are plugins/later.
8. What Tier 2 eBPF event set is worth the overhead by default.


---

# 66. Investigation Workspace

Для глубокого расследования фиксируется отдельная компоновка:

```text
Incident Story
    +
Investigation Graph
    +
Entity Inspect
```

Цель:

> пользователь должен видеть одновременно **что происходило**, **как связаны сущности** и **что представляет собой выбранный объект**.

Wide-layout:

```text
 P U L S E / INVESTIGATE                       HISTORY ◀ 13:07:18
──────────────────────────────────────────────────────────────────────────────

 STORY                         GRAPH / CONTEXT
────────────────────────────┬─────────────────────────────────────────────────
 13:01 ! deploy             │ filesystem /var
       │                    │      │
 13:03 ◇ disk pressure      │      ├─ inode 63107
       │                    │      │    │
 13:05 ◆ filesystem 98%     │      │    └─ fd 17
       │                    │      │         │
 13:06 ! file deleted       │      │         └─ python3/18421
       │                    │      │              │
>13:07 ▲ filesystem full    │      │              └─ checkout.service
       │                    │      │
 13:09 ! service restart    │      └─ nvme0n1p3
────────────────────────────┴─────────────────────────────────────────────────

 INSPECT / inode 63107
──────────────────────────────────────────────────────────────────────────────

 path          /var/log/checkout/app.log (deleted)
 size          18.4 GiB
 holders       1
 mount         /var
 filesystem    ext4

 held by
 > python3/18421   fd 17   WRITE|APPEND

──────────────────────────────────────────────────────────────────────────────
 ↑↓ story   ←→ relation   Enter follow   Esc back   E evidence   T timeline
```

---

# 67. Почему Investigation Workspace нужен отдельно от обычного Inspect

Обычный Inspect отвечает:

```text
Что это за объект?
```

Investigation Workspace отвечает:

```text
Как этот объект участвует в проблеме?
```

То есть один и тот же inode может выглядеть по-разному:

## Inspect

```text
inode 63107
path
size
mount
holders
locks
```

## Investigate

```text
inode 63107
↓
почему он вообще оказался в текущем incident
↓
какие события к нему относятся
↓
какие сущности находятся до/после него в цепочке
```

---

# 68. Сценарий 1: Disk full из-за deleted-but-open file

Это один из типичных production incidents, который PULSE должен делать почти тривиальным.

Симптом:

```text
filesystem /var
usage 100%
```

При этом обычный `du` может не показать занятое пространство, если большой файл уже удалён, но всё ещё открыт процессом.

PULSE investigation path:

```text
FILESYSTEM FULL
      ↓
mount /var
      ↓
space accounting mismatch
      ↓
deleted-but-open inode
      ↓
FD holder
      ↓
process
      ↓
service/container/pod
```

---

# 69. Overview при инциденте

```text
 P U L S E      prod-api-07       LIVE ●             1 critical
──────────────────────────────────────────────────────────────────────────────

      STATE

        ○   ◇
      ○   ◆   ▲
    ○   ●   ◆   ▲              STORAGE CRITICAL
      ○   ◆   ▲
        ○   ◇

 CPU 18%       MEM 44%       IO PSI 21%       FS /var 100%

──────────────────────────────────────────────────────────────────────────────

 WHAT NEEDS ATTENTION

 > ▲ /var filesystem full
     space available: 0
     started 2m14s ago

   ◆ IO latency elevated
     nvme0n1p3
```

`Enter` на `/var filesystem full` открывает Problem Detail / Investigation Workspace.

---

# 70. Incident Story для deleted-open-file

```text
 INCIDENT / filesystem /var                            LIVE ●

 12:58  ○ filesystem nominal
         │
 13:01  ! checkout log rotation
         │
 13:03  ◇ filesystem usage 91%
         │
 13:05  ◆ filesystem usage 98%
         │
 13:06  ! /var/log/checkout/app.log deleted
         │
 13:06  ◆ inode still held by PID 18421
         │
 13:07  ▲ filesystem reached 100%
         │
 13:07  ! write failures started
         │
 13:09  ! checkout.service restarted
         │
 13:09  ○ 18.4 GiB released
```

Важно:

```text
file deleted
```

и:

```text
inode still held
```

это разные события/состояния.

PULSE не должен считать удалённый pathname существующим файлом.

В UI:

```text
/var/log/checkout/app.log (deleted)
```

означает:

> pathname больше не существует, но inode всё ещё открыт процессом.

---

# 71. Первый экран расследования

```text
 P U L S E / INVESTIGATE / FS FULL                  LIVE ●
──────────────────────────────────────────────────────────────────────────────

 STORY                         INVESTIGATION
────────────────────────────┬─────────────────────────────────────────────────
 13:03 ◇ FS 91%             │                /var
       │                    │                 │
 13:05 ◆ FS 98%             │                 ▼
       │                    │           inode 63107
 13:06 ! file deleted       │           18.4 GiB
       │                    │           (deleted)
 13:06 ◆ still held         │                 │
       │                    │                 ▼
>13:07 ▲ FS 100%            │              fd 17
       │                    │                 │
 13:07 ! writes failing     │                 ▼
                            │          python3 / 18421
                            │                 │
                            │                 ▼
                            │       checkout.service
────────────────────────────┴─────────────────────────────────────────────────

 WHY THIS MATTERS

 18.4 GiB is still allocated because inode 63107 has an open holder.

 EVIDENCE
 observed      fd 17 references inode 63107
 observed      pathname is deleted
 deterministic inode belongs to mount /var
 deterministic fd 17 belongs to PID 18421

──────────────────────────────────────────────────────────────────────────────
 Enter follow   E evidence   T timeline   : actions   Esc back
```

---

# 72. Навигация по расследованию

## Шаг 1 — filesystem

```text
/var
```

Enter:

```text
FILESYSTEM /var
```

## Шаг 2 — выделяем suspicious inode

```text
DELETED BUT OPEN

> inode 63107     18.4 GiB     holder 1
  inode 71201      1.2 GiB     holder 2
```

Enter:

```text
INODE 63107
```

## Шаг 3 — holder

```text
HOLDERS

> python3/18421     fd 17
```

Enter:

```text
FD 17
```

## Шаг 4 — FD

```text
FD 17

target       /var/log/checkout/app.log (deleted)
inode        63107
offset       18.4 GiB
flags        WRITE | APPEND

owner        python3/18421
```

Enter on owner:

```text
PROCESS 18421
```

## Шаг 5 — process

```text
python3/18421
  ↓
checkout.service
  ↓
container checkout
  ↓
pod checkout-7d9f
```

---

# 73. Reverse investigation

Пользователь может начать не с filesystem, а с процесса:

```text
python3/18421
  ↓ Files
deleted open files
  ↓
fd 17
  ↓
inode 63107
  ↓
mount /var
  ↓
filesystem full
```

То есть Investigation Graph не имеет фиксированного направления.

---

# 74. Files view процесса

```text
 FILES / python3 18421

 OPEN FILES                                              142

 STATE   FD   SIZE       PATH

 ○       3    —          /dev/null
 ○       4    2.1 MiB    /opt/app/config.json
 ◆      17   18.4 GiB    /var/log/checkout/app.log (deleted)
 ○      23    128 KiB    /tmp/cache.db

────────────────────────────────────────────────────────────

 SUMMARY

 regular files          83
 sockets                31
 pipes                  11
 eventfd/epoll          17
 deleted-but-open        1
```

`◆` здесь означает abnormal object state, не «сам файл warning по проценту».

---

# 75. Filesystem view

```text
 FILESYSTEM /var                                      ▲ FULL

 device        nvme0n1p3
 type          ext4
 used          100%
 available     0

 SPACE ATTRIBUTION

 visible files                 72.1 GiB
 deleted-but-open              18.4 GiB   ▲
 metadata / reserved            3.2 GiB

 DELETED BUT OPEN

 > inode 63107                 18.4 GiB
   inode 71201                  212 MiB
```

Если точная attribution не может быть рассчитана, PULSE не должен подставлять оценку как факт.

Например:

```text
visible files       unknown
```

или:

```text
estimated
```

с явной маркировкой.

---

# 76. Что делает такой workflow сильнее обычного набора CLI

Классический поиск может включать:

```text
df
du
lsof +L1
/proc/<pid>/fd
stat
systemctl
docker/containerd
kubectl
```

PULSE превращает это в один navigation chain:

```text
FS full
  ↓
deleted inode
  ↓
FD
  ↓
process
  ↓
service/container/pod
```

Важно:

PULSE не скрывает underlying evidence.

В `Evidence` должны быть доступны исходные факты, на которых построена связь.

---

# 77. Contextual Quick Paths

На Problem Detail можно показывать 2–5 наиболее вероятных следующих направлений расследования.

Например:

```text
QUICK PATHS

> Deleted-open files         18.6 GiB
  Top writers                checkout.service
  IO latency                 nvme0n1p3
  Inode usage                42%
```

Это не «AI recommendation» по умолчанию.

Quick Path формируется из уже наблюдаемой topology/evidence.

---

# 78. Investigation Graph: visual grammar

Тип сущности должен различаться подписью, а не десятком новых Unicode-symbols.

Пример:

```text
[fs] /var
   │
   ▼
[file] inode 63107
   │
   ▼
[fd] 17
   │
   ▼
[proc] python3/18421
   │
   ▼
[unit] checkout.service
```

В более визуальном режиме:

```text
 /var
  ○
  │
  ◆ inode 63107
  │
  ● fd 17
  │
  ● python3/18421
  │
  ○ checkout.service
```

Severity symbols остаются про состояние, а entity-kind подписывается текстом.

---

# 79. Focus model Investigation Workspace

В workspace существует один активный focus:

```text
STORY
GRAPH
INSPECT
```

Переключение:

```text
Tab / Shift+Tab
```

или:

```text
← / →
```

когда горизонтальная навигация не занята самим graph.

Предлагаемое правило:

```text
Tab          next pane
Shift+Tab    previous pane

↑↓           move inside pane
Enter        follow/open
Esc          back
```

Это уменьшает неоднозначность стрелок.

---

# 80. Selecting Story event changes Graph context

Если пользователь выбирает:

```text
13:06 ! file deleted
```

Graph перестраивается вокруг события:

```text
event
  ↓
inode 63107
  ↓
fd 17
  ↓
python3/18421
```

Если выбирает:

```text
13:07 ▲ filesystem full
```

Graph:

```text
filesystem /var
  ├─ deleted-open 18.4 GiB
  ├─ visible files
  └─ nvme0n1p3
```

То есть Story и Graph связаны bidirectionally.

---

# 81. Selecting Graph entity filters Story

Обратная связь тоже работает.

Выбрали:

```text
python3/18421
```

Story подсвечивает только события, связанные с ним:

```text
13:02 ! exec python3
13:06 ! file deleted while FD remained open
13:07 ! write failed ENOSPC
13:09 ! process exit
```

Остальные события становятся dimmed, но не исчезают полностью.

---

# 82. Evidence levels визуально

Предлагаемая маркировка связей:

```text
────  deterministic
····  correlated
????  hypothesis only in Evidence/Analysis view
```

Пример:

```text
deploy
  ···· memory pressure
          │
          │ deterministic evidence
          ▼
       reclaim
```

Но в основном Story лучше не перегружать типом линии.

Полная epistemic детализация раскрывается через:

```text
E
```

---

# 83. Event grouping

При тысячах однотипных событий Story показывает aggregate node:

```text
! write failures ×18,421
```

Enter:

```text
EVENT GROUP

first       13:07:03.182
last        13:07:48.903
count       18,421

errno
  ENOSPC    18,417
  EIO            4

entities
  checkout/18421     17,901
  fluent-bit/2199       520
```

---

# 84. Diagnostic Journey Catalog

PULSE должен иметь внутренний каталог типовых investigation patterns.

Не как жёсткие wizard'ы, а как knowledge of relations.

Первый MVP catalog:

```text
filesystem_full
inode_exhaustion
deleted_open_file
fd_exhaustion
port_in_use
connection_timeout
memory_pressure
oom
cpu_pressure
cpu_throttling
high_load_low_cpu
io_latency
process_hang
service_crash_loop
fork_storm
```

Каждый journey определяет:

```text
entry symptoms
relevant entity types
relevant relations
recommended evidence
quick paths
```

---

# 85. MVP Investigation Graph boundaries

В MVP стоит поддержать хорошо:

```text
Process
Thread basic
Executable
Script/interpreter basic
FD
File
Inode
Mount
Filesystem
Socket
Port
Endpoint
Cgroup
SystemdUnit
Container
Pod basic
Event
Problem
```

Later:

```text
full source-level stack
JVM/Python/.NET runtime frames
deep block IO attribution
DNS transaction graph
TLS internals
database protocol introspection
GPU stack
```

Причина:

основная ценность Navigation Graph уже появляется без сложнейших profilers.

---

# 86. Next scenario

Следующим рекомендуется проработать сетевой incident:

```text
DB timeout
  ↓
span/request
  ↓
process
  ↓
socket
  ↓
remote endpoint
  ↓
retransmits / connect failures / queue
  ↓
remote service
```

Он проверит:

- socket/port navigation;
- endpoint identity;
- OTel integration;
- relationship between Incident Story and Trace;
- difference between correlation and causality;
- how external/remote entities appear in local-first PULSE.


---

# 87. Adaptive Terminal Layout

PULSE не должен проектироваться под один размер терминала.

Layout определяется одновременно:

```text
width
height
current mode
content priority
focus
```

Интерфейс не масштабируется как картинка.

Он **перекомпоновывается**.

Главный принцип:

> При уменьшении терминала PULSE сначала убирает второстепенное, потом меняет композицию, и только в самом конце сокращает данные.

---

# 88. Размер терминала — часть runtime state

На каждом render pass layout получает:

```text
TerminalSize {
    columns
    rows
}
```

Изменение размера окна должно применяться сразу.

Пользователь может:

```text
растянуть окно
сжать окно
развернуть terminal
split tmux pane
изменить font size
```

и PULSE не требует restart.

---

# 89. Не фиксированные экраны, а layout policies

Каждый screen описывается не абсолютными координатами:

```text
x=4
y=12
width=80
```

а правилами:

```text
priority
min_width
preferred_width
min_height
grow
shrink
collapse_mode
```

Пример:

```text
Pane {
    name: Story
    priority: High
    min_width: 28
    preferred_width: 40
    grow: 1
    collapse_mode: Tabs
}
```

---

# 90. Основные width classes

Это не жёсткие стандарты, а стартовые breakpoints.

## XL

```text
>= 160 columns
```

Можно одновременно показывать:

```text
Story
Graph
Inspect
Metrics
```

Пример:

```text
┌────────────┬──────────────────────┬───────────────────────┐
│ STORY      │ GRAPH                │ INSPECT               │
│            │                      │                       │
│            │                      │                       │
└────────────┴──────────────────────┴───────────────────────┘
┌───────────────────────────────────────────────────────────┐
│ METRICS / STATE RIVER                                     │
└───────────────────────────────────────────────────────────┘
```

---

## Wide

```text
120–159 columns
```

Основной режим:

```text
[ Story ][ Graph ]
[ Inspect        ]
```

или:

```text
[ Story ][ Graph ][ compact Inspect ]
```

в зависимости от высоты.

---

## Medium

```text
90–119 columns
```

Две панели:

```text
[ Story ][ Graph ]
[ Inspect        ]
```

Inspect занимает нижнюю часть.

---

## Narrow

```text
60–89 columns
```

Одна активная панель + tabs:

```text
[ STORY | GRAPH | INSPECT ]

content
```

Переключение:

```text
Tab / Shift+Tab
```

---

## Tiny

```text
< 60 columns
```

Показывается только основное:

```text
title
state
focused content
footer
```

Все второстепенные детали доступны через Enter / tabs / overlays.

---

# 91. Height classes

Ширина — не единственный фактор.

## Tall

```text
>= 40 rows
```

Можно показывать:

```text
header
glyph
story
graph
inspect
metrics
footer
```

## Normal

```text
24–39 rows
```

Используется основной рабочий layout.

## Short

```text
16–23 rows
```

Скрываются:

```text
decorative headers
secondary metrics
empty sections
extended help/footer
```

## Very short

```text
< 16 rows
```

PULSE переходит в focus mode.

---

# 92. Priority system

Каждый UI block получает priority.

## P0 — never hide

```text
current mode
time state LIVE/HISTORY
current selection
critical problem state
active content
navigation hint
```

## P1 — important

```text
State River
primary metric
Story
Graph
Inspect summary
```

## P2 — useful

```text
secondary metrics
Recent Changes
related entities
breadcrumbs
```

## P3 — optional

```text
long descriptions
secondary labels
decorative whitespace
extended footer
```

При уменьшении окна скрытие идёт:

```text
P3 → P2 → layout change → P1 compression
```

P0 не исчезает.

---

# 93. Compression strategy

PULSE использует несколько уровней сжатия.

## Level 0 — full

```text
MEMORY PRESSURE
memory PSI        38.4%
direct reclaim    2.1 GiB/s
affected          23 entities
```

## Level 1 — compact

```text
MEM PRESSURE  ◆
PSI 38%  reclaim 2.1G/s  affected 23
```

## Level 2 — terse

```text
MEM ◆  PSI38%  R2.1G
```

## Level 3 — icon/state only

```text
MEM ◆
```

Смысл не должен исчезать при compression.

---

# 94. Table column degradation

Таблицы не должны просто обрезаться справа.

Колонки имеют приоритет.

Пример Entity table:

```text
ENTITY | KIND | CPU | MEM | IO | NET | OWNER | STATE
```

При сужении:

## Full

```text
ENTITY          KIND       CPU   MEM   IO   NET   OWNER          STATE
checkout-api    process    82%   1.8G  4M   9M    checkout.svc   ◆
```

## Medium

```text
ENTITY          CPU   MEM   OWNER          STATE
checkout-api    82%   1.8G  checkout.svc   ◆
```

## Narrow

```text
ENTITY          CPU   MEM   S
checkout-api    82%   1.8G  ◆
```

## Tiny

```text
checkout-api  ◆
```

---

# 95. Column priority metadata

Каждая table column должна иметь:

```text
priority
min_width
preferred_width
truncate
align
hide_below
```

Пример:

```text
STATE       priority 0
ENTITY      priority 0
CPU         priority 1
MEM         priority 1
OWNER       priority 2
KIND        priority 2
IO          priority 3
NET         priority 3
```

---

# 96. Text truncation

Длинные значения не должны ломать layout.

Пример:

```text
checkout-api-production-worker.service
```

может стать:

```text
checkout-api-prod…
```

или при очень узком layout:

```text
checkout…
```

Полное значение доступно через:

```text
Enter
Inspect
tooltip-like status line
```

---

# 97. Breadcrumb compression

Full:

```text
asuspc > system.slice > checkout.service > container > pod > python3/18421
```

Medium:

```text
asuspc > … > checkout.service > python3/18421
```

Narrow:

```text
… > python3/18421
```

Breadcrumb не должен съедать рабочую ширину.

---

# 98. Glyph scaling

Glyph должен иметь несколько rendering sizes.

## Large

```text
        ○   ○
      ○   ●   ○
    ○   ●   ●   ○
  ○   ●   ●   ●   ○
    ○   ●   ●   ○
      ○   ●   ○
        ○   ○
```

## Medium

```text
  ○ ● ○
○ ● ● ● ○
  ○ ● ○
```

## Compact

```text
○●●◆○
```

## Minimal

```text
◆
```

Важно:

> при уменьшении glyph сокращается его spatial detail, но severity остаётся читаемой.

---

# 99. Glyph budget

Glyph не имеет фиксированную высоту.

Renderer получает:

```text
available_width
available_height
importance
```

и выбирает preset:

```text
Large
Medium
Compact
Minimal
Hidden
```

`Hidden` допустим только если состояние уже явно показано в header или focused content.

---

# 100. State River responsive rendering

State River естественно масштабируется по ширине.

Wide:

```text
○○○○○○○◇◇◇◆◆▲▲◆◇◇○○○○○○○○○○○○
```

Narrow:

```text
○○◇◆▲◆◇○
```

Tiny:

```text
○◇▲◇○
```

Downsampling выполняется через semantic buckets, а не простым удалением символов.

Critical markers `!` / `▲` не теряются.

---

# 101. Incident Story compression

Wide:

```text
13:06 ! /var/log/checkout/app.log deleted
       │
13:07 ▲ filesystem reached 100%
```

Medium:

```text
13:06 ! file deleted
13:07 ▲ FS full
```

Narrow:

```text
13:06 ! delete
13:07 ▲ full
```

Tiny:

```text
! delete
▲ full
```

Полные details доступны в Inspect.

---

# 102. Investigation Workspace adaptive layouts

## XL

```text
┌────────────┬──────────────────┬──────────────────┐
│ STORY      │ GRAPH            │ INSPECT          │
└────────────┴──────────────────┴──────────────────┘
```

## Wide

```text
┌────────────┬──────────────────────────────┐
│ STORY      │ GRAPH                        │
├────────────┴──────────────────────────────┤
│ INSPECT                                   │
└───────────────────────────────────────────┘
```

## Medium

```text
┌───────────────────────────────────────────┐
│ STORY                                     │
├───────────────────────────────────────────┤
│ GRAPH                                     │
├───────────────────────────────────────────┤
│ INSPECT summary                           │
└───────────────────────────────────────────┘
```

## Narrow

```text
[ STORY | GRAPH | INSPECT ]

active pane only
```

## Tiny

```text
INVESTIGATE  ▲

[GRAPH]

inode 63107
  ↓
fd 17
  ↓
python3/18421

Tab panes
```

---

# 103. Focus enlarges the active pane

Даже на wide layout активная панель может получить больше места.

Например пользователь работает в Graph:

До:

```text
Story 35% | Graph 40% | Inspect 25%
```

После focus:

```text
Story 20% | Graph 60% | Inspect 20%
```

Опциональная команда:

```text
z
```

или:

```text
Space
```

может toggle:

```text
Focus Pane
```

Full-screen pane:

```text
Graph only
```

Повторное нажатие возвращает split layout.

---

# 104. Overlay вместо постоянной панели

Некоторые данные лучше не ужимать.

Например:

```text
Help
Command Palette
Evidence Detail
Full event payload
Full argv
Full path
```

На маленьком терминале они открываются как full-screen overlay.

Это лучше, чем пытаться держать 4 панели одновременно.

---

# 105. Footer compression

Wide:

```text
↑↓ select  ←→ relation  Enter inspect  Tab pane  E evidence  T timeline  : actions  ? help
```

Medium:

```text
↑↓ select  Enter open  Tab pane  : actions  ? help
```

Narrow:

```text
↕ select  ↵ open  Tab pane  ? help
```

Tiny:

```text
? keys
```

---

# 106. Header compression

Wide:

```text
PULSE / INVESTIGATE   prod-api-07   HISTORY ◀ 13:07:18   2 problems
```

Medium:

```text
PULSE INVESTIGATE   HISTORY 13:07   ▲2
```

Narrow:

```text
PULSE  HIST 13:07  ▲2
```

Tiny:

```text
HIST 13:07 ▲2
```

---

# 107. Empty sections collapse automatically

Не нужно резервировать место под пустые панели.

Например:

```text
PROBLEMS
  none
```

в compact layout исчезает полностью.

В wide layout может остаться:

```text
✓ nominal
```

одной строкой.

---

# 108. Content-aware resizing

Размер pane зависит не только от терминала, но и от содержимого.

Например:

```text
Story contains 2 events
```

не должен занимать 40% экрана.

Свободное место отдаётся:

```text
Graph
Inspect
Entities
```

И наоборот, если Story содержит большой incident, она получает больше высоты.

---

# 109. Stable layout rule

При незначительном изменении терминала layout не должен постоянно прыгать между режимами.

Нужен breakpoint hysteresis.

Например:

```text
enter Wide at >= 122 columns
leave Wide at <= 116 columns
```

Это предотвращает визуальное дрожание при resize около границы.

---

# 110. Minimum supported terminal

Предварительно:

```text
minimum usable: 50 × 12
recommended:    80 × 24
comfortable:   120 × 30
ideal wide:    160 × 40+
```

Если окно меньше minimum:

```text
PULSE

terminal too small
need at least 50×12
current 43×9

q quit
```

Точные числа должны быть проверены на реальном prototype.

---

# 111. Responsive design invariants

- Никакой horizontal scroll для основных экранов.
- Critical state никогда не скрывается.
- Выбранная entity всегда видна.
- LIVE/HISTORY всегда виден.
- `Enter`, `Esc`, `Tab`, `/`, `:` сохраняют смысл при любом размере.
- Secondary panels могут превращаться в tabs.
- Колонки исчезают по priority, а не случайно.
- Glyph масштабируется дискретными presets.
- State River downsample semantic, а не character-drop.
- Empty blocks автоматически collapse.
- Resize не должен сбрасывать selection или navigation stack.
- Resize не должен менять текущую time position.
- Resize не должен закрывать Investigation context.

---

# 112. Layout Engine data model

Предлагаемая модель:

```text
LayoutContext {
    terminal_width
    terminal_height

    screen
    focus_pane

    live_or_history
    severity

    content_density
}
```

Результат:

```text
LayoutPlan {
    mode

    panes[]
    visible_columns[]
    glyph_preset
    header_preset
    footer_preset
}
```

Каждый pane:

```text
PanePlan {
    visible
    rect
    compression_level
    focusable
}
```

---

# 113. Layout selection order

Renderer делает:

```text
1. measure terminal
2. determine width/height class
3. determine required P0 content
4. allocate focused pane
5. allocate P1 panes
6. collapse/merge P2 panes
7. drop P3 content
8. choose table columns
9. choose glyph preset
10. render
```

Это должно быть deterministic.

Одинаковый размер + screen + focus → одинаковый layout.

---

# 114. Resize behavior

При resize сохраняются:

```text
selected entity
selected event
scroll position where possible
navigation stack
time cursor
A/B markers
focus pane
search/filter state
```

Меняется только presentation.

---

# 115. Example: same investigation at four sizes

## 160×40

```text
STORY | GRAPH | INSPECT
full labels
large glyph
full footer
metrics visible
```

## 120×30

```text
STORY | GRAPH
INSPECT below
medium glyph
reduced columns
```

## 80×24

```text
STORY
GRAPH
INSPECT compact
compact glyph
minimal footer
```

## 60×18

```text
[STORY|GRAPH|INSPECT]

active pane only
state symbol in header
full details on Enter
```

Functionality remains the same.

Only simultaneous visibility changes.

---

# 116. Key principle

Responsive PULSE means:

```text
same information model
same navigation model
same investigation context
different presentation density
```

Not:

```text
desktop UI
↓
cut pieces off until it fits
```

---

# 117. Next implementation proof

Before implementing full collectors, build a fake-data TUI prototype that can be resized live through at least:

```text
50×12
60×18
80×24
100×30
120×30
160×40
200×50
```

Test screens:

```text
Overview healthy
Overview critical
Timeline
Investigation Workspace
Process Inspect
Entity table
```

This prototype should validate:

```text
breakpoints
glyph presets
table column priorities
pane collapse
focus behavior
footer/header compression
```

before the architecture becomes expensive to change.


---

# 118. Specification precision: normative TUI mockups

Начиная с v0.7, важные экраны и блоки должны описываться не только словами, но и **нормативными TUI mockups в fenced code blocks**.

Причина:

```text
"показать State Glyph сверху"
```

слишком неоднозначно.

Нужно фиксировать:

```text
- относительное положение блоков;
- минимальные отступы;
- порядок информации;
- что показывается при пустом состоянии;
- что скрывается при resize;
- какие строки selectable;
- что считается событием;
- что НЕ считается событием.
```

Mockup не является pixel-perfect контрактом для каждого terminal size, но задаёт:

```text
information hierarchy
interaction hierarchy
visual rhythm
minimum content
forbidden substitutions
```

---

# 119. Large State Glyph: fixed 9×7 visible matrix

Для основного Overview фиксируется видимый preset:

```text
9 columns × 7 rows
```

Каждая позиция матрицы **рендерится всегда**.

Фоновая/неактивная ячейка:

```text
·
```

Базовый healthy glyph:

```text
· · · ○ ○ ○ · · ·
· · ○ ● ● ● ○ · ·
· ○ ● ● ● ● ● ○ ·
○ ● ● ● ● ● ● ● ○
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·
```

Это считается **reference healthy silhouette** для Large preset.

Пустые позиции не заменяются пробелами.

Нельзя:

```text
      ○ ○ ○
    ○ ● ● ● ○
  ○ ● ● ● ● ● ○
○ ● ● ● ● ● ● ● ○
```

Нужно:

```text
· · · ○ ○ ○ · · ·
· · ○ ● ● ● ○ · ·
· ○ ● ● ● ● ● ○ ·
○ ● ● ● ● ● ● ● ○
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·
```

Причина:

- фиксированное visual field;
- стабильная геометрия;
- проще замечать локальную деформацию;
- glyph выглядит как instrument/matrix, а не декоративная россыпь;
- `·` визуально связывает PULSE с dot-matrix языком.

---

# 120. Glyph cell semantics

Видимый алфавит:

```text
·  ○  ●  ◉  ◇  ◆  ▲  ×
```

Дополнительно Timeline использует:

```text
!
```

но `!` **не является Glyph cell state**.

Правила:

```text
· = background / no active state contribution
○ = normal edge/reference
● = normal active mass
◉ = saturated/hot but controlled
◇ = degraded
◆ = warning
▲ = critical
× = lost/failed function
```

---

# 121. Healthy / Busy / Degraded / Critical reference glyphs

## Healthy

```text
· · · ○ ○ ○ · · ·
· · ○ ● ● ● ○ · ·
· ○ ● ● ● ● ● ○ ·
○ ● ● ● ● ● ● ● ○
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·
```

## Busy but controlled

```text
· · · ○ ○ ○ · · ·
· · ○ ● ● ● ○ · ·
· ○ ● ● ◉ ● ● ○ ·
○ ● ● ◉ ◉ ◉ ● ● ○
· ○ ● ● ◉ ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·
```

## Degraded on the right side

Example: memory/capacity side is degrading.

```text
· · · ○ ○ ◇ · · ·
· · ○ ● ● ● ◇ · ·
· ○ ● ● ● ● ◆ ◇ ·
○ ● ● ● ● ◆ ◆ ◆ ·
· ○ ● ● ● ● ◆ ◇ ·
· · ○ ● ● ● ◇ · ·
· · · ○ ○ ◇ · · ·
```

## Critical on the right side

```text
· · · ○ ◇ ▲ · · ·
· · ○ ● ◆ ▲ ▲ · ·
· ○ ● ● ◆ ▲ ▲ ▲ ·
○ ● ● ◆ ▲ ▲ ▲ ▲ ·
· ○ ● ● ◆ ▲ ▲ ▲ ·
· · ○ ● ◆ ▲ ▲ · ·
· · · ○ ◇ ▲ · · ·
```

## Failure

`×` появляется только там, где функция уже потеряна:

```text
· · · ○ ◇ ▲ · · ·
· · ○ ● ◆ ▲ ▲ · ·
· ○ ● ● ◆ × ▲ ▲ ·
○ ● ● ◆ ▲ ▲ ▲ ▲ ·
· ○ ● ● ◆ ▲ ▲ ▲ ·
· · ○ ● ◆ ▲ ▲ · ·
· · · ○ ◇ ▲ · · ·
```

---

# 122. Glyph spatial semantics

Для System Health Glyph фиксируется ориентировочная spatial mapping:

```text
          COMPUTE
             ↑

 NETWORK  ← glyph →  MEMORY/CAPACITY

             ↓
          IO/STORAGE
```

Это не означает, что конкретная ячейка является конкретной метрикой.

Смысл:

```text
top deformation     → compute-related stress
right deformation   → memory/capacity stress
bottom deformation  → storage/IO stress
left deformation    → network stress
```

Комбинированная проблема может деформировать несколько областей.

Пример:

```text
CPU + memory
→ upper-right deformation
```

Severity всё ещё определяется символом:

```text
◇ ◆ ▲ ×
```

а не позицией.

---

# 123. Overview screen — normative Wide example

Для healthy host wide-screen должен визуально быть близок к:

```text
P U L S E   asuspc        LIVE ●        14:26:41        up 22h37m
──────────────────────────────────────────────────────────────────────────────

STATE

· · · ○ ○ ○ · · ·          SYSTEM NOMINAL
· · ○ ● ● ● ○ · ·          0 problems
· ○ ● ● ● ● ● ○ ·          stable for 22h37m
○ ● ● ● ● ● ● ● ○
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·

CPU 0%      MEM 11%      PSI MEM 0%      IO 0%      NET 207 MiB / 7.2 MiB
──────────────────────────────────────────────────────────────────────────────
WHAT NEEDS ATTENTION

✓ nothing requires attention
──────────────────────────────────────────────────────────────────────────────
RECENT CHANGES

14:21  ! docker-desktop restarted
13:58  ! network interface wlan0 reconnected
12:43  ! package configuration changed
──────────────────────────────────────────────────────────────────────────────
ENTITIES 241                                      sort: relevance  filter: all

NAME                         KIND       CPU    MEM       STATE   OWNER
> pulse                      process    0.03   6.9 MiB   ○       init.scope
  system.slice               cgroup     0.02   624 MiB   ○       —
  docker-desktop             container  0.00   15.4 MiB  ○       —
  ModemManager.service       unit       0.00   5.3 MiB   ○       system.slice

──────────────────────────────────────────────────────────────────────────────
↑↓ select   Enter inspect   / search   : commands   P problems   T timeline
```

Важно:

- `STATE` и glyph образуют один блок;
- metrics strip находится **после glyph**, а не разбивает его;
- `WHAT NEEDS ATTENTION` не получает огромную пустую высоту;
- `RECENT CHANGES` показывает только meaningful changes;
- entities получают оставшееся пространство;
- default sort для Overview — `relevance`, а не обязательно CPU.

---

# 124. What counts as Recent Changes

`Recent Changes` — не raw entity discovery feed.

Нельзя показывать стартовую инвентаризацию:

```text
14:26:38 + created pulse
14:26:38 + created bash
14:26:38 + created systemd
14:26:38 + created login
14:26:38 + created 200 more entities
```

При запуске PULSE текущие объекты формируют:

```text
BASELINE SNAPSHOT
```

и **не считаются изменениями**, если они уже существовали до старта наблюдения.

`Recent Changes` включает только события после baseline или известные исторические события:

```text
process/service restart
exec of interesting process
container/pod replacement
config change
mount change
network link change
deploy
OOM
crash
resource-state transition
significant relation change
```

Короткоживущие обычные процессы могут быть suppressed/aggregated.

---

# 125. Baseline semantics

При первом старте:

```text
t0 = observer attached
```

PULSE строит:

```text
BaselineSnapshot
```

Существующие entities:

```text
state = existing
```

а не:

```text
event = created
```

Только объекты, появившиеся **после t0**, получают:

```text
! created / ! exec / ! started
```

Исключение:

если historical collector/backend уже знает реальное время создания объекта, это может быть показано в history, но не должно массово выглядеть как событие текущего запуска PULSE.

---

# 126. Problems empty screen — normative behavior

Текущий вариант:

```text
PROBLEMS
проблем не обнаружено
<пустой экран>
```

нежелателен.

Если Problems пуст:

```text
P U L S E   asuspc      LIVE ●       PROBLEMS
──────────────────────────────────────────────────────────────────────────────

✓ NO ACTIVE PROBLEMS

System has remained nominal for 22h37m.

STATE
· · · ○ ○ ○ · · ·
· · ○ ● ● ● ○ · ·
· ○ ● ● ● ● ● ○ ·
○ ● ● ● ● ● ● ● ○
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·

RECENT RESOLVED
13:41  ◇ wlan0 packet loss          resolved in 18s
11:07  ◆ docker-desktop restart     resolved in 42s

Enter resolved incident to inspect history.

──────────────────────────────────────────────────────────────────────────────
Esc overview   T timeline   / search
```

Если historical data отсутствует:

```text
✓ NO ACTIVE PROBLEMS

No problems have been observed since PULSE started 4m18s ago.

Esc overview   T timeline
```

Не резервировать пустой экран.

---

# 127. Problems non-empty screen — normative example

```text
P U L S E   prod-api-07    LIVE ●       PROBLEMS 3
──────────────────────────────────────────────────────────────────────────────

STATE       CURRENT PROBLEMS

◆           > ▲ Memory pressure
               host / prod-api-07
               critical for 2m14s
               affects checkout-api, postgres

              ◆ DB timeout burst
               checkout-api → 10.42.8.32:5432
               184 failures / 42s

              ◇ nvme0n1 latency
               p99 48ms
──────────────────────────────────────────────────────────────────────────────

SELECTED / MEMORY PRESSURE

WHY
memory PSI 38% → direct reclaim 2.1 GiB/s → checkout latency elevated

AFFECTED
postgres ▲    checkout-api ◆    redis ◇

RECENT
14:21:07  ◇ pressure started
14:21:44  ◆ reclaim increased
14:22:31  ! timeout burst
14:23:02  ▲ critical

──────────────────────────────────────────────────────────────────────────────
↑↓ select   Enter investigate   E evidence   T timeline   Esc overview
```

---

# 128. Entities screen — normative Wide example

На wide terminal:

```text
P U L S E   asuspc      LIVE ●       ENTITIES 241
──────────────────────────────────────────────────────────────────────────────

ENTITIES                                  INSPECTOR
───────────────────────────────────────┬──────────────────────────────────────
NAME             KIND      CPU   MEM   │ cgroup /
> cgroup         cgroup    .06   0 B   │ STATE ○
  init.scope     unit      .04   90M   │
  pulse          process   .04   10M   │ CPU       0.06 cores
  system.slice   cgroup    .00   628M  │ CPU limit none
  angie.service  unit      .00   376M  │ IO PSI    0%
                                         │
                                         │ RELATIONS
                                         │ parent     host/asuspc
                                         │ children   12
                                         │ backed by  6 disks
                                         │
                                         │ LABELS
                                         │ path       /
                                         │ disks      sda … sdf
───────────────────────────────────────┴──────────────────────────────────────
↑↓ select   Enter inspect   Tab focus   f filter   s sort   Esc overview
```

Правила:

- table и inspector имеют независимые minimum widths;
- relation values не должны обрезаться до бессмысленных фрагментов;
- если inspector не помещается — он превращается в tab, а не в узкую испорченную колонку;
- inspector показывает агрегированные отношения (`backed by 6 disks`) и раскрывает список по Enter;
- не выводить 6 почти одинаковых строк `backed_by disk/sdX`, если summary полезнее.

---

# 129. Entities Medium / Narrow examples

## Medium

```text
ENTITIES 241

NAME                 CPU    MEM      S
> cgroup              .06   0 B      ○
  init.scope          .04   90 MiB   ○
  pulse               .04   10 MiB   ○
  system.slice        .00   628 MiB  ○

────────────────────────────────────────
INSPECT / cgroup /

CPU       0.06
IO PSI    0%
parent    host/asuspc
backed    6 disks

Tab inspector   Enter open
```

## Narrow

```text
ENTITIES 241

> cgroup          ○
  init.scope      ○
  pulse           ○
  system.slice    ○

[LIST | INSPECT]

Enter open   Tab pane
```

---

# 130. Full Inspector — normative example

Full Inspector should not leave most of the screen meaningless.

```text
P U L S E / INSPECT / cgroup /
──────────────────────────────────────────────────────────────────────────────

cgroup /                                                     STATE ○

SUMMARY
CPU             0.15 cores
CPU limit       none
MEM             628 MiB
IO PSI          0%

RELATIONS
parent          host/asuspc
children        12 cgroups
backed by       6 block devices

RESOURCE GRAPH
CPU    ▁▁▂▁▁▂▃▂▁
MEM    ▄▄▄▄▄▄▄▄▄
IO     ▁▁▁▁▂▁▁▁▁

CHILDREN
> init.scope
  system.slice
  user.slice
  sys-fs-fuse-connections.mount

BLOCK DEVICES
sda  ○   sdb  ○   sdc  ○   sdd  ○   sde  ○   sdf  ○

RECENT EVENTS
14:22  ! child cgroup created: docker-...
13:57  ! memory limit changed

──────────────────────────────────────────────────────────────────────────────
↑↓ select relation   Enter follow   T history   E evidence   Esc back
```

Если данных для нижних блоков нет, layout автоматически отдаёт место другим блокам.

---

# 131. Timeline — normative initial view

Timeline не должен начинаться как raw event list.

Нормативный верх:

```text
P U L S E / TIME MACHINE                       LIVE ●
──────────────────────────────────────────────────────────────────────────────

14:00       14:10       14:20       14:30       NOW
──○──────────○───────────◇────◆──────○───────────●──>
                         │    │
                       PSI   ! restart

STATE
○ ○ ○ ○ ○ ○ ○ ○ ◇ ◇ ◆ ◆ ◇ ○ ○ ○ ○ ○ ○ ○

CPU   ▁▁▁▂▂▂▃▄▅▇█▆▄▃▂▂▁▁
MEM   ▂▂▂▂▃▄▅▆▇████▇▆▄▃▂
IO    ▁▁▁▁▁▂▃▅▆█▇▄▂▁▁▁▁▁
──────────────────────────────────────────────────────────────────────────────

STORY

14:21  ◇ memory pressure
14:23  ◆ reclaim increased
14:26  ! docker-desktop restarted
14:27  ○ recovered

──────────────────────────────────────────────────────────────────────────────
←→ scrub   +/- zoom   ↑↓ story   Enter inspect   A/B mark   D diff   F live
```

Raw event stream существует как отдельный subview:

```text
: raw events
```

но не является основным Timeline UX.

---

# 132. Timeline baseline rule

На первом запуске PULSE:

Нельзя:

```text
TIMELINE 1/200

14:26:38 + created pulse
14:26:38 + created docker-desktop
14:26:38 + created bash
...
```

Нужно:

```text
14:26:38  ● PULSE observation started
           baseline: 241 entities
```

При Enter:

```text
BASELINE SNAPSHOT

processes    117
cgroups       82
units         31
containers     4
other          7
```

Одна baseline event вместо сотен искусственных `created`.

---

# 133. Event punctuation

Timeline grammar:

```text
● = ordinary meaningful lifecycle/change
! = discrete important event
◇ = degraded state transition
◆ = warning state transition
▲ = critical state transition
× = loss/failure
○ = recovery/nominal
```

Пример:

```text
08:30       08:40       08:50       09:00       NOW
─────●─────────!──────────◇──────────!────────────○────>
     start     deploy     PSI        OOM          recovered
```

`! OOM` означает событие.

Если OOM одновременно привёл system/entity state в critical:

```text
! OOM
  └─ state ◆ → ▲
```

То есть event punctuation и state severity остаются разными каналами.

---

# 134. Section separator rules

Чтобы экран не выглядел как набор случайных строк:

Section title:

```text
WHAT NEEDS ATTENTION ───────────────────────────────────────
```

или:

```text
WHAT NEEDS ATTENTION
────────────────────────────────────────────────────────────
```

Нельзя смешивать оба варианта бессистемно внутри одного screen style.

Для default PULSE фиксируется первый вариант:

```text
SECTION TITLE  ─────────────────────────────────────────────
```

если ширина позволяет.

При narrow:

```text
SECTION TITLE
```

без линии.

---

# 135. Vertical whitespace rules

Большая пустая область допустима только если она намеренна:

```text
Calm / focus mode
large graph viewport
timeline viewport
```

Не допускается:

```text
section with one line
+
20 пустых строк
+
next section
```

Если section content короткий:

```text
its height = content + minimal padding
```

а свободное пространство передаётся:

```text
entities
graph
timeline
inspect
```

---

# 136. Selectable content grammar

Любая selectable строка начинается:

```text
>
```

только у текущего selected item.

Пример:

```text
> pulse      process  .03  6.9 MiB  ○
  systemd    process  .01  4.1 MiB  ○
```

Не использовать дополнительные произвольные маркеры для selection.

Focused pane обозначается border/title accent, а не вторым `>`.

---

# 137. Implementation acceptance test: Overview

При fake healthy data реализация считается соответствующей, если:

```text
[ ] glyph имеет полный 9×7 field с `·`
[ ] STATE расположен до metrics strip
[ ] Problems занимает максимум несколько строк при empty state
[ ] Recent Changes не содержит baseline discovery
[ ] Entities заполняет остаток высоты
[ ] state symbols видны без цвета
[ ] footer не переносится на две строки при recommended 80×24
```

---

# 138. Implementation acceptance test: Timeline

```text
[ ] baseline inventory представлен одной snapshot event
[ ] основной экран содержит horizontal time rail
[ ] виден State River
[ ] значимые events имеют markers
[ ] raw events не занимают весь screen by default
[ ] `!` используется как event marker, не severity
[ ] cursor/scrub визуально очевиден
[ ] `F` возвращает в LIVE
```

---

# 139. Implementation acceptance test: Inspector

```text
[ ] relation list агрегируется, где это полезно
[ ] long values не обрезаются до бессмысленного текста
[ ] screen не остаётся на 80% пустым при наличии доступных subviews
[ ] related entities selectable
[ ] Enter следует relation
[ ] Esc возвращает navigation stack
[ ] narrow layout превращает inspector в отдельный pane/tab
```

---

# 140. Screen mockups are part of the spec

Начиная с этого раздела fenced TUI mockups считаются **частью дизайн-контракта**.

При расхождении реализации и prose:

1. semantic invariants имеют высший приоритет;
2. затем normative mockup;
3. затем explanatory prose.

Если mockup устарел после принятого решения, новый раздел должен явно написать:

```text
SUPERSEDES section N
```

Старые разделы не удаляются из append-only spec.

---

# 141. Immediate corrections suggested by current prototype

По текущим screenshot-прототипам необходимо исправить:

```text
1. Overview glyph:
   использовать fixed 9×7 matrix + `·` background.

2. Recent Changes:
   убрать initial discovery entity flood.

3. Problems:
   empty state не должен быть пустым full-screen.

4. Entities:
   inspector relations агрегировать;
   при нехватке ширины переключать в tab.

5. Inspector:
   заполнять свободное пространство полезными related blocks,
   а не оставлять пустой viewport.

6. Timeline:
   initial inventory → one Baseline Snapshot;
   main mode → Time Rail + State River + Story;
   raw events → secondary subview.

7. Section spacing:
   убрать большие случайные vertical gaps.

8. Default Overview sort:
   relevance/problem-first, CPU sort только по явному выбору.
```

---

# 142. Next design work

После приведения prototype к этим screen contracts продолжить:

```text
DB timeout Investigation Journey
+
Network/Socket screen contracts
+
remote endpoint identity
+
cross-host PULSE graph
```

Но сначала Overview / Problems / Entities / Inspector / Timeline должны соответствовать v0.7 mockups, иначе дальнейшие экраны будут наследовать неоднозначную layout grammar.


---

# 143. SUPERSEDES parts of 123, 124, 128: XL Overview correction

Текущий prototype показал, что правила v0.7 всё ещё допускают нежелательную интерпретацию:

```text
wide terminal
→ растянуть все секции на всю ширину
→ отдать почти весь экран flat entity table
```

Это НЕ является целевым поведением.

Главный экран PULSE должен использовать дополнительную ширину для:

```text
context
selection preview
relations
diagnostic information
```

а не для бесконечного расширения колонок таблицы.

---

# 144. Anti-stretch rule

Каждый блок имеет не только:

```text
min_width
preferred_width
```

но и:

```text
max_useful_width
```

После достижения `max_useful_width` дополнительное место:

```text
1. передаётся соседней полезной панели;
2. используется как deliberate whitespace;
3. либо включает additional contextual pane.
```

Нельзя:

```text
OWNER column = 60 chars simply because terminal is wide
```

или:

```text
separator line = 190 columns while block has one short sentence
```

---

# 145. Canonical XL Overview

Для терминала примерно:

```text
>= 150 columns
>= 30 rows
```

основной layout должен быть ближе к следующему:

```text
P U L S E   asuspc       LIVE ●       19:19:09       up 23h56m       OVERVIEW
────────────────────────────────────────────────────────────────────────────────────────────────────────────────

STATE                                      ATTENTION
────────────────────────────────────────   ───────────────────────────────────────────────────────────────────────

· · · ○ ○ ○ · · ·                        ✓ NO ACTIVE PROBLEMS
· · ○ ● ● ● ○ · ·
· ○ ● ● ● ● ● ○ ·                        observed nominal for 7s
○ ● ● ● ● ● ● ● ○                        observation started 19:19:02
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·

SYSTEM NOMINAL
no active problem state

SIGNALS
CPU 0%    MEM 10%    PSI MEM 0%    IO WAIT 0%    NET ↓0 B/s ↑0 B/s
────────────────────────────────────────────────────────────────────────────────────────────────────────────────
RECENT CHANGES

19:19:02  ● PULSE observation started      baseline 245 entities
           Enter → baseline snapshot

────────────────────────────────────────────────────────────────────────────────────────────────────────────────
RELEVANT ENTITIES                                        SELECTED
──────────────────────────────────────────────────────   ────────────────────────────────────────────────────────
NAME                    KIND        CPU    MEM      S     init.scope
> init.scope            unit        .03    1.8GiB   ○
  system.slice          cgroup      .00    845MiB   ○     state       ○
  angie.service         unit        .00    376MiB   ○     cpu         .03
  systemd-journald      unit        .00    222MiB   ○     mem         1.8GiB
  docker-desktop        process     .00    26MiB    ○
                                                         relations
                                                         children     3
                                                         cgroup       /init.scope
                                                         owner        host/asuspc

                                                         recent
                                                         no meaningful changes
────────────────────────────────────────────────────────────────────────────────────────────────────────────────
↑↓ select   Enter inspect   Tab pane   / search   : commands   P problems   E entities   T timeline   ? help
```

Главное отличие:

```text
нижняя область = relevant entities + contextual preview
```

а не:

```text
нижняя область = giant flat inventory table
```

---

# 146. XL Overview column proportions

Ориентировочно:

```text
top STATE       28–40 cols
top ATTENTION   remaining useful width, but bounded

bottom entities 55–65%
bottom selected 35–45%
```

Конкретные проценты могут адаптироваться.

Но таблица Overview не должна занимать:

```text
100% terminal width
```

на XL layout, если справа может быть полезный selected/context pane.

---

# 147. Overview is not the full inventory

На Overview показывается:

```text
RELEVANT ENTITIES
```

а не:

```text
ALL ENTITIES
```

Full inventory находится в:

```text
E → Entities
```

Default Overview relevance может учитывать:

```text
abnormal state
recent change
high stress
high resource use
relationship to active problem
user selection/history
important system role
```

В healthy состоянии список всё равно короткий и полезный.

Пример:

```text
RELEVANT ENTITIES

pulse
system.slice
docker-desktop
angie.service
systemd-journald
```

Не нужно выводить 40 одинаковых `angie` processes только потому, что места много.

---

# 148. Logical entity folding

По умолчанию связанные technical entities не должны дублироваться как независимые равноправные строки.

Типичный пример:

```text
init.scope unit
init.scope cgroup
```

или:

```text
angie.service unit
angie.service cgroup
angie process × 12
```

Для default logical view они группируются:

```text
angie.service          unit      ○
  processes            12
  cgroup               1
```

или compact:

```text
angie.service          unit+12p  ○
```

Enter раскрывает:

```text
unit
cgroup
processes
sockets
files
```

---

# 149. Entities screen gets two presentation modes

## Logical — default

```text
E / logical
```

Показывает primary operational entities:

```text
service
workload
container
pod
standalone process
filesystem
interface
etc.
```

с folded technical relations.

## Technical

```text
E / technical
```

Показывает полный graph inventory:

```text
unit
cgroup
each process
each socket
each mount
...
```

Этот режим полезен для глубокой диагностики, но не должен определять первое впечатление.

---

# 150. Repeated process folding

Пример prototype:

```text
angie
angie
angie
angie
angie
...
```

Default logical list должен агрегировать это как:

```text
angie.service          12 processes      376 MiB      ○
```

Если один worker abnormal:

```text
angie.service          12 processes      376 MiB      ◆
                       1 abnormal
```

Enter:

```text
PROCESSES / angie.service

> angie/1421    ○
  angie/1422    ○
  angie/1423    ◆
  ...
```

---

# 151. Observation duration wording

До появления достаточной historical truth нельзя писать:

```text
stable for 22h37m
```

если PULSE наблюдает машину всего 7 секунд.

И даже:

```text
stable for 7s
```

может звучать как утверждение о реальном прошлом системы.

Нормативная формулировка:

```text
observed nominal for 7s
```

или compact:

```text
nominal · observed 7s
```

Если backend/history действительно покрывает 22h:

```text
nominal for 22h37m
```

тогда допустимо.

---

# 152. Uptime and observation age are separate

Header:

```text
up 23h56m
```

означает host uptime.

State block:

```text
observed 7s
```

означает длину доступного PULSE observation window.

Они никогда не должны подменять друг друга.

---

# 153. Baseline event must stay compact

Нежелательно:

```text
19:19:02 ● PULSE observation started — baseline: 245 entities: processes 47,
cgroups 102, units 85, containers 1, pods 0, disks 8, netifs 1, other 1
```

На Overview:

```text
19:19:02  ● PULSE observation started      baseline 245 entities
```

Enter:

```text
BASELINE SNAPSHOT

processes       47
cgroups        102
units           85
containers       1
pods             0
disks            8
interfaces       1
other            1
```

---

# 154. Metric units contract

PULSE must distinguish:

```text
counter
rate
gauge
ratio
duration
```

and must never infer `/s` merely from a byte-valued field.

Examples:

## Counter

```text
read total       9.1 GiB
```

## Rate

```text
read rate        12.4 MiB/s
```

## Gauge

```text
memory           845 MiB
```

## Ratio

```text
memory used      10%
```

## Duration

```text
IO wait          18 ms
```

---

# 155. Overview IO / NET notation

Ambiguous:

```text
NET 207 MiB / 7.2 MiB
```

or:

```text
NET 0 B / 0 B
```

should not be default.

Preferred live-rate notation:

```text
NET ↓207 KiB/s ↑7.2 KiB/s
```

or ASCII fallback:

```text
NET RX 207 KiB/s TX 7.2 KiB/s
```

For IO:

```text
IO ↓12 MiB/s ↑4 MiB/s
```

if showing throughput.

If showing pressure:

```text
IO PSI 0%
```

If showing cumulative bytes, explicitly label:

```text
IO total R 9.1 GiB W 2.4 GiB
```

---

# 156. Top signal strip is current-state oriented

Overview top strip should prefer:

```text
current pressure
current utilization
current rates
```

rather than lifetime cumulative counters.

Recommended:

```text
CPU 0%
MEM 10%
PSI MEM 0%
IO WAIT 0%
NET ↓0 B/s ↑0 B/s
```

Cumulative counters belong in Inspect/History unless specifically useful.

---

# 157. LIVE marker

`LIVE` must be visually distinct from health-state vocabulary.

Preferred:

```text
LIVE ●
```

where the dot is treated as a mode/activity indicator, not a State Glyph cell.

If the terminal/theme makes this ambiguous, alternative:

```text
● LIVE
```

or reverse-video `LIVE`.

Avoid:

```text
LIVE ○
```

because `○` is already semantically `normal` in the state alphabet.

---

# 158. Problem count in header

Healthy:

```text
PULSE  asuspc  LIVE ●  19:19:09  up 23h56m  OVERVIEW
```

Do not redundantly show:

```text
0 problems
```

in both header and State summary.

When non-zero:

```text
PULSE  prod-api-07  LIVE ●  ▲2  19:19:09  OVERVIEW
```

The problem count becomes useful because it is exceptional.

---

# 159. Empty Attention on healthy XL screen

Healthy Attention should be compact:

```text
ATTENTION
✓ no active problems
```

It must not reserve a tall fixed box.

Remaining height belongs to:

```text
State
Recent Changes
Relevant Entities
Selected preview
```

---

# 160. Large terminal is not "more rows of htop"

When terminal grows:

Wrong:

```text
more width  → more table columns
more height → more process rows
```

Correct:

```text
more width
→ more context visible simultaneously

more height
→ more Story / Relations / Relevant Entities / History
```

Raw inventory density is available in dedicated Entities/Technical view.

---

# 161. XL resize progression

Example:

## 100 columns

```text
STATE
ATTENTION
RECENT
ENTITIES
```

stacked.

## 130 columns

```text
STATE | ATTENTION
RECENT
ENTITIES
```

## 160 columns

```text
STATE | ATTENTION
RECENT
ENTITIES | SELECTED
```

## 200 columns

Do NOT endlessly stretch.

Instead optionally:

```text
STATE | ATTENTION | RECENT SUMMARY
ENTITIES | SELECTED / RELATIONS
```

with bounded useful widths.

---

# 162. Screenshot acceptance criteria for current prototype

Against the prototype that motivated this section:

```text
[ ] `LIVE ○` becomes unambiguous LIVE indicator.
[ ] `stable for 7s` becomes `observed nominal for 7s`.
[ ] baseline details collapse behind Enter.
[ ] Overview table becomes Relevant Entities, not full inventory.
[ ] repeated `angie` processes fold by default.
[ ] unit/cgroup twins fold in logical mode.
[ ] XL lower area includes selected/context pane.
[ ] table columns stop growing after max useful width.
[ ] IO/NET values explicitly distinguish rate vs counter.
[ ] healthy Attention block does not consume unused vertical area.
[ ] header does not redundantly repeat `0 problems`.
```

---

# 163. Core Overview identity

The intended first-screen composition is now:

```text
STATE / GLYPH
      +
ATTENTION
      +
CURRENT SIGNALS
      +
RECENT MEANINGFUL CHANGES
      +
RELEVANT LOGICAL ENTITIES
      +
SELECTED CONTEXT when space permits
```

Not:

```text
small status header
      +
full process/entity inventory
```

This distinction is part of the product identity, not merely a layout preference.


---

# 164. SUPERSEDES navigation/focus parts of earlier sections

Current prototype exposed interaction problems not fully constrained by v0.8.

This section supersedes earlier ambiguous rules for:

```text
Inspector as a top-level screen
Tab behavior
hidden pane focus
search input
global screen hotkeys
entity relevance explanation
Inspector navigation stack
Timeline startup layout
Overview pane geometry
```

The target interaction model is:

```text
visible UI state
        =
focus state
        =
keyboard target
```

There must be no hidden selection or background navigation state that can react to `Enter`.

---

# 165. Overview relevance must be explainable

`RELEVANT ENTITIES` is not just a renamed Top-N resource table.

Every entity included in this list must have at least one explicit reason.

Allowed relevance reasons:

```text
problem      related to an active problem
critical     entity itself critical
warning      entity itself warning
changed      meaningful recent change
cpu          unusually relevant CPU pressure/use
memory       unusually relevant memory pressure/use
io           relevant IO pressure/activity
network      relevant network issue/activity
system       important system role
pinned       explicitly pinned by user
selected     current investigation context
```

Normative Wide table:

```text
RELEVANT ENTITIES                           sort: relevance   view: logical

NAME                  CPU    MEM      S   WHY
> system.slice        .00    880MiB   ○   system
  init.scope          .28    1.7GiB   ○   cpu
  angie.service       .00    687MiB   ○   memory
  docker-desktop      .00     26MiB   ○   changed
```

On narrower layouts the `WHY` column may collapse:

```text
NAME                  CPU    MEM      S
> system.slice        .00    880MiB   ○
```

but the selected preview must still expose:

```text
RELEVANCE
system role
```

or the corresponding reason.

If no entity has meaningful relevance beyond inventory existence, the block title may become:

```text
KEY ENTITIES
```

rather than claiming unsupported relevance.

---

# 166. Selected preview is mandatory when layout permits

For Overview:

```text
terminal width >= 140 columns
body height >= 18 rows
```

and at least one entity exists:

```text
SELECTED / CONTEXT pane MUST be visible.
```

The first row is selected automatically when entering Overview if no prior selection exists.

Example:

```text
RELEVANT ENTITIES                         SELECTED / system.slice
───────────────────────────────────────   ──────────────────────────────────
NAME               CPU   MEM      S WHY   STATE        ○
> system.slice     .00   880MiB   ○ sys   RELEVANCE    system role
  init.scope       .28   1.7GiB   ○ cpu
  angie.service    .00   687MiB   ○ mem   CPU          .00
                                           MEM          880 MiB

                                           RELATIONS
                                           parent       host/asuspc
                                           children     18
```

The selected preview is a preview, not a full Inspector.

`Enter` opens contextual Inspector.

---

# 167. Remove inline "Enter → ..." instructions from content

Text such as:

```text
Enter → baseline snapshot
```

inside content is removed.

Why:

- it looks like data;
- it duplicates footer/navigation hints;
- it visually competes with event text;
- selection itself already implies `Enter`.

Correct:

```text
20:54:36  ● PULSE observation started     baseline 242 entities
```

When selected:

```text
>20:54:36 ● PULSE observation started     baseline 242 entities
```

Footer:

```text
Enter open
```

If needed, the selected preview can show:

```text
BASELINE SNAPSHOT
processes 46
cgroups   101
...
```

---

# 168. Inspector is contextual, not a top-level page

This supersedes the earlier model that exposed an empty global `INSPECTOR` screen.

Remove Inspector from the primary top-level screen cycle.

Top-level primary screens:

```text
Overview
Problems
Entities
Timeline
```

Help is an overlay.

Inspector is opened only when there is a concrete selected entity/relation/event:

```text
selected actionable object
        ↓ Enter
Inspector(object)
```

There must be no state:

```text
INSPECTOR
entity not selected
```

as a normal navigable screen.

If no entity is selected, an Inspector-opening action is disabled/no-op and a short status hint may be shown.

---

# 169. Top-level navigation keys

Canonical numeric navigation:

```text
1    Overview
2    Problems
3    Entities
4    Timeline
```

Optional aliases may exist:

```text
P/p  Problems
E/e  Entities
T     Timeline
```

but must be deterministic.

Important:

```text
pressing the key for the screen already open = idempotent no-op
```

It must NEVER toggle/hide the screen.

For navigation aliases, input handling must normalize shift consistently:

```text
E and e
P and p
```

where there is no contextual conflict.

If a lower-case key has a contextual action conflict, prefer the numeric global navigation key and do not silently overload the same key.

---

# 170. Tab only navigates visible panes

`Tab` / `Shift+Tab` do NOT switch top-level screens.

They only cycle through currently visible and focusable panes of the current screen.

Example Overview XL:

```text
RELEVANT ENTITIES  <Tab>  SELECTED
```

Example Entities Wide:

```text
ENTITY LIST  <Tab>  PREVIEW
```

Example Timeline Wide:

```text
STORY  <Tab>  DETAILS/SNAPSHOT
```

Rules:

```text
hidden pane      = never focusable
empty pane       = never focusable unless it contains an explicit empty-state action
collapsed pane   = never receives keyboard events
overlay open     = underlying panes do not receive keyboard events
```

---

# 171. Focus must always be visible

A pane that owns keyboard focus must have a visible focus treatment.

Allowed treatments:

```text
accented title
accented border
clear active-tab marker
```

Not allowed:

```text
keyboard focus exists only in internal state
```

The user must always be able to answer:

> Where will Enter act right now?

without guessing.

---

# 172. No background Enter

`Enter` executes only against:

```text
visible
+
focused
+
selected
+
actionable
```

content.

If any of these are false:

```text
Enter = no-op
```

Optionally show:

```text
nothing to open
```

for ~1–2 seconds in status/footer.

This explicitly prohibits the current behavior where an empty Inspector is visible but `Enter` opens a process selected in a hidden/background entity list.

---

# 173. Focus reset on screen changes

When changing top-level screen:

```text
Overview → Entities
Entities → Timeline
...
```

focus is reassigned to the default visible pane of the destination screen.

Examples:

```text
Overview  → Relevant Entities
Problems  → Problem list
Entities  → Entity list
Timeline  → Story if it has events, otherwise Time Rail
```

Previous hidden pane focus must not survive into another screen.

---

# 174. Inspector navigation stack must be cycle-safe

The Investigation Graph can contain cycles:

```text
process → cgroup → unit → process
```

A naïve browser-like push stack causes:

```text
Enter
Enter
Enter
Enter
...
```

to create duplicate history entries and require the same number of `Esc` presses.

This is prohibited.

Use canonical entity identity:

```text
EntityKey
```

for Inspector navigation.

When following a relation:

## New entity not in current path

```text
A → B → C
```

push normally.

## Target already exists earlier in current path

Example:

```text
A → B → C
        ↓
        A
```

Do NOT push another `A`.

Instead truncate/jump the path to the existing node:

```text
A
```

or to the appropriate existing path position.

No duplicate cycle growth.

---

# 175. Inspector origin is separate from relation history

When Inspector opens from Overview:

```text
origin = Overview
```

Relation traversal:

```text
process → fd → socket → endpoint
```

is internal Inspector history.

`Esc` behavior:

```text
if relation history has previous unique node:
    go to previous unique node
else:
    exit Inspector to origin screen
```

Thus an accidental graph cycle cannot create dozens of Esc steps.

The origin screen restores:

```text
selection
scroll
focus pane
filter/sort
```

---

# 176. Sorting/filter status is always visible on list screens

Restore explicit state.

Examples:

```text
RELEVANT ENTITIES  12   sort:relevance   view:logical
```

```text
ENTITIES 242   sort:MEM↓   filter:all   view:logical
```

```text
PROCESSES 18   sort:CPU↓   filter:angie
```

Rules:

- `↓` descending;
- `↑` ascending;
- current sort always shown;
- active filter always shown;
- if filter is default, show `filter:all` while space permits.

Compact:

```text
ENTITIES 242  MEM↓  all
```

---

# 177. Search is an explicit modal input

Pressing `/` must immediately produce visible input UI.

Example overlay/footer input:

```text
SEARCH  /angi█
────────────────────────────────────────────────────────────────
18 matches   Enter apply/open   Esc cancel
```

or narrow:

```text
/ angi█   18 matches
```

While Search is active:

```text
typed characters → search query only
Tab              → query completion/result focus if implemented
Enter            → apply/open selected result
Esc              → cancel search and restore previous filter/result set
Backspace        → edit query
```

Underlying screen shortcuts do not fire.

A hidden search mode is prohibited.

---

# 178. Search result semantics

For Entities screen, incremental filtering may happen while typing, but the visible search field remains on screen.

Example:

```text
SEARCH / angie

ENTITIES 14/242   sort:MEM↓   filter:search("angie")
```

If search is cancelled:

```text
Esc
```

restore the previous filter and selection.

If applied:

```text
Enter
```

search becomes an explicit filter until cleared.

---

# 179. Global key dispatch is single-shot and deterministic

The current symptom:

```text
first Shift+E appears to do nothing
second Shift+E changes/hides content
```

indicates ambiguous routing/state.

Required dispatch order:

```text
1. active modal/overlay input
2. active visible pane
3. global navigation/action
4. otherwise no-op
```

One physical key event must result in at most one semantic action.

No action may depend on a previous invisible partial key state unless an explicit chord/prefix UI is visible.

---

# 180. Help is an overlay, not a Tab target

The footer currently visually resembles a global tab bar, creating the expectation that repeated `Tab` should eventually reach Help.

That is not the intended model.

Help is opened globally with:

```text
?
```

and optionally:

```text
F1
```

`Tab` stays pane-local.

The footer must therefore look like **key hints**, not tabs.

Preferred:

```text
[1]Overview  [2]Problems  [3]Entities  [4]Timeline  [:]Commands  [?]Help
```

rather than:

```text
P problems   E entities   T timeline   ? help
```

with no visual grouping.

---

# 181. Help screen fixes

Help should reflect the actual current keymap.

Remove obsolete entries such as:

```text
1 … 5 Overview / Problems / Entities / Inspect / Timeline
```

if Inspector is no longer a top-level page.

Canonical section:

```text
NAVIGATION

1              Overview
2              Problems
3              Entities
4              Timeline
Enter          Open selected object
Esc            Back / close Inspector or overlay
Tab            Next visible pane
Shift+Tab      Previous visible pane
/              Search
:              Command palette
? / F1         Help
q / Ctrl+C     Quit
```

Context-specific keys are shown in a separate section.

---

# 182. Timeline current prototype does not match target

The prototype currently resembles:

```text
time line
STATE ○○○○○○○○○○○○○○○○...
CPU  ───────────────────
MEM  ───────────────────
IO   ───────────────────
STORY
baseline row
```

Problems:

- State cells are visually crushed together;
- metric lanes are empty separator-like lines;
- baseline details compete with Story;
- current cursor is not visually strong enough;
- default view references `raw events`;
- it does not read as State River + Incident Story + Historical Snapshot.

This implementation is not accepted as the intended Timeline.

---

# 183. Timeline healthy-start normative XL screen

For a newly started healthy observer with only baseline data:

```text
P U L S E / TIME MACHINE                  LIVE ●              window 5m
────────────────────────────────────────────────────────────────────────────────────────────

20:54:36                     20:57:00                     20:59:00                   NOW
●────────────────────────────────────────────────────────────────────────────────────────●
observation started                                                                     live

STATE
○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○ ○

CPU    ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁ ▁
MEM    ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂ ▂
IO     · collecting history
────────────────────────────────────────────────────────────────────────────────────────────

STORY                                                       SNAPSHOT / SELECTED
────────────────────────────────────────────────────────   ─────────────────────────────────
>20:54:36 ● PULSE observation started                      BASELINE
                                                            entities       242
                                                            processes       46
                                                            cgroups        101
                                                            units           84
                                                            containers       1
                                                            disks            8

────────────────────────────────────────────────────────────────────────────────────────────
←→ scrub   +/- zoom   Tab pane   Enter open   A/B mark   D diff   F live   Esc back
```

Notes:

- baseline detail is in the right details pane, not embedded as one long Story line;
- metrics with insufficient history show `collecting history`, not fake flat graphs;
- state cells have spacing when width permits;
- `raw events` is not a default-mode label.

---

# 184. Timeline when an incident exists

```text
P U L S E / TIME MACHINE               HISTORY ◀ 21:07:18          window 30m
────────────────────────────────────────────────────────────────────────────────────────────

20:45          20:50          20:55          21:00          21:05          NOW
──○─────────────○──────────────◇─────────◆────!──────────────◇──────────────○──>
                                  PSI       timeout          OOM

STATE
○ ○ ○ ○ ○ ○ ○ ○ ◇ ◇ ◇ ◆ ◆ ◆ ▲ ◆ ◆ ◇ ◇ ○ ○ ○ ○ ○ ○

CPU    ▁ ▁ ▁ ▂ ▂ ▃ ▄ ▅ ▆ ▇ █ ▆ ▄ ▃ ▂
MEM    ▂ ▂ ▃ ▄ ▅ ▆ ▇ █ █ █ █ ▇ ▆ ▅ ▃
IO     ▁ ▁ ▁ ▁ ▂ ▃ ▅ ▆ █ ▇ ▅ ▃ ▁ ▁ ▁
────────────────────────────────────────────────────────────────────────────────────────────

STORY                                                       SNAPSHOT @ 21:07:18
────────────────────────────────────────────────────────   ─────────────────────────────────
 21:02 ◇ memory pressure                                   STATE        ▲
 21:04 ◆ reclaim increased                                 MEM PSI      38%
 21:06 ! DB timeout burst ×184                             reclaim      2.1GiB/s
>21:07 ! OOM                                               affected     23
 21:07 ● postgres restarted
 21:11 ◇ recovering

────────────────────────────────────────────────────────────────────────────────────────────
←→ scrub   +/- zoom   Tab pane   Enter inspect   A/B mark   D diff   F live
```

This is the canonical visual direction.

---

# 185. Timeline focus

Visible focusable panes:

```text
TIME RAIL
STORY
SNAPSHOT/DETAILS
```

Metric lanes are not focusable by default.

At startup with Story content:

```text
focus = STORY
```

If Story is empty:

```text
focus = TIME RAIL
```

Hidden snapshot pane on narrow layouts cannot receive focus.

---

# 186. Timeline data insufficiency

Do not fabricate a visual history before enough samples exist.

Examples:

```text
CPU   · collecting
MEM   · collecting
IO    · collecting
```

After samples exist:

```text
CPU   ▁ ▁ ▂ ▁ ▁
```

Similarly, derived State River begins at the observation boundary.

Do not imply system state before available history.

---

# 187. Strict Overview XL geometry

For:

```text
width >= 150 columns
height >= 30 rows
```

use the following vertical allocation order:

```text
header                 1 row
top state/attention    9 rows preferred, 7 min, 11 max
signals                2 rows
recent changes         content-sized, 2 min, 5 max
lower area             flex, minimum 8 rows
footer                 1 row
```

Lower area:

```text
Relevant/Key Entities  58–64%
Selected Context       36–42%
```

Suggested width limits:

```text
entity pane:
  min 58
  preferred 72
  max useful 88

selected pane:
  min 38
  preferred 52
  max useful 68
```

Extra width beyond useful maxima is deliberate whitespace or used by richer context, not by stretching columns.

---

# 188. Strict Overview top-row geometry

The STATE glyph/status side should not consume half the screen.

Suggested:

```text
STATE pane
min            30 cols
preferred      36 cols
max useful     42 cols
```

ATTENTION:

```text
min            42 cols
preferred      58 cols
max useful     76 cols
```

If width remains beyond both maxima, do not extend separator lines just to fill the entire terminal.

---

# 189. Section line width follows pane width

A section separator belongs to its pane.

Wrong:

```text
ATTENTION ───────────────────────────────────────────────────────────────────────────────────────────
```

when content is only 50 columns wide.

Correct:

```text
ATTENTION ─────────────────────────────────────
```

within the pane's bounded rectangle.

This is required to preserve intentional composition on XL terminals.

---

# 190. Overview canonical 180×40 sketch

```text
P U L S E   asuspc        LIVE ●        21:08:22        up 1d1h        OVERVIEW
────────────────────────────────────────────────────────────────────────────────────────────────────────────

STATE                                   ATTENTION
────────────────────────────────────    ───────────────────────────────────────────────────────────

· · · ○ ○ ○ · · ·                     ✓ NO ACTIVE PROBLEMS
· · ○ ● ● ● ○ · ·
· ○ ● ● ● ● ● ○ ·                     observed nominal for 13m46s
○ ● ● ● ● ● ● ● ○                     observation started 20:54:36
· ○ ● ● ● ● ● ○ ·
· · ○ ● ● ● ○ · ·
· · · ○ ○ ○ · · ·

SYSTEM NOMINAL

SIGNALS
CPU 1%   MEM 11%   PSI MEM 0%   IO WAIT 0%   NET ↓0 B/s ↑0 B/s
────────────────────────────────────────────────────────────────────────────────────────────────────────────
RECENT CHANGES

20:54:36  ● PULSE observation started                 baseline 242 entities
────────────────────────────────────────────────────────────────────────────────────────────────────────────

RELEVANT ENTITIES                                      SELECTED / init.scope
sort:relevance  filter:all  view:logical               ─────────────────────────────────────────────────────
────────────────────────────────────────────────────
NAME                 CPU    MEM      S   WHY            STATE       ○
> init.scope         .28    1.7GiB   ○   cpu            RELEVANCE   cpu
  system.slice       .00    904MiB   ○   system
  angie.service      .00    687MiB   ○   memory         CPU         .28
  docker-desktop     .00     26MiB   ○   changed        MEM         1.7GiB
  journald.service   .00     15MiB   ○   system
                                                         RELATIONS
                                                         owner       host/asuspc
                                                         cgroup      /init.scope
                                                         processes   12

                                                         RECENT
                                                         no meaningful changes

────────────────────────────────────────────────────────────────────────────────────────────────────────────
[1]Overview  [2]Problems  [3]Entities  [4]Timeline  [/]Search  [:]Commands  [?]Help
```

---

# 191. Entities screen canonical behavior

Entities is the full inventory-oriented screen.

Header:

```text
ENTITIES 242   sort:MEM↓   filter:all   view:logical
```

Wide:

```text
ENTITY LIST                                             PREVIEW
────────────────────────────────────────────────────   ───────────────────────────────
...
```

`Tab` only switches:

```text
ENTITY LIST ↔ PREVIEW
```

It does not open a new top-level page.

`Enter` in ENTITY LIST opens Inspector for the selected row.

`Enter` in PREVIEW follows the selected visible relation if one is selected.

---

# 192. Prevent invisible preview selection

Preview relation selection exists only while PREVIEW has focus.

When focus leaves PREVIEW:

```text
preview_relation_selection may be retained visually
```

but cannot react to Enter until PREVIEW regains focus.

If PREVIEW is hidden due resize:

```text
focus automatically moves to ENTITY LIST
```

and preview key handlers are disabled.

---

# 193. Footer is not focus navigation

The bottom line is a hotkey legend.

It does not participate in Tab order.

Canonical appearance:

```text
[1]Overview  [2]Problems  [3]Entities  [4]Timeline  [/]Search  [:]Commands  [?]Help
```

This directly resolves the expectation that Tab should eventually reach Help.

---

# 194. Acceptance criteria for the reported prototype issues

## Relevance / selected

```text
[ ] Every relevant entity has a reason.
[ ] WHY visible on Wide/XL or reason visible in Selected preview.
[ ] Selected pane appears at >=140 cols if entities exist.
```

## Baseline row

```text
[ ] Remove inline `Enter → baseline snapshot`.
[ ] Baseline row itself is selectable.
[ ] Details appear in preview/details pane.
```

## Inspector cycles

```text
[ ] Canonical EntityKey used.
[ ] Following relation to entity already in current path does not push duplicate.
[ ] Esc count is based on unique navigation path only.
[ ] Inspector exits to its origin screen when relation history is exhausted.
```

## Sort status

```text
[ ] Sort direction visible.
[ ] Filter state visible.
[ ] Logical/technical view visible.
```

## Search

```text
[ ] `/` renders input immediately.
[ ] Query text always visible.
[ ] Esc cancels and restores previous state.
[ ] No hidden text-capture mode.
```

## E key / navigation

```text
[ ] One keypress = one action.
[ ] Pressing current screen shortcut is idempotent.
[ ] Repeated E cannot hide Entities.
[ ] Shift normalization deterministic.
```

## Timeline

```text
[ ] Matches sections 183/184.
[ ] Default is not raw event list.
[ ] Metrics do not render as empty fake lines.
[ ] Snapshot/details separated from Story.
```

## Tab/focus

```text
[ ] Tab cycles visible panes only.
[ ] Empty/hidden Inspector cannot receive focus.
[ ] Enter cannot act on hidden list selection.
[ ] Focus owner visibly indicated.
```

## Help

```text
[ ] Help opens with ? or F1.
[ ] Help is not expected in pane Tab order.
[ ] Footer visually communicates hotkeys, not tabs.
```

## Layout

```text
[ ] Pane max useful widths enforced.
[ ] Overview 180×40 resembles section 190.
[ ] XL extra space becomes context/whitespace, not stretched table.
```

---

# 195. Design principle exposed by these bugs

The application must have exactly one authoritative interaction state:

```text
ScreenState
  ├─ visible panes
  ├─ focused pane
  ├─ selected item per visible pane
  ├─ active modal
  └─ navigation origin/history
```

Rendering and event dispatch both derive from that same state.

Do not maintain one hidden navigation model and a separate visual model.

If an item is not visible/focused/actionable on screen, keyboard dispatch must not pretend that it is.

---

# 196. Recommended screen-state model

Conceptually:

```text
AppUiState {
    screen: TopLevelScreen,

    overlay: Option<Overlay>,

    panes: VisiblePaneSet,
    focus: PaneId,

    overview: OverviewState,
    problems: ProblemsState,
    entities: EntitiesState,
    timeline: TimelineState,

    inspector: Option<InspectorSession>,
}
```

Inspector:

```text
InspectorSession {
    origin: ScreenOrigin,
    path: Vec<EntityKey>,
    current: EntityKey,
    focused_subview: InspectorPane,
}
```

The renderer calculates visible panes.

The input router accepts events only for:

```text
overlay
or focused visible pane
or global key
```

in that order.

---

# 197. Next implementation gate

Do not continue adding new diagnostic journeys until the following five flows are stable:

```text
Overview
→ search
→ Entities
→ Inspector
→ back to origin

Overview
→ selected preview
→ Inspector
→ relation traversal
→ Esc

Overview
→ Timeline
→ scrub
→ Story selection
→ Snapshot
→ F live

resize XL → Medium → Narrow → XL

? Help → Esc
```

These flows should be covered by deterministic UI/state tests, not only screenshot review.
