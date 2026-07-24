use crate::daemon::proto::ServerEvent;
use crate::visual::event_router::EventRouter;
use notify::{Config, Event, EventKind, RecommendedWatcher, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    router: Arc<EventRouter>,
    wg_path: PathBuf,
    last_read: Option<String>,
    last_broadcast: Instant,
    debounce: Duration,
}

impl FileWatcher {
    /// Start watching workgraph.json changes under `root`.
    pub fn new(root: &Path, router: Arc<EventRouter>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res| { let _ = tx.send(res); },
            Config::default(),
        )?;

        let wg_path = root.join("workgraph.json");
        // Watch the parent directory (atomic rename changes the inode)
        if let Some(parent) = wg_path.parent() {
            watcher.watch(parent, notify::RecursiveMode::NonRecursive)?;
        }

        let last_read = std::fs::read_to_string(&wg_path).ok();

        Ok(Self {
            _watcher: watcher,
            rx,
            router,
            wg_path,
            last_read,
            last_broadcast: Instant::now(),
            debounce: Duration::from_millis(300),
        })
    }

    /// Poll for file changes (call from main loop).
    pub fn poll(&mut self) {
        while let Ok(Ok(event)) = self.rx.try_recv() {
            let is_wg = match event.kind {
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {
                    event.paths.iter().any(|p: &std::path::PathBuf| p.ends_with("workgraph.json"))
                }
                _ => false,
            };
            if !is_wg {
                continue;
            }
            // Debounce
            if self.last_broadcast.elapsed() < self.debounce {
                continue;
            }
            match std::fs::read_to_string(&self.wg_path) {
                Ok(content) => {
                    if Some(&content) == self.last_read.as_ref() {
                        continue; // no real change
                    }
                    self.last_read = Some(content.clone());
                    self.last_broadcast = Instant::now();
                    let ev = ServerEvent::BusNotice {
                        source: "workgraph".into(),
                        text: "workgraph changed".into(),
                    };
                    self.router.ingest(ev);
                    // No need for __wg_update__ hack — bus_notice already triggers loadWorkgraph()
                }
                Err(_) => {}
            }
        }
    }

    pub fn dummy() -> Self {
        let rx = mpsc::channel::<notify::Result<notify::Event>>().1;
        let watcher: RecommendedWatcher = notify::RecommendedWatcher::new(
            move |_| {},
            notify::Config::default(),
        ).expect("RecommendedWatcher::new should always succeed");
        Self {
            _watcher: watcher,
            rx,
            router: Arc::new(EventRouter::new()),
            wg_path: PathBuf::from("/dev/null"),
            last_read: None,
            last_broadcast: Instant::now(),
            debounce: Duration::from_millis(300),
        }
    }
}