//! Строки таблицы сущностей: единый источник данных для всех экранов.
//!
//! Вынесено из экранов, потому что одну и ту же строку показывают Overview,
//! Entities и Inspector, а расхождение в вычислении состояния или владельца
//! означало бы, что один экран противоречит другому.

use pulse_core::entity::{Entity, EntityId, EntityKind};
use pulse_core::metric::ids;
use pulse_core::problem::Severity;
use pulse_core::snapshot::Snapshot;
use pulse_core::RelationKind;

use crate::app::{App, SortKey};
use crate::state::StateClass;

/// Строка таблицы сущностей.
#[derive(Clone, Debug)]
pub struct EntityRow {
    pub id: EntityId,
    pub kind: EntityKind,
    pub name: String,
    pub cpu: f64,
    pub memory: f64,
    pub owner: String,
    pub severity: Option<Severity>,
    /// Класс состояния сущности: колонка STATE читается до подписей.
    pub state: StateClass,
    /// Дисковый поток, если он измерен для этого вида сущности.
    ///
    /// `None` означает «величина неизвестна», и колонка покажет заглушку.
    /// Печатать `0.00` там, где измерения нет, - это ложь в интерфейсе.
    pub io: Option<f64>,
    /// Сетевой поток, если он измерен.
    pub net: Option<f64>,
}

pub fn entity_rows(snapshot: &Snapshot, app: &App) -> Vec<EntityRow> {
    let query = app.search_query().unwrap_or("").to_lowercase();
    let mut rows: Vec<EntityRow> = snapshot
        .entities
        .iter()
        .filter(|entity| entity.kind != EntityKind::Host)
        .filter(|entity| {
            app.entities
                .kind_filter
                .is_none_or(|kind| entity.kind == kind)
        })
        .filter(|entity| {
            query.is_empty()
                || entity.name.to_lowercase().contains(&query)
                || entity
                    .labels
                    .get("cmdline")
                    .is_some_and(|c| c.to_lowercase().contains(&query))
        })
        .map(|entity| {
            let severity = snapshot.problems_of(entity.id).map(|p| p.severity).max();
            EntityRow {
                id: entity.id,
                kind: entity.kind,
                name: entity.name.clone(),
                cpu: cpu_of(snapshot, entity),
                memory: memory_of(snapshot, entity),
                owner: owner_of(snapshot, entity),
                severity,
                state: state_of(snapshot, entity, severity),
                io: io_of(snapshot, entity),
                net: net_of(snapshot, entity),
            }
        })
        .collect();

    match app.entities.sort {
        // Раздел 123: по умолчанию сортировка по значимости, а не по CPU.
        // Проблемная сущность обязана быть видна без прокрутки, даже если
        // потребляет мало: нагрузка не равна важности.
        SortKey::Relevance => rows.sort_by(|a, b| {
            b.state
                .cmp(&a.state)
                .then_with(|| {
                    b.cpu
                        .partial_cmp(&a.cpu)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.name.cmp(&b.name))
        }),
        SortKey::Cpu => rows.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        }),
        SortKey::Memory => rows.sort_by(|a, b| {
            b.memory
                .partial_cmp(&a.memory)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        }),
        SortKey::Name => rows.sort_by(|a, b| a.name.cmp(&b.name)),
    }
    // Numeric/relevance comparators above are descending; Name is ascending.
    // Flip only when requested direction differs from the comparator default.
    let native_descending = app.entities.sort != SortKey::Name;
    let requested_descending = app.entities.direction == crate::app::SortDirection::Descending;
    if native_descending != requested_descending {
        rows.reverse();
    }
    rows
}

/// Потребление CPU сущностью в ядрах.
pub fn cpu_of(snapshot: &Snapshot, entity: &Entity) -> f64 {
    let metric = match entity.kind {
        EntityKind::Process => ids::PROC_CPU_CORES,
        _ => ids::CG_CPU_CORES,
    };
    snapshot.value_or(entity.id, metric, 0.0)
}

/// Потребление памяти сущностью в байтах.
pub fn memory_of(snapshot: &Snapshot, entity: &Entity) -> f64 {
    let metric = match entity.kind {
        EntityKind::Process => ids::PROC_RSS,
        _ => ids::CG_MEM_CURRENT,
    };
    snapshot.value_or(entity.id, metric, 0.0)
}

/// Класс состояния сущности для колонки STATE.
///
/// Открытая проблема важнее любых долей: она уже подтверждена гистерезисом и
/// несёт доказательства. Если проблем нет, состояние выводится из доли лимита,
/// а при её отсутствии — из активности, чтобы здоровые сущности выглядели
/// одинаково и аномальная выделялась до чтения подписей.
/// Дисковая скорость сущности: сумма чтения и записи.
///
/// Читаются ТОЛЬКО rate-метрики (единица `BytesPerSecond`). Накопительные
/// счётчики `*_BYTES` здесь запрещены: их показ со суффиксом `/s` и был тем
/// дефектом, из-за которого прототип печатал «9.1 GiB/s» на простаивающем
/// хосте (раздел 154).
///
/// `None` означает «скорость не измерена». Это не то же самое, что ноль:
/// на первом такте разницы ещё нет, и колонка обязана честно молчать.
#[must_use]
pub fn io_of(snapshot: &Snapshot, entity: &Entity) -> Option<f64> {
    let pair = match entity.kind {
        EntityKind::Cgroup | EntityKind::Unit | EntityKind::Container | EntityKind::Pod => {
            (ids::CG_IO_READ_THROUGHPUT, ids::CG_IO_WRITE_THROUGHPUT)
        }
        EntityKind::Disk => (ids::DISK_READ_THROUGHPUT, ids::DISK_WRITE_THROUGHPUT),
        _ => return None,
    };
    let read = snapshot.value(entity.id, pair.0);
    let write = snapshot.value(entity.id, pair.1);
    match (read, write) {
        (None, None) => None,
        (read, write) => Some(read.unwrap_or(0.0) + write.unwrap_or(0.0)),
    }
}

/// Сетевая скорость: измеряется у интерфейсов и хоста.
#[must_use]
pub fn net_of(snapshot: &Snapshot, entity: &Entity) -> Option<f64> {
    if !matches!(entity.kind, EntityKind::NetIf | EntityKind::Host) {
        return None;
    }
    let rx = snapshot.value(entity.id, ids::NETIF_RX_THROUGHPUT);
    let tx = snapshot.value(entity.id, ids::NETIF_TX_THROUGHPUT);
    match (rx, tx) {
        (None, None) => None,
        (rx, tx) => Some(rx.unwrap_or(0.0) + tx.unwrap_or(0.0)),
    }
}

pub fn state_of(snapshot: &Snapshot, entity: &Entity, severity: Option<Severity>) -> StateClass {
    if let Some(severity) = severity {
        return StateClass::from_severity(severity);
    }
    let ratio = match entity.kind {
        EntityKind::Process => None,
        _ => snapshot
            .value(entity.id, ids::CG_MEM_UTIL)
            .into_iter()
            .chain(snapshot.value(entity.id, ids::CG_CPU_THROTTLE_RATIO))
            .fold(None, |worst: Option<f64>, value| {
                Some(worst.map_or(value, |current| current.max(value)))
            }),
    };
    match ratio {
        Some(value) => StateClass::from_ratio(value),
        // Без лимита доля не определена: показываем активность, а не «норму»,
        // иначе занятый процесс выглядел бы как спящий.
        None if cpu_of(snapshot, entity) >= 0.5 => StateClass::Active,
        None => StateClass::Normal,
    }
}

/// Имя владельца: unit, контейнер или pod, а не сырой путь cgroup.
///
/// Это одна из главных ценностей графа: оператор думает про «payment-api»,
/// а не про `/kubepods.slice/pod3f2b.../docker-abc...scope`.
pub fn owner_of(snapshot: &Snapshot, entity: &Entity) -> String {
    // Сущность не может быть владельцем самой себя: подъём по иерархии через
    // cgroup возвращает исходный unit обратно, и в колонке OWNER появлялось бы
    // «unit/init.scope» у самого `init.scope`.
    let named = |candidate: &Entity| -> Option<String> {
        if candidate.id == entity.id {
            return None;
        }
        matches!(
            candidate.kind,
            EntityKind::Unit | EntityKind::Container | EntityKind::Pod
        )
        .then(|| format!("{}/{}", candidate.kind.as_str(), candidate.name))
    };

    // Сначала прямые связи владения.
    for relation in snapshot.relations_of(entity.id, Some(RelationKind::OwnedBy)) {
        let other = if relation.from == entity.id {
            relation.to
        } else {
            relation.from
        };
        if let Some(name) = snapshot.entity(other).and_then(named) {
            return name;
        }
    }
    // Затем — вверх по иерархии.
    for ancestor in snapshot.ancestry(entity.id).into_iter().rev() {
        if ancestor == entity.id {
            continue;
        }
        if let Some(owner) = snapshot.entity(ancestor) {
            if let Some(name) = named(owner) {
                return name;
            }
            for relation in snapshot.relations_of(owner.id, Some(RelationKind::OwnedBy)) {
                let other = if relation.from == owner.id {
                    relation.to
                } else {
                    relation.from
                };
                if let Some(name) = snapshot.entity(other).and_then(named) {
                    return name;
                }
            }
        }
    }
    // Заглушку выбирает рендер: в ASCII-режиме тире недопустимо, а помощник
    // строк не знает возможностей терминала.
    String::new()
}
