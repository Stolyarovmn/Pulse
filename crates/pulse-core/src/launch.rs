//! Цепочка запуска процесса по `ppid` и осторожная классификация источника.
//!
//! Это **spawn-lineage**, а не ownership-граф. `unit → cgroup → process`
//! отвечает «кто владеет ресурсом», но не доказывает, кто породил PID.
//! Родительская цепочка строится только по фактически собранным процессам и
//! обязана явно сообщать обрыв: бюджет обхода, churn или права могут оставить
//! предка вне снимка. Молчаливое `… → nginx` было бы ложной каузальностью.

use std::collections::{HashMap, HashSet};

use crate::{Entity, EntityId, EntityKey, EntityKind, Snapshot};

/// Максимальная глубина защиты от повреждённой/cyclic цепочки.
const MAX_DEPTH: usize = 64;

/// Один подтверждённый шаг родительской цепочки.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessHop {
    pub entity: EntityId,
    pub pid: i32,
    pub name: String,
}

/// Почему цепочка закончилась.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AncestryEnd {
    /// Дошли до процесса с PPID 0 — корень дерева процессов.
    Root,
    /// Родительский PID известен, но его сущности нет в текущем снимке.
    MissingParent { pid: i32 },
    /// У сущности нет пригодной метки `ppid`.
    MissingPpid,
    /// Повреждённая цепочка зациклилась.
    Cycle { pid: i32 },
    /// Сработал внутренний предел глубины.
    DepthLimit,
}

impl AncestryEnd {
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, AncestryEnd::Root)
    }
}

/// Выявленный источник запуска. Это эвристика, не установленная причина.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchSource {
    ContainerRuntime,
    Cron,
    Ssh,
    InteractiveShell,
    Systemd,
    Unknown,
}

impl LaunchSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            LaunchSource::ContainerRuntime => "container runtime",
            LaunchSource::Cron => "cron",
            LaunchSource::Ssh => "ssh session",
            LaunchSource::InteractiveShell => "interactive shell",
            LaunchSource::Systemd => "systemd",
            LaunchSource::Unknown => "unknown",
        }
    }
}

/// Эвристика и видимое доказательство, на котором она основана.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchInference {
    pub source: LaunchSource,
    pub evidence: Option<ProcessHop>,
}

/// Подтверждённая часть ancestry и честная причина окончания.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessAncestry {
    /// От старшего найденного предка к целевому процессу.
    pub chain: Vec<ProcessHop>,
    pub end: AncestryEnd,
    pub inference: LaunchInference,
}

/// Строит ancestry для process-сущности текущего снимка.
///
/// Возвращает `None` для не-процесса или отсутствующей сущности. Это важно при
/// `collect_processes = false`: каузальность не подделывается из cgroup/unit.
#[must_use]
pub fn process_ancestry(snapshot: &Snapshot, target: EntityId) -> Option<ProcessAncestry> {
    let target = snapshot.entity(target)?;
    if !matches!(&target.key, EntityKey::Process { .. }) {
        return None;
    }

    let by_pid: HashMap<i32, &Entity> = snapshot
        .entities_of_kind(EntityKind::Process)
        .filter_map(|entity| match entity.key {
            EntityKey::Process { pid, .. } => Some((pid, entity)),
            _ => None,
        })
        .collect();

    let mut reversed = Vec::new();
    let mut seen = HashSet::new();
    let mut current = target;
    let mut end = AncestryEnd::DepthLimit;

    for _ in 0..MAX_DEPTH {
        let EntityKey::Process { pid, .. } = current.key else {
            break;
        };
        if !seen.insert(current.id) {
            end = AncestryEnd::Cycle { pid };
            break;
        }
        reversed.push(hop(current, pid));

        let Some(ppid) = current
            .labels
            .get("ppid")
            .and_then(|raw| raw.parse::<i32>().ok())
        else {
            end = AncestryEnd::MissingPpid;
            break;
        };
        if ppid <= 0 {
            end = AncestryEnd::Root;
            break;
        }
        let Some(parent) = by_pid.get(&ppid).copied() else {
            end = AncestryEnd::MissingParent { pid: ppid };
            break;
        };
        current = parent;
    }

    reversed.reverse();
    let inference = infer_source(&reversed);
    Some(ProcessAncestry {
        chain: reversed,
        end,
        inference,
    })
}

fn hop(entity: &Entity, pid: i32) -> ProcessHop {
    ProcessHop {
        entity: entity.id,
        pid,
        name: entity.name.clone(),
    }
}

fn infer_source(chain: &[ProcessHop]) -> LaunchInference {
    // Target не является собственным источником. Для `systemd → sshd` сам
    // daemon запущен systemd; только его потомок может считаться SSH-сессией.
    let ancestors = chain
        .split_last()
        .map_or(&[][..], |(_, ancestors)| ancestors);
    // Более специфичные источники важнее shell/systemd. В цепочке
    // `systemd → sshd → bash → app` ответ — SSH, а не bash и не systemd.
    if let Some(evidence) = ancestors.iter().find(|hop| {
        let name = hop.name.to_ascii_lowercase();
        name.starts_with("containerd-shim")
            || matches!(
                name.as_str(),
                "containerd" | "dockerd" | "podman" | "conmon"
            )
    }) {
        return LaunchInference {
            source: LaunchSource::ContainerRuntime,
            evidence: Some(evidence.clone()),
        };
    }
    if let Some(evidence) = ancestors.iter().find(|hop| {
        matches!(
            hop.name.to_ascii_lowercase().as_str(),
            "cron" | "crond" | "anacron"
        )
    }) {
        return LaunchInference {
            source: LaunchSource::Cron,
            evidence: Some(evidence.clone()),
        };
    }
    if let Some(evidence) = ancestors.iter().find(|hop| {
        let name = hop.name.to_ascii_lowercase();
        name == "sshd" || name.starts_with("sshd:")
    }) {
        return LaunchInference {
            source: LaunchSource::Ssh,
            evidence: Some(evidence.clone()),
        };
    }

    if let Some(evidence) = ancestors.iter().find(|hop| {
        matches!(
            hop.name.to_ascii_lowercase().as_str(),
            "bash" | "sh" | "zsh" | "fish" | "dash"
        )
    }) {
        return LaunchInference {
            source: LaunchSource::InteractiveShell,
            evidence: Some(evidence.clone()),
        };
    }
    if let Some(evidence) = ancestors
        .iter()
        .find(|hop| hop.pid == 1 && hop.name.eq_ignore_ascii_case("systemd"))
    {
        return LaunchInference {
            source: LaunchSource::Systemd,
            evidence: Some(evidence.clone()),
        };
    }
    LaunchInference {
        source: LaunchSource::Unknown,
        evidence: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{EntityKey, EntitySpec};
    use crate::time::Timestamp;
    use crate::{AgentStats, EntityGraph, LatestValues};

    fn snapshot(processes: &[(i32, i32, &str)]) -> Snapshot {
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        for (pid, ppid, name) in processes {
            let mut spec = EntitySpec::new(
                EntityKey::Process {
                    pid: *pid,
                    start_ticks: *pid as u64 + 100,
                },
                *name,
            );
            spec.labels.set("ppid", ppid.to_string());
            graph.upsert(spec);
        }
        let batch = graph.end_tick();
        Snapshot::build(
            &graph,
            LatestValues::new(),
            Vec::new(),
            batch.events,
            AgentStats::default(),
            "host",
            "boot",
        )
    }

    fn id_of(snapshot: &Snapshot, pid: i32) -> EntityId {
        snapshot
            .entities_of_kind(EntityKind::Process)
            .find_map(|entity| match entity.key {
                EntityKey::Process { pid: found, .. } if found == pid => Some(entity.id),
                _ => None,
            })
            .expect("process")
    }

    #[test]
    fn ssh_chain_is_visible_and_classified_as_heuristic() {
        let snapshot = snapshot(&[
            (1, 0, "systemd"),
            (100, 1, "sshd"),
            (101, 100, "bash"),
            (102, 101, "pulse"),
        ]);
        let ancestry = process_ancestry(&snapshot, id_of(&snapshot, 102)).expect("ancestry");
        let names: Vec<&str> = ancestry.chain.iter().map(|hop| hop.name.as_str()).collect();
        assert_eq!(names, vec!["systemd", "sshd", "bash", "pulse"]);
        assert!(ancestry.end.is_complete());
        assert_eq!(ancestry.inference.source, LaunchSource::Ssh);
        assert_eq!(
            ancestry.inference.evidence.as_ref().map(|hop| hop.pid),
            Some(100)
        );
    }

    #[test]
    fn sshd_daemon_itself_is_started_by_systemd_not_by_ssh() {
        let snapshot = snapshot(&[(1, 0, "systemd"), (100, 1, "sshd")]);
        let ancestry = process_ancestry(&snapshot, id_of(&snapshot, 100)).expect("ancestry");
        assert_eq!(ancestry.inference.source, LaunchSource::Systemd);
        assert_eq!(
            ancestry.inference.evidence.as_ref().map(|hop| hop.pid),
            Some(1)
        );
    }

    #[test]
    fn missing_parent_is_explicit_not_a_fake_complete_chain() {
        let snapshot = snapshot(&[(500, 499, "worker")]);
        let ancestry = process_ancestry(&snapshot, id_of(&snapshot, 500)).expect("ancestry");
        assert_eq!(ancestry.end, AncestryEnd::MissingParent { pid: 499 });
        assert!(!ancestry.end.is_complete());
        assert_eq!(ancestry.chain.len(), 1);
    }

    #[test]
    fn no_process_data_never_fabricates_ancestry_from_ownership() {
        let graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        let snapshot = Snapshot::build(
            &graph,
            LatestValues::new(),
            Vec::new(),
            Vec::new(),
            AgentStats::default(),
            "host",
            "boot",
        );
        assert_eq!(process_ancestry(&snapshot, snapshot.host), None);
    }

    #[test]
    fn container_runtime_prefix_is_recognised() {
        let snapshot = snapshot(&[
            (1, 0, "systemd"),
            (200, 1, "containerd-shim-runc-v2"),
            (201, 200, "app"),
        ]);
        let ancestry = process_ancestry(&snapshot, id_of(&snapshot, 201)).expect("ancestry");
        assert_eq!(ancestry.inference.source, LaunchSource::ContainerRuntime);
        assert_eq!(
            ancestry.inference.evidence.as_ref().map(|hop| hop.pid),
            Some(200)
        );
    }
}
