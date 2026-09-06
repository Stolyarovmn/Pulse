//! Коллектор сетевых интерфейсов.
//!
//! Интерфейс — сущность графа: он может быть ресурсной зависимостью для
//! контейнера и участвовать в ранжировании доказательств. Петля `lo`
//! пропускается: её трафик — это трафик самого хоста с самим собой, и в списке
//! он только создаёт шум.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use pulse_core::entity::{EntityKey, EntitySpec};
use pulse_core::graph::{CollectCtx, CollectError, Collector};
use pulse_core::metric::ids;
use pulse_core::relation::RelationKind;

use crate::fs::{FsSource, FsSourceExt};
use crate::parse::{self, NetStat};

/// Коллектор интерфейсов.
#[derive(Debug)]
pub struct NetCollector {
    fs: Arc<dyn FsSource>,
    proc_root: PathBuf,
    previous: HashMap<String, NetStat>,
    /// Предыдущий агрегат по хосту: `(rx, tx)` накопленных байт.
    ///
    /// Хранится отдельно от карты интерфейсов: агрегат обязан выжить при
    /// появлении и исчезновении отдельных интерфейсов.
    previous_host: Option<(f64, f64)>,
}

impl NetCollector {
    #[must_use]
    pub fn new(fs: Arc<dyn FsSource>, proc_root: PathBuf) -> Self {
        NetCollector {
            fs,
            proc_root,
            previous: HashMap::new(),
            previous_host: None,
        }
    }
}

impl Collector for NetCollector {
    fn name(&self) -> &'static str {
        "net"
    }

    fn collect(&mut self, ctx: &mut CollectCtx<'_>) -> Result<(), CollectError> {
        let path = self.proc_root.join("net/dev");
        let Some(text) = self.fs.read_opt(&path) else {
            return Err(CollectError::Unavailable("net/dev"));
        };

        let host = ctx.host();
        let interval = ctx.interval_secs().max(0.001);
        let mut host_rx = 0.0;
        let mut host_tx = 0.0;
        // Имена интерфейсов не вечны: veth контейнера исчезает вместе с ним.
        // Без чистки карта прошлых значений росла бы весь срок жизни агента.
        let mut seen: HashSet<String> = HashSet::with_capacity(self.previous.len().max(8));

        for stat in parse::parse_net_dev(&text) {
            if stat.name == "lo" {
                continue;
            }

            let entity = ctx.upsert(
                EntitySpec::new(
                    EntityKey::NetIf {
                        name: stat.name.clone().into(),
                    },
                    stat.name.clone(),
                )
                .parent(host),
            );
            ctx.relate(host, RelationKind::ParentOf, entity);

            host_rx += stat.rx_bytes;
            host_tx += stat.tx_bytes;

            ctx.sample(entity, ids::NETIF_RX_BYTES, stat.rx_bytes);
            ctx.sample(entity, ids::NETIF_TX_BYTES, stat.tx_bytes);
            ctx.sample(entity, ids::NETIF_RX_PACKETS, stat.rx_packets);
            ctx.sample(entity, ids::NETIF_TX_PACKETS, stat.tx_packets);
            ctx.sample(entity, ids::NETIF_RX_ERRORS, stat.rx_errors);
            ctx.sample(entity, ids::NETIF_TX_ERRORS, stat.tx_errors);
            ctx.sample(entity, ids::NETIF_RX_DROPS, stat.rx_drops);
            ctx.sample(entity, ids::NETIF_TX_DROPS, stat.tx_drops);

            if let Some(previous) = self.previous.get(&stat.name) {
                let d_rx = stat.rx_bytes - previous.rx_bytes;
                let d_tx = stat.tx_bytes - previous.tx_bytes;
                // Отрицательная разность — переподключение интерфейса или
                // переполнение 32-битного счётчика: скорость не публикуем.
                if d_rx >= 0.0 {
                    ctx.sample(entity, ids::NETIF_RX_THROUGHPUT, d_rx / interval);
                }
                if d_tx >= 0.0 {
                    ctx.sample(entity, ids::NETIF_TX_THROUGHPUT, d_tx / interval);
                }
            }

            let _ = seen.insert(stat.name.clone());
            let _ = self.previous.insert(stat.name.clone(), stat);
        }

        self.previous.retain(|name, _| seen.contains(name));

        ctx.sample(host, ids::HOST_NET_RX_BYTES, host_rx);
        ctx.sample(host, ids::HOST_NET_TX_BYTES, host_tx);

        // Скорость по хосту считается из агрегата, а не выводится из
        // накопительного счётчика: Overview показывает `NET ↓ ↑` как rate, и
        // раньше читал несуществующую серию `NETIF_*_THROUGHPUT` у host,
        // из-за чего сеть всегда выглядела нулевой.
        if let Some((previous_rx, previous_tx)) = self.previous_host {
            let d_rx = host_rx - previous_rx;
            let d_tx = host_tx - previous_tx;
            // Интерфейс мог исчезнуть: агрегат уменьшился, но это не
            // отрицательная скорость.
            if d_rx >= 0.0 {
                ctx.sample(host, ids::HOST_NET_RX_THROUGHPUT, d_rx / interval);
            }
            if d_tx >= 0.0 {
                ctx.sample(host, ids::HOST_NET_TX_THROUGHPUT, d_tx / interval);
            }
        }
        self.previous_host = Some((host_rx, host_tx));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FixtureFs;
    use pulse_core::entity::EntityKind;
    use pulse_core::sample::SeriesKey;
    use pulse_core::time::Timestamp;
    use pulse_core::{EntityGraph, TickBatch};

    fn net_dev(rx: u64, tx: u64) -> String {
        format!(
            "Inter-|   Receive                    |  Transmit\n\
             face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
    lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n\
  eth0: {rx} 10 0 0 0 0 0 0 {tx} 20 0 0 0 0 0 0\n"
        )
    }

    fn value(batch: &TickBatch, key: SeriesKey) -> Option<f64> {
        batch
            .samples
            .iter()
            .find(|s| s.series == key)
            .map(|s| s.value)
    }

    #[test]
    fn loopback_is_skipped() {
        let fs = Arc::new(FixtureFs::new().file("/proc/net/dev", &net_dev(1_000, 2_000)));
        let mut collector = NetCollector::new(fs, PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор сети");
        }
        let _ = graph.end_tick();
        let names: Vec<String> = graph
            .entities_of_kind(EntityKind::NetIf)
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, vec!["eth0".to_string()]);
    }

    #[test]
    fn throughput_is_computed_from_delta() {
        let mut fixture = FixtureFs::new().file("/proc/net/dev", &net_dev(1_000, 2_000));
        let first = Arc::new(FixtureFs::new().file("/proc/net/dev", &net_dev(1_000, 2_000)));
        let mut collector = NetCollector::new(first, PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));

        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let batch1 = graph.end_tick();
        let iface = graph
            .entities_of_kind(EntityKind::NetIf)
            .next()
            .map(|e| e.id)
            .unwrap_or(pulse_core::EntityId::NONE);
        assert!(
            value(&batch1, SeriesKey::new(iface, ids::NETIF_RX_THROUGHPUT)).is_none(),
            "на первом такте скорости нет"
        );

        fixture.set("/proc/net/dev", &net_dev(6_000, 2_500));
        collector.fs = Arc::new(fixture);
        graph.begin_tick(Timestamp::from_millis(3_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 2");
        }
        let batch2 = graph.end_tick();
        let rx =
            value(&batch2, SeriesKey::new(iface, ids::NETIF_RX_THROUGHPUT)).unwrap_or_default();
        let tx =
            value(&batch2, SeriesKey::new(iface, ids::NETIF_TX_THROUGHPUT)).unwrap_or_default();
        assert!((rx - 5_000.0).abs() < 1e-9, "rx = {rx}");
        assert!((tx - 500.0).abs() < 1e-9, "tx = {tx}");
    }

    /// Overview читает скорость сети у host. Раньше такой серии не было, и
    /// экран печатал уверенный `↓0 B/s` при работающем трафике.
    #[test]
    fn host_aggregate_publishes_throughput_not_only_counters() {
        let fixture = FixtureFs::new().file("/proc/net/dev", &net_dev(1_000, 500));
        let mut collector = NetCollector::new(Arc::new(fixture), PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(0));

        graph.begin_tick(Timestamp::from_millis(1_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let first = graph.end_tick();
        let host = graph.host();
        assert!(
            value(&first, SeriesKey::new(host, ids::HOST_NET_RX_THROUGHPUT)).is_none(),
            "на первом такте скорости нет"
        );

        // +2000 принято и +1000 передано за секунду.
        collector.fs = Arc::new(FixtureFs::new().file("/proc/net/dev", &net_dev(3_000, 1_500)));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 2");
        }
        let batch = graph.end_tick();
        let rx = value(&batch, SeriesKey::new(host, ids::HOST_NET_RX_THROUGHPUT))
            .expect("скорость приёма по хосту");
        let tx = value(&batch, SeriesKey::new(host, ids::HOST_NET_TX_THROUGHPUT))
            .expect("скорость передачи по хосту");
        assert!((rx - 2_000.0).abs() < 1e-6, "rx = {rx}");
        assert!((tx - 1_000.0).abs() < 1e-6, "tx = {tx}");

        // Накопительные счётчики остаются накопительными.
        let total =
            value(&batch, SeriesKey::new(host, ids::HOST_NET_RX_BYTES)).expect("счётчик хоста");
        assert!((total - 3_000.0).abs() < 1e-6, "counter = {total}");
    }

    /// Исчезновение интерфейса уменьшает агрегат: это не отрицательная скорость.
    #[test]
    fn host_throughput_survives_interface_disappearance() {
        let two = "Inter-|   Receive                    |  Transmit\n                   face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n                       eth0: 5000 10 0 0 0 0 0 0 4000 8 0 0 0 0 0 0\n                      veth9: 5000 10 0 0 0 0 0 0 4000 8 0 0 0 0 0 0\n";
        let mut collector = NetCollector::new(
            Arc::new(FixtureFs::new().file("/proc/net/dev", two)),
            PathBuf::from("/proc"),
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(0));
        graph.begin_tick(Timestamp::from_millis(1_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let _ = graph.end_tick();

        // veth9 исчез: агрегат стал меньше предыдущего.
        collector.fs = Arc::new(FixtureFs::new().file("/proc/net/dev", &net_dev(5_100, 4_100)));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 2");
        }
        let batch = graph.end_tick();
        let host = graph.host();
        assert!(
            value(&batch, SeriesKey::new(host, ids::HOST_NET_RX_THROUGHPUT)).is_none(),
            "уменьшение агрегата не является отрицательной скоростью"
        );
        assert!(
            batch.samples.iter().all(|sample| sample.value >= 0.0),
            "отрицательных значений быть не может"
        );
    }

    #[test]
    fn counter_wrap_does_not_produce_negative_throughput() {
        let mut fixture = FixtureFs::new().file("/proc/net/dev", &net_dev(9_000, 9_000));
        let first = Arc::new(FixtureFs::new().file("/proc/net/dev", &net_dev(9_000, 9_000)));
        let mut collector = NetCollector::new(first, PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 1");
        }
        let _ = graph.end_tick();

        fixture.set("/proc/net/dev", &net_dev(10, 10));
        collector.fs = Arc::new(fixture);
        graph.begin_tick(Timestamp::from_millis(3_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт 2");
        }
        let batch = graph.end_tick();
        let iface = graph
            .entities_of_kind(EntityKind::NetIf)
            .next()
            .map(|e| e.id)
            .unwrap_or(pulse_core::EntityId::NONE);
        assert!(value(&batch, SeriesKey::new(iface, ids::NETIF_RX_THROUGHPUT)).is_none());
    }

    #[test]
    fn missing_net_dev_is_unavailable() {
        let fs = Arc::new(FixtureFs::new());
        let mut collector = NetCollector::new(fs, PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        let mut ctx = CollectCtx::new(&mut graph, 1.0);
        assert!(collector.collect(&mut ctx).is_err());
    }

    #[test]
    fn host_aggregate_excludes_loopback() {
        let fs = Arc::new(FixtureFs::new().file("/proc/net/dev", &net_dev(1_000, 2_000)));
        let mut collector = NetCollector::new(fs, PathBuf::from("/proc"));
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("сбор");
        }
        let batch = graph.end_tick();
        let host = graph.host();
        assert_eq!(
            value(&batch, SeriesKey::new(host, ids::HOST_NET_RX_BYTES)),
            Some(1_000.0)
        );
    }
    #[test]
    fn vanished_interfaces_are_dropped_from_previous_values() {
        // veth контейнера живёт вместе с контейнером: карта прошлых значений
        // обязана сжиматься, иначе она растёт весь срок жизни агента.
        let with_veth = "Inter-|   Receive                    |  Transmit\n\
             face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
    lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n\
  eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n\
 veth9: 500 5 0 0 0 0 0 0 700 7 0 0 0 0 0 0\n";
        let mut collector = NetCollector::new(
            Arc::new(FixtureFs::new().file("/proc/net/dev", with_veth)),
            PathBuf::from("/proc"),
        );
        let mut graph = EntityGraph::new("boot", "host", Timestamp::from_millis(1_000));
        graph.begin_tick(Timestamp::from_millis(2_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт с veth");
        }
        let _ = graph.end_tick();
        assert_eq!(collector.previous.len(), 2, "eth0 и veth9");

        // Контейнер удалён: veth9 исчез, появился veth11.
        let after = "Inter-|   Receive                    |  Transmit\n\
             face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
    lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0\n\
  eth0: 3000 30 0 0 0 0 0 0 4000 40 0 0 0 0 0 0\n\
 veth11: 10 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n";
        collector.fs = Arc::new(FixtureFs::new().file("/proc/net/dev", after));
        graph.begin_tick(Timestamp::from_millis(3_000));
        {
            let mut ctx = CollectCtx::new(&mut graph, 1.0);
            collector.collect(&mut ctx).expect("такт без veth9");
        }
        let batch = graph.end_tick();

        assert_eq!(
            collector.previous.len(),
            2,
            "мёртвый интерфейс обязан выпасть: {:?}",
            collector.previous.keys().collect::<Vec<_>>()
        );
        assert!(!collector.previous.contains_key("veth9"));

        // У новой идентичности базы нет, поэтому скорость не публикуется.
        let fresh = graph
            .entities_of_kind(EntityKind::NetIf)
            .find(|e| e.name == "veth11")
            .map(|e| e.id)
            .expect("veth11 в графе");
        assert!(value(&batch, SeriesKey::new(fresh, ids::NETIF_RX_THROUGHPUT)).is_none());
    }
}
