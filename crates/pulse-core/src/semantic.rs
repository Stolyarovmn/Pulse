//! Семантическая обработка событий: сырое наблюдение → значимое изменение.
//!
//! Production-инцидент: на реальном хосте ядро генерирует непрерывный поток
//! переименований per-cpu воркеров (`kworker/58:0-mm_percpu_wq` →
//! `kworker/58:0-events`). Это наблюдаемый факт, но не операционное изменение:
//! попадая в Recent Changes, Story и relevance, он вытесняет оттуда всё
//! диагностически ценное.
//!
//! Здесь живёт единственное место, где решается, что считать значимым.
//! Сырой поток при этом не уничтожается: он остаётся в журнале истории и
//! доступен через отдельный вид.
//!
//! Конвейер:
//!
//! ```text
//! RAW → classify → drop Tier0 → group by identity → budget → MEANINGFUL
//! ```

use std::collections::HashMap;

use crate::entity::{EntityId, EntityKind};
use crate::event::{Event, EventKind};
use crate::problem::Severity;
use crate::time::Timestamp;

/// Уровень значимости события.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Significance {
    /// Tier 0: рутинный шум ядра и метаданных. Не показывается по умолчанию.
    Noise,
    /// Tier 1: операционное изменение. Может попасть в Recent Changes.
    Operational,
    /// Tier 2: диагностическое событие. Показывается всегда.
    Diagnostic,
}

impl Significance {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Significance::Noise => "noise",
            Significance::Operational => "operational",
            Significance::Diagnostic => "diagnostic",
        }
    }
}

/// Имя принадлежит потоку ядра, а не пользовательскому процессу.
///
/// Ядро постоянно переименовывает per-cpu воркеры под текущую очередь работ.
/// Точного признака «поток ядра» в имени нет, поэтому используется список
/// стабильных префиксов ядра Linux: он не меняется между релизами и не может
/// совпасть с именем сервиса или контейнера.
#[must_use]
pub fn is_kernel_thread(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "kworker/",
        "ksoftirqd/",
        "migration/",
        "cpuhp/",
        "idle_inject/",
        "irq/",
        "rcu_",
        "rcuo",
        "kswapd",
        "kcompactd",
        "khugepaged",
        "kthreadd",
        "kblockd",
        "kdevtmpfs",
        "kintegrityd",
        "kverityd",
        "ksmd",
        "khungtaskd",
        "kauditd",
        "oom_reaper",
        "writeback",
        "watchdogd",
        "netns",
        "kstrp",
        "zswap",
        "scsi_",
        "nvme-",
        "jbd2/",
        "ext4-",
        "xfs-",
        "md",
        "dm_bufio",
        "edac-poller",
        "acpi_thermal_pm",
        "devfreq_wq",
        "inet_frag_wq",
        "kmpath",
        "loop",
        "blkcg_punt_bio",
    ];
    let name = name.trim_start_matches('[').trim_end_matches(']');
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Имя принадлежит служебному сетевому интерфейсу контейнерной сети.
///
/// `veth`-пара создаётся и удаляется на каждый старт и стоп контейнера, а
/// `docker0`/`br-*`/`cni*` — мосты рантайма. Их жизненный цикл — следствие
/// операции с контейнером, а не отдельный операционный факт: на реальном хосте
/// Story состояла из строк `! vethcf9dac0 deleted`.
#[must_use]
pub fn is_virtual_netif(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "veth",
        "docker",
        "br-",
        "cni",
        "flannel",
        "cali",
        "lxc",
        "tap",
        "virbr",
        "tun",
        "vxlan",
        "dummy",
        "nodelocaldns",
        "kube-ipvs",
        "vnet",
    ];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Имя выглядит как непрозрачный идентификатор контейнера.
///
/// Событие `a2dc50289218 deleted` не сообщает оператору ничего: он не знает,
/// что это было. Пока имя не разрешимо в человекочитаемое, такое событие не
/// является операционным изменением и живёт в сыром потоке.
#[must_use]
pub fn is_opaque_identifier(name: &str) -> bool {
    let core = name
        .trim_start_matches("docker-")
        .trim_start_matches("crio-")
        .trim_start_matches("cri-containerd-")
        .trim_start_matches("libpod-")
        .trim_end_matches(".scope");
    core.len() >= 8 && core.chars().all(|c| c.is_ascii_hexdigit())
}

/// Классифицирует событие по значимости.
///
/// Правило: значимость определяется парой «вид события + вид сущности», а не
/// одним видом события. `metadata_changed` у сервиса — операционный факт,
/// `metadata_changed` у per-cpu воркера ядра — шум.
#[must_use]
pub fn significance(event: &Event) -> Significance {
    match event.kind {
        // Диагностика: показывается всегда, независимо от сущности.
        EventKind::OomKill
        | EventKind::ProblemOpened
        | EventKind::ProblemClosed
        | EventKind::CollectorError
        | EventKind::ObservationStarted => Significance::Diagnostic,

        // Перезапуск логической сущности — всегда диагностика: именно его ищут
        // при разборе инцидента.
        EventKind::Restarted => {
            if is_kernel_thread(&event.entity_name) {
                Significance::Noise
            } else {
                Significance::Diagnostic
            }
        }

        EventKind::Created | EventKind::Deleted => {
            if is_kernel_thread(&event.entity_name) || is_opaque_identifier(&event.entity_name) {
                return Significance::Noise;
            }
            if matches!(event.entity_kind, Some(EntityKind::NetIf))
                && is_virtual_netif(&event.entity_name)
            {
                return Significance::Noise;
            }
            match event.entity_kind {
                // Появление и исчезновение сервиса, контейнера, pod, диска или
                // интерфейса — операционное изменение.
                Some(
                    EntityKind::Unit
                    | EntityKind::Container
                    | EntityKind::Pod
                    | EntityKind::Disk
                    | EntityKind::NetIf,
                ) => Significance::Operational,
                // Процессы и cgroup появляются и исчезают тысячами за минуту.
                _ => Significance::Noise,
            }
        }

        EventKind::MetadataChanged | EventKind::Reparented => {
            if is_kernel_thread(&event.entity_name) || is_opaque_identifier(&event.entity_name) {
                return Significance::Noise;
            }
            if matches!(event.entity_kind, Some(EntityKind::NetIf))
                && is_virtual_netif(&event.entity_name)
            {
                return Significance::Noise;
            }
            match event.entity_kind {
                Some(EntityKind::Unit | EntityKind::Container | EntityKind::Pod) => {
                    Significance::Operational
                }
                Some(EntityKind::Disk | EntityKind::NetIf) => Significance::Operational,
                // Переименование и смена родителя процесса — следствие того,
                // как ядро переиспользует задачи, а не операционное решение.
                _ => Significance::Noise,
            }
        }
    }
}

/// Значимое событие: одно наблюдение или свёрнутая группа повторов.
#[derive(Clone, PartialEq, Debug)]
pub struct MeaningfulEvent {
    /// Время первого наблюдения группы.
    pub at: Timestamp,
    /// Время последнего наблюдения группы.
    pub last_at: Timestamp,
    pub kind: EventKind,
    pub severity: Severity,
    pub significance: Significance,
    pub entity: Option<EntityId>,
    pub entity_kind: Option<EntityKind>,
    pub entity_name: String,
    pub detail: String,
    /// Сколько сырых событий свёрнуто в эту строку.
    pub count: u32,
}

impl MeaningfulEvent {
    /// Группа, а не одиночное наблюдение: строка раскрывается по `Enter`.
    #[must_use]
    pub const fn is_group(&self) -> bool {
        self.count > 1
    }
}

/// Результат семантической обработки одного окна событий.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// Значимые события, свежие первыми.
    pub items: Vec<MeaningfulEvent>,
    /// Сколько сырых Tier0-событий подавлено.
    pub suppressed: u64,
    /// Сколько сырых событий свёрнуто в группы.
    pub grouped: u64,
    /// Сколько значимых событий не поместилось в бюджет.
    pub over_budget: u64,
}

/// Ключ группировки: одинаковые повторы одного факта.
///
/// Идентичность сущности в ключ не входит намеренно: сотня перезапусков сотни
/// контейнеров одного сервиса — это один факт «контейнеры перезапускаются», а
/// не сотня отдельных строк. Разделение по виду сущности сохраняется, чтобы не
/// смешивать перезапуск unit и перезапуск pod.
type GroupKey = (EventKind, Option<EntityKind>, String);

/// Сворачивает сырой поток в значимые события с информационным бюджетом.
///
/// `budget` — максимум строк для одного видимого окна (§ Timeline event budget).
/// Порядок: сначала диагностика, затем свежесть. Это гарантирует, что OOM не
/// вытеснится десятком операционных изменений.
#[must_use]
pub fn summarize(events: &[Event], budget: usize) -> Summary {
    let mut summary = Summary::default();
    let mut groups: HashMap<GroupKey, usize> = HashMap::new();

    for event in events {
        let significance = significance(event);
        if significance == Significance::Noise {
            summary.suppressed = summary.suppressed.saturating_add(1);
            continue;
        }

        let key = (
            event.kind,
            event.entity_kind,
            group_label(event).to_string(),
        );
        if let Some(&index) = groups.get(&key) {
            if let Some(existing) = summary.items.get_mut(index) {
                existing.count = existing.count.saturating_add(1);
                existing.last_at = existing.last_at.max(event.at);
                existing.at = existing.at.min(event.at);
                existing.severity = existing.severity.max(event.severity);
                summary.grouped = summary.grouped.saturating_add(1);
                continue;
            }
        }

        groups.insert(key, summary.items.len());
        summary.items.push(MeaningfulEvent {
            at: event.at,
            last_at: event.at,
            kind: event.kind,
            severity: event.severity,
            significance,
            entity: event.entity,
            entity_kind: event.entity_kind,
            entity_name: event.entity_name.clone(),
            detail: event.detail.clone(),
            count: 1,
        });
    }

    // Диагностика важнее операционных изменений; внутри уровня — свежие раньше.
    summary.items.sort_by(|a, b| {
        b.significance
            .cmp(&a.significance)
            .then(b.severity.cmp(&a.severity))
            .then(b.last_at.as_millis().cmp(&a.last_at.as_millis()))
    });

    if summary.items.len() > budget {
        summary.over_budget = u64::try_from(summary.items.len() - budget).unwrap_or(u64::MAX);
        summary.items.truncate(budget);
    }
    summary
}

/// Подпись группы: имя сущности или её вид, если имя уникально для экземпляра.
///
/// Контейнеры и процессы имеют уникальные имена/хеши, поэтому сотня их
/// перезапусков схлопывается по виду. Сервисы и pod имеют устойчивые имена, и
/// схлопывать их между собой нельзя: `postgres restarted` и `redis restarted` —
/// разные факты.
fn group_label(event: &Event) -> &str {
    match event.entity_kind {
        Some(EntityKind::Unit | EntityKind::Pod | EntityKind::Disk | EntityKind::NetIf) => {
            event.entity_name.as_str()
        }
        Some(EntityKind::Container) => "container",
        Some(EntityKind::Process) => "process",
        Some(EntityKind::Cgroup) => "cgroup",
        Some(EntityKind::Host) | None => event.entity_name.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: EventKind, name: &str, entity_kind: Option<EntityKind>) -> Event {
        let mut event = Event::new(Timestamp::from_millis(1_000), kind, name);
        event.entity_kind = entity_kind;
        event
    }

    #[test]
    fn kernel_worker_churn_is_noise() {
        let names = [
            "kworker/58:0-events",
            "kworker/52:3-cgroup_free",
            "ksoftirqd/3",
            "migration/7",
            "rcu_preempt",
            "kswapd0",
            "jbd2/sda2-8",
        ];
        for name in names {
            assert!(is_kernel_thread(name), "{name} обязано быть потоком ядра");
            let raw = event(EventKind::MetadataChanged, name, Some(EntityKind::Process));
            assert_eq!(
                significance(&raw),
                Significance::Noise,
                "{name} обязано классифицироваться как шум"
            );
        }
    }

    #[test]
    fn service_names_are_not_kernel_threads() {
        for name in [
            "docker.service",
            "postgres",
            "containerd.service",
            "checkout-api",
            "kube-proxy",
        ] {
            assert!(!is_kernel_thread(name), "{name} не является потоком ядра");
        }
    }

    #[test]
    fn diagnostic_events_survive_classification() {
        for kind in [
            EventKind::OomKill,
            EventKind::ProblemOpened,
            EventKind::ProblemClosed,
            EventKind::CollectorError,
            EventKind::ObservationStarted,
        ] {
            let raw = event(kind, "host", Some(EntityKind::Host));
            assert_eq!(significance(&raw), Significance::Diagnostic, "{kind}");
        }
    }

    #[test]
    fn service_restart_is_diagnostic_and_process_rename_is_not() {
        let restart = event(
            EventKind::Restarted,
            "docker.service",
            Some(EntityKind::Unit),
        );
        assert_eq!(significance(&restart), Significance::Diagnostic);

        let rename = event(
            EventKind::MetadataChanged,
            "python3",
            Some(EntityKind::Process),
        );
        assert_eq!(significance(&rename), Significance::Noise);
    }

    #[test]
    fn container_network_churn_is_noise() {
        // Реальный хост: Story состояла из удалений veth-пар и хешей контейнеров.
        let veth = event(EventKind::Deleted, "vethcf9dac0", Some(EntityKind::NetIf));
        assert_eq!(significance(&veth), Significance::Noise);

        let hash = event(
            EventKind::Deleted,
            "a2dc50289218",
            Some(EntityKind::Container),
        );
        assert_eq!(
            significance(&hash),
            Significance::Noise,
            "неразрешённый хеш не сообщает оператору, что исчезло"
        );

        // Настоящий интерфейс и названный контейнер остаются операционными.
        let eth = event(EventKind::Deleted, "eth0", Some(EntityKind::NetIf));
        assert_eq!(significance(&eth), Significance::Operational);
        let named = event(
            EventKind::Deleted,
            "checkout-api",
            Some(EntityKind::Container),
        );
        assert_eq!(significance(&named), Significance::Operational);
    }

    #[test]
    fn virtual_netif_detection_covers_runtimes_but_not_real_links() {
        for name in ["veth1a2b", "docker0", "br-1f2e", "cni0", "cali7ab", "tap0"] {
            assert!(is_virtual_netif(name), "{name} служебный");
        }
        for name in ["eth0", "eno1", "enp3s0", "wlan0", "bond0"] {
            assert!(!is_virtual_netif(name), "{name} настоящий");
        }
    }

    #[test]
    fn summarize_suppresses_noise_and_keeps_diagnostics() {
        let mut events = Vec::new();
        for index in 0..300 {
            events.push(event(
                EventKind::MetadataChanged,
                &format!("kworker/{index}:0-events"),
                Some(EntityKind::Process),
            ));
        }
        events.push(event(
            EventKind::OomKill,
            "postgres",
            Some(EntityKind::Process),
        ));

        let summary = summarize(&events, 20);
        assert_eq!(
            summary.suppressed, 300,
            "весь kworker-шум обязан быть подавлен"
        );
        assert_eq!(summary.items.len(), 1, "остаётся только диагностика");
        assert_eq!(summary.items[0].kind, EventKind::OomKill);
        assert!(!summary.items[0].is_group());
    }

    #[test]
    fn repeated_meaningful_events_collapse_into_groups() {
        let mut events = Vec::new();
        for index in 0..14 {
            let mut raw = event(
                EventKind::Restarted,
                &format!("container-{index}"),
                Some(EntityKind::Container),
            );
            raw.at = Timestamp::from_millis(1_000 + index * 100);
            events.push(raw);
        }

        let summary = summarize(&events, 20);
        assert_eq!(summary.items.len(), 1, "повторы обязаны свернуться");
        let group = &summary.items[0];
        assert_eq!(group.count, 14);
        assert!(group.is_group());
        assert_eq!(group.at, Timestamp::from_millis(1_000));
        assert_eq!(group.last_at, Timestamp::from_millis(2_300));
        assert_eq!(summary.grouped, 13);
    }

    #[test]
    fn distinct_services_do_not_collapse() {
        let events = vec![
            event(
                EventKind::Restarted,
                "postgres.service",
                Some(EntityKind::Unit),
            ),
            event(
                EventKind::Restarted,
                "redis.service",
                Some(EntityKind::Unit),
            ),
        ];
        let summary = summarize(&events, 20);
        assert_eq!(summary.items.len(), 2, "разные сервисы — разные факты");
    }

    #[test]
    fn budget_is_enforced_and_diagnostics_win() {
        let mut events = Vec::new();
        for index in 0..50 {
            events.push(event(
                EventKind::Created,
                &format!("unit-{index}.service"),
                Some(EntityKind::Unit),
            ));
        }
        let mut oom = event(EventKind::OomKill, "postgres", Some(EntityKind::Process));
        oom.at = Timestamp::from_millis(500);
        events.insert(0, oom);

        let summary = summarize(&events, 5);
        assert_eq!(summary.items.len(), 5);
        assert_eq!(
            summary.items[0].kind,
            EventKind::OomKill,
            "диагностика обязана выжить в бюджете даже будучи самой старой"
        );
        assert_eq!(summary.over_budget, 46);
    }
}
