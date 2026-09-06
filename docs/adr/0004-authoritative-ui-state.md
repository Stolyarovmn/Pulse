# 0004 — Authoritative UI State и однократная маршрутизация ввода

**Контекст**
Прототипы TUI до v0.9 хранили состояние интерфейса разрозненно: `screen`,
`focus`, `trail`, `input`, `selected` жили рядом, а обработка клавиш была
размазана между циклом приложения (`pulse-tui/src/lib.rs`) и отдельными
рендерерами. Это давало устойчивый класс дефектов:

- `Enter` действовал на строку, которую текущая раскладка не показывает
  (невидимый keyboard target);
- `Tab` переключал top-level экраны и одновременно панели;
- повторное нажатие `E` при уже открытом экране Entities меняло состояние;
- `Esc` из Inspector возвращал «примерно туда же», теряя sort/filter/selection;
- переход по связи к уже посещённой сущности бесконечно наращивал стек пути.

Спецификация `pulse_tui_visual_navigation_spec_v0.9.md` (§164–§197) фиксирует
инвариант:

```text
visible UI state == focus state == keyboard target
```

**Решение**

1. `pulse-tui::app::App` — единственный authoritative state:
   `screen` (ровно четыре top-level: Overview, Problems, Entities, Timeline),
   `overlay` (`Search` | `Palette` | `Help`), `visible_panes`, `pane`,
   per-screen состояния и `inspector: Option<InspectorSession>`.
2. `App::dispatch` — единственный вход событий. Порядок разрешения:
   **overlay → Inspector → focused visible pane → global**. Один физический
   key event производит максимум одно семантическое действие.
3. Renderer каждый кадр вызывает `App::set_visible_panes` со списком панелей,
   которые он фактически нарисовал. Если resize скрыл focused pane, фокус
   детерминированно переходит на первую видимую панель. Скрытая selection
   не может получить `Enter`.
4. `InspectorSession { origin, path: Vec<EntityKey>, current }`: переход по
   связи к сущности, уже присутствующей в `path`, **усекает** путь до неё
   вместо `push`. `Esc` идёт по уникальному пути, затем восстанавливает
   точный origin (screen, pane, selection, sort, filter, view).
5. Inspector не является top-level экраном, Help и Search — overlay'и с полным
   владением вводом; footer — hotkey legend, а не tab bar.

**Последствия**

- **Тестируемость ввода**: interaction-тесты в
  `pulse-tui/src/screens/acceptance.rs` прогоняют `App::dispatch` без
  терминала и проверяют инварианты (idempotent hotkeys, modal ownership,
  cycle-safe path, focus repair при resize).
- **Render-путь стал частью контракта ввода**: renderer обязан публиковать
  видимые панели. Это цена инварианта; альтернатива — вычислять раскладку
  дважды (в state и в renderer) — расходилась бы при первом же изменении
  геометрии.
- **Нет глобального перехвата `Enter`**: прежний перехват в `lib.rs` удалён,
  иначе он обходил бы порядок разрешения.
- **Ограничение**: `visible_panes` отражает предыдущий кадр. Для клавиатуры
  это корректно (пользователь реагирует на то, что видит), но означает, что
  первый кадр после запуска не имеет published-раскладки; `App::default`
  задаёт безопасный минимум.

**Альтернативы**

- *Состояние в рендерерах (прежняя схема)*: отброшено — невозможно проверить
  ввод без терминала и невозможно доказать отсутствие невидимого target.
- *Раскладка как чистая функция размера, вычисляемая и в state, и в renderer*:
  отброшено — дублирование правил ширины/высоты расходится, а именно оно
  определяет видимость панелей.
- *Inspector как пятый top-level экран*: отброшено — допускает пустой экран
  без origin, что нарушает §165 и делает `Esc` неопределённым.

**Статус**
Реализовано: `pulse-tui/src/app.rs`, `screens/mod.rs`, `screens/inspector.rs`,
`layout.rs`. Контракт зафиксирован в `docs/CONTRACTS.md` (раздел
«pulse-tui: единое UI state и dispatch»). Гейты: `cargo fmt --check`,
`cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`.
