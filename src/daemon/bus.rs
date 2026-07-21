// Daemon 级事件总线（ADR 0032 client-server；迁移计划 Task 8 stretch）。
// 持有每个连接注册的 combined_tx（mpsc::Sender<ServerEvent>）；broadcast 时
// 向每个订阅 send 一条 BusNotice。订阅以单调递增的 SubscriptionId keying；
// 连接关闭时显式调用 `unregister` 移除 bus 端的 sender clone，使 combined_rx
// 关闭、writer 线程的 iter() 结束而干净退出——不依赖任何广播/sleep hack。
// broadcast 仍 retain 掉 send 失败的死订阅（兜底，正常路径用不到）。
use crate::daemon::proto::ServerEvent;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::Mutex;

#[cfg(test)]
use std::sync::mpsc;

/// 订阅注册句柄；连接关闭时把它传给 `unregister` 以移除该连接的 sender，
/// 使其 combined_rx 关闭、writer 线程退出。
pub type SubscriptionId = u64;

pub struct EventBus {
    next_id: Mutex<u64>,
    subscribers: Mutex<HashMap<SubscriptionId, Sender<ServerEvent>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(0),
            subscribers: Mutex::new(HashMap::new()),
        }
    }

    /// 注册一个连接的 combined_tx，返回订阅 id（关闭时传给 `unregister`）。
    pub fn register(&self, tx: Sender<ServerEvent>) -> SubscriptionId {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        self.subscribers.lock().unwrap().insert(id, tx);
        id
    }

    /// 连接关闭时调用：移除其 sender → combined_rx 关闭 → writer 退出。
    pub fn unregister(&self, id: SubscriptionId) {
        self.subscribers.lock().unwrap().remove(&id);
    }

    /// 向所有订阅广播一条 BusNotice。仍 retain 掉 send 失败的死订阅（兜底）。
    pub fn broadcast(&self, source: &str, text: &str) {
        let ev = ServerEvent::BusNotice { source: source.into(), text: text.into() };
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|_, tx| tx.send(ev.clone()).is_ok());
    }

    /// 当前活跃订阅数（测试 / 诊断用）。正常路径上 unregister 后立即归零；
    /// 出错路径上若 ConnGuard 没运行，会残留。
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_broadcast_delivers() {
        let bus = EventBus::new();
        let (tx, rx) = mpsc::channel::<ServerEvent>();
        let _id = bus.register(tx);
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
        let _id1 = bus.register(tx1);
        let _id2 = bus.register(tx2);
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

    #[test]
    fn unregister_removes_subscriber() {
        // 注册一个订阅，广播能收到；unregister 后广播不再送达，且 subscribers 数量降为 0。
        let bus = EventBus::new();
        let (tx, rx) = mpsc::channel::<ServerEvent>();
        let id = bus.register(tx);
        assert_eq!(bus.subscribers.lock().unwrap().len(), 1);
        bus.broadcast("src", "first");
        assert!(matches!(
            rx.recv().unwrap(),
            ServerEvent::BusNotice { .. }
        ));
        bus.unregister(id);
        assert_eq!(bus.subscribers.lock().unwrap().len(), 0, "unregister must remove the subscriber");
        bus.broadcast("src", "second");
        assert!(rx.try_recv().is_err(), "after unregister no broadcast should be delivered");
    }

    #[test]
    fn register_returns_distinct_ids() {
        let bus = EventBus::new();
        let (tx1, _rx1) = mpsc::channel::<ServerEvent>();
        let (tx2, _rx2) = mpsc::channel::<ServerEvent>();
        let id1 = bus.register(tx1);
        let id2 = bus.register(tx2);
        assert_ne!(id1, id2, "register must hand back distinct SubscriptionIds");
    }
}
