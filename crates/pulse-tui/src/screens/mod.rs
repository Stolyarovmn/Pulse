//! Оркестрация экранов по спецификации v0.7.
//!
//! Экран не знает своих координат: он получает готовый [`LayoutPlan`] и решает
//! только, что показать при таком бюджете (разделы 89, 112, 113). Порядок
//! распределения места фиксирован разделом 113 и обязан быть детерминированным.
//!
//! Шапка и футер - блоки приоритета P0: режим наблюдения и число проблем не
//! скрываются ни при каком размере (раздел 111).

mod acceptance;
pub mod entities;
pub mod help;
pub mod inspector;
pub mod overview;
pub mod pipe;
pub mod problems;
pub mod timeline;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use pulse_core::metric::ids;
use pulse_core::snapshot::Snapshot;
use pulse_core::time::format_duration;

use crate::app::{story_events, App, Overlay, Screen};
use crate::fold::LogicalRow;
use crate::format;
use crate::layout::{HeaderPreset, LayoutContext, LayoutPlan};
use crate::table;
use crate::theme::Theme;
use crate::ui;

/// Базовый стиль полотна.
#[must_use]
pub fn base_style(theme: &Theme) -> Style {
    theme.text()
}

/// Полная отрисовка кадра.
pub fn render(
    frame: &mut Frame<'_>,
    snapshot: &Snapshot,
    app: &mut App,
    theme: &Theme,
    history: Option<&pulse_store::History>,
) {
    let area = frame.area();
    let story_len = u16::try_from(story_events(snapshot).len()).unwrap_or(u16::MAX);
    let plan = app.layout.plan(LayoutContext {
        width: area.width,
        height: area.height,
        screen: app.screen,
        focus: app.pane,
        has_problem: !snapshot.problems.is_empty(),
        story_len,
    });

    if plan.too_small {
        render_too_small(frame, area, theme);
        return;
    }

    // §190 и канонические эскизы Problems/Entities/Inspector/Timeline
    // (строки 5732, 5775, 5818, 5898, 5947, 7957) рисуют full-width правило
    // прямо под шапкой. Это не панельный разделитель: §188 и §189 ограничивают
    // ширину разделителей *внутри* bounded-панелей, а шапка принадлежит экрану
    // целиком.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    let body = rect(&chunks, 2);

    // Renderer публикует тот же VisiblePaneSet, который input router использует
    // до следующего кадра (разделы 170, 195, 196).
    let visible = visible_panes(snapshot, app, &plan, body);
    app.set_visible_panes(&visible);

    render_header(frame, rect(&chunks, 0), snapshot, app, &plan, theme);
    render_header_rule(frame, rect(&chunks, 1), theme);
    render_footer(frame, rect(&chunks, 3), &plan, theme, app.icons);

    if matches!(app.overlay, Some(Overlay::Help)) {
        help::render(frame, body, &plan, theme);
        return;
    }

    if let Some(Overlay::Pipe(state)) = app.overlay.clone() {
        pipe::render(frame, body, snapshot, app, &state, &plan, theme);
        return;
    }

    if app.inspector.is_some() {
        inspector::render(frame, body, snapshot, app, &plan, theme);
    } else {
        match app.screen {
            Screen::Overview => overview::render(frame, body, snapshot, app, &plan, theme),
            Screen::Problems => problems::render(frame, body, snapshot, app, &plan, theme),
            Screen::Entities => entities::render(frame, body, snapshot, app, &plan, theme),
            Screen::Timeline => {
                timeline::render(frame, body, snapshot, app, &plan, theme, history);
            }
        }
    }

    if app.overlay.is_some() {
        render_input_overlay(frame, body, snapshot, app, theme);
    }
}

/// Фактически видимые focusable panes текущего кадра.
fn visible_panes(
    snapshot: &Snapshot,
    app: &App,
    plan: &LayoutPlan,
    body: Rect,
) -> Vec<crate::layout::Pane> {
    use crate::layout::Pane;
    if app.inspector.is_some() || app.overlay.is_some() {
        return vec![Pane::Primary];
    }
    match app.screen {
        Screen::Overview => {
            let no_entities = app.derived(snapshot).relevant.clone().is_empty();
            if body.width >= 140 && body.height >= 18 && !no_entities {
                vec![Pane::Primary, Pane::Inspector]
            } else {
                vec![Pane::Primary]
            }
        }
        Screen::Problems => vec![Pane::Primary],
        Screen::Entities => {
            if plan.inspector_beside && !snapshot.entities.is_empty() {
                vec![Pane::Primary, Pane::Inspector]
            } else {
                vec![Pane::Primary]
            }
        }
        Screen::Timeline => {
            let no_story = story_events(snapshot).is_empty();
            if no_story {
                vec![Pane::Primary]
            } else if body.width >= 120 && body.height >= 18 {
                vec![Pane::Primary, Pane::Story, Pane::Inspector]
            } else {
                vec![Pane::Primary, Pane::Story]
            }
        }
    }
}

/// Видимый Search/Palette modal. underlying screen уже отрисован, но router
/// полностью блокирует его ввод (разделы 177-179).
fn render_input_overlay(
    frame: &mut Frame<'_>,
    body: Rect,
    snapshot: &Snapshot,
    app: &App,
    theme: &Theme,
) {
    let height = 3_u16.min(body.height);
    let area = Rect {
        y: body.y.saturating_add(body.height.saturating_sub(height)),
        height,
        ..body
    };
    frame.render_widget(Clear, area);
    let (title, query, status) = match &app.overlay {
        Some(Overlay::Search(search)) => {
            let matches = app.derived(snapshot).rows.len();
            (
                " SEARCH ",
                format!("SEARCH / {}█", search.query),
                format!("{matches} matches   Enter apply/open   Esc cancel"),
            )
        }
        Some(Overlay::Palette { query }) => (
            " COMMANDS ",
            format!(": {query}█"),
            "Enter run   Esc cancel".to_string(),
        ),
        _ => return,
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .title(title)
        .style(theme.strong());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(query, theme.strong())),
            Line::from(Span::styled(status, theme.dim())),
        ])
        .block(block),
        area,
    );
}

/// Экран «терминал слишком мал» (раздел 110).
///
/// Показывает требуемый и текущий размер: пользователь должен понять, что
/// делать, а не смотреть на обрезанную вёрстку.
fn render_too_small(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let lines = vec![
        Line::from(Span::styled("PULSE", theme.strong())),
        Line::from(""),
        Line::from(Span::styled("terminal too small", theme.text())),
        Line::from(Span::styled(
            format!(
                "need at least {}×{}",
                crate::layout::MIN_WIDTH,
                crate::layout::MIN_HEIGHT
            ),
            theme.dim(),
        )),
        Line::from(Span::styled(
            format!("current {}×{}", area.width, area.height),
            theme.dim(),
        )),
        Line::from(""),
        Line::from(Span::styled("q quit", theme.dim())),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// Шапка по пресетам раздела 106 с правками разделов 152, 157, 158.
///
/// Три правила, нарушение которых видно на скриншоте прототипа:
///
/// * `LIVE ●` - точка означает режим наблюдения, а не состояние. `LIVE ○`
///   запрещён: `○` уже занят в алфавите состояния значением «норма» (§157);
/// * `0 problems` в шапке не показывается: счётчик полезен именно тем, что
///   исключителен, и дублировать его рядом с блоком состояния незачем (§158);
/// * `up` - аптайм хоста, и он никогда не подменяет длину наблюдения (§152).
fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &App,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    let live = ui::live_label(app.paused, plan.header);
    let live_style = if app.paused {
        theme.severity(pulse_core::problem::Severity::Info)
    } else {
        theme.strong()
    };
    let problems = snapshot.problems.len();
    let worst = snapshot.worst_severity();

    let mut spans: Vec<Span<'_>> = Vec::new();
    let wide = matches!(plan.header, HeaderPreset::Wide);

    spans.push(Span::styled(
        if wide { "P U L S E" } else { "PULSE" },
        theme.strong(),
    ));

    if !matches!(plan.header, HeaderPreset::Tiny) {
        spans.push(Span::raw(if wide { "   " } else { "  " }));
        spans.push(Span::styled(snapshot.hostname.clone(), theme.text()));
    }

    spans.push(Span::raw(if wide { "   " } else { "  " }));
    // Иконка режима идёт перед подписью, а точка `●` остаётся на своём месте:
    // она часть нормативного макета и означает «поток данных живой».
    let icon_set = if matches!(theme.capability, crate::theme::Capability::Ascii) {
        pulse_core::config::IconSet::Off
    } else {
        app.icons
    };
    let mode_icon = crate::icons::prefix(
        if app.paused {
            crate::icons::Icon::Paused
        } else {
            crate::icons::Icon::Live
        },
        icon_set,
    );
    if !mode_icon.is_empty() {
        spans.push(Span::styled(mode_icon, live_style));
    }
    spans.push(Span::styled(live, live_style));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(ui::live_mark(theme.capability), live_style));

    // Пометка режима стоит сразу за индикатором наблюдения и не убирается
    // ни на одном пресете: кадр демо содержит настоящие проблемы настоящих
    // правил, и спутать его с состоянием своей машины недопустимо.
    if app.demo {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            " DEMO ",
            theme.severity(pulse_core::problem::Severity::Warn),
        ));
    }

    // Счётчик проблем появляется только когда он что-то значит.
    if problems > 0 {
        let class = worst.map_or(crate::state::StateClass::Degraded, |severity| {
            crate::state::StateClass::from_severity(severity)
        });
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            format!("{}{problems}", class.symbol(theme.capability)),
            theme.severity(worst.unwrap_or(pulse_core::problem::Severity::Info)),
        ));
    }

    if !matches!(plan.header, HeaderPreset::Tiny) {
        spans.push(Span::raw(if wide { "   " } else { "  " }));
        spans.push(Span::styled(format!("{}", snapshot.at), theme.text()));
    }

    if wide {
        // Аптайм хоста: подписан `up` и относится к машине, а не к наблюдению.
        let uptime = snapshot.value_or(snapshot.host, ids::HOST_UPTIME, 0.0);
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            format!(
                "up {}",
                format_duration(std::time::Duration::from_secs(uptime as u64))
            ),
            theme.dim(),
        ));
        // Текущий экран: оператор всегда знает, где он.
        spans.push(Span::raw("   "));
        spans.push(Span::styled(app.title(), theme.dim()));
    } else {
        // Current mode — P0 даже в compact header (v0.9 §171).
        spans.push(Span::raw("  "));
        spans.push(Span::styled(app.title(), theme.dim()));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Футер по пресетам раздела 105.
fn render_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    plan: &LayoutPlan,
    theme: &Theme,
    icons: pulse_core::config::IconSet,
) {
    let line = ui::footer_line(plan.footer, area.width, theme.capability, icons);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(line, theme.dim()))),
        area,
    );
}

/// Full-width правило под шапкой (§190 и эскизы 5732, 5775, 5818, 5898, 5947).
///
/// Единственный разделитель, который законно занимает всю ширину терминала:
/// он принадлежит экрану, а не bounded-панели, поэтому §188/§189 к нему
/// не применяются.
fn render_header_rule(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let dash = if matches!(theme.capability, crate::theme::Capability::Ascii) {
        '-'
    } else {
        '─'
    };
    let rule: String = std::iter::repeat_n(dash, usize::from(area.width)).collect();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(rule, theme.dim()))),
        area,
    );
}

/// Заголовок раздела по правилам 134: `TITLE ─────`.
pub(crate) fn section<'a>(
    title: &str,
    width: u16,
    plan: &LayoutPlan,
    theme: &'a Theme,
) -> Line<'a> {
    let text = ui::section_title(title, width, plan.section_rules, theme.capability);
    Line::from(Span::styled(text, theme.dim()))
}

/// Section title показывает keyboard focus border-accent'ом (§171).
///
/// Маркер `>` здесь запрещён: он зарезервирован за выбранной строкой (§136),
/// и второй `>` на заголовке панели читался бы как ещё одна selection.
pub(crate) fn focused_section<'a>(
    title: &str,
    width: u16,
    focused: bool,
    plan: &LayoutPlan,
    theme: &'a Theme,
) -> Line<'a> {
    let ascii = matches!(theme.capability, crate::theme::Capability::Ascii);
    let accent = match (focused, ascii) {
        (true, true) => "| ",
        (true, false) => "▌ ",
        (false, _) => "  ",
    };
    let content_width = width.saturating_sub(2);
    let text = ui::section_title(title, content_width, plan.section_rules, theme.capability);
    Line::from(vec![
        Span::styled(accent, if focused { theme.strong() } else { theme.dim() }),
        Span::styled(text, if focused { theme.strong() } else { theme.dim() }),
    ])
}

/// Полоса текущих сигналов (разделы 155, 156).
///
/// Показываются давление, утилизация и скорости - то, что описывает систему
/// сейчас. Накопительные счётчики сюда не попадают: их место в Inspect.
/// Направление трафика обозначено стрелками, в ASCII-режиме - метками RX/TX.
pub(crate) fn vitals_line<'a>(snapshot: &Snapshot, theme: &'a Theme) -> Line<'a> {
    let host = snapshot.host;
    let cpu = snapshot.value_or(host, ids::HOST_CPU_UTIL, 0.0);
    let memory = snapshot.value_or(host, ids::HOST_MEM_UTIL, 0.0);
    let psi_mem = snapshot.value_or(host, ids::HOST_PSI_MEM_FULL_AVG10, 0.0);
    let io_wait = snapshot.value_or(host, ids::HOST_CPU_IOWAIT, 0.0);
    // Серии host, а не интерфейса: `NETIF_*_THROUGHPUT` на host не существует,
    // и раньше `value_or(..., 0.0)` печатал уверенный ноль вместо измерения.
    let rx = snapshot.value(host, ids::HOST_NET_RX_THROUGHPUT);
    let tx = snapshot.value(host, ids::HOST_NET_TX_THROUGHPUT);
    let disk_read = snapshot.value(host, ids::HOST_DISK_READ_THROUGHPUT);
    let disk_write = snapshot.value(host, ids::HOST_DISK_WRITE_THROUGHPUT);
    // Кратко и только значимое: корень плюс худшая из значимых ФС, если она
    // не корень. Сотни overlay-точек Docker отфильтрованы в коллекторе.
    let fs_root = snapshot.value(host, ids::HOST_FS_ROOT_UTIL);
    let fs_worst = snapshot.value(host, ids::HOST_FS_WORST_UTIL);
    let ascii = matches!(theme.capability, crate::theme::Capability::Ascii);

    let mut spans = vec![
        Span::styled("CPU ", theme.dim()),
        Span::styled(format::percent(cpu), theme.ratio(cpu)),
        Span::raw("    "),
        Span::styled("MEM ", theme.dim()),
        Span::styled(format::percent(memory), theme.ratio(memory)),
        Span::raw("    "),
        Span::styled("PSI MEM ", theme.dim()),
        Span::styled(format::percent(psi_mem), theme.ratio(psi_mem * 8.0)),
        Span::raw("    "),
        Span::styled("IO WAIT ", theme.dim()),
        Span::styled(format::percent(io_wait), theme.ratio(io_wait * 4.0)),
        Span::raw("    "),
        Span::styled("NET ", theme.dim()),
        Span::styled(format::rx_tx(rx, tx, ascii), theme.text()),
        Span::raw("    "),
        Span::styled("DISK ", theme.dim()),
        Span::styled(format::disk_rw(disk_read, disk_write, ascii), theme.text()),
    ];
    if let Some(root) = fs_root {
        spans.push(Span::raw("    "));
        spans.push(Span::styled("/ ", theme.dim()));
        spans.push(Span::styled(format::fs_percent(root), theme.ratio(root)));
        // Худшая ФС показывается только если она не корень: иначе строка
        // повторяет одно и то же число дважды.
        if let Some(worst) = fs_worst.filter(|worst| *worst > root + 0.01) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("worst fs ", theme.dim()));
            spans.push(Span::styled(format::fs_percent(worst), theme.ratio(worst)));
        }
    }
    Line::from(spans)
}

/// Таблица логических сущностей (разделы 94, 95, 136, 144, 148).
///
/// Ширина ограничена полезным пределом: остаток отдаётся вызывающему, который
/// решает, показать рядом контекст или оставить пустоту (раздел 144). Строка
/// показывает логический объект, а его состав - в колонке вида (`unit+12p`).
pub(crate) fn render_logical_table(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[LogicalRow],
    selected: usize,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let columns = table::entity_columns();
    let marker = u16::try_from(ui::SELECTED.len()).unwrap_or(2);
    let plan = table::plan(&columns, area.width.saturating_sub(marker));

    let mut lines: Vec<Line<'_>> = Vec::with_capacity(usize::from(area.height));

    let header: String = plan
        .visible
        .iter()
        .zip(plan.widths.iter())
        .filter_map(|(index, width)| {
            let column = columns.get(*index)?;
            Some(table::cell(
                plan.title(column),
                *width,
                column.align,
                theme.capability,
            ))
        })
        .collect::<Vec<_>>()
        .join("  ");
    lines.push(Line::from(vec![
        Span::raw(ui::UNSELECTED),
        Span::styled(header, theme.dim()),
    ]));

    let visible = usize::from(area.height).saturating_sub(1).max(1);
    let start = selected.saturating_sub(visible.saturating_sub(1));

    for (index, logical) in rows.iter().enumerate().skip(start).take(visible) {
        let row = &logical.row;
        let is_selected = index == selected;
        let mut spans = vec![Span::styled(
            ui::row_marker(is_selected),
            if is_selected {
                theme.selection()
            } else {
                theme.dim()
            },
        )];
        for (position, (column_index, width)) in
            plan.visible.iter().zip(plan.widths.iter()).enumerate()
        {
            if position > 0 {
                spans.push(Span::raw("  "));
            }
            let Some(column) = columns.get(*column_index) else {
                continue;
            };
            let (text, style) = match column.title {
                "NAME" => (row.name.clone(), theme.text()),
                "STATE" => (
                    row.state.symbol(theme.capability).to_string(),
                    row.state.style(theme),
                ),
                "CPU" => (format::cores(row.cpu), theme.text()),
                "MEM" => (format::bytes(row.memory), theme.text()),
                // Вид несёт состав объекта: `unit+12p` честнее, чем `unit`.
                "KIND" => (logical.kind_label(), theme.dim()),
                "OWNER" => (
                    if row.owner.is_empty() {
                        no_owner(theme).to_string()
                    } else {
                        row.owner.clone()
                    },
                    theme.dim(),
                ),
                // Скорость печатается только там, где она измерена.
                "IO" => (
                    row.io
                        .map_or_else(|| no_owner(theme).to_string(), format::rate),
                    theme.dim(),
                ),
                "NET" => (
                    row.net
                        .map_or_else(|| no_owner(theme).to_string(), format::rate),
                    theme.dim(),
                ),
                _ => (String::new(), theme.dim()),
            };
            spans.push(Span::styled(
                table::cell(&text, *width, column.align, theme.capability),
                if is_selected {
                    theme.selection()
                } else {
                    style
                },
            ));
        }
        lines.push(Line::from(spans));

        // Ненормальная часть внутри свёрнутого объекта обязана быть названа:
        // иначе свёртка скрыла бы проблему (раздел 150).
        if logical.abnormal > 0 && lines.len() < usize::from(area.height) {
            lines.push(Line::from(vec![
                Span::raw(ui::UNSELECTED),
                Span::styled(
                    format!("  {} abnormal", logical.abnormal),
                    theme.severity(pulse_core::problem::Severity::Warn),
                ),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Сколько времени PULSE наблюдает этот хост.
///
/// Хост создаётся в момент подключения наблюдателя, поэтому его `first_seen` -
/// точное начало наблюдения. Это единственная величина, которой PULSE вправе
/// подкреплять утверждения вида «проблем не было»: аптайм хоста относится ко
/// времени, когда нас ещё не было, и подменять одно другим - ложь в интерфейсе.
pub(crate) fn observation(snapshot: &Snapshot) -> std::time::Duration {
    let started = snapshot
        .entity(snapshot.host)
        .map_or(snapshot.at, |host| host.first_seen);
    std::time::Duration::from_millis(snapshot.at.as_millis().saturating_sub(started.as_millis()))
}

/// Заглушка отсутствующего владельца.
///
/// В ASCII-режиме тире `—` недопустимо: старый терминал покажет мусор.
pub(crate) const fn no_owner(theme: &Theme) -> &'static str {
    if matches!(theme.capability, crate::theme::Capability::Ascii) {
        "-"
    } else {
        "—"
    }
}

/// Безопасный доступ к прямоугольнику раскладки.
///
/// `Layout::split` возвращает столько областей, сколько задано ограничений, но
/// индексация в рабочем коде запрещена: при нехватке места лучше не нарисовать
/// блок, чем уронить процесс наблюдения. Пустая область в ratatui - no-op.
pub(crate) fn rect(list: &[Rect], index: usize) -> Rect {
    list.get(index).copied().unwrap_or_default()
}
