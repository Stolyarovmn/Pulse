//! Инспектор как дело расследования.
//!
//! Экран отвечает на вопросы следователя, и каждый блок — ответ на один:
//!
//! * **TRAIL** — где я и как сюда пришёл: пронумерованный след, текущий шаг
//!   выделен цветом места;
//! * **CASE** — что здесь не так: открытые проблемы объекта с
//!   доказательствами и формой величины за минуту;
//! * **LEADS** — где интересное: измеренные факты о конкретных объектах,
//!   по которым можно перейти (`crate::leads`);
//! * **INSIDE** — из чего объект состоит: каждый логический объект отдельной
//!   строкой, сначала ненормальные, затем нагруженные, с долей CPU;
//! * **AROUND** — кто рядом: родитель, активные соседи по диску, ресурсы;
//! * **DOSSIER** — кто это: вид, путь, доли хоста, лимиты, для процесса —
//!   источник запуска и детали;
//! * **RECENT** — что с ним происходило.
//!
//! Три списка (INSIDE, LEADS, AROUND) выбираемы; `Tab` передаёт им `Enter`
//! по кругу, активный выделен цветом места и маркером `▌`. На широком
//! терминале дело раскладывается в три колонки, на среднем — в две, на узком
//! идёт стопкой: сначала беда и зацепки, потом всё остальное.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use pulse_core::entity::{Entity, EntityKind};
use pulse_core::launch::{process_ancestry, AncestryEnd};
use pulse_core::metric::ids;
use pulse_core::problem::Problem;
use pulse_core::snapshot::Snapshot;
use pulse_core::time::format_duration;

use crate::app::{App, InspectorList};
use crate::investigate::{Descent, Step};
use crate::layout::LayoutPlan;
use crate::leads::Lead;
use crate::state::StateClass;
use crate::theme::{Capability, Theme};
use crate::ui;

/// Ширина подписи в досье: `CPU limit` — самая длинная.
const LABEL: usize = 10;

/// Ширина, с которой дело раскладывается в три колонки.
const THREE_COLUMNS: u16 = 150;

/// Ширина, с которой дело раскладывается в две колонки.
const TWO_COLUMNS: u16 = 100;

/// Всё, что нужно блокам дела. Отдельная структура вместо восьми аргументов.
struct Case<'a> {
    snapshot: &'a Snapshot,
    entity: &'a Entity,
    theme: &'a Theme,
    plan: &'a LayoutPlan,
    history: Option<&'a crate::series::SeriesReader<'a>>,
    focus: InspectorList,
    selected: usize,
}

impl Case<'_> {
    fn ascii(&self) -> bool {
        matches!(self.theme.capability, Capability::Ascii)
    }

    /// Выбран ли элемент `index` списка `list`.
    fn is_selected(&self, list: InspectorList, index: usize) -> bool {
        self.focus == list && self.selected == index
    }
}

/// Отрисовка инспектора.
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &mut App,
    plan: &LayoutPlan,
    theme: &Theme,
    history: Option<&crate::series::SeriesReader<'_>>,
) {
    // Inspector существует только как contextual session; normal empty page
    // удалён разделом 168. Если target исчез между тактами, экран просто не
    // создаёт невидимую actionable selection.
    let Some(entity) = app.inspector_entity(snapshot) else {
        return;
    };

    let inside = app.inspector_relations(snapshot);
    let leads = app.inspector_leads(snapshot);
    let around = app.inspector_side_steps(snapshot);
    let trail = trail_names(snapshot, app);
    let focus = app
        .inspector
        .as_ref()
        .map_or(InspectorList::Chain, |session| session.list);
    let keys: Vec<pulse_core::EntityKey> = match focus {
        InspectorList::Chain => inside.iter().map(|row| row.key.clone()).collect(),
        InspectorList::Leads => leads.iter().map(|lead| lead.key.clone()).collect(),
        InspectorList::Related => around.iter().map(|step| step.key.clone()).collect(),
    };
    let focus_len = keys.len();
    let mut raw_selected = 0;
    if let Some(session) = &mut app.inspector {
        // Курсор держится за объект, а не за номер строки: список
        // пересортировался за такт — выбранный объект остался выбранным.
        if let Some(position) = session
            .shown
            .get(session.relation_selected)
            .and_then(|previous| keys.iter().position(|key| key == previous))
        {
            session.relation_selected = position;
        }
        // Длина активного списка: по нему двигается курсор. Ключи — то, что
        // откроет `Enter`.
        session.relation_len = focus_len;
        session.shown = keys;
        raw_selected = session.relation_selected;
    }
    let details = process_details(app, snapshot, entity);

    let case = Case {
        snapshot,
        entity,
        theme,
        plan,
        history,
        focus,
        selected: raw_selected.min(focus_len.saturating_sub(1)),
    };

    let problems = case_problems(snapshot, entity);
    let header = header_lines(&case, &trail, &problems, leads.len(), area.width);
    let header_height = u16::try_from(header.len()).unwrap_or(u16::MAX);
    let [head, body] = split_vertical(area, header_height);
    frame.render_widget(Paragraph::new(header), head);

    let columns = if area.width >= THREE_COLUMNS {
        3
    } else if area.width >= TWO_COLUMNS {
        2
    } else {
        1
    };
    let widths = column_rects(body, columns);

    // Колонки и их блоки. Порядок в стопке — порядок расследования.
    let dossier = |width: u16| dossier_block(&case, details.as_ref(), width);
    let case_block = |width: u16| case_block(&case, &problems, width);
    let leads_block = |width: u16| leads_block(&case, &leads, width);
    let around_block = |width: u16| around_block(&case, &around, width);
    let recent = |width: u16| recent_block(&case, width);

    match widths.as_slice() {
        [left, middle, right] => {
            render_column(frame, *left, vec![dossier(left.width), recent(left.width)]);
            let around_lines = around_block(middle.width);
            let room = usize::from(middle.height).saturating_sub(around_lines.len() + 1);
            render_column(
                frame,
                *middle,
                vec![
                    inside_block(&case, &inside, middle.width, room),
                    around_lines,
                ],
            );
            render_column(
                frame,
                *right,
                vec![case_block(right.width), leads_block(right.width)],
            );
        }
        [left, right] => {
            render_column(
                frame,
                *left,
                vec![
                    case_block(left.width),
                    leads_block(left.width),
                    dossier(left.width),
                ],
            );
            let around_lines = around_block(right.width);
            let recent_lines = recent(right.width);
            let room = usize::from(right.height)
                .saturating_sub(around_lines.len() + recent_lines.len() + 2);
            render_column(
                frame,
                *right,
                vec![
                    inside_block(&case, &inside, right.width, room),
                    around_lines,
                    recent_lines,
                ],
            );
        }
        [single] => {
            let width = single.width;
            render_column(
                frame,
                *single,
                vec![
                    case_block(width),
                    leads_block(width),
                    inside_block(&case, &inside, width, 10),
                    around_block(width),
                    dossier(width),
                    recent(width),
                ],
            );
        }
        _ => {}
    }
}

/// Шапка и тело по вертикали.
fn split_vertical(area: Rect, head: u16) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(head), Constraint::Min(0)])
        .split(area);
    [super::rect(&chunks, 0), super::rect(&chunks, 1)]
}

/// Прямоугольники колонок с промежутком в два символа.
fn column_rects(body: Rect, columns: usize) -> Vec<Rect> {
    const GAP: u16 = 2;
    let constraints: Vec<Constraint> = match columns {
        3 => vec![
            Constraint::Ratio(1, 3),
            Constraint::Length(GAP),
            Constraint::Ratio(1, 3),
            Constraint::Length(GAP),
            Constraint::Ratio(1, 3),
        ],
        2 => vec![
            Constraint::Ratio(1, 2),
            Constraint::Length(GAP),
            Constraint::Ratio(1, 2),
        ],
        _ => return vec![body],
    };
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(body);
    chunks.iter().step_by(2).copied().collect()
}

/// Колонка: блоки через пустую строку, пустые блоки пропускаются.
fn render_column(frame: &mut Frame<'_>, area: Rect, blocks: Vec<Vec<Line<'static>>>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for block in blocks.into_iter().filter(|block| !block.is_empty()) {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.extend(block);
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Имена шагов следа по canonical key.
fn trail_names(snapshot: &Snapshot, app: &App) -> Vec<String> {
    app.inspector
        .as_ref()
        .map(|session| {
            session
                .path
                .iter()
                .filter_map(|key| {
                    snapshot
                        .entities
                        .iter()
                        .find(|candidate| &candidate.key == key)
                        .map(|candidate| crate::fold::display_name(snapshot, candidate))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Открытые проблемы объекта и его двойника (cgroup ↔ владелец).
fn case_problems<'a>(snapshot: &'a Snapshot, entity: &Entity) -> Vec<&'a Problem> {
    let twins = crate::leads::twins(snapshot, entity.id);
    let mut problems: Vec<&Problem> = snapshot
        .problems
        .iter()
        .filter(|problem| twins.contains(&problem.id.entity))
        .collect();
    problems.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.title.cmp(&b.title)));
    problems
}

/// Шапка дела: след, имя с состоянием и вердикт справа.
fn header_lines(
    case: &Case<'_>,
    trail: &[String],
    problems: &[&Problem],
    leads: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let theme = case.theme;
    let width = usize::from(width);
    let separator = if case.ascii() { " > " } else { " › " };

    // След: номер шага и имя, текущий шаг — цветом места. Не помещается —
    // первые шаги уходят под многоточие, текущий виден всегда.
    let label = "TRAIL  ";
    let steps: Vec<String> = trail
        .iter()
        .enumerate()
        .map(|(index, name)| format!("{} {name}", index + 1))
        .collect();
    let mut first = 0;
    let used = |from: usize| -> usize {
        steps
            .iter()
            .skip(from)
            .map(|step| ui::width_of(step))
            .sum::<usize>()
            + ui::width_of(separator) * steps.len().saturating_sub(from + 1)
            + ui::width_of(label)
            + if from > 0 { 4 } else { 0 }
    };
    while first + 1 < steps.len() && used(first) > width {
        first += 1;
    }
    let mut trail_spans = vec![Span::styled(label, theme.dim())];
    if first > 0 {
        trail_spans.push(Span::styled(
            if case.ascii() { "... " } else { "… " },
            theme.faint(),
        ));
    }
    let last = steps.len().saturating_sub(1);
    for (index, step) in steps.iter().enumerate().skip(first) {
        if index > first {
            trail_spans.push(Span::styled(separator, theme.faint()));
        }
        let style = if index == last {
            theme.accent()
        } else {
            theme.dim()
        };
        trail_spans.push(Span::styled(
            ui::truncate(
                step,
                width.saturating_sub(ui::width_of(label)),
                theme.capability,
            ),
            style,
        ));
    }

    // Имя и состояние слева, вердикт справа.
    let severity = problems.iter().map(|problem| problem.severity).max();
    let state = crate::rows::state_of(case.snapshot, case.entity, severity);
    let name = crate::fold::display_name(case.snapshot, case.entity);
    let left = vec![
        Span::styled(format!("{}  ", case.entity.kind.as_str()), theme.dim()),
        Span::styled(name.clone(), theme.strong()),
        Span::raw("  "),
        Span::styled(
            state.symbol(theme.capability).to_string(),
            state.style(theme),
        ),
        Span::styled(format!(" {}", state.label()), theme.dim()),
    ];
    let mut right: Vec<Span<'static>> = Vec::new();
    match severity {
        Some(severity) => {
            let class = StateClass::from_severity(severity);
            let count = problems.len();
            right.push(Span::styled(
                format!(
                    "{} {count} open problem{}",
                    class.symbol(theme.capability),
                    if count == 1 { "" } else { "s" }
                ),
                theme.severity(severity),
            ));
        }
        None => right.push(Span::styled(
            format!("{} nothing open here", check(case.ascii())),
            theme.nominal(),
        )),
    }
    if leads > 0 {
        right.push(Span::styled(
            format!("  ·  {leads} lead{}", if leads == 1 { "" } else { "s" }),
            theme.dim(),
        ));
    }
    let left_width: usize = left.iter().map(|span| ui::width_of(&span.content)).sum();
    let right_width: usize = right.iter().map(|span| ui::width_of(&span.content)).sum();
    let mut title = left;
    if left_width + right_width + 2 <= width {
        title.push(Span::raw(" ".repeat(width - left_width - right_width)));
        title.extend(right);
    }

    vec![Line::from(trail_spans), Line::from(title), Line::from("")]
}

/// Галочка «всё в порядке».
const fn check(ascii: bool) -> &'static str {
    if ascii {
        "+"
    } else {
        "✓"
    }
}

/// Заголовок блока. Выбираемый блок подсказывает клавишу, активный выделен
/// цветом места и маркером `▌`.
fn heading(case: &Case<'_>, title: &str, list: Option<InspectorList>, width: u16) -> Line<'static> {
    let focused = list == Some(case.focus);
    let hint = match list {
        Some(_) if focused => "  enter",
        Some(_) => "  tab",
        None => "",
    };
    let text = ui::section_title(
        &format!("{title}{hint}"),
        width.saturating_sub(2),
        case.plan.section_rules,
        case.theme.capability,
    );
    let bar = match (focused, case.ascii()) {
        (true, true) => "| ",
        (true, false) => "▌ ",
        (false, _) => "  ",
    };
    let style = if focused {
        case.theme.accent()
    } else {
        case.theme.dim()
    };
    Line::from(vec![Span::styled(bar, style), Span::styled(text, style)])
}

/// Строка списка: выбранная получает маркер и фон места на всю ширину.
///
/// Цвет символов состояния сохраняется: меняется только фон, иначе выбор
/// стирал бы severity строки, на которой оператор стоит.
fn list_line(
    spans: Vec<Span<'static>>,
    selected: bool,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let mut out = vec![Span::styled(
        ui::row_marker(selected),
        if selected {
            theme.selection()
        } else {
            theme.dim()
        },
    )];
    out.extend(spans);
    if selected {
        let background = theme.selection().bg;
        let used: usize = out.iter().map(|span| ui::width_of(&span.content)).sum();
        for span in &mut out {
            span.style = Style {
                bg: background,
                ..span.style
            };
        }
        out.push(Span::styled(
            " ".repeat(usize::from(width).saturating_sub(used)),
            theme.selection(),
        ));
    }
    Line::from(out)
}

/// Полоса доли: заполненное — текстом, пустое — самым тихим слоем.
fn share_bar(ratio: f64, cells: usize, theme: &Theme) -> Vec<Span<'static>> {
    let clamped = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = ((clamped * cells as f64).round() as usize).min(cells);
    vec![
        Span::styled(
            theme.glyphs.bar_full.to_string().repeat(filled),
            theme.text(),
        ),
        // Пустое — тихой точкой, а не `░`: у двухсот объектов с нулевой долей
        // штриховка складывалась в серую стену, а ноль должен быть тишиной.
        Span::styled(
            if matches!(theme.capability, Capability::Ascii) {
                ".".repeat(cells - filled)
            } else {
                "·".repeat(cells - filled)
            },
            theme.faint(),
        ),
    ]
}

/// CASE: что здесь не так.
fn case_block(case: &Case<'_>, problems: &[&Problem], width: u16) -> Vec<Line<'static>> {
    let theme = case.theme;
    let mut lines = vec![heading(case, "CASE", None, width)];
    if problems.is_empty() {
        let observed = format_duration(super::observation(case.snapshot));
        lines.push(Line::from(vec![
            Span::raw(ui::UNSELECTED),
            Span::styled(
                format!("{} nothing open here", check(case.ascii())),
                theme.nominal(),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  no rule fired in {observed} of observation"),
            theme.dim(),
        )));
        return lines;
    }
    let source = super::overview::TrendSource {
        snapshot: case.snapshot,
        history: case.history,
    };
    let text_width = usize::from(width).saturating_sub(4);
    for problem in problems {
        let class = StateClass::from_severity(problem.severity);
        let held = format_duration(std::time::Duration::from_millis(
            case.snapshot
                .at
                .as_millis()
                .saturating_sub(problem.since.as_millis()),
        ));
        lines.push(Line::from(vec![
            Span::raw(ui::UNSELECTED),
            Span::styled(
                format!("{} ", class.symbol(theme.capability)),
                class.style(theme),
            ),
            Span::styled(
                ui::truncate(&problem.title, text_width, theme.capability),
                theme.severity(problem.severity),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!(
                "    held {held} · {} consecutive ticks · {}",
                problem.streak, problem.entity_name
            ),
            theme.dim(),
        )));
        // Ширина подписи — по самой длинной у этой проблемы плюс зазор:
        // фиксированная колонка склеивала «PSI memory full» со значением.
        let label_width = problem
            .evidence
            .iter()
            .map(|evidence| ui::width_of(&evidence.label))
            .max()
            .unwrap_or(0)
            + 2;
        for evidence in &problem.evidence {
            let mut spans = vec![
                Span::styled(format!("    {:<label_width$}", evidence.label), theme.dim()),
                Span::styled(evidence.value.clone(), theme.text()),
            ];
            if let Some(threshold) = &evidence.threshold {
                spans.push(Span::styled(format!("  {threshold}"), theme.dim()));
            }
            lines.push(Line::from(spans));
            if let Some(trend) = super::problems::evidence_trend(&source, evidence, width, theme) {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(trend.spans);
                lines.push(Line::from(spans));
            }
        }
    }
    lines
}

/// LEADS: где интересное. Каждая зацепка — имя цели и измеренный факт.
fn leads_block(case: &Case<'_>, leads: &[Lead], width: u16) -> Vec<Line<'static>> {
    let theme = case.theme;
    let mut lines = vec![heading(case, "LEADS", Some(InspectorList::Leads), width)];
    if leads.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no leads: nothing inside stands out",
            theme.dim(),
        )));
        return lines;
    }
    let text_width = usize::from(width).saturating_sub(8);
    for (index, lead) in leads.iter().enumerate() {
        let selected = case.is_selected(InspectorList::Leads, index);
        lines.push(list_line(
            vec![
                Span::styled(format!("{} ", index + 1), theme.dim()),
                Span::styled(
                    format!("{} ", lead.class.symbol(theme.capability)),
                    lead.class.style(theme),
                ),
                Span::styled(
                    ui::truncate(&lead.name, text_width, theme.capability),
                    theme.strong(),
                ),
            ],
            selected,
            width,
            theme,
        ));
        lines.push(list_line(
            vec![Span::styled(
                format!(
                    "    {}",
                    ui::truncate(&lead.reason, text_width, theme.capability)
                ),
                theme.dim(),
            )],
            selected,
            width,
            theme,
        ));
    }
    lines
}

/// INSIDE: из чего объект состоит.
///
/// Окно списка следует за курсором: у `system.slice` двести объектов, и
/// выбранный обязан быть виден. Полоса — доля CPU объекта, которую несёт
/// строка: кто здесь главный, видно до чтения чисел.
fn inside_block(case: &Case<'_>, rows: &[Descent], width: u16, room: usize) -> Vec<Line<'static>> {
    let theme = case.theme;
    let title = if rows.is_empty() {
        "INSIDE".to_string()
    } else {
        format!("INSIDE ({})", rows.len())
    };
    let mut lines = vec![heading(case, &title, Some(InspectorList::Chain), width)];
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  nothing inside: the chain ends here",
            theme.dim(),
        )));
        return lines;
    }

    let total = measured_row(case.snapshot, case.entity);
    // Знаменатель — большее из CPU объекта и суммы частей. CPU cgroup и CPU
    // процессов меряются разными выборками, и за такт сумма частей бывает
    // больше целого: на кадре `pulse` показывал 100 % при 0.02c из 0.01c.
    // Доля обязана оставаться долей.
    let parts: f64 = rows.iter().map(|row| row.cpu).sum();
    let whole = total.cpu.max(parts);
    let visible = room.saturating_sub(2).max(3);
    let cursor = if case.focus == InspectorList::Chain {
        case.selected
    } else {
        0
    };
    let start = cursor.saturating_sub(visible.saturating_sub(1));
    let wide = width >= 70;
    let fixed = 2 + 2 + 7 + 7 + 10 + if wide { 13 } else { 0 };
    let name_width = usize::from(width).saturating_sub(fixed).max(8);

    if start > 0 {
        lines.push(Line::from(Span::styled(
            format!("  {} {start} above", up_arrow(case.ascii())),
            theme.faint(),
        )));
    }
    for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
        let mut spans = vec![
            Span::styled(
                format!("{} ", row.state.symbol(theme.capability)),
                row.state.style(theme),
            ),
            Span::styled(
                format!(
                    "{:<name_width$}",
                    ui::truncate(&row.name, name_width, theme.capability)
                ),
                theme.text(),
            ),
        ];
        if wide {
            spans.push(Span::styled(
                format!(
                    " {:<12}",
                    ui::truncate(&row.kind_label, 12, theme.capability)
                ),
                theme.dim(),
            ));
        }
        spans.push(Span::styled(
            format!(" {:>6}", crate::format::cores(row.cpu)),
            theme.text(),
        ));
        spans.push(Span::raw(" "));
        if whole > 0.0 {
            spans.extend(share_bar(row.cpu / whole, 5, theme));
        } else {
            spans.push(Span::raw("     "));
        }
        spans.push(Span::styled(
            format!(
                " {:>9}",
                row.memory
                    .map_or_else(|| super::no_owner(theme).to_string(), crate::format::bytes)
            ),
            theme.dim(),
        ));
        lines.push(list_line(
            spans,
            case.is_selected(InspectorList::Chain, index),
            width,
            theme,
        ));
    }
    let below = rows.len().saturating_sub(start + visible);
    if below > 0 {
        lines.push(Line::from(Span::styled(
            format!("  {} {below} more", down_arrow(case.ascii())),
            theme.faint(),
        )));
    }
    lines
}

const fn up_arrow(ascii: bool) -> &'static str {
    if ascii {
        "^"
    } else {
        "↑"
    }
}

const fn down_arrow(ascii: bool) -> &'static str {
    if ascii {
        "v"
    } else {
        "↓"
    }
}

/// AROUND: кто рядом — переходы в сторону и ресурсы.
fn around_block(case: &Case<'_>, steps: &[Step], width: u16) -> Vec<Line<'static>> {
    let theme = case.theme;
    let snapshot = case.snapshot;
    let resources = crate::investigate::context_resources(snapshot, case.entity.id);
    if steps.is_empty() && resources.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![heading(case, "AROUND", Some(InspectorList::Related), width)];
    let name_width = usize::from(width).saturating_sub(14);
    let state_of_key = |key: &pulse_core::EntityKey| {
        snapshot
            .entities
            .iter()
            .find(|candidate| &candidate.key == key)
            .map(|target| {
                let severity = snapshot.problems_of(target.id).map(|p| p.severity).max();
                crate::rows::state_of(snapshot, target, severity)
            })
    };
    for (index, step) in steps.iter().enumerate() {
        let mut spans = vec![
            Span::styled(format!("{:<11}", step.label), theme.dim()),
            Span::styled(
                ui::truncate(&step.name, name_width, theme.capability),
                theme.text(),
            ),
        ];
        if let Some(state) = state_of_key(&step.key) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                state.symbol(theme.capability).to_string(),
                state.style(theme),
            ));
        }
        lines.push(list_line(
            spans,
            case.is_selected(InspectorList::Related, index),
            width,
            theme,
        ));
    }
    // Ресурсы — контекст, а не звенья: диском пользуются десятки несвязанных
    // сервисов, и переход в него делал бы из ресурса пересадочный узел.
    if !resources.is_empty() {
        let mut spans = vec![Span::styled(format!("  {:<11}", "resources"), theme.dim())];
        for (index, resource) in resources.iter().take(8).enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(resource.name.clone(), theme.text()));
            if let Some(state) = state_of_key(&resource.key) {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    state.symbol(theme.capability).to_string(),
                    state.style(theme),
                ));
            }
        }
        if resources.len() > 8 {
            spans.push(Span::styled(
                format!("  +{}", resources.len() - 8),
                theme.dim(),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Пара «подпись — значение» досье.
fn fact(label: &str, value: Vec<Span<'static>>, theme: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("  {label:<LABEL$}"), theme.dim())];
    spans.extend(value);
    Line::from(spans)
}

/// DOSSIER: кто это.
fn dossier_block(
    case: &Case<'_>,
    details: Option<&pulse_core::ProcessDetails>,
    width: u16,
) -> Vec<Line<'static>> {
    let theme = case.theme;
    let snapshot = case.snapshot;
    let entity = case.entity;
    let value_width = usize::from(width).saturating_sub(LABEL + 3);
    let text = |value: String| vec![Span::styled(value, theme.text())];
    let mut lines = vec![heading(case, "DOSSIER", None, width)];

    lines.push(fact("kind", text(entity.kind.as_str().to_string()), theme));
    if let Some(path) = entity.labels.get("path") {
        lines.push(fact(
            "path",
            text(ui::truncate(path, value_width, theme.capability)),
            theme,
        ));
    }
    let row = measured_row(snapshot, entity);
    if !row.owner.is_empty() {
        lines.push(fact("owner", text(row.owner.clone()), theme));
    }
    if let Some(cmdline) = entity.labels.get("cmdline") {
        lines.push(fact(
            "cmd",
            text(ui::truncate(cmdline, value_width, theme.capability)),
            theme,
        ));
    }

    // Доли хоста: число без знаменателя не говорит, много это или мало.
    let host = snapshot.host;
    let cpus = snapshot
        .value(host, ids::HOST_CPU_COUNT)
        .filter(|v| *v > 0.0);
    let mut cpu = vec![Span::styled(
        format!("{:<9}", crate::format::cores(row.cpu)),
        theme.text(),
    )];
    if let Some(cpus) = cpus {
        cpu.extend(share_bar(row.cpu / cpus, 8, theme));
        cpu.push(Span::styled(
            format!(" {:.0}% of {cpus:.0} cores", row.cpu / cpus * 100.0),
            theme.dim(),
        ));
    }
    lines.push(fact("CPU", cpu, theme));

    let source = super::overview::TrendSource {
        snapshot,
        history: case.history,
    };
    let key = pulse_core::sample::SeriesKey::new(entity.id, crate::rows::cpu_metric(entity.kind));
    if let Some(found) = source.history.and_then(|_| {
        crate::trend::of_series(source.history, snapshot, key, 16, theme)
            .filter(crate::trend::Trend::is_measured)
    }) {
        lines.push(fact(
            "CPU 60s",
            vec![
                Span::styled(found.lane, theme.text()),
                Span::styled(
                    format!(
                        " peak {} avg {}",
                        crate::format::cores(found.peak),
                        crate::format::cores(found.mean)
                    ),
                    theme.dim(),
                ),
            ],
            theme,
        ));
    }

    let mut memory = vec![Span::styled(
        format!("{:<9}", crate::rows::memory_text(&row, "—")),
        theme.text(),
    )];
    let total = snapshot
        .value(host, ids::HOST_MEM_TOTAL)
        .filter(|v| *v > 0.0);
    if let (true, Some(total)) = (row.memory_measured, total) {
        memory.extend(share_bar(row.memory / total, 8, theme));
        memory.push(Span::styled(
            format!(" {:.0}% of host", row.memory / total * 100.0),
            theme.dim(),
        ));
    } else if !row.memory_measured {
        memory = text("not measured".to_string());
    }
    lines.push(fact("MEM", memory, theme));

    if let Some(io) = row.io.filter(|io| *io > 0.0) {
        lines.push(fact("IO", text(crate::format::rate(io)), theme));
    }
    for (label, value) in limits(snapshot, entity) {
        lines.push(fact(label, text(value), theme));
    }

    if entity.kind == EntityKind::Process {
        lines.push(Line::from(""));
        lines.extend(why_it_exists(case, width));
    }
    if let Some(details) = details {
        lines.push(Line::from(""));
        lines.extend(process_block(case, details, width));
    }
    lines
}

/// Строка величин объекта; у владельца без своих серий — величины его cgroup.
///
/// Коллектор дублирует метрики cgroup на владельца, но при отброшенном по
/// лимиту серий сэмпле у unit их нет, и досье печатало `0.00c` у сервиса,
/// чья cgroup ест четверть ядра. Число обязано описывать тот же объект, что
/// и INSIDE, поэтому источник — тот же корень поддерева.
fn measured_row(snapshot: &Snapshot, entity: &Entity) -> crate::rows::EntityRow {
    let own = crate::rows::row_of(snapshot, entity);
    let root = crate::leads::subtree_root(snapshot, entity);
    let measured = snapshot
        .value(entity.id, crate::rows::cpu_metric(entity.kind))
        .is_some();
    match snapshot.entity(root) {
        Some(backing) if root != entity.id && !measured => {
            let backing = crate::rows::row_of(snapshot, backing);
            crate::rows::EntityRow {
                cpu: backing.cpu,
                memory: if own.memory_measured {
                    own.memory
                } else {
                    backing.memory
                },
                memory_measured: own.memory_measured || backing.memory_measured,
                io: own.io.or(backing.io),
                net: own.net.or(backing.net),
                ..own
            }
        }
        _ => own,
    }
}

/// Лимиты и давление: только осмысленные для этого вида величины.
fn limits(snapshot: &Snapshot, entity: &Entity) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if matches!(
        entity.kind,
        EntityKind::Cgroup | EntityKind::Unit | EntityKind::Container | EntityKind::Pod
    ) {
        rows.push((
            "CPU limit",
            crate::rows::cpu_limit_text(snapshot.value(entity.id, ids::CG_CPU_LIMIT_CORES)),
        ));
    }
    if let Some(psi) = snapshot.value(entity.id, ids::CG_PSI_IO_FULL_AVG10) {
        rows.push(("IO PSI", crate::format::percent(psi)));
    }
    if let Some(await_ms) = snapshot.value(entity.id, ids::DISK_AWAIT) {
        rows.push(("await", crate::format::millis(await_ms)));
    }
    rows
}

/// WHY IT EXISTS: подтверждённая ppid-цепочка процесса.
///
/// Источник называется эвристикой и всегда сопровождается доказательством;
/// ownership-цепочка unit/cgroup сюда не подмешивается.
fn why_it_exists(case: &Case<'_>, width: u16) -> Vec<Line<'static>> {
    let theme = case.theme;
    let value_width = usize::from(width).saturating_sub(LABEL + 3);
    let text = |value: String| vec![Span::styled(value, theme.text())];
    let mut lines = vec![Line::from(Span::styled(
        ui::section_title(
            "  WHY IT EXISTS (HEURISTIC)",
            width,
            case.plan.section_rules,
            theme.capability,
        ),
        theme.dim(),
    ))];
    let Some(ancestry) = process_ancestry(case.snapshot, case.entity.id) else {
        lines.push(Line::from(Span::styled(
            "  unavailable: process collection is disabled or target disappeared",
            theme.severity(pulse_core::Severity::Warn),
        )));
        return lines;
    };
    let arrow = if case.ascii() { " -> " } else { " → " };
    let chain = ancestry
        .chain
        .iter()
        .map(|hop| format!("{}({})", hop.name, hop.pid))
        .collect::<Vec<_>>()
        .join(arrow);
    lines.push(fact(
        "ancestry",
        text(ui::truncate(&chain, value_width, theme.capability)),
        theme,
    ));
    if ancestry.children_total > 0 {
        let shown = ancestry
            .children
            .iter()
            .map(|hop| format!("{}({})", hop.name, hop.pid))
            .collect::<Vec<_>>()
            .join("  ");
        let hidden = ancestry.children_total - ancestry.children.len();
        let value = if hidden > 0 {
            format!("{} total: {shown}  +{hidden}", ancestry.children_total)
        } else {
            format!("{} total: {shown}", ancestry.children_total)
        };
        lines.push(fact(
            "children",
            text(ui::truncate(&value, value_width, theme.capability)),
            theme,
        ));
    }
    let source = match &ancestry.inference.evidence {
        Some(evidence) => format!(
            "{} (evidence: {}({}))",
            ancestry.inference.source.as_str(),
            evidence.name,
            evidence.pid
        ),
        None => ancestry.inference.source.as_str().to_string(),
    };
    lines.push(fact(
        "source",
        text(ui::truncate(&source, value_width, theme.capability)),
        theme,
    ));
    let coverage = match ancestry.end {
        AncestryEnd::Root => "complete: reached PPID 0".to_string(),
        AncestryEnd::MissingParent { pid } => {
            format!("incomplete: parent PID {pid} not in snapshot (budget/churn/permissions)")
        }
        AncestryEnd::MissingPpid => {
            "incomplete: ppid unavailable (collection disabled or partial)".to_string()
        }
        AncestryEnd::Cycle { pid } => format!("incomplete: cycle detected at PID {pid}"),
        AncestryEnd::DepthLimit => "incomplete: ancestry depth limit reached".to_string(),
    };
    let style = if ancestry.end.is_complete() {
        theme.dim()
    } else {
        theme.severity(pulse_core::Severity::Warn)
    };
    lines.push(fact(
        "coverage",
        vec![Span::styled(
            ui::truncate(&coverage, value_width, theme.capability),
            style,
        )],
        theme,
    ));
    lines
}

/// PROCESS DETAILS: терминальный уровень — кто это и что он держит.
fn process_block(
    case: &Case<'_>,
    details: &pulse_core::ProcessDetails,
    width: u16,
) -> Vec<Line<'static>> {
    let theme = case.theme;
    let value_width = usize::from(width).saturating_sub(LABEL + 3);
    let text = |value: String| vec![Span::styled(value, theme.text())];
    let mut lines = vec![Line::from(Span::styled(
        ui::section_title(
            "  PROCESS DETAILS",
            width,
            case.plan.section_rules,
            theme.capability,
        ),
        theme.dim(),
    ))];
    for (label, value) in details_rows(details) {
        lines.push(fact(
            label,
            text(ui::truncate(&value, value_width, theme.capability)),
            theme,
        ));
    }
    if !details.ports.is_empty() {
        let ports: Vec<String> = details
            .ports
            .iter()
            .take(8)
            .map(|port| format!("{}:{} {}", port.address, port.port, port.protocol))
            .collect();
        lines.push(fact(
            "listens",
            text(ui::truncate(
                &ports.join("  "),
                value_width,
                theme.capability,
            )),
            theme,
        ));
    }
    if details.fd_truncated {
        lines.push(fact(
            "listens",
            vec![Span::styled(
                format!(
                    "incomplete: scanned {} descriptors; ports may be missing",
                    details.fd_total
                ),
                theme.severity(pulse_core::Severity::Warn),
            )],
            theme,
        ));
    }
    if details.restricted {
        // Молчаливо пустой блок читался бы как «файлов нет».
        lines.push(fact(
            "files",
            vec![Span::styled("restricted: run as root to read", theme.dim())],
            theme,
        ));
    } else if !details.files.is_empty() {
        let count = if details.fd_truncated {
            // Точное число неизвестно: обход остановлен бюджетом.
            format!("{}+ open, scan truncated", details.fd_total)
        } else {
            format!("{} open", details.fd_total)
        };
        lines.push(fact("files", vec![Span::styled(count, theme.dim())], theme));
        for file in details.files.iter().take(6) {
            lines.push(Line::from(vec![
                Span::styled(format!("    {:<6}", file.fd), theme.dim()),
                Span::styled(
                    ui::truncate(&file.target, value_width, theme.capability),
                    theme.text(),
                ),
            ]));
        }
    }
    for observation in details.observations() {
        lines.push(fact(
            "note",
            vec![Span::styled(
                observation,
                theme.severity(pulse_core::Severity::Info),
            )],
            theme,
        ));
    }
    lines
}

/// RECENT: что с объектом происходило, свежие первыми.
fn recent_block(case: &Case<'_>, width: u16) -> Vec<Line<'static>> {
    let theme = case.theme;
    let twins = crate::leads::twins(case.snapshot, case.entity.id);
    let events: Vec<&pulse_core::Event> = case
        .snapshot
        .events
        .iter()
        .rev()
        .filter(|event| event.entity.is_some_and(|id| twins.contains(&id)))
        .take(5)
        .collect();
    if events.is_empty() {
        return Vec::new();
    }
    let detail_width = usize::from(width).saturating_sub(28);
    let mut lines = vec![heading(case, "RECENT", None, width)];
    for event in events {
        lines.push(Line::from(vec![
            Span::styled(format!("  {}  ", event.at), theme.dim()),
            Span::styled(
                event.kind.glyph().to_string(),
                theme.severity(event.severity),
            ),
            Span::styled(format!(" {:<15}", event.kind.as_str()), theme.text()),
            Span::styled(
                ui::truncate(&event.detail, detail_width, theme.capability),
                theme.dim(),
            ),
        ]));
    }
    lines
}

/// Детали процесса, если объект — процесс и источник подключён.
///
/// Для cgroup или сервиса деталей нет: файлы и порты держит процесс, а
/// приписывать их владельцу означало бы врать о том, кто именно слушает порт.
fn process_details(
    app: &App,
    snapshot: &Snapshot,
    entity: &Entity,
) -> Option<pulse_core::ProcessDetails> {
    let pulse_core::EntityKey::Process { pid, start_ticks } = entity.key else {
        return None;
    };
    let since = snapshot
        .problems
        .iter()
        .filter(|problem| problem.id.entity == entity.id)
        .map(|problem| problem.since)
        .min();
    let details = app.process_details(
        snapshot,
        pulse_core::ProcessIdentity::new(pid, start_ticks),
        pulse_core::DetailsQuery {
            since,
            journal: true,
        },
    )?;
    if details.is_empty() {
        return None;
    }
    Some(details)
}

/// Строки деталей: только заполненные поля.
fn details_rows(details: &pulse_core::ProcessDetails) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if details.identity_changed {
        // Смесь фактов от двух процессов хуже отсутствия ответа, поэтому
        // источник вернул пустоту. Оператор обязан знать причину: иначе экран
        // выглядит как «деталей нет», хотя процесс просто сменился.
        rows.push((
            "process",
            "сменился под этим pid, детали не читались".to_string(),
        ));
        return rows;
    }
    if let Some(user) = &details.user {
        rows.push(("user", user.clone()));
    } else if let Some(uid) = details.uid {
        rows.push(("uid", uid.to_string()));
    }
    if let Some(exe) = &details.exe {
        rows.push(("exe", exe.clone()));
    }
    if let Some(cwd) = &details.cwd {
        rows.push(("cwd", cwd.clone()));
    }
    if let Some(status) = &details.journal_status {
        rows.push(("journal", status.clone()));
    } else {
        for (index, line) in details.journal.iter().take(3).enumerate() {
            rows.push((if index == 0 { "journal" } else { "↳" }, line.clone()));
        }
        if details.journal_truncated {
            rows.push(("↳", "неполно: достигнут лимит 16 КиБ".to_string()));
        }
    }
    rows
}
