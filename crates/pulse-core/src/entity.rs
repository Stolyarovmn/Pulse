//! Сущности temporal entity graph: устойчивая идентичность, вид, метки, время жизни.
//!
//! Идентичность — центральный инвариант Pulse. PID переиспользуется ядром, путь cgroup
//! может быть переименован, имя контейнера может повторяться. Поэтому ключ сущности
//! включает достаточную для различения информацию, а внутренний `EntityId` живёт
//! в арене и защищён счётчиком поколения от ошибок ABA.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// Внутренний идентификатор сущности: индекс в арене + поколение слота.
///
/// Поколение растёт при повторном использовании слота, поэтому устаревший `EntityId`
/// не может случайно указать на другую сущность.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId {
    index: u32,
    generation: u32,
}

impl EntityId {
    /// Специальное значение «сущности нет». Не выдаётся ареной.
    pub const NONE: EntityId = EntityId {
        index: u32::MAX,
        generation: u32::MAX,
    };

    #[must_use]
    pub const fn new(index: u32, generation: u32) -> Self {
        EntityId { index, generation }
    }

    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    #[must_use]
    pub const fn from_u64(raw: u64) -> Self {
        EntityId {
            index: (raw & 0xFFFF_FFFF) as u32,
            generation: (raw >> 32) as u32,
        }
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == EntityId::NONE {
            f.write_str("EntityId(none)")
        } else {
            write!(f, "EntityId({}g{})", self.index, self.generation)
        }
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_u64())
    }
}

/// Вид сущности. Определяет и навигацию в TUI, и набор применимых метрик.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Host,
    Cgroup,
    Unit,
    Process,
    Container,
    Pod,
    Disk,
    NetIf,
}

impl EntityKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EntityKind::Host => "host",
            EntityKind::Cgroup => "cgroup",
            EntityKind::Unit => "unit",
            EntityKind::Process => "process",
            EntityKind::Container => "container",
            EntityKind::Pod => "pod",
            EntityKind::Disk => "disk",
            EntityKind::NetIf => "netif",
        }
    }

    /// Приоритет вида при сортировке дерева сущностей: владельцы выше исполнителей.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            EntityKind::Host => 0,
            EntityKind::Pod => 1,
            EntityKind::Container => 2,
            EntityKind::Unit => 3,
            EntityKind::Cgroup => 4,
            EntityKind::Process => 5,
            EntityKind::Disk => 6,
            EntityKind::NetIf => 7,
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Известные container runtime, распознаваемые по пути cgroup.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Docker,
    Containerd,
    CriO,
    Podman,
    Lxc,
    Unknown,
}

impl Runtime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Runtime::Docker => "docker",
            Runtime::Containerd => "containerd",
            Runtime::CriO => "cri-o",
            Runtime::Podman => "podman",
            Runtime::Lxc => "lxc",
            Runtime::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Устойчивый ключ идентичности сущности.
///
/// Инварианты:
/// * `Process` различается по паре `(pid, start_ticks)` — защита от переиспользования PID;
/// * `Cgroup` идентифицируется inode каталога, а не путём — защита от переименования;
/// * `Unit` идентифицируется именем: перезапуск unit сохраняет логическую сущность,
///   смена backing-cgroup порождает событие `EntityRestarted`.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum EntityKey {
    Host { boot_id: Box<str> },
    Cgroup { cgroup_id: u64 },
    Unit { name: Box<str> },
    Process { pid: i32, start_ticks: u64 },
    Container { runtime: Runtime, id: Box<str> },
    Pod { namespace: Box<str>, name: Box<str> },
    Disk { name: Box<str> },
    NetIf { name: Box<str> },
}

impl EntityKey {
    #[must_use]
    pub const fn kind(&self) -> EntityKind {
        match self {
            EntityKey::Host { .. } => EntityKind::Host,
            EntityKey::Cgroup { .. } => EntityKind::Cgroup,
            EntityKey::Unit { .. } => EntityKind::Unit,
            EntityKey::Process { .. } => EntityKind::Process,
            EntityKey::Container { .. } => EntityKind::Container,
            EntityKey::Pod { .. } => EntityKind::Pod,
            EntityKey::Disk { .. } => EntityKind::Disk,
            EntityKey::NetIf { .. } => EntityKind::NetIf,
        }
    }

    /// PID процесса, если ключ описывает процесс. Используется действиями (signal) и inspector.
    #[must_use]
    pub const fn pid(&self) -> Option<i32> {
        match self {
            EntityKey::Process { pid, .. } => Some(*pid),
            _ => None,
        }
    }

    /// Строковый идентификатор для экспорта и журналов. Уже безопасен для терминала:
    /// все компоненты либо числовые, либо санитизированы при создании сущности.
    #[must_use]
    pub fn identity(&self) -> String {
        match self {
            EntityKey::Host { boot_id } => format!("host/{boot_id}"),
            EntityKey::Cgroup { cgroup_id } => format!("cgroup/{cgroup_id}"),
            EntityKey::Unit { name } => format!("unit/{name}"),
            EntityKey::Process { pid, start_ticks } => format!("process/{pid}/{start_ticks}"),
            EntityKey::Container { runtime, id } => format!("container/{runtime}/{id}"),
            EntityKey::Pod { namespace, name } => format!("pod/{namespace}/{name}"),
            EntityKey::Disk { name } => format!("disk/{name}"),
            EntityKey::NetIf { name } => format!("netif/{name}"),
        }
    }
}

/// Метки сущности. Малое число пар на сущность, поэтому вектор дешевле хеш-таблицы.
///
/// Ключ — статическая строка: набор ключей задаётся кодом, а не входными данными.
/// Это ограничивает cardinality на входе, а не в экспортере.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Labels(Vec<(&'static str, String)>);

impl Labels {
    #[must_use]
    pub const fn new() -> Self {
        Labels(Vec::new())
    }

    /// Максимальное число метк на сущность. Защита от неограниченного роста памяти.
    pub const MAX: usize = 16;

    /// Максимальная длина значения метки. Совпадает по смыслу с лимитами VictoriaMetrics.
    pub const MAX_VALUE_LEN: usize = 512;

    /// Добавляет или заменяет метку.
    ///
    /// Значение приходит из недоверенного источника (`cmdline`, путь cgroup,
    /// цель `exe`), поэтому здесь и проходит граница доверия:
    ///
    /// 1. `sanitize_display` снимает управляющие последовательности —
    ///    `SECURITY.md` обещает, что в доверенном состоянии лежат уже
    ///    очищенные строки, но раньше этот инвариант API не обеспечивал;
    /// 2. усечение идёт по границе символа. `String::truncate` требует
    ///    границу UTF-8 и паникует внутри многобайтового символа, то есть
    ///    локальный процесс мог уронить агент, подобрав длину аргумента.
    pub fn set(&mut self, key: &'static str, value: impl Into<String>) {
        let mut value = crate::redact::sanitize_with_limit(&value.into(), Self::MAX_VALUE_LEN);
        if value.len() > Self::MAX_VALUE_LEN {
            // Ближайшая граница символа не позже лимита: значение остаётся
            // валидным UTF-8 и не превышает предел.
            let mut cut = Self::MAX_VALUE_LEN;
            while cut > 0 && !value.is_char_boundary(cut) {
                cut -= 1;
            }
            value.truncate(cut);
        }
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => {
                if self.0.len() < Self::MAX {
                    self.0.push((key, value));
                }
            }
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.0.iter().map(|(k, v)| (*k, v.as_str()))
    }
}

/// Живая сущность графа.
#[derive(Clone, Debug)]
pub struct Entity {
    pub id: EntityId,
    pub key: EntityKey,
    pub kind: EntityKind,
    /// Отображаемое имя. Санитизировано: без управляющих символов и ANSI-escape.
    pub name: String,
    pub parent: Option<EntityId>,
    pub labels: Labels,
    /// Логическое имя для детекции перезапуска (`postgres.service`, `payment-api`).
    pub logical: Option<String>,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
    pub alive: bool,
}

impl Entity {
    /// Отображаемая строка вида `container/payment-api`.
    #[must_use]
    pub fn display(&self) -> String {
        format!("{}/{}", self.kind.as_str(), self.name)
    }
}

/// Описание сущности, которое передаёт коллектор. Граф сам решает,
/// создать новую сущность или обновить существующую.
#[derive(Clone, Debug)]
pub struct EntitySpec {
    pub key: EntityKey,
    pub name: String,
    pub parent: Option<EntityId>,
    pub logical: Option<String>,
    pub labels: Labels,
}

impl EntitySpec {
    #[must_use]
    pub fn new(key: EntityKey, name: impl Into<String>) -> Self {
        EntitySpec {
            key,
            name: name.into(),
            parent: None,
            logical: None,
            labels: Labels::new(),
        }
    }

    #[must_use]
    pub fn parent(mut self, parent: EntityId) -> Self {
        self.parent = Some(parent);
        self
    }

    #[must_use]
    pub fn logical(mut self, logical: impl Into<String>) -> Self {
        self.logical = Some(logical.into());
        self
    }

    #[must_use]
    pub fn label(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.labels.set(key, value);
        self
    }
}

/// Запись жизненного цикла сущности для истории. Хранилище держит их
/// в пределах retention и по ним восстанавливает граф на момент `T`.
#[derive(Clone, Debug)]
pub struct EntityRecord {
    pub id: EntityId,
    pub key: EntityKey,
    pub kind: EntityKind,
    pub name: String,
    pub parent: Option<EntityId>,
    pub labels: Labels,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
    pub alive: bool,
}

impl EntityRecord {
    #[must_use]
    pub fn from_entity(e: &Entity) -> Self {
        EntityRecord {
            id: e.id,
            key: e.key.clone(),
            kind: e.kind,
            name: e.name.clone(),
            parent: e.parent,
            labels: e.labels.clone(),
            first_seen: e.first_seen,
            last_seen: e.last_seen,
            alive: e.alive,
        }
    }

    /// Была ли сущность живой в момент `at`.
    #[must_use]
    pub const fn alive_at(&self, at: Timestamp) -> bool {
        at.as_millis() >= self.first_seen.as_millis()
            && at.as_millis() <= self.last_seen.as_millis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_roundtrips_through_u64() {
        let id = EntityId::new(7, 3);
        assert_eq!(EntityId::from_u64(id.as_u64()), id);
        assert_eq!(id.index(), 7);
        assert_eq!(id.generation(), 3);
    }

    #[test]
    fn same_pid_different_start_is_different_entity() {
        let a = EntityKey::Process {
            pid: 100,
            start_ticks: 1,
        };
        let b = EntityKey::Process {
            pid: 100,
            start_ticks: 2,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn labels_are_bounded_in_count_and_length() {
        let mut labels = Labels::new();
        for i in 0..(Labels::MAX + 5) {
            // Ключи статические, поэтому используем небольшой фиксированный набор.
            let key: &'static str = match i % 3 {
                0 => "a",
                1 => "b",
                _ => "c",
            };
            labels.set(key, "x".repeat(Labels::MAX_VALUE_LEN + 10));
        }
        assert!(labels.len() <= Labels::MAX);
        for (_, v) in labels.iter() {
            assert_eq!(v.len(), Labels::MAX_VALUE_LEN);
        }
    }

    #[test]
    fn label_set_replaces_existing_key() {
        let mut labels = Labels::new();
        labels.set("state", "running");
        labels.set("state", "sleeping");
        assert_eq!(labels.len(), 1);
        assert_eq!(labels.get("state"), Some("sleeping"));
    }
}
