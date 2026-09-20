# RED-репродьюсеры аудита

Здесь учтены тесты, которые описывают **желаемый** контракт и потому падают
на текущей реализации. Их назначение — превратить статическую находку аудита
в воспроизводимое доказательство до того, как кто-то начнёт править код.

Правила:

- тест помечен `#[ignore]`, поэтому обязательный `cargo test --workspace --locked`
  остаётся зелёным, а красный обязательный CI не блокирует работу;
- `bash scripts/audit-red.sh` доказывает, что каждый такой тест падает
  **по ожидаемой причине**; неожиданный успех — отказ harness с требованием
  перевести тест в обычный регресс;
- фиксировать текущее сломанное поведение как контракт запрещено: утверждение
  всегда описывает то, как должно быть;
- после исправления дефекта `#[ignore]` снимается, и тест навсегда остаётся
  в обязательном наборе.

Базовая линия аудита: `3b79b43db0f73e9bce819cbe6be90915f26bb165`.
Прогон ниже выполнен на `3b79b43` + PR-01 (инфраструктура проверок), Rust
1.85.1, `cargo test -p <crate> --test audit_red <test> --locked -- --ignored --exact`.

**Состояние на PR-06: список репродьюсеров пуст.** Все 13 воспроизведённых
дефектов исправлены, их тесты продвинуты в обязательный набор и защищают
исправления от регрессии. Поэтому `scripts/audit-red.sh` на пустом списке
завершается успешно: harness не имеет права блокировать CI за отсутствие
дефектов. Непустой список с нулём воспроизведённых - по-прежнему отказ:
это значит, что harness перестал доказывать то, ради чего существует.

## Статусы

| Finding | Тест | Крейт | Наблюдаемое падение | Статус |
|---|---|---|---|---|
| PULSE-007 / PULSE-043 | `audit_red_labels_truncate_on_char_boundary` | pulse-core | `panicked at crates/pulse-core/src/entity.rs:242: assertion failed: self.is_char_boundary(new_len)` | PROMOTED в PR-08 |
| PULSE-087 | `audit_red_labels_are_terminal_safe` | pulse-core | `метка не имеет права содержать ESC: "safe\u{1b}]8;;https://evil…"` | PROMOTED в PR-08 |
| PULSE-023 | `audit_red_reject_crit_below_warn` | pulse-core | `порог crit ниже warn обязан быть отклонён, получено: Ok(())` | PROMOTED в PR-08 |
| PULSE-081 | `audit_red_reject_zero_operational_limits` | pulse-core | `нулевые лимиты обязаны отклоняться, приняты: ["export.rate_limit_per_minute", "export.max_series", "store.max_series"]` | PROMOTED в PR-08 |
| PULSE-002 / PULSE-048 | `audit_red_stale_value_is_not_current` | pulse-store | `значение без свежего наблюдения не имеет права возвращаться как текущее: Some(0.9)` | PROMOTED в PR-02 |
| PULSE-010 / PULSE-035 | `audit_red_window_crossing_warm_and_hot_keeps_prefix` | pulse-store | `начало запрошенного окна обязано остаться в ответе, получено 32000 мс` | PROMOTED в PR-04 |
| PULSE-038 | `audit_red_oldest_reflects_warm_retention` | pulse-store | `oldest обязан учитывать warm-слой, получено 32000 мс` | PROMOTED в PR-04 |
| PULSE-004 / PULSE-044 | `audit_red_entities_at_returns_historical_name` | pulse-store | `left: "new-name", right: "old-name"` | PROMOTED в PR-03 |
| PULSE-004 / PULSE-044 | `audit_red_entities_at_returns_historical_parent` + `diff_detects_reparent_between_moments` | pulse-store, pulse-engine | `left: EntityId(3g0), right: EntityId(1g0)` | PROMOTED в PR-03 |
| PULSE-003 / PULSE-045 | `audit_red_open_problem_survives_hysteresis_dead_zone` | pulse-engine | `в нейтральной зоне проблема обязана оставаться открытой` | PROMOTED в PR-05 |
| PULSE-059 | `audit_red_problem_events_keep_entity_identity` | pulse-engine | `событие обязано указывать сущность графа: left: None, right: Some(EntityId(0g0))` | PROMOTED в PR-05 |
| PULSE-084 | `audit_red_host_collector_reports_missing_critical_source` | pulse-collect | `отсутствие /proc/stat обязано быть заявлено как отказ сбора, получено Ok и 0 ошибок` | PROMOTED в PR-02 |
| PULSE-070 | `audit_red_process_export_keeps_incarnation_identity` | pulse-export | `идентичность ряда обязана включать start_ticks: ["pulse_process_cpu_cores{pid=\"123\"}", "pulse_process_cpu_cores{pid=\"123\"}"]` | PROMOTED в PR-06 |
| PULSE-039 | `audit_red_counter_rate_through_warm_layer` | pulse-store | — | UNEXPECTED_GREEN → PROMOTED (см. ниже) |
| PULSE-066 | `cgroup_limit_stops_the_walk_and_reports_truncation` + `repeated_truncation_preserves_unvisited_entities_and_baselines` | pulse-collect | молчаливое усечение выдавало частичный граф за полный и после двух тактов удаляло непосещённые сущности | FIXED в PR-07b |
| PULSE-086 | `ports_come_from_namespace_of_the_process_not_of_the_agent` + live netns inode probe | pulse-collect | inode слушающего сокета находился в `/proc/<pid>/net/tcp` и отсутствовал в `/proc/net/tcp` агента | FIXED в PR-06 |
| PULSE-064 | `equal_display_names_of_distinct_units_do_not_fold_together` + `equal_process_names_without_owner_remain_separate` | pulse-tui | system/user `dbus.socket` склеивались в одну строку по display name | FIXED в PR-12 |
| PULSE-065 | `main_process_is_oldest_not_smallest_pid` + подпись в `unit_resolves_processes_that_are_its_siblings` | pulse-tui | минимальный PID выбирал нового воркера после оборота счётчика и выдавал эвристику за фактический MainPID | FIXED в PR-12 |
| PULSE-080 | `proc_walk_work_is_bounded_by_max_processes` + `cgroup_child_listing_is_bounded_by_budget` | pulse-collect | обход материализовал весь каталог и применял бюджет уже после: 20 000 записей `/proc` при лимите 64 | FIXED в PR-07c |
| PULSE-071 | `descriptor_scan_work_is_bounded_by_budget` + `descriptor_scan_within_budget_is_not_truncated` | pulse-collect | `read_link` выполнялся для каждого дескриптора: 20 000 разыменований при бюджете 64, усечение не заявлялось | FIXED в PR-14 |
| PULSE-078 | `tick_duration_covers_storage_phase` | pulse-cli | длительность такта фиксировалась до записи в историю: на живом стенде 28.5 мс вместо 51.4 мс, `ticks_skipped` не видел перерасхода | FIXED в PR-15 |
| PULSE-079 | `drawing_after_a_read_does_not_block_the_writer` + контрольный `holding_the_guard_across_a_draw_does_block_the_writer` | pulse-tui | кадр удерживал read-замок истории всю отрисовку: писатель ждал её окончания (в опыте — 400 мс) | FIXED в PR-16 |
| PULSE-015 | `agent_rss_follows_kernel_page_size` + `process_rss_without_status_follows_kernel_page_size` | pulse-collect | размер страницы был константой 4096: на ядре с 64 КиБ страницами RSS занижался в шестнадцать раз | FIXED в PR-17 |
| PULSE-085 | `self_metrics_option_changes_the_frame` | pulse-tui | `ui.show_self_metrics` нигде не читалась: включение опции не меняло ни одного кадра | FIXED в PR-18 |
| PULSE-082 | `scripts/measure-memory-bound.sh` + лейн `.github/workflows/memory.yml` | pulse-cli, pulse-store | живой прогон с бюджетом 2 МиБ: пик истории 2.4 МиБ, вытеснение 9 раз без прогресса; факт был виден только в `scorecard` и не экспортировался | FIXED в PR-22: пик и отказ вытеснения ушли в `/metrics`, контракт «либо в бюджете, либо заявлено» закреплён гейтом |
| PULSE-077 | `every_produced_metric_is_described` (`pulse-collect/tests/registry.rs`) + `process_state_is_never_exported` | pulse-collect, pulse-export | образец с метрикой вне реестра молча пропускался экспортёром: метрика существовала в истории и отсутствовала в `/metrics` | FIXED в PR-23. Обратная проверка «у каждой объявленной метрики есть производитель» отклонена как нестабильная: набор произведённых метрик зависит от подсистем хоста |
| PULSE-001 | пять обязательных команд на `1.85.1` | workspace | `rustc 1.85.1 is not supported by: darling@0.24.1 requires rustc 1.88.0; instability@0.3.13 requires rustc 1.88` | FIXED в PR-01 |

### PULSE-039: почему тест зелёный

Аудит ожидал, что скорость счётчика через warm-слой считается по средним
значениям бакетов. Проверка на неравномерном ряде со сбросом счётчика внутри
warm-слоя показала обратное: `History::rate` считает по фактическим
приращениям и переживает сброс. Тест **не** помечен `#[ignore]` и оставлен
в обязательном наборе как защита от регрессии, а не как доказательство
дефекта.

## BLOCKED_BY_DESIGN и LIVE_REQUIRED

Эти находки нельзя воспроизвести честно без изменения production-API или без
настоящего Linux-стенда. Фальшивые тесты вместо них запрещены.

| Finding | Причина | Куда уходит |
|---|---|---|
| PULSE-009 (TOCTOU между проверкой и `kill`) | детерминированного репродьюсера гонки ядра не существует; закрыт конструктивно | FIXED в PR-20: сигнал идёт через `pidfd_open`+`pidfd_send_signal`, живой тест `live_signal_reaches_the_child_process` доказывает доставку новым путём; остаточный риск на ядрах < 5.3 записан в `docs/SECURITY.md` §5 |
| PULSE-067 (ширина в колонках терминала) | требовался переход на unicode-width в собственных помощниках | FIXED в PR-21: `ui::width_of`/`ui::take_columns` считают колонки; переведены `truncate`, `breadcrumb`, `section_title`, футер, ячейки таблицы, перенос по словам и хвост `elide_path`. Тесты `width_and_truncation_count_terminal_columns` и `path_is_elided_by_segments` падают при возврате к счёту символов |
| PULSE-069 (advisory `lru 0.12.5`) | обновление невозможно: `ratatui 0.29` требует `^0.12`, исправления вышли в `0.16.3`/`0.18.2`; оба пути (`IterMut`, паникующий `Drop` ключа) в графе недостижимы | ACCEPTED в PR-19: обоснование в `docs/SECURITY.md` §13.1, поимённые `--ignore` и страж области исключения в `supply-chain.yml` |

## Инструменты проверки

| Что | Где | Зачем |
|---|---|---|
| `CountingFs` | `crates/pulse-collect/src/test_support.rs` | измеряет фактическое число чтений, каталогов, ссылок и inode: без этого «bounded work» недоказуем |
| `ScriptedFs` | там же | детерминированно меняет ответ по одному пути между вызовами: PID reuse и исчезновение файла без sleep и гонок |
| property-тесты | `crates/pulse-core/tests/properties.rs`, `crates/pulse-store/tests/properties.rs` | закрепляют уже работающие инварианты санитайзера, арифметики времени и отсева не-finite значений |
| `scripts/audit-red.sh` | скрипты | доказывает, что каждый RED падает по своей причине, и требует продвижения теста, если дефект исправлен |
