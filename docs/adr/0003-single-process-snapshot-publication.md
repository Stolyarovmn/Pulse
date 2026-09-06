# 0003 — Single‑Process Snapshot Publication

**Контекст**
Требуется отдавать последнее состояние системы как для интерактивного
TUI, так и для HTTP‑экспортера OpenMetrics, не блокируя поток сбора.
Снимок должен быть целостным и неизменяемым.

**Решение**
1. В `pulse-core` реализован тип `Snapshot`, который
   копирует все данные из `EntityGraph`, `LatestValues`, `Problem`,
   `Event`, `GraphStats`, `AgentStats` и метаданные хоста.
2. В цикле сбора (`pulse-cli::AgentRuntime`) после `History::ingest`
   и `Analyzer::evaluate` вызывается:
   ```rust
   let snapshot = Snapshot::build(&graph, latest, problems, events, agent, &hostname, &boot_id);
   ```
3. Снимок публикуется в `ArcSwap<Snapshot>` (`published.store(Arc::new(snapshot))`).
4. TUI (`pulse-tui`) и Exporter (`pulse-export`) читают его через
   `ArcSwap::load()`; чтение не блокирует запись, а сама
   структура `Snapshot` неизменна.

**Последствия**
- **Lock‑free чтение** – UI и Exporter работают параллельно,
  не задерживая сбор.  
- **Целостность** – snapshot содержит полную, согласованную
  информацию о графе и событиях.  
- **Идеальная совместимость** – `ArcSwap` обеспечивает
  атомарный замену, а `Arc` гарантирует безопасный доступ
  из разных потоков.  
- **Небольшая задержка** – создание snapshot
  занимает O(n) по количеству сущностей и событий, но это
  минимальный, по сравнению с чтением `/proc`.

**Статус**
Полностью реализовано в `pulse-core::Snapshot`, `pulse-cli::AgentRuntime`
и `pulse-tui`. Все ключевые типы (Snapshot, AgentStats, GraphStats)
сохраняют типы и структуры, как описано в `docs/CONTRACTS.md`.
