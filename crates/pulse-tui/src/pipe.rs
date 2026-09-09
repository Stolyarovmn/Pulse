//! Пайп расследования: выбранная сущность и факты о ней.
//!
//! Цепочка обязана заканчиваться ответом, а список связей ответом не является:
//! оператор спрашивает «что известно про этот процесс», а не «какие у него
//! рёбра». Поэтому здесь не топологический граф, а фиксированный набор
//! категорий факта глубиной два уровня: категория и её строки. Пересечений
//! линий при такой глубине не бывает по построению, поэтому кадр остаётся
//! выровненным без трассировки.
//!
//! Ленивость обязательна: обход `/proc/<pid>/fd` стоит десятки syscall, и
//! делать его для свёрнутой ветки нельзя. Ветка, требующая деталей, читает
//! источник только в раскрытом виде — см. [`Branch::needs_details`] и
//! [`PipeState::needs_details`].

use std::collections::BTreeSet;

use pulse_core::config::IconSet;
use pulse_core::metric::ids;
use pulse_core::{EntityId, ProcessDetails, RelationKind, Severity, Snapshot};

use crate::format;
use crate::theme::Capability;

/// Ширина бокса в граф-режиме и место под значение внутри.
pub const BOX_WIDTH: usize = 22;
const INNER: usize = BOX_WIDTH - 4;
const PER_ROW: usize = 3;
const GAP: usize = 2;

/// Ширина ствола древа слева от боксов.
const SPINE: usize = 2;

/// Минимальная ширина кадра для вида древом: ствол, три бокса и промежутки.
pub const GRAPH_MIN_COLS: usize = SPINE + PER_ROW * BOX_WIDTH + (PER_ROW - 1) * GAP;

/// Сколько строк значений показывает раскрытая ветка до свёртки остатка.
const MAX_VALUES: usize = 4;

/// Категория факта о сущности.
///
/// Порядок фиксирован и отражает полезность в расследовании: сначала «кто
/// это», затем «чем занят», затем «что о нём пишут».
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Branch {
    Exe,
    User,
    Cwd,
    Ports,
    Owner,
    Limits,
    Files,
    Log,
    Journal,
    Problems,
    Procs,
    Resources,
}

/// Ветвь древа: вопрос расследования, на который отвечает ряд боксов.
///
/// Ряд перестаёт быть произвольной тройкой «по три в строку»: линия ветви
/// связывает именно те факты, которые отвечают на один вопрос. Пока ряды
/// набирались подряд, связи были декором — линия соединяла соседей по
/// раскладке, а не по смыслу, и ничего не сообщала.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Limb {
    Identity,
    Activity,
    Placement,
    Voice,
}

impl Limb {
    /// Ветви в порядке расследования.
    pub const ALL: [Self; 4] = [Self::Identity, Self::Activity, Self::Placement, Self::Voice];

    /// Подпись ветви: вопрос, а не имя раздела.
    ///
    /// В ASCII-режиме подпись латиницей: терминал с `TERM=dumb` не обязан
    /// уметь ничего, кроме ASCII, и русская подпись там превратилась бы
    /// в мусор.
    #[must_use]
    pub const fn label(self, capability: Capability) -> &'static str {
        if matches!(capability, Capability::Ascii) {
            return match self {
                Self::Identity => "who is it",
                Self::Activity => "what it does",
                Self::Placement => "where it lives",
                Self::Voice => "what it reports",
            };
        }
        match self {
            Self::Identity => "кто это",
            Self::Activity => "чем занят",
            Self::Placement => "где живёт",
            Self::Voice => "что сообщает",
        }
    }

    /// Категории этой ветви.
    #[must_use]
    pub const fn branches(self) -> [Branch; 3] {
        match self {
            Self::Identity => [Branch::Exe, Branch::User, Branch::Cwd],
            Self::Activity => [Branch::Ports, Branch::Files, Branch::Procs],
            Self::Placement => [Branch::Owner, Branch::Limits, Branch::Resources],
            Self::Voice => [Branch::Log, Branch::Journal, Branch::Problems],
        }
    }
}

impl Branch {
    /// Все категории в порядке показа: ветвь за ветвью.
    ///
    /// Порядок обязан совпадать с порядком ветвей, потому что курсор ходит
    /// по индексам этого перечня, а кадр рисует ряды по ветвям: расхождение
    /// вернуло бы ту же болезнь — движение не по видимой сетке.
    pub const ALL: [Self; 12] = [
        Self::Exe,
        Self::User,
        Self::Cwd,
        Self::Ports,
        Self::Files,
        Self::Procs,
        Self::Owner,
        Self::Limits,
        Self::Resources,
        Self::Log,
        Self::Journal,
        Self::Problems,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Exe => "exe",
            Self::User => "user",
            Self::Cwd => "cwd",
            Self::Ports => "ports",
            Self::Owner => "owner",
            Self::Limits => "limits",
            Self::Files => "files",
            Self::Log => "log",
            Self::Journal => "journal",
            Self::Problems => "problems",
            Self::Procs => "procs",
            Self::Resources => "resources",
        }
    }

    /// Смысл категории в словаре иконок интерфейса.
    const fn icon_kind(self) -> crate::icons::Icon {
        match self {
            Self::Exe => crate::icons::Icon::Exe,
            Self::User => crate::icons::Icon::User,
            Self::Cwd => crate::icons::Icon::Cwd,
            Self::Ports => crate::icons::Icon::Ports,
            Self::Owner => crate::icons::Icon::Owner,
            Self::Limits => crate::icons::Icon::Limits,
            Self::Files => crate::icons::Icon::Files,
            Self::Procs => crate::icons::Icon::Procs,
            Self::Log => crate::icons::Icon::Log,
            Self::Journal => crate::icons::Icon::Journal,
            Self::Problems => crate::icons::Icon::Problems,
            Self::Resources => crate::icons::Icon::Resources,
        }
    }

    /// Иконка категории для выбранного набора.
    ///
    /// Символы живут в одном словаре на весь интерфейс (`crate::icons`):
    /// две таблицы неизбежно разошлись бы, и пайп получил бы иконки, которых
    /// нет у остальных экранов.
    #[must_use]
    pub const fn icon(self, set: IconSet) -> &'static str {
        self.icon_kind().glyph(set)
    }

    /// Требует чтения `/proc`: раскрытие стоит один вызов источника деталей.
    #[must_use]
    pub const fn needs_details(self) -> bool {
        matches!(
            self,
            Self::Exe | Self::User | Self::Cwd | Self::Ports | Self::Files
        )
    }
}

/// Состояние пайпа: что раскрыто и где курсор.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeState {
    pub selected: usize,
    pub expanded: BTreeSet<Branch>,
    /// Граф-режим боксами вместо отступов.
    pub boxes: bool,
}

impl Default for PipeState {
    /// Вид древом — вид по умолчанию.
    ///
    /// Нормативный макет рисует пайп боксами, поэтому запасной вид отступами
    /// обязан включаться только от нехватки ширины (`GRAPH_MIN_COLS`), а не
    /// от того, что оператор не нажал переключатель. Автоматический откат
    /// на узком кадре уже есть в `render`, и заголовок про него сообщает.
    fn default() -> Self {
        Self {
            selected: 0,
            expanded: BTreeSet::new(),
            boxes: true,
        }
    }
}

impl PipeState {
    #[must_use]
    pub fn selected_branch(&self) -> Branch {
        Branch::ALL
            .get(self.selected)
            .copied()
            .unwrap_or(Branch::Exe)
    }

    /// Перемещает курсор по осям сетки боксов.
    ///
    /// Раньше движение было линейным по списку категорий, а раскладка —
    /// сеткой из `PER_ROW` колонок, поэтому «вниз» визуально уводило вправо:
    /// оператор нажимал одну ось, а курсор ехал по другой. Здесь курсор
    /// живёт в тех же координатах, что и кадр: `dx` — колонка, `dy` — ряд.
    ///
    /// `cols` передаёт экран, потому что число колонок решает кадр: в виде
    /// боксами их `PER_ROW`, в узком виде отступами — одна.
    pub fn move_by(&mut self, dx: isize, dy: isize, cols: usize) {
        let count = Branch::ALL.len();
        let cols = cols.clamp(1, count);
        let last_row = (count - 1) / cols;
        let row = self.selected / cols;
        let column = self.selected % cols;

        let column = column.saturating_add_signed(dx).min(cols - 1);
        let row = row.saturating_add_signed(dy).min(last_row);
        // Последний ряд может быть неполным: смещение по колонке не имеет
        // права уводить курсор за границу перечня категорий.
        self.selected = (row * cols + column).min(count - 1);
    }

    /// Раскрывает свёрнутую ветку и сворачивает раскрытую.
    ///
    /// Один ключ на оба действия: оси стрелок заняты перемещением, а держать
    /// раскрытие на той же стрелке, что и движение вправо, значит объединять
    /// два разных смысла в одной клавише — на это и была жалоба.
    pub fn toggle(&mut self) -> bool {
        let branch = self.selected_branch();
        if self.expanded.contains(&branch) {
            self.expanded.remove(&branch);
        } else {
            self.expanded.insert(branch);
        }
        true
    }

    /// Число колонок сетки для кадра такой ширины.
    #[must_use]
    pub const fn columns_for(width: usize, boxes: bool) -> usize {
        if boxes && width >= GRAPH_MIN_COLS {
            PER_ROW
        } else {
            1
        }
    }

    /// Раскрывает выбранную ветку. `true`, если состояние изменилось.
    pub fn expand(&mut self) -> bool {
        let branch = self.selected_branch();
        self.expanded.insert(branch)
    }

    /// Сворачивает выбранную ветку. `true`, если состояние изменилось.
    pub fn collapse(&mut self) -> bool {
        let branch = self.selected_branch();
        self.expanded.remove(&branch)
    }

    #[must_use]
    pub fn is_expanded(&self, branch: Branch) -> bool {
        self.expanded.contains(&branch)
    }

    /// Нужно ли читать детали процесса для текущего состояния.
    ///
    /// Пока ни одна требующая деталей ветка не раскрыта, источник не
    /// вызывается вовсе: это условие приёмки, а не оптимизация.
    #[must_use]
    pub fn needs_details(&self) -> bool {
        self.expanded.iter().any(|branch| branch.needs_details())
    }
}

/// Одна категория с уже готовыми строками значений.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub branch: Branch,
    pub values: Vec<String>,
    pub expanded: bool,
    pub severity: Option<Severity>,
}

/// Строка кадра и её уровень серьёзности для раскраски.
pub type RenderedLine = (String, Option<Severity>);

/// Собирает пайп для сущности. Свёрнутые ветки значений не получают.
#[must_use]
pub fn build(
    snapshot: &Snapshot,
    entity: EntityId,
    details: Option<&ProcessDetails>,
    state: &PipeState,
) -> Vec<Node> {
    // Детали живут у процесса, а пайп открывают и на сервисе: разрешаем
    // главный процесс один раз и подписываем, чьи это факты.
    let owner_pid = main_process(snapshot, entity).map(|(_, identity)| identity.pid);
    let self_process = process_identity(snapshot, entity).is_some();
    Branch::ALL
        .iter()
        .map(|branch| {
            let expanded = state.is_expanded(*branch);
            let (values, severity) = if expanded {
                values_for(*branch, snapshot, entity, details, owner_pid, self_process)
            } else {
                (Vec::new(), None)
            };
            Node {
                branch: *branch,
                values,
                expanded,
                severity,
            }
        })
        .collect()
}

/// Где искать процессы сущности.
///
/// На живом хосте выяснилось, что процессы сервиса не потомки unit: и unit,
/// и процессы висят на одном cgroup, то есть они друг другу братья. Поэтому
/// областью поиска служит ближайший cgroup вверх по владению, а не сама
/// сущность. Для хоста и ресурсов области нет: «главный процесс хоста» —
/// бессмысленный ответ.
#[must_use]
fn process_scope(snapshot: &Snapshot, entity: EntityId) -> Option<EntityId> {
    let found = snapshot.entity(entity)?;
    match found.kind {
        pulse_core::EntityKind::Process => Some(entity),
        pulse_core::EntityKind::Cgroup => Some(entity),
        pulse_core::EntityKind::Unit
        | pulse_core::EntityKind::Container
        | pulse_core::EntityKind::Pod => {
            let mut current = found.parent;
            while let Some(id) = current {
                let parent = snapshot.entity(id)?;
                if parent.kind == pulse_core::EntityKind::Cgroup {
                    return Some(id);
                }
                current = parent.parent;
            }
            Some(entity)
        }
        pulse_core::EntityKind::Host
        | pulse_core::EntityKind::Disk
        | pulse_core::EntityKind::NetIf => None,
    }
}

/// Процесс, по которому берутся детали для сущности любого вида.
///
/// Оператор открывает пайп на сервисе или контейнере, а `exe`, порты и файлы
/// живут у процесса. Раньше такая ветка отвечала «не процесс», то есть
/// перекладывала на человека работу спуска, которую цепочка обязана делать
/// сама. Берётся процесс с наименьшим pid: у сервиса это, как правило,
/// главный процесс, и выбор детерминирован.
#[must_use]
pub fn main_process(
    snapshot: &Snapshot,
    entity: EntityId,
) -> Option<(EntityId, pulse_core::ProcessIdentity)> {
    if let Some(identity) = process_identity(snapshot, entity) {
        return Some((entity, identity));
    }
    let scope = process_scope(snapshot, entity)?;
    let mut best: Option<(EntityId, pulse_core::ProcessIdentity)> = None;
    let mut queue: Vec<EntityId> = vec![scope];
    let mut seen = 0_usize;
    while let Some(current) = queue.pop() {
        // Ограничение обхода: у корневых слайсов потомков тысячи, а ответ
        // нужен за один кадр.
        seen += 1;
        if seen > 512 {
            break;
        }
        for child in snapshot.children(current) {
            if let Some(identity) = process_identity(snapshot, child.id) {
                if best.is_none_or(|(_, known)| identity.pid < known.pid) {
                    best = Some((child.id, identity));
                }
            } else {
                queue.push(child.id);
            }
        }
    }
    best
}

/// Идентичность процесса: пара `(pid, start_ticks)`, а не номер.
///
/// Детали читаются под этой парой, потому что между кадром и чтением `/proc`
/// процесс мог умереть, а номер - достаться другому.
fn process_identity(snapshot: &Snapshot, entity: EntityId) -> Option<pulse_core::ProcessIdentity> {
    match snapshot.entity(entity).map(|found| &found.key) {
        Some(pulse_core::EntityKey::Process { pid, start_ticks }) => {
            Some(pulse_core::ProcessIdentity::new(*pid, *start_ticks))
        }
        _ => None,
    }
}

/// Подпись сущности: корневой cgroup зовётся `cgroup`, и в цепочке это
/// читается как `cgroup cgroup`. Показываем его корнем.
fn entity_label(snapshot: &Snapshot, entity: EntityId) -> String {
    let Some(found) = snapshot.entity(entity) else {
        return "неизвестная сущность".to_string();
    };
    if found.kind == pulse_core::EntityKind::Cgroup && found.name == "cgroup" {
        return "cgroup /".to_string();
    }
    format!("{} {}", found.kind.as_str(), found.name)
}

fn values_for(
    branch: Branch,
    snapshot: &Snapshot,
    entity: EntityId,
    details: Option<&ProcessDetails>,
    owner_pid: Option<i32>,
    self_process: bool,
) -> (Vec<String>, Option<Severity>) {
    // Факты процесса, открытые на сервисе, обязаны быть подписаны: иначе
    // оператор прочитает `exe` сервиса как «единственный», а это главный
    // процесс из нескольких.
    let note = match (self_process, owner_pid) {
        (false, Some(pid)) => Some(format!("главный процесс pid {pid}")),
        _ => None,
    };
    match branch {
        Branch::Exe => (
            detail_line(details, owner_pid, note, |d| d.exe.clone()),
            None,
        ),
        Branch::User => (
            detail_line(details, owner_pid, note, |d| {
                d.user.as_ref().map(|user| match d.uid {
                    Some(uid) => format!("{user} uid {uid}"),
                    None => user.clone(),
                })
            }),
            None,
        ),
        Branch::Cwd => (
            detail_line(details, owner_pid, note, |d| d.cwd.clone()),
            None,
        ),
        Branch::Ports => (ports(details, owner_pid, note), None),
        Branch::Owner => (owner(snapshot, entity), None),
        Branch::Limits => (limits(snapshot, entity), None),
        Branch::Files => (files(details, owner_pid, note), None),
        Branch::Procs => (procs(snapshot, entity), None),
        Branch::Log => (
            vec!["нет источника в этой сборке".to_string()],
            Some(Severity::Info),
        ),
        Branch::Journal => (
            vec!["нет источника в этой сборке".to_string()],
            Some(Severity::Info),
        ),
        Branch::Problems => problems(snapshot, entity),
        Branch::Resources => (resources(snapshot, entity), None),
    }
}

/// Значение из деталей: отказ в доступе показывается словом, а не пустотой.
fn detail_line<F>(
    details: Option<&ProcessDetails>,
    owner_pid: Option<i32>,
    note: Option<String>,
    pick: F,
) -> Vec<String>
where
    F: Fn(&ProcessDetails) -> Option<String>,
{
    let Some(details) = details else {
        return vec![missing(owner_pid)];
    };
    let mut out = if details.restricted && pick(details).is_none() {
        vec!["restricted: нет прав".to_string()]
    } else {
        vec![pick(details).unwrap_or_else(|| "—".to_string())]
    };
    if let Some(note) = note {
        out.push(note);
    }
    out
}

/// Почему деталей нет: отсутствие процесса и отсутствие прав — разные ответы.
fn missing(owner_pid: Option<i32>) -> String {
    match owner_pid {
        Some(pid) => format!("детали недоступны для pid {pid}"),
        None => "процессов под сущностью нет".to_string(),
    }
}

fn ports(
    details: Option<&ProcessDetails>,
    owner_pid: Option<i32>,
    note: Option<String>,
) -> Vec<String> {
    let Some(details) = details else {
        return vec![missing(owner_pid)];
    };
    let mut out: Vec<String> = if details.ports.is_empty() {
        if details.restricted {
            vec!["restricted: нет прав".to_string()]
        } else {
            vec!["не слушает".to_string()]
        }
    } else {
        let mut listed: Vec<String> = details
            .ports
            .iter()
            .take(MAX_VALUES)
            .map(|port| format!("{} {}:{}", port.protocol, port.address, port.port))
            .collect();
        if details.ports.len() > MAX_VALUES {
            listed.push(format!("(+{})", details.ports.len() - MAX_VALUES));
        }
        listed
    };
    if let Some(note) = note {
        out.push(note);
    }
    out
}

fn files(
    details: Option<&ProcessDetails>,
    owner_pid: Option<i32>,
    note: Option<String>,
) -> Vec<String> {
    let Some(details) = details else {
        return vec![missing(owner_pid)];
    };
    if details.restricted && details.files.is_empty() {
        return vec!["restricted: нет прав".to_string()];
    }
    let regular: Vec<&str> = details
        .files
        .iter()
        .filter(|file| file.is_regular())
        .map(|file| file.target.as_str())
        .collect();
    let mut out = vec![format!(
        "{} из {} дескрипторов",
        regular.len(),
        details.fd_total
    )];
    for path in regular.iter().take(MAX_VALUES - 1) {
        out.push(elide_path(path));
    }
    if let Some(note) = note {
        out.push(note);
    }
    out
}

/// Процессы под сущностью: у сервиса их несколько, и это часть ответа.
fn procs(snapshot: &Snapshot, entity: EntityId) -> Vec<String> {
    if process_identity(snapshot, entity).is_some() {
        return vec!["это сам процесс".to_string()];
    }
    let Some(scope) = process_scope(snapshot, entity) else {
        return vec!["процессов нет".to_string()];
    };
    let mut found: Vec<(i32, String)> = Vec::new();
    let mut queue: Vec<EntityId> = vec![scope];
    let mut seen = 0_usize;
    while let Some(current) = queue.pop() {
        seen += 1;
        if seen > 512 {
            break;
        }
        for child in snapshot.children(current) {
            if let Some(identity) = process_identity(snapshot, child.id) {
                found.push((identity.pid, child.name.clone()));
            } else {
                queue.push(child.id);
            }
        }
    }
    if found.is_empty() {
        return vec!["процессов нет".to_string()];
    }
    found.sort_unstable();
    let total = found.len();
    let mut out: Vec<String> = found
        .into_iter()
        .take(MAX_VALUES)
        .map(|(pid, name)| format!("{name} pid {pid}"))
        .collect();
    if total > MAX_VALUES {
        out.push(format!("(+{})", total - MAX_VALUES));
    }
    out
}

fn owner(snapshot: &Snapshot, entity: EntityId) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = snapshot.entity(entity).and_then(|found| found.parent);
    while let Some(id) = current {
        out.push(entity_label(snapshot, id));
        if out.len() >= MAX_VALUES {
            break;
        }
        current = snapshot.entity(id).and_then(|found| found.parent);
    }
    if out.is_empty() {
        out.push("владельца нет: корень".to_string());
    }
    out
}

fn limits(snapshot: &Snapshot, entity: EntityId) -> Vec<String> {
    // Лимиты живут на cgroup: у процесса их нет, поэтому поднимаемся к
    // владельцу, у которого метрика заполнена.
    let mut current = Some(entity);
    while let Some(id) = current {
        let limit = snapshot.value_or(id, ids::CG_MEM_LIMIT, 0.0);
        let used = snapshot.value_or(id, ids::CG_MEM_CURRENT, 0.0);
        let throttle = snapshot.value_or(id, ids::CG_CPU_THROTTLE_RATIO, 0.0);
        if limit > 0.0 || used > 0.0 {
            let mut out = Vec::new();
            if limit > 0.0 {
                out.push(format!(
                    "mem {}/{}",
                    format::bytes(used),
                    format::bytes(limit)
                ));
            } else {
                out.push(format!("mem {}", format::bytes(used)));
            }
            out.push(format!("throttle {}", format::percent(throttle)));
            return out;
        }
        current = snapshot.entity(id).and_then(|found| found.parent);
    }
    vec!["лимитов нет".to_string()]
}

fn problems(snapshot: &Snapshot, entity: EntityId) -> (Vec<String>, Option<Severity>) {
    let mine: Vec<&pulse_core::Problem> = snapshot
        .problems
        .iter()
        .filter(|problem| problem.id.entity == entity)
        .collect();
    if mine.is_empty() {
        return (vec!["проблем нет".to_string()], None);
    }
    let worst = mine.iter().map(|problem| problem.severity).max();
    let mut out: Vec<String> = mine
        .iter()
        .take(MAX_VALUES)
        .map(|problem| format!("{} {}", problem.id.rule.0, problem.severity.as_str()))
        .collect();
    if mine.len() > MAX_VALUES {
        out.push(format!("(+{})", mine.len() - MAX_VALUES));
    }
    (out, worst)
}

/// Ресурсы сущности: носитель и интерфейс, а не её собственный владелец.
///
/// Фильтр по типу связи оказался недостаточным: на живом хосте в ветку
/// попадал сам cgroup сервиса, потому что связь владения тоже проходила
/// проверку. Ресурс определяется видом сущности на другом конце.
fn resources(snapshot: &Snapshot, entity: EntityId) -> Vec<String> {
    let mut out = Vec::new();
    for relation in snapshot.relations_of(entity, None) {
        if matches!(relation.kind, RelationKind::ParentOf) {
            continue;
        }
        let other = if relation.from == entity {
            relation.to
        } else {
            relation.from
        };
        if other == entity {
            continue;
        }
        let Some(found) = snapshot.entity(other) else {
            continue;
        };
        if !matches!(
            found.kind,
            pulse_core::EntityKind::Disk | pulse_core::EntityKind::NetIf
        ) {
            continue;
        }
        let label = entity_label(snapshot, other);
        if !out.contains(&label) {
            out.push(label);
        }
        if out.len() >= MAX_VALUES {
            break;
        }
    }
    if out.is_empty() {
        out.push("ресурсов нет".to_string());
    }
    out
}

/// Сокращает путь по сегментам: `/var/log/nginx/access.log` → `/var/…/access.log`.
///
/// Обрезка посередине токена запрещена: обрезанный путь читается как другой
/// путь. Если сегментами не помещается, режем по границе с явным знаком.
#[must_use]
pub fn elide_path(path: &str) -> String {
    if path.chars().count() <= INNER {
        return path.to_string();
    }
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if let (Some(head), Some(tail)) = (parts.first(), parts.last()) {
        if parts.len() > 2 {
            let candidate = format!("/{head}/…/{tail}");
            if candidate.chars().count() <= INNER {
                return candidate;
            }
        }
    }
    let tail: String = path
        .chars()
        .skip(path.chars().count().saturating_sub(INNER - 1))
        .collect();
    format!("…{tail}")
}

/// Перенос по словам: рвать слово посередине нельзя.
///
/// Разрыв внутри слова читается как другое слово: `restricted` в виде
/// `restricted: нет пр` и `ав` не сообщает ничего, кроме того, что раскладка
/// сломана. Слово длиннее строки (единственный длинный токен, например путь)
/// режется по границе ширины — но только когда иначе не помещается вовсе.
fn wrap(value: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    if value.chars().count() <= width {
        return vec![value.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        let word_len = word.chars().count();
        let line_len = line.chars().count();
        if line.is_empty() {
            if word_len <= width {
                line.push_str(word);
            } else {
                // Один токен шире строки: режем по границе, иначе он не
                // покажется вовсе.
                let chars: Vec<char> = word.chars().collect();
                for chunk in chars.chunks(width) {
                    out.push(chunk.iter().collect());
                }
            }
            continue;
        }
        if line_len + 1 + word_len <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            if word_len <= width {
                line.push_str(word);
            } else {
                let chars: Vec<char> = word.chars().collect();
                for chunk in chars.chunks(width) {
                    out.push(chunk.iter().collect());
                }
            }
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

struct Charset {
    top_left: char,
    top_right: char,
    bottom_left: char,
    bottom_right: char,
    horizontal: char,
    vertical: char,
    collapsed: &'static str,
    branch: &'static str,
    last: &'static str,
    pipe: char,
}

impl Charset {
    const fn thin(capability: Capability) -> Self {
        if matches!(capability, Capability::Ascii) {
            Self {
                top_left: '+',
                top_right: '+',
                bottom_left: '+',
                bottom_right: '+',
                horizontal: '-',
                vertical: '|',
                collapsed: ">",
                branch: "|-",
                last: "`-",
                pipe: '|',
            }
        } else {
            Self {
                top_left: '╭',
                top_right: '╮',
                bottom_left: '╰',
                bottom_right: '╯',
                horizontal: '─',
                vertical: '│',
                collapsed: "▸",
                branch: "├─",
                last: "╰─",
                pipe: '│',
            }
        }
    }

    const fn focus(capability: Capability) -> Self {
        if matches!(capability, Capability::Ascii) {
            Self::thin(capability)
        } else {
            Self {
                top_left: '┏',
                top_right: '┓',
                bottom_left: '┗',
                bottom_right: '┛',
                horizontal: '━',
                vertical: '┃',
                collapsed: "▸",
                branch: "├─",
                last: "╰─",
                pipe: '│',
            }
        }
    }
}

/// Кадр пайпа: заголовок фокуса и категории.
///
/// Возвращает строки вместе с уровнем серьёзности: раскраску делает экран,
/// а раскладка остаётся текстом и потому проверяется тестом.
#[must_use]
pub fn render(
    title: &str,
    nodes: &[Node],
    selected: Branch,
    width: usize,
    style: Look,
) -> Vec<RenderedLine> {
    if style.boxes && width >= GRAPH_MIN_COLS {
        render_boxes(title, nodes, selected, style)
    } else {
        render_indent(title, nodes, selected, width, style)
    }
}

/// Вид кадра: возможности терминала, режим и иконки.
///
/// Одна структура вместо трёх параметров: у функций отрисовки иначе
/// набирается семь аргументов, и запрет проекта на такие подписи срабатывает
/// не зря — читать их невозможно.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub capability: Capability,
    pub boxes: bool,
    pub icons: IconSet,
}

impl Look {
    /// Включён ли набор для этого терминала.
    ///
    /// В ASCII-режиме иконок нет ни в одном наборе: там гарантированно
    /// доступны только ASCII-символы.
    const fn icons_on(self) -> bool {
        !matches!(self.icons, IconSet::Off) && !matches!(self.capability, Capability::Ascii)
    }

    /// Префикс категории: иконка или пусто.
    fn prefix(self, branch: Branch) -> String {
        if self.icons_on() {
            format!("{} ", branch.icon(self.icons))
        } else {
            String::new()
        }
    }
}

fn render_indent(
    title: &str,
    nodes: &[Node],
    selected: Branch,
    width: usize,
    style: Look,
) -> Vec<RenderedLine> {
    let capability = style.capability;
    let boxes_requested = style.boxes;
    let set = Charset::thin(capability);
    let mut out: Vec<RenderedLine> = Vec::new();
    let mut head = format!("PIPE {title}");
    if boxes_requested {
        // Молча подменять вид нельзя: оператор должен видеть, что кадр сужен.
        head.push_str(&format!("  (граф-режим требует {GRAPH_MIN_COLS} колонок)"));
    }
    out.push((head, None));
    let label_width = Branch::ALL
        .iter()
        .map(|branch| branch.label().chars().count())
        .max()
        .unwrap_or(8);
    // Ширина колонки считается с иконкой: иначе включение набора сдвигает
    // значения, и кадр перестаёт совпадать с кадром без иконок.
    let icon_width = if style.icons_on() { 2 } else { 0 };
    let value_width = width.saturating_sub(label_width + icon_width + 6).max(8);
    for (index, node) in nodes.iter().enumerate() {
        let last = index + 1 == nodes.len();
        let stem = if last { set.last } else { set.branch };
        // Ветвь названа и в узком виде: деградация раскладки не имеет права
        // отнимать смысл группировки.
        if let Some(limb) = Limb::ALL
            .iter()
            .find(|limb| limb.branches().first() == Some(&node.branch))
        {
            out.push((format!("{} {}", set.pipe, limb.label(capability)), None));
        }
        let marker = if node.branch == selected { "◂" } else { " " };
        let label = format!(
            "{}{:<label_width$}",
            style.prefix(node.branch),
            node.branch.label()
        );
        if !node.expanded {
            out.push((
                format!("{stem} {label} {} {marker}", set.collapsed),
                node.severity,
            ));
            continue;
        }
        let mut first = true;
        for value in &node.values {
            for chunk in wrap(value, value_width) {
                let prefix = if first {
                    format!("{stem} {label} ")
                } else {
                    let pipe = if last { ' ' } else { set.pipe };
                    format!("{pipe}  {:<pad$} ", "", pad = label_width + icon_width)
                };
                let tail = if first { marker } else { " " };
                out.push((format!("{prefix}{chunk} {tail}"), node.severity));
                first = false;
            }
        }
    }
    out
}

/// Кадр древом: ствол слева, ряды боксов на ветвях.
///
/// Сетка боксов без связей — не древо: оператор видит плитки и не понимает,
/// что все категории принадлежат одной сущности. Ствол и горизонтальные
/// ветви делают отношение «сущность → её факты» видимым.
fn render_boxes(title: &str, nodes: &[Node], selected: Branch, style: Look) -> Vec<RenderedLine> {
    let set = Charset::thin(style.capability);
    let mut out: Vec<RenderedLine> = Vec::new();
    out.push((format!("PIPE {title}"), None));
    let rows = Limb::ALL.len();
    for (row, limb) in Limb::ALL.iter().enumerate() {
        let last_row = row + 1 == rows;
        // Подпись ветви названа на стволе: линия сообщает, на какой вопрос
        // отвечает ряд, и перестаёт быть декором.
        let selected_here = limb.branches().contains(&selected);
        out.push((
            format!(
                "{}{} {} {}",
                if last_row { set.last } else { set.branch },
                set.horizontal,
                limb.label(style.capability),
                if selected_here { set.collapsed } else { " " }
            ),
            None,
        ));
        let chunk: Vec<&Node> = limb
            .branches()
            .iter()
            .filter_map(|branch| nodes.iter().find(|node| node.branch == *branch))
            .collect();
        let boxes: Vec<Vec<RenderedLine>> = chunk
            .iter()
            .map(|node| single_box(node, selected, style))
            .collect();
        let height = boxes.iter().map(Vec::len).max().unwrap_or(0);
        for line in 0..height {
            // Верхняя граница ряда продолжает ветвь: промежутки между боксами
            // заполняются линией, поэтому три бокса читаются как три ответа
            // на один вопрос, а не как соседи по раскладке.
            let top = line == 0;
            let stem = if last_row {
                "  ".to_string()
            } else {
                format!("{} ", set.pipe)
            };
            let filler = if top {
                set.horizontal.to_string().repeat(GAP)
            } else {
                " ".repeat(GAP)
            };
            let mut row_text = stem;
            let mut severity = None;
            for (position, drawn) in boxes.iter().enumerate() {
                if position > 0 {
                    row_text.push_str(&filler);
                }
                match drawn.get(line) {
                    Some((text, node_severity)) => {
                        row_text.push_str(text);
                        severity = severity.or(*node_severity);
                    }
                    None => row_text.push_str(&" ".repeat(BOX_WIDTH)),
                }
            }
            out.push((row_text.trim_end().to_string(), severity));
        }
    }
    out
}

/// Один бокс фиксированной ширины.
///
/// Значение никогда не расширяет рамку: превышение внутренней ширины
/// переносится строкой, иначе правые границы боксов в ряду разъезжаются.
fn single_box(node: &Node, selected: Branch, style: Look) -> Vec<RenderedLine> {
    let capability = style.capability;
    let focused = node.branch == selected;
    let set = if focused {
        Charset::focus(capability)
    } else {
        Charset::thin(capability)
    };
    let label = format!("{}{}", style.prefix(node.branch), node.branch.label());
    let mut out: Vec<RenderedLine> = Vec::new();
    let suffix = if node.expanded {
        String::new()
    } else {
        format!("{} ", set.collapsed)
    };
    // Подпись вписана в границу, как в панелях omp: `╭─── files ───╮`.
    // Заголовок внутри рамки, а не над ней, потому что ряд боксов иначе
    // теряет соответствие подписи и содержимого при переносе значений.
    let mut head = format!("{}{} {label} {suffix}", set.top_left, {
        let mut lead = String::new();
        for _ in 0..3 {
            lead.push(set.horizontal);
        }
        lead
    });
    while head.chars().count() < BOX_WIDTH - 1 {
        head.push(set.horizontal);
    }
    let head: String = head.chars().take(BOX_WIDTH - 1).collect();
    let head = format!("{head}{}", set.top_right);
    out.push((head, node.severity));
    if node.expanded {
        for value in &node.values {
            // Путь в боксе сокращается по сегментам, а не рвётся по границе:
            // `/home/user/.cache/` и `pulse-target/debug` читаются как два
            // разных пути, которых на диске нет.
            let value = if value.starts_with('/') && value.chars().count() > INNER {
                elide_path(value)
            } else {
                value.clone()
            };
            for chunk in wrap(&value, INNER) {
                let pad = INNER.saturating_sub(chunk.chars().count());
                out.push((
                    format!(
                        "{} {chunk}{} {}",
                        set.vertical,
                        " ".repeat(pad),
                        set.vertical
                    ),
                    node.severity,
                ));
            }
        }
    }
    out.push((
        format!(
            "{}{}{}",
            set.bottom_left,
            set.horizontal.to_string().repeat(BOX_WIDTH - 2),
            set.bottom_right
        ),
        node.severity,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Дефект с живого хоста: пайп на unit отвечал «процессов под сущностью
    /// нет», потому что процессы сервиса не потомки unit — и unit, и процессы
    /// висят на одном cgroup, то есть они друг другу братья.
    #[test]
    fn unit_resolves_processes_that_are_its_siblings() {
        use pulse_core::{AgentStats, EntityGraph, EntityKey, EntitySpec, LatestValues, Timestamp};

        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let host = graph.host();
        let cgroup = graph.upsert(
            EntitySpec::new(EntityKey::Cgroup { cgroup_id: 7 }, "angie.service").parent(host),
        );
        let unit = graph.upsert(
            EntitySpec::new(
                EntityKey::Unit {
                    name: "angie.service".into(),
                },
                "angie.service",
            )
            .parent(cgroup),
        );
        for pid in [300, 262, 264] {
            let _ = graph.upsert(
                EntitySpec::new(
                    EntityKey::Process {
                        pid,
                        start_ticks: 10,
                    },
                    "angie",
                )
                .parent(cgroup),
            );
        }
        let _ = graph.end_tick();
        let snapshot = Snapshot::build(
            &graph,
            LatestValues::new(),
            Vec::new(),
            Vec::new(),
            AgentStats::default(),
            "host",
            "boot",
        );

        assert_eq!(
            main_process(&snapshot, unit).map(|(_, identity)| identity.pid),
            Some(262),
            "у unit обязан находиться главный процесс с наименьшим pid"
        );
        let listed = procs(&snapshot, unit);
        assert!(
            listed.iter().any(|line| line.contains("pid 262")),
            "ветка procs обязана перечислять процессы сервиса: {listed:?}"
        );
        assert_eq!(
            procs(&snapshot, host),
            vec!["процессов нет".to_string()],
            "у хоста главного процесса нет: это бессмысленный ответ"
        );
    }

    fn state_with(branches: &[Branch]) -> PipeState {
        let mut state = PipeState::default();
        for branch in branches {
            state.expanded.insert(*branch);
        }
        state
    }

    #[test]
    fn collapsed_state_never_needs_details() {
        let state = PipeState::default();
        assert!(!state.needs_details(), "свёрнутый пайп не читает /proc");
        let state = state_with(&[Branch::Problems, Branch::Owner]);
        assert!(
            !state.needs_details(),
            "ветки графа и проблем деталей не требуют"
        );
        let state = state_with(&[Branch::Ports]);
        assert!(state.needs_details(), "порты требуют чтения деталей");
    }

    fn look(boxes: bool) -> Look {
        Look {
            capability: Capability::TrueColor,
            boxes,
            icons: IconSet::Off,
        }
    }

    fn ascii_look() -> Look {
        Look {
            capability: Capability::Ascii,
            boxes: true,
            icons: IconSet::Nerd,
        }
    }

    /// Вид по умолчанию — древо боксами, как в нормативном макете.
    ///
    /// Раньше состояние брало производный `Default`, где `bool` даёт `false`,
    /// и оператор без нажатия переключателя видел запасной вид отступами.
    #[test]
    fn default_view_is_the_box_tree() {
        let state = PipeState::default();
        assert!(state.boxes, "нормативный вид пайпа — боксы");

        let nodes = build_stub();
        let wide = render(
            "proc",
            &nodes,
            Branch::Exe,
            GRAPH_MIN_COLS,
            look(state.boxes),
        );
        assert!(
            wide.iter().any(|(text, _)| text.contains('╭')),
            "на достаточной ширине кадр обязан быть древом боксами"
        );

        let narrow = render(
            "proc",
            &nodes,
            Branch::Exe,
            GRAPH_MIN_COLS - 1,
            look(state.boxes),
        );
        assert!(
            narrow[0].0.contains("граф-режим требует"),
            "откат на узком кадре обязан быть назван в заголовке"
        );
    }

    /// Набор Nerd Font обязан давать глиф каждой категории, и это должны быть
    /// именно глифы из приватной области: иначе «включил nerd» тихо рисует
    /// обычные символы, и обещание набора не выполняется.
    #[test]
    fn nerd_set_covers_every_branch_with_private_use_glyphs() {
        for branch in Branch::ALL {
            let glyph = branch.icon(IconSet::Nerd);
            let mut chars = glyph.chars();
            let symbol = chars.next().expect("глиф набора Nerd");
            assert!(chars.next().is_none(), "иконка обязана быть одним символом");
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&symbol),
                "{} обязан быть глифом Nerd Font: {symbol:?}",
                branch.label()
            );
            assert!(
                !branch.icon(IconSet::Unicode).is_empty(),
                "у запасного набора тоже обязан быть символ"
            );
            assert!(branch.icon(IconSet::Off).is_empty());
        }
    }

    /// Включение набора не двигает значения: колонка считается с иконкой.
    #[test]
    fn icons_do_not_shift_values() {
        let nodes = build_stub();
        let plain = render("proc", &nodes, Branch::Exe, 100, look(false));
        let nerd = render(
            "proc",
            &nodes,
            Branch::Exe,
            100,
            Look {
                capability: Capability::TrueColor,
                boxes: false,
                icons: IconSet::Nerd,
            },
        );
        for (plain_line, nerd_line) in plain.iter().zip(nerd.iter()).skip(1) {
            let plain_value = plain_line.0.trim_end();
            let nerd_value = nerd_line.0.trim_end();
            // Подписи ветвей иконок не несут: они называют вопрос, а не
            // категорию, поэтому их ширина обязана совпадать без сдвига.
            let is_limb = Limb::ALL
                .iter()
                .any(|limb| plain_value.contains(limb.label(Capability::TrueColor)));
            let expected = if is_limb { 0 } else { 2 };
            assert_eq!(
                plain_value.chars().count() + expected,
                nerd_value.chars().count(),
                "иконка добавляет ровно {expected} колонок: {plain_value:?} против {nerd_value:?}"
            );
        }
    }

    #[test]
    fn box_row_keeps_every_border_aligned() {
        let nodes = vec![
            Node {
                branch: Branch::Exe,
                values: vec!["/usr/sbin/nginx".to_string()],
                expanded: true,
                severity: None,
            },
            Node {
                branch: Branch::User,
                values: vec!["www-data uid 33 и очень длинное значение".to_string()],
                expanded: true,
                severity: None,
            },
            Node {
                branch: Branch::Cwd,
                values: Vec::new(),
                expanded: false,
                severity: None,
            },
        ];
        let lines = render_boxes("proc 851", &nodes, Branch::Exe, look(true));
        for (text, _) in lines.iter().skip(1) {
            let width = text.chars().count();
            assert!(
                width <= GRAPH_MIN_COLS,
                "ряд не должен выходить за {GRAPH_MIN_COLS}: {width} в {text:?}"
            );
        }
    }

    #[test]
    fn long_value_wraps_instead_of_stretching_box() {
        let node = Node {
            branch: Branch::Exe,
            values: vec!["/very/long/path/that/does/not/fit/in/one/line".to_string()],
            expanded: true,
            severity: None,
        };
        let drawn = single_box(&node, Branch::User, look(true));
        let widths: BTreeSet<usize> = drawn.iter().map(|(text, _)| text.chars().count()).collect();
        assert_eq!(
            widths.len(),
            1,
            "все строки бокса обязаны быть одной ширины: {widths:?}"
        );
        assert_eq!(widths.into_iter().next(), Some(BOX_WIDTH));
    }

    #[test]
    fn ascii_frame_has_no_unicode() {
        let nodes = vec![Node {
            branch: Branch::Files,
            values: vec!["8 из 214".to_string()],
            expanded: false,
            severity: None,
        }];
        let lines = render_boxes("proc", &nodes, Branch::Files, ascii_look());
        for (text, _) in &lines {
            assert!(
                text.is_ascii(),
                "в ASCII-режиме рамка обязана быть ASCII: {text:?}"
            );
        }
    }

    #[test]
    fn narrow_frame_falls_back_to_indent_and_says_so() {
        let nodes = build_stub();
        let lines = render("proc 851", &nodes, Branch::Exe, 60, look(true));
        assert!(
            lines[0].0.contains("граф-режим требует"),
            "подмена вида обязана быть подписана: {:?}",
            lines[0].0
        );
    }

    #[test]
    fn render_is_deterministic() {
        let nodes = build_stub();
        let first = render("proc", &nodes, Branch::Ports, 120, look(true));
        let second = render("proc", &nodes, Branch::Ports, 120, look(true));
        assert_eq!(first, second);
    }

    #[test]
    fn wrap_never_splits_a_word() {
        let lines = wrap("restricted: нет прав", 18);
        assert_eq!(
            lines,
            vec!["restricted: нет".to_string(), "прав".to_string()],
            "перенос обязан идти по пробелу, а не посередине слова"
        );
        let lines = wrap("cgroup angie.service", 18);
        assert_eq!(
            lines,
            vec!["cgroup".to_string(), "angie.service".to_string()]
        );
        // Единственный длинный токен иначе не покажется вовсе.
        let lines = wrap("/proc/self/very-long-name", 10);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn path_is_elided_by_segments() {
        assert_eq!(
            elide_path("/var/log/nginx/access.log"),
            "/var/…/access.log",
            "путь сокращается по сегментам, а не обрезается посередине"
        );
        assert_eq!(elide_path("/short"), "/short");
    }

    fn build_stub() -> Vec<Node> {
        vec![
            Node {
                branch: Branch::Exe,
                values: vec!["/usr/sbin/nginx".to_string()],
                expanded: true,
                severity: None,
            },
            Node {
                branch: Branch::Ports,
                values: vec!["tcp 0.0.0.0:80".to_string()],
                expanded: true,
                severity: None,
            },
            Node {
                branch: Branch::Journal,
                values: Vec::new(),
                expanded: false,
                severity: None,
            },
        ]
    }

    /// Курсор двигается по тем же осям, что видит оператор.
    ///
    /// Дефект с живого прогона: движение было линейным по списку из двенадцати
    /// категорий, а кадр — сеткой из трёх колонок, поэтому «вниз» уводило
    /// курсор вправо.
    #[test]
    fn cursor_moves_along_screen_axes() {
        let mut state = PipeState::default();
        assert_eq!(state.selected_branch(), Branch::Exe);

        state.move_by(1, 0, PER_ROW);
        assert_eq!(
            state.selected_branch(),
            Branch::User,
            "вправо — соседний бокс"
        );

        state.move_by(0, 1, PER_ROW);
        assert_eq!(
            state.selected_branch(),
            Branch::Files,
            "вниз — тот же столбец следующей ветви"
        );

        state.move_by(-1, 0, PER_ROW);
        assert_eq!(state.selected_branch(), Branch::Ports);

        state.move_by(0, -1, PER_ROW);
        assert_eq!(state.selected_branch(), Branch::Exe);

        // Границы сетки держат курсор: за край он не уходит и не заворачивается.
        state.move_by(-1, -1, PER_ROW);
        assert_eq!(state.selected_branch(), Branch::Exe);
        state.move_by(9, 9, PER_ROW);
        assert_eq!(state.selected_branch(), Branch::Problems);
    }

    /// В узком кадре сетка одноколоночная, и оси обязаны это учитывать.
    #[test]
    fn narrow_frame_navigates_as_single_column() {
        assert_eq!(PipeState::columns_for(GRAPH_MIN_COLS, true), PER_ROW);
        assert_eq!(PipeState::columns_for(GRAPH_MIN_COLS - 1, true), 1);
        assert_eq!(PipeState::columns_for(200, false), 1);

        let mut state = PipeState::default();
        state.move_by(0, 1, 1);
        assert_eq!(
            state.selected_branch(),
            Branch::User,
            "вниз — следующая ветка"
        );
    }

    /// Пробел раскрывает и сворачивает одну и ту же ветку.
    #[test]
    fn toggle_expands_then_collapses() {
        let mut state = PipeState::default();
        state.toggle();
        assert!(state.is_expanded(Branch::Exe));
        state.toggle();
        assert!(!state.is_expanded(Branch::Exe));
    }

    /// Кадр древом связывает боксы: ствол слева и ветвь по верхней границе.
    ///
    /// Без связей это была сетка плиток — ровно та жалоба, что «древа нет».
    #[test]
    fn box_view_draws_tree_connections() {
        let nodes: Vec<Node> = Branch::ALL
            .iter()
            .map(|branch| Node {
                branch: *branch,
                values: Vec::new(),
                expanded: false,
                severity: None,
            })
            .collect();
        let lines = render("unit x", &nodes, Branch::Exe, GRAPH_MIN_COLS, look(true));
        let body: Vec<&String> = lines.iter().skip(1).map(|(text, _)| text).collect();

        let stems: Vec<char> = body.iter().filter_map(|line| line.chars().next()).collect();
        assert!(
            stems.contains(&'├'),
            "ряды обязаны висеть на стволе: {stems:?}"
        );
        assert!(
            stems.contains(&'╰'),
            "последний ряд закрывает ствол: {stems:?}"
        );
        assert!(stems.contains(&'│'), "ствол продолжается между рядами");

        // Каждая ветвь названа вопросом расследования: линия сообщает, что
        // именно связывает, и потому не декор.
        for limb in Limb::ALL {
            assert!(
                body.iter()
                    .any(|line| line.contains(limb.label(Capability::TrueColor))),
                "ветвь {:?} обязана быть названа на стволе",
                limb
            );
        }

        let boxes_row = body
            .iter()
            .find(|line| line.contains('╭') || line.contains('┏'))
            .expect("ряд боксов");
        assert!(
            boxes_row.contains("╮──╭") || boxes_row.contains("┓──╭"),
            "боксы одной ветви соединены линией: {boxes_row}"
        );
    }
}
