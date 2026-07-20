// Daemon 级事件总线（ADR 0032 client-server；迁移计划 Task 8 stretch）。
// 持有每个连接注册的 combined_tx（mpsc::Sender<ServerEvent>）；broadcast 时
// 向每个订阅 send 一条 BusNotice。死订阅（receiver 已 drop）在 send 失败时
// 由 retain 剔除——一个关闭的连接不会阻塞广播。
use crate::daemon::proto::ServerEvent;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

#[cfg(test)]
use std::sync::mpsc;

pub struct EventBus {
    subscribers: Mutex<Vec<Sender<ServerEvent>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self { subscribers: Mutex::new(Vec::new()) }
    }

    /// 注册一个连接的 combined_tx。bus 持有 Sender；连接关闭后它变死，
    /// 下次 broadcast 的 retain 会剔除。
    pub fn register(&self, tx: Sender<ServerEvent>) {
        self.subscribers.lock().unwrap().push(tx);
    }

    /// 向所有订阅广播一条 BusNotice。死/满订阅被 retain 剔除。
    /// （mpsc::channel 是无界的，send 仅在 receiver drop 时失败。）
    pub fn broadcast(&self, source: &str, text: &str) {
        let ev = ServerEvent::BusNotice { source: source.into(), text: text.into() };
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(ev.clone()).is_ok());
    }
}

impl Default for EventBus {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_broadcast_delivers() {
        let bus = EventBus::new();
        let (tx, rx) = mpsc::channel::<ServerEvent>();
        bus.register(tx);
        bus.broadcast("workgraph", "milestone #1 done");
        let ev = rx.recv().unwrap();
        match ev {
            ServerEvent::BusNotice { source, text } => {
                assert_eq!(source, "workgraph");
                assert_eq!(text, "milestone #1 done");
            }
            other => panic!("expected BusNotice, got {other:?}"),
        }
    }

    #[test]
    fn broadcast_reaps_dead_subscribers() {
        let bus = EventBus::new();
        let (tx1, rx1) = mpsc::channel::<ServerEvent>();
        let (tx2, _rx2) = mpsc::channel::<ServerEvent>(); // rx2 dropped below → dead
        bus.register(tx1);
        bus.register(tx2);
        drop(_rx2); // kill subscriber 2
        bus.broadcast("supervisor", "crashed");
        // live subscriber still receives
        let ev = rx1.recv().unwrap();
        assert!(matches!(ev, ServerEvent::BusNotice { .. }));
        // dead subscriber reaped: only 1 remains
        let n = bus.subscribers.lock().unwrap().len();
        assert_eq!(n, 1, "dead subscriber must be reaped");
    }

    #[test]
    fn broadcast_to_no_subscribers_is_noop() {
        let bus = EventBus::new();
        bus.broadcast("x", "y"); // must not panic
    }
}
