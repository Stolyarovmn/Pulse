//! Полный инспектор сущности (разделы 17, 18, 130, 139 спецификации v0.7).
//!
//! Раздел 139 требует: экран не остаётся на 80% пустым, если есть доступные
//! подвиды; длинные значения не обрезаются до бессмысленного текста; связанные
//! сущности выбираемы, а `Enter` идёт по связи.
//!
//! Свободное место отдаётся блокам, у которых есть содержимое (раздел 135):
//! если у сущности нет детей, место получают события и графики.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use pulse_core::entity::{Entity, EntityKind};
use pulse_core::metric::ids;
use pulse_core::snapshot::Snapshot;

use crate::app::App;
use crate::layout::{LayoutPlan, Priority};
use crate::theme::Theme;
use crate::ui;

use super::section;

/// Отрисовка инспектора.
pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &Snapshot,
    app: &mut App,
    plan: &LayoutPlan,
    theme: &Theme,
) {
    // Inspector существует только как contextual session; normal empty page
    // удалён разделом 168. Если target исчез между тактами, экран просто не
    // создаёт невидимую actionable selection.
    let Some(entity) = app.inspector_entity(snapshot) else {
        return;
    };

    let width = usize::from(area.width);
    let mut lines: Vec<Line<'_>> = Vec::new();

    // Breadcrumb: путь до сущности, сжимаемый по разделу 97.
    let trail: Vec<String> = app
        .inspector
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
                        .map(|candidate| candidate.name.clone())
                })
                .collect()
        })
        .unwrap_or_default();
    lines.push(Line::from(Span::styled(
        ui::breadcrumb(&trail, width, theme.capability),
        theme.dim(),
    )));

    let severity = snapshot
        .problems_of(entity.id)
        .map(|problem| problem.severity)
        .max();
    let state = crate::rows::state_of(snapshot, entity, severity);
    lines.push(Line::from(vec![
        Span::styled(entity.kind.as_str().to_string(), theme.dim()),
        Span::raw(" "),
        Span::styled(entity.name.clone(), theme.strong()),
        Span::raw("      "),
        Span::styled("STATE ", theme.dim()),
        Span::styled(
            state.symbol(theme.capability).to_string(),
            state.style(theme),
        ),
    ]));
    lines.push(Line::from(""));

    // SUMMARY.
    lines.push(section("SUMMARY", area.width, plan, theme));
    for (label, value) in summary(snapshot, entity) {
        lines.push(Line::from(vec![
            Span::styled(format!("{label:<16}"), theme.dim()),
            Span::styled(value, theme.text()),
        ]));
    }

    // Проблемы этой сущности - важнее графиков.
    let problems: Vec<&pulse_core::problem::Problem> = snapshot.problems_of(entity.id).collect();
    if !problems.is_empty() {
        lines.push(Line::from(""));
        lines.push(section("PROBLEMS", area.width, plan, theme));
        for problem in &problems {
            let class = crate::state::StateClass::from_severity(problem.severity);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", class.symbol(theme.capability)),
                    class.style(theme),
                ),
                Span::styled(problem.title.clone(), theme.text()),
            ]));
        }
    }

    // Evidence уже присутствует в problem model; скрывать его и оставлять
    // пустой viewport означало бы терять доступный диагностический контекст
    // (v0.9 §130, §139).
    let evidence: Vec<&pulse_core::problem::Evidence> = problems
        .iter()
        .flat_map(|problem| problem.evidence.iter())
        .collect();
    if !evidence.is_empty() {
        lines.push(Line::from(""));
        lines.push(section("EVIDENCE", area.width, plan, theme));
        for item in evidence.iter().take(6) {
            let mut spans = vec![
                Span::styled(format!("{:<16}", item.label), theme.dim()),
                Span::styled(item.value.clone(), theme.text()),
            ];
            if let Some(threshold) = &item.threshold {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(threshold.clone(), theme.dim()));
            }
            lines.push(Line::from(spans));
        }
    }

    // Два списка: цепочка вниз и боковые переходы. `Tab` переключает, какой
    // из них получает `Enter`; активный помечен в заголовке, иначе оператор
    // нажимал бы Enter вслепую.
    let relations = app.inspector_relations(snapshot);
    let side = app.inspector_side_steps(snapshot);
    let side_active = app
        .inspector
        .as_ref()
        .is_some_and(|session| session.side_active);
    let selected = app
        .inspector
        .as_ref()
        .map_or(0, |session| session.relation_selected);

    let mut step_block = |title: &str, steps: &[crate::investigate::Step], active: bool| {
        if steps.is_empty() {
            return;
        }
        lines.push(Line::from(""));
        let heading = if active {
            format!("{title} <ENTER>")
        } else {
            format!("{title} <TAB>")
        };
        lines.push(section(&heading, area.width, plan, theme));
        let cursor = selected.min(steps.len().saturating_sub(1));
        for (index, target) in steps.iter().enumerate() {
            let is_selected = active && index == cursor;
            lines.push(Line::from(vec![
                Span::styled(
                    ui::row_marker(is_selected),
                    if is_selected {
                        theme.selection()
                    } else {
                        theme.dim()
                    },
                ),
                Span::styled(format!("{:<14}", target.label), theme.dim()),
                Span::styled(
                    ui::truncate(&target.name, width.saturating_sub(18), theme.capability),
                    if is_selected {
                        theme.selection()
                    } else {
                        theme.text()
                    },
                ),
            ]));
        }
    };
    step_block("CHAIN", &relations, !side_active);
    step_block("RELATED", &side, side_active);

    if let Some(session) = &mut app.inspector {
        // Длина активного списка: по нему двигается курсор.
        session.relation_len = if side_active {
            side.len()
        } else {
            relations.len()
        };
    }

    // CHILDREN: дети сворачиваются логически (разделы 148-150), иначе сервис
    // из двенадцати процессов заполнил бы блок двенадцатью строками.
    if plan.shows(Priority::P2) {
        let child_rows: Vec<crate::rows::EntityRow> = snapshot
            .children(entity.id)
            .map(|child| {
                let severity = snapshot.problems_of(child.id).map(|p| p.severity).max();
                crate::rows::EntityRow {
                    id: child.id,
                    kind: child.kind,
                    name: child.name.clone(),
                    cpu: crate::rows::cpu_of(snapshot, child),
                    memory: crate::rows::memory_of(snapshot, child),
                    owner: crate::rows::owner_of(snapshot, child),
                    severity,
                    state: crate::rows::state_of(snapshot, child, severity),
                    io: crate::rows::io_of(snapshot, child),
                    net: crate::rows::net_of(snapshot, child),
                }
            })
            .collect();
        let folded = crate::fold::fold(snapshot, &child_rows);
        if !folded.is_empty() {
            lines.push(Line::from(""));
            lines.push(section("CHILDREN", area.width, plan, theme));
            for child in folded.iter().take(8) {
                lines.push(Line::from(vec![
                    Span::raw(ui::UNSELECTED),
                    Span::styled(format!("{:<12}", child.kind_label()), theme.dim()),
                    Span::styled(
                        ui::truncate(&child.row.name, width.saturating_sub(16), theme.capability),
                        theme.text(),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        child.row.state.symbol(theme.capability).to_string(),
                        child.row.state.style(theme),
                    ),
                ]));
            }
        }
    }

    // RESOURCES: ресурсы, которыми пользуется объект. Не звенья цепочки:
    // диском пользуются десятки несвязанных сервисов, и переход в него делал
    // бы из ресурса пересадочный узел (см. `crate::investigate`).
    let resources = crate::investigate::context_resources(snapshot, entity.id);
    if !resources.is_empty() && plan.shows(Priority::P2) {
        lines.push(Line::from(""));
        lines.push(section("RESOURCES", area.width, plan, theme));
        let mut spans: Vec<Span<'_>> = Vec::new();
        for resource in resources.iter().take(8) {
            if !spans.is_empty() {
                spans.push(Span::raw("   "));
            }
            let state = snapshot
                .entities
                .iter()
                .find(|candidate| candidate.key == resource.key)
                .map(|target| {
                    let severity = snapshot.problems_of(target.id).map(|p| p.severity).max();
                    crate::rows::state_of(snapshot, target, severity)
                });
            spans.push(Span::styled(resource.name.clone(), theme.text()));
            if let Some(state) = state {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    state.symbol(theme.capability).to_string(),
                    state.style(theme),
                ));
            }
        }
        if resources.len() > 8 {
            spans.push(Span::styled(
                format!("   +{}", resources.len() - 8),
                theme.dim(),
            ));
        }
        lines.push(Line::from(spans));
    }

    // DETAILS: терминальный уровень расследования. Здесь цепочка обязана
    // закончиться ответом «кто это и что он держит», а не ещё одной таблицей.
    if let Some(details) = process_details(app, snapshot, entity) {
        lines.push(Line::from(""));
        lines.push(section("PROCESS DETAILS", area.width, plan, theme));
        for (label, value) in details_rows(&details) {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<10}"), theme.dim()),
                Span::styled(
                    ui::truncate(&value, width.saturating_sub(12), theme.capability),
                    theme.text(),
                ),
            ]));
        }
        if !details.ports.is_empty() {
            let ports: Vec<String> = details
                .ports
                .iter()
                .take(8)
                .map(|port| format!("{}:{} {}", port.address, port.port, port.protocol))
                .collect();
            lines.push(Line::from(vec![
                Span::styled(format!("{:<10}", "listens"), theme.dim()),
                Span::styled(
                    ui::truncate(
                        &ports.join("  "),
                        width.saturating_sub(12),
                        theme.capability,
                    ),
                    theme.text(),
                ),
            ]));
        }
        if details.restricted {
            // Молчаливо пустой блок читался бы как «файлов нет».
            lines.push(Line::from(vec![
                Span::styled(format!("{:<10}", "files"), theme.dim()),
                Span::styled("restricted: run as root to read", theme.dim()),
            ]));
        } else if !details.files.is_empty() && plan.shows(Priority::P2) {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<10}", "files"), theme.dim()),
                Span::styled(format!("{} open", details.fd_total), theme.dim()),
            ]));
            for file in details.files.iter().take(6) {
                lines.push(Line::from(vec![
                    Span::raw(ui::UNSELECTED),
                    Span::styled(format!("{:<6}", file.fd), theme.dim()),
                    Span::styled(
                        ui::truncate(&file.target, width.saturating_sub(12), theme.capability),
                        theme.text(),
                    ),
                ]));
            }
        }
    }

    // RECENT EVENTS этой сущности.
    if plan.shows(Priority::P3) {
        let events: Vec<&pulse_core::Event> = snapshot
            .events
            .iter()
            .rev()
            .filter(|event| event.entity == Some(entity.id))
            .take(4)
            .collect();
        if !events.is_empty() {
            lines.push(Line::from(""));
            lines.push(section("RECENT EVENTS", area.width, plan, theme));
            for event in events {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}  ", event.at), theme.dim()),
                    Span::styled(event.kind.glyph().to_string(), theme.text()),
                    Span::raw(" "),
                    Span::styled(event.kind.as_str().to_string(), theme.dim()),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
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
    let details =
        app.process_details(snapshot, pulse_core::ProcessIdentity::new(pid, start_ticks))?;
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
    rows
}

/// Числовая сводка: только осмысленные для этого вида величины.
fn summary(snapshot: &Snapshot, entity: &Entity) -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = vec![
        (
            "CPU",
            format!(
                "{} cores",
                crate::format::cores(crate::rows::cpu_of(snapshot, entity))
            ),
        ),
        (
            "MEM",
            crate::format::bytes(crate::rows::memory_of(snapshot, entity)),
        ),
    ];

    if matches!(
        entity.kind,
        EntityKind::Cgroup | EntityKind::Unit | EntityKind::Container | EntityKind::Pod
    ) {
        rows.push((
            "CPU limit",
            snapshot
                .value(entity.id, ids::CG_CPU_LIMIT_CORES)
                .map_or_else(|| "none".to_string(), crate::format::cores),
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
