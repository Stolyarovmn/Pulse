//! Главный экран (разделы 123, 143-147, 151-163 спецификации v0.8).
//!
//! Информационный порядок фиксирован разделом 163 и не подлежит перестановке:
//!
//! ```text
//! STATE / GLYPH
//! ATTENTION
//! CURRENT SIGNALS
//! RECENT MEANINGFUL CHANGES
//! RELEVANT LOGICAL ENTITIES
//! SELECTED CONTEXT когда есть место
//! ```
//!
//! Главное отличие от прошлой реализации: дополнительная ширина уходит в
//! контекст, а не в растяжение таблицы (разделы 143, 160). Нижняя область - это
//! значимые логические сущности плюс панель выбранного, а не плоский инвентарь.
//!
//! Полный инвентарь доступен на экране Entities (раздел 147).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use pulse_core::snapshot::{Snapshot, RECENT_CHANGES_BUDGET};
use pulse_core::time::format_duration;
use pulse_core::{EventKind, MeaningfulEvent};

use crate::app::{logical_relation_targets, App};
use crate::fold::LogicalRow;
use crate::glyph::GlyphSurface;
use crate::layout::{bounded, BlockKind, LayoutPlan, OverviewComposition, Priority};
use crate::state::StateGlyph;
use crate::theme::{Capability, Theme};

use super::{focused_section, observation, rect, section, vitals_line};

/// Сколько изменений показывать на главном экране.
///
/// Раздел 153: подробности baseline уходят за `Enter`, поэтому строк нужно мало.
const CHANGES_LIMIT: usize = 3;

/// Отрисовка главного экрана.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &mut App,
    plan: &LayoutPlan,
    theme: &Theme,
    history: Option<&pulse_store::History>,
) {
    let composition = OverviewComposition::resolve_dimensions(area.width, area.height);

    // Значимые логические сущности, а не весь инвентарь (разделы 147-150).
    let derived = app.derived(snapshot);
    let rows: &[LogicalRow] = &derived.relevant;
    app.overview.list_len = rows.len();
    let selected = app.overview.selected.min(rows.len().saturating_sub(1));
    app.overview.preview_relation_len = rows
        .get(selected)
        .map_or(0, |row| logical_relation_targets(snapshot, row).len());

    let glyph = StateGlyph::from_snapshot(snapshot);
    let surface = GlyphSurface::build(&glyph, plan.glyph);
    let attention = attention_lines(snapshot, theme);
    let changes = changes_lines(snapshot, theme);

    let side_by_side = !matches!(composition, OverviewComposition::Stacked);
    // Блок состояния: заголовок, фигура, вердикт.
    let state_h = u16::try_from(surface.height() + 4).unwrap_or(u16::MAX);
    let attention_h = u16::try_from(attention.len() + 1).unwrap_or(2);
    let show_changes = !changes.is_empty() && area.height >= 20;

    let mut constraints: Vec<Constraint> = Vec::new();
    constraints.push(Constraint::Length(if side_by_side {
        state_h.max(attention_h)
    } else {
        state_h
    }));
    if !side_by_side {
        constraints.push(Constraint::Length(attention_h));
    }
    // Полоса сигналов: заголовок и строка значений.
    constraints.push(Constraint::Length(if plan.shows(Priority::P2) {
        2
    } else {
        1
    }));
    if show_changes {
        constraints.push(Constraint::Length(
            u16::try_from(changes.len() + 1).unwrap_or(2),
        ));
    }
    constraints.push(Constraint::Min(2));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut slot = 0_usize;

    // 1. STATE + ATTENTION.
    let head = rect(&chunks, slot);
    slot += 1;
    if side_by_side {
        // Раздел 146: состояние 28-40 колонок, внимание - остаток, но с
        // ограничением полезной ширины (раздел 144).
        let (state_w, _) = bounded(BlockKind::StateBlock, 42.min(head.width));
        let attention_available = head.width.saturating_sub(state_w).saturating_sub(3);
        let (attention_w, _) = bounded(BlockKind::Attention, attention_available);
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(state_w),
                Constraint::Length(3),
                Constraint::Length(attention_w),
                Constraint::Min(0),
            ])
            .split(head);
        render_state(
            frame,
            rect(&halves, 0),
            snapshot,
            &glyph,
            &surface,
            plan,
            theme,
        );
        render_attention(frame, rect(&halves, 2), &attention, plan, theme);
    } else {
        render_state(frame, head, snapshot, &glyph, &surface, plan, theme);
        let block = rect(&chunks, slot);
        slot += 1;
        render_attention(frame, block, &attention, plan, theme);
    }

    // 2. SIGNALS: текущее состояние, а не накопленные счётчики (раздел 156).
    //
    // §123 и §190 рисуют разделители секций, идущих в стопке, на всю ширину
    // экрана: линия отделяет секции друг от друга. Ограничение §188/§189
    // касается панелей, делящих один ряд (STATE|ATTENTION, ENTITY|SELECTED) -
    // там линия обязана кончаться на краю своей панели. Линия, обрывающаяся
    // посреди пустого ряда, читается как дефект отрисовки.
    let signals = rect(&chunks, slot);
    slot += 1;
    let mut signal_lines: Vec<Line<'_>> = Vec::new();
    if plan.shows(Priority::P2) {
        signal_lines.push(section("SIGNALS", signals.width, plan, theme));
    }
    signal_lines.push(vitals_line(snapshot, theme));
    frame.render_widget(Paragraph::new(signal_lines), signals);

    // 3. RECENT CHANGES.
    if show_changes {
        let area = rect(&chunks, slot);
        slot += 1;
        let mut lines = vec![section("RECENT CHANGES", area.width, plan, theme)];
        lines.extend(changes);
        frame.render_widget(Paragraph::new(lines), area);
    }

    // 4. RELEVANT ENTITIES + SELECTED.
    let bottom = rect(&chunks, slot);
    if composition.shows_selected() && !rows.is_empty() {
        // Раздел 146: таблица 55-65%, панель выбранного 35-45%.
        let (table_w, _) = bounded(BlockKind::EntityTable, bottom.width * 6 / 10);
        let selected_available = bottom.width.saturating_sub(table_w).saturating_sub(3);
        let (selected_w, _) = bounded(BlockKind::Selected, selected_available);
        let halves = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(table_w),
                Constraint::Length(3),
                Constraint::Length(selected_w),
                Constraint::Min(0),
            ])
            .split(bottom);
        render_entities(
            frame,
            rect(&halves, 0),
            rows,
            &TableView {
                selected,
                focused: app.pane == crate::layout::Pane::Primary,
                rule_width: None,
            },
            plan,
            theme,
        );
        render_selected(
            frame,
            rect(&halves, 2),
            snapshot,
            rows.get(selected),
            (
                app.pane == crate::layout::Pane::Inspector,
                app.overview.preview_relation_selected,
            ),
            Some(&TrendSource { snapshot, history }),
            plan,
            theme,
        );
    } else {
        // Содержимое остаётся ограниченным полезной шириной (раздел 144):
        // остаток справа - намеренная пустота, а не место под растягивание.
        // Правило же принадлежит ряду, которым панель владеет целиком, поэтому
        // §123 рисует его на всю ширину. Это разные величины, и путать их
        // нельзя: `bounded` отвечает за layout, а не за длину линии.
        let (table_w, _) = bounded(BlockKind::EntityTable, bottom.width);
        let area = Rect {
            width: table_w,
            ..bottom
        };
        render_entities(
            frame,
            area,
            rows,
            &TableView {
                selected,
                focused: true,
                rule_width: Some(bottom.width),
            },
            plan,
            theme,
        );
    }
}

/// Блок состояния: фигура и вердикт под ней (раздел 145).
fn render_state(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    glyph: &StateGlyph,
    surface: &GlyphSurface,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let mut lines = vec![section("STATE", area.width, plan, theme)];

    for row in surface.cells() {
        let mut spans: Vec<Span<'_>> = Vec::new();
        for (index, class) in row.iter().enumerate() {
            if index > 0 && !matches!(plan.glyph, crate::layout::GlyphPreset::Compact) {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                class.symbol(theme.capability).to_string(),
                class.style(theme),
            ));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    let overall = glyph.overall();
    lines.push(Line::from(Span::styled(
        glyph.verdict().to_string(),
        overall.style(theme),
    )));
    // §163 запрещает дублировать «0 problems», а §190 показывает в панели
    // состояния только фигуру и вердикт. Счётчик уместен лишь тогда, когда
    // он несёт число, которого нет ни в шапке, ни в ATTENTION.
    if plan.shows(Priority::P2) && !snapshot.problems.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} active problems", snapshot.problems.len()),
            theme.dim(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Блок внимания: компактный в здоровом состоянии (раздел 159).
fn render_attention(
    frame: &mut Frame<'_>,
    area: Rect,
    body: &[Line<'static>],
    plan: &LayoutPlan,
    theme: &Theme,
) {
    // Панель уже получила ровно ту ширину, которую ей отдал layout: сжимать
    // линию ещё раз значит оборвать её внутри собственной панели.
    let mut lines = vec![section("ATTENTION", area.width, plan, theme)];
    lines.extend(body.iter().cloned());
    frame.render_widget(Paragraph::new(lines), area);
}

/// Строка о сокращении глубины истории, если потолок памяти уже сработал.
///
/// Раньше этот факт существовал только как `tracing::warn!`, а в TUI-режиме
/// журнал печатался в терминал, занятый ratatui: строка оставалась висеть
/// посреди кадра и переживала переключение экранов. Место такого факта — в
/// интерфейсе, рядом с остальным состоянием наблюдения.
fn retention_line(snapshot: &Snapshot, theme: &Theme) -> Option<Line<'static>> {
    let agent = &snapshot.agent;
    let reduced = agent.history_evicted_buckets
        + agent.history_evicted_hot_ticks
        + agent.history_evicted_events;
    if reduced == 0 {
        return None;
    }
    // Число — фактически удерживаемая память, а не настроенный бюджет:
    // подписывать его «budget» значило бы назвать одну величину другой.
    let held = crate::format::bytes(agent.store_bytes as f64);
    let text = if agent.history_eviction_no_progress > 0 {
        format!("history at memory ceiling: retention minimal, {held} held")
    } else {
        format!("history retention reduced by memory ceiling: {held} held")
    };
    Some(Line::from(Span::styled(text, theme.dim())))
}

/// Содержимое блока внимания.
///
/// Здоровое состояние - две строки: вердикт и длительность наблюдения. Высокий
/// фиксированный блок не резервируется, остаток высоты уходит вниз (раздел 159).
fn attention_lines(snapshot: &Snapshot, theme: &Theme) -> Vec<Line<'static>> {
    let ascii = matches!(theme.capability, Capability::Ascii);
    if snapshot.problems.is_empty() {
        let mark = if ascii { "ok" } else { "✓" };
        let observed = observation(snapshot);
        let started = snapshot
            .entity(snapshot.host)
            .map_or(snapshot.at, |host| host.first_seen);
        let mut lines = vec![
            Line::from(Span::styled(
                format!("{mark} NO ACTIVE PROBLEMS"),
                theme.strong(),
            )),
            Line::from(""),
            // Раздел 151: утверждение о наблюдении, а не о прошлом системы.
            Line::from(Span::styled(
                format!("observed nominal for {}", format_duration(observed)),
                theme.text(),
            )),
            Line::from(Span::styled(
                format!("observation started {started}"),
                theme.dim(),
            )),
        ];
        lines.extend(retention_line(snapshot, theme));
        return lines;
    }

    let mut problems: Vec<&pulse_core::problem::Problem> = snapshot.problems.iter().collect();
    problems.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.title.cmp(&b.title)));
    let mut lines: Vec<Line<'static>> = problems
        .iter()
        .take(4)
        .map(|problem| {
            let class = crate::state::StateClass::from_severity(problem.severity);
            Line::from(vec![
                Span::styled(
                    format!("{} ", class.symbol(theme.capability)),
                    class.style(theme),
                ),
                Span::styled(problem.title.clone(), theme.text()),
                Span::raw("  "),
                Span::styled(problem.entity_name.clone(), theme.dim()),
            ])
        })
        .collect();
    lines.extend(retention_line(snapshot, theme));
    lines
}

/// Как панель таблицы показывается в текущем ряду.
struct TableView {
    selected: usize,
    focused: bool,
    /// Ширина правила, если панель владеет рядом целиком: линия отделяет
    /// секцию от соседней и потому идёт на всю ширину ряда (§123), тогда как
    /// содержимое остаётся в границах полезной ширины (раздел 144). Величины
    /// разные, и путать их нельзя: `bounded` отвечает за layout, а не за линию.
    rule_width: Option<u16>,
}

/// Источник тренда для строк таблицы.
///
/// Отдельная структура, а не два параметра: у функции отрисовки иначе
/// набирается восемь аргументов, и подпись перестаёт читаться.
pub(crate) struct TrendSource<'a> {
    pub snapshot: &'a Snapshot,
    pub history: Option<&'a pulse_store::History>,
}

/// Explainable Relevant/Key Entities (v0.9 §165, §176, §190).
fn render_entities(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[LogicalRow],
    view: &TableView,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let title = if rows.iter().any(|row| !row.reasons.is_empty()) {
        "RELEVANT ENTITIES"
    } else {
        "KEY ENTITIES"
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(area);
    let head = Rect {
        width: view.rule_width.unwrap_or(area.width),
        ..rect(&chunks, 0)
    };
    frame.render_widget(
        Paragraph::new(focused_section(
            title,
            head.width,
            view.focused,
            plan,
            theme,
        )),
        head,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  sort:relevance  filter:all  view:logical",
            theme.dim(),
        ))),
        rect(&chunks, 1),
    );

    let table = rect(&chunks, 2);
    let show_why = table.width >= 58;
    // Дорожки CPU за минуту здесь нет намеренно. На спокойном хосте каждая
    // строка даёт 0.00c, поэтому двенадцать ячеек дорожки и подписанный пик
    // превращались в серое полотно, одинаковое у всех строк: место занято,
    // информации ноль. Освободившиеся двадцать колонок отданы имени - на
    // скриншоте владельца `systemd-journald.serv…` и `unattended-upgrades.s…`
    // обрезались именно из-за дорожки. Форма нагрузки осталась там, где она
    // отвечает на заданный вопрос: в панели выбранной сущности и на экране
    // Entities.
    const NAME: usize = 34;
    let mut lines: Vec<Line<'_>> = Vec::new();
    let header = if show_why {
        "  NAME                                CPU      MEM      S  WHY"
    } else {
        "  NAME                                CPU      MEM      S"
    };
    lines.push(Line::from(Span::styled(header, theme.dim())));
    let visible = usize::from(table.height).saturating_sub(1);
    let start = view.selected.saturating_sub(visible.saturating_sub(1));
    for (index, logical) in rows.iter().enumerate().skip(start).take(visible) {
        let selected_row = index == view.selected;
        let marker = crate::ui::row_marker(selected_row);
        let name = crate::ui::truncate(&logical.row.name, NAME, theme.capability);
        let why = logical
            .reasons
            .first()
            .map_or("key", |reason| reason.label());
        let text = if show_why {
            format!(
                "{marker}{name:<NAME$} {:>6} {:>8}  {}  {why}",
                crate::format::cores(logical.row.cpu),
                crate::format::bytes(logical.row.memory),
                logical.row.state.symbol(theme.capability),
            )
        } else {
            format!(
                "{marker}{name:<NAME$} {:>6} {:>8}  {}",
                crate::format::cores(logical.row.cpu),
                crate::format::bytes(logical.row.memory),
                logical.row.state.symbol(theme.capability),
            )
        };
        lines.push(Line::from(Span::styled(
            text,
            if selected_row {
                theme.selection()
            } else {
                theme.text()
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines), table);
}

/// Панель выбранной сущности (разделы 145, 146).
///
/// Отвечает на вопрос «что это такое», не заставляя уходить в Inspect. Связи
/// агрегированы: список раскрывается по `Enter` (раздел 14 задания).
#[allow(clippy::too_many_arguments)]
fn render_selected(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    row: Option<&LogicalRow>,
    interaction: (bool, usize),
    trend: Option<&TrendSource<'_>>,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let (focused, relation_selected) = interaction;
    let (width, _) = bounded(BlockKind::Selected, area.width);
    let Some(logical) = row else {
        frame.render_widget(
            Paragraph::new(focused_section("SELECTED", width, focused, plan, theme)),
            area,
        );
        return;
    };
    let selected_title = format!("SELECTED / {}", logical.row.name);
    let mut lines = vec![focused_section(
        &crate::ui::truncate(&selected_title, usize::from(width), theme.capability),
        width,
        focused,
        plan,
        theme,
    )];

    let entity = snapshot.entity(logical.row.id);
    lines.push(Line::from(""));
    let pair = |label: &'static str, value: String| {
        Line::from(vec![
            Span::styled(format!("{label:<12}"), theme.dim()),
            Span::styled(value, theme.text()),
        ])
    };

    // §190: порядок и регистр подписей заданы каноническим эскизом:
    // STATE -> RELEVANCE -> CPU -> MEM -> RELATIONS -> RECENT. RELEVANCE -
    // одно значение, а не список: панель отвечает «почему здесь», а перечень
    // всех совпавших признаков её только шумит.
    lines.push(pair(
        "STATE",
        logical.row.state.symbol(theme.capability).to_string(),
    ));
    lines.push(pair(
        "RELEVANCE",
        logical
            .reasons
            .first()
            .map_or_else(|| "key entity".to_string(), |r| r.label().to_string()),
    ));
    if let Some(id) = &logical.technical_id {
        // Имя заменено на узнаваемое, но идентификатор нужен для `docker inspect`
        // и потому остаётся рядом (§Logical view).
        lines.push(pair("ID", id.clone()));
    }
    lines.push(Line::from(""));
    lines.push(pair("CPU", crate::format::cores(logical.row.cpu)));
    // Пик и среднее за минуту рядом с текущим значением: одно число
    // не отличает «всегда столько» от «только что подскочило», а форма
    // в колонке тренда нормирована по пику и без него не читается.
    if let Some(found) = trend.and_then(|source| {
        let key = pulse_core::sample::SeriesKey::new(
            logical.row.id,
            crate::rows::cpu_metric(logical.row.kind),
        );
        crate::trend::of_series(source.history, source.snapshot, key, 0, theme)
            .filter(crate::trend::Trend::is_measured)
    }) {
        lines.push(pair(
            "CPU 60s",
            format!(
                "peak {}  avg {}",
                crate::format::cores(found.peak),
                crate::format::cores(found.mean)
            ),
        ));
    }
    lines.push(pair("MEM", crate::format::bytes(logical.row.memory)));
    if let Some(io) = logical.row.io {
        lines.push(pair("IO", crate::format::rate(io)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("RELATIONS", theme.dim())));
    if logical.processes > 0 {
        lines.push(pair("processes", format!("{}", logical.processes)));
    }
    if entity.is_some() {
        let targets = logical_relation_targets(snapshot, logical);
        for (index, target) in targets.iter().take(6).enumerate() {
            let selected = focused && index == relation_selected;
            lines.push(Line::from(vec![
                Span::styled(
                    crate::ui::row_marker(selected),
                    if selected {
                        theme.selection()
                    } else {
                        theme.dim()
                    },
                ),
                Span::styled(format!("{:<10}", target.label), theme.dim()),
                Span::styled(
                    crate::ui::truncate(
                        &target.name,
                        usize::from(width).saturating_sub(14),
                        theme.capability,
                    ),
                    if selected {
                        theme.selection()
                    } else {
                        theme.text()
                    },
                ),
            ]));
        }
    }

    if plan.shows(Priority::P3) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("RECENT", theme.dim())));
        let touched = snapshot
            .meaningful
            .iter()
            .filter(|event| event.entity.is_some_and(|id| logical.members.contains(&id)))
            .take(3)
            .collect::<Vec<_>>();
        if touched.is_empty() {
            lines.push(Line::from(Span::styled(
                "no meaningful changes",
                theme.dim(),
            )));
        } else {
            for event in touched {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", event.last_at), theme.dim()),
                    Span::styled(event.kind.as_str().to_string(), theme.text()),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Строки блока изменений (разделы 124, 153).
///
/// Baseline занимает ровно одну selectable строку; детали живут в Selected pane.
fn changes_lines(snapshot: &Snapshot, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Источник — только значимые события снимка: рутинный churn ядра сюда не
    // попадает по построению, а не по фильтру этого экрана.
    for event in snapshot
        .meaningful
        .iter()
        .take(CHANGES_LIMIT.min(RECENT_CHANGES_BUDGET))
    {
        lines.push(Line::from(vec![
            Span::styled(format!("{}  ", event.last_at), theme.dim()),
            event_marker(event.kind, theme),
            Span::raw(" "),
            Span::styled(headline(event), theme.text()),
            Span::raw("      "),
            Span::styled(summary(event), theme.dim()),
        ]));
    }
    if lines.is_empty() {
        // Честная пустота лучше шума: §Recent Changes прямо это требует.
        lines.push(Line::from(Span::styled(
            "no meaningful changes",
            theme.dim(),
        )));
    }
    if snapshot.suppressed_noise > 0 {
        // Оператор обязан знать, что сырой поток существует и где он лежит.
        lines.push(Line::from(Span::styled(
            format!(
                "{} routine observations suppressed · : raw events",
                snapshot.suppressed_noise
            ),
            theme.dim(),
        )));
    }
    lines
}

/// Заголовок события: что произошло.
fn headline(event: &MeaningfulEvent) -> String {
    if event.kind == EventKind::ObservationStarted {
        return "PULSE observation started".to_string();
    }
    if event.is_group() {
        return format!(
            "{} {} ×{}",
            event.entity_name,
            event.kind.as_str(),
            event.count
        );
    }
    format!("{} {}", event.entity_name, event.kind.as_str())
}

/// Краткая правая колонка события (раздел 153).
///
/// Для baseline это только число сущностей: разбор по видам не помещается в
/// строку и уводит внимание от того, что наблюдение началось.
fn summary(event: &MeaningfulEvent) -> String {
    if event.kind == EventKind::ObservationStarted {
        return event
            .detail
            .split(';')
            .next()
            .unwrap_or(&event.detail)
            .trim()
            .to_string();
    }
    event.detail.clone()
}

/// Пунктуация события (раздел 133): `!` - событие, символы состояния - severity.
fn event_marker(kind: EventKind, theme: &Theme) -> Span<'static> {
    let ascii = matches!(theme.capability, Capability::Ascii);
    match kind {
        EventKind::ObservationStarted => {
            Span::styled(if ascii { "O" } else { "●" }.to_string(), theme.strong())
        }
        EventKind::ProblemClosed => {
            Span::styled(if ascii { "o" } else { "○" }.to_string(), theme.dim())
        }
        _ => Span::styled(
            "!".to_string(),
            theme.severity(pulse_core::problem::Severity::Info),
        ),
    }
}
