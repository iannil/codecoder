use crate::daemon::proto::ServerEvent;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

const CATCH_UP_COUNT: usize = 50;
const BUFFER_MAX: usize = 200;

pub struct EventRouter {
    inner: Arc<Mutex<EventRouterInner>>,
}

struct EventRouterInner {
    next_id: u64,
    clients: HashMap<u64, Sender<ServerEvent>>,
    buffer: VecDeque<ServerEvent>,
}

impl EventRouter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EventRouterInner {
                next_id: 0,
                clients: HashMap::new(),
                buffer: VecDeque::with_capacity(BUFFER_MAX),
            })),
        }
    }

    /// Register a new SSE client. Returns the client ID and a Receiver.
    pub fn register_sse(&self) -> (u64, Receiver<ServerEvent>) {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.clients.insert(id, tx);
        (id, rx)
    }

    /// Remove an SSE client by ID.
    pub fn remove_sse(&self, id: u64) {
        self.inner.lock().unwrap().clients.remove(&id);
    }

    /// Get catch-up events for a new client (most recent CATCH_UP_COUNT).
    pub fn catch_up(&self) -> Vec<ServerEvent> {
        let inner = self.inner.lock().unwrap();
        inner
            .buffer
            .iter()
            .rev()
            .take(CATCH_UP_COUNT)
            .cloned()
            .rev()
            .collect()
    }

    /// Ingest an event and broadcast to all SSE clients.
    pub fn ingest(&self, ev: ServerEvent) {
        let mut inner = self.inner.lock().unwrap();
        // Buffer
        inner.buffer.push_back(ev.clone());
        if inner.buffer.len() > BUFFER_MAX {
            inner.buffer.pop_front();
        }
        // Broadcast: send blocks briefly but that's fine for SSE workloads.
        // Disconnected clients are removed on error.
        inner.clients.retain(|_id, tx| {
            tx.send(ev.clone()).is_ok()
        });
    }

    /// Number of connected SSE clients.
    pub fn client_count(&self) -> usize {
        self.inner.lock().unwrap().clients.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn register_and_receive_event() {
        let router = EventRouter::new();
        let (id, rx) = router.register_sse();
        let ev = ServerEvent::TurnComplete;
        router.ingest(ev.clone());

        let received = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(received, ev);
        router.remove_sse(id);
    }

    #[test]
    fn catch_up_on_register() {
        let router = EventRouter::new();
        for i in 0..10 {
            router.ingest(ServerEvent::Notice {
                text: format!("ev{i}"),
            });
        }
        let (_id, _rx) = router.register_sse();
        let catch_up = router.catch_up();
        assert_eq!(catch_up.len(), 10);
    }

    #[test]
    fn buffer_capped_at_200() {
        let router = EventRouter::new();
        for i in 0..300 {
            router.ingest(ServerEvent::Notice {
                text: format!("ev{i}"),
            });
        }
        assert_eq!(router.catch_up().len(), 50); // CATCH_UP_COUNT
        let all = router.inner.lock().unwrap();
        assert_eq!(all.buffer.len(), 200);
    }

    #[test]
    fn dead_client_removed() {
        let router = EventRouter::new();
        let (id, rx) = router.register_sse();
        drop(rx); // disconnect
        router.ingest(ServerEvent::TurnComplete);
        assert_eq!(router.client_count(), 0);
        assert!(router.inner.lock().unwrap().clients.get(&id).is_none());
    }
}
