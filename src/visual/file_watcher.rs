use crate::visual::event_router::EventRouter;
use notify::{Config, Event, RecommendedWatcher, Watcher};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    /// Channel to receive notify events
    rx: mpsc::Receiver<notify::Result<Event>>,
}

impl FileWatcher {
    /// Start watching workgraph.json changes under `root`.
    /// Phase 2 will add full `router`-based broadcasting.
    pub fn new(root: &Path, _router: Arc<EventRouter>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )?;

        let wg_path = root.join("workgraph.json");
        if wg_path.exists() {
            watcher.watch(&wg_path, notify::RecursiveMode::NonRecursive)?;
        }

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// A stub watcher for error recovery.
    /// Delete this once Phase 2 provides a real fallback.
    pub fn dummy() -> Self {
        let (_tx, rx) = mpsc::channel();
        Self {
            _watcher: RecommendedWatcher::new(move |_| {}, Config::default())
                .expect("RecommendedWatcher::new should always succeed"),
            rx,
        }
    }

    /// Drain any pending file-change events (used in the main loop).
    pub fn drain_events(&self) -> Vec<notify::Result<Event>> {
        let mut events = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            events.push(ev);
        }
        events
    }
}