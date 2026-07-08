// Capability: a self-authored executable artifact (ADR 0021). Declares where it
// runs (Environment) and how long it lives (Lifecycle). Distinct from Tool/Skill.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Shell,
    Wasm,
    Docker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    OneShot,
    OnDemand,
    Persistent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityManifest {
    pub name: String,
    pub description: String,
    pub environment: Environment,
    pub lifecycle: Lifecycle,
    /// The command run inside the environment (`sh -c <entry>`).
    #[serde(default)]
    pub entry: String,
    /// Docker image for the `Docker` environment (e.g. "python:3.12-slim"). Required
    /// when environment is Docker.
    #[serde(default)]
    pub image: String,
    /// Address a `Persistent` service listens on (e.g. "http://127.0.0.1:8080"),
    /// recorded in the running-service table so later calls reach it (ADR 0021).
    #[serde(default)]
    pub address: String,
    /// How to invoke it via run_capability (params, examples).
    #[serde(default)]
    pub usage: String,
}

/// How a persistent service is backed: a host child process (Shell) or a
/// detached Docker container (by name).
pub enum Service {
    Process(std::process::Child),
    Container(String),
}

pub struct ServiceHandle {
    pub service: Service,
    pub address: String,
}

/// In-memory map of live Persistent capabilities (ADR 0021). Distinct from the
/// Registry (authored-on-disk). Bound to process lifetime: killed on exit; no
/// auto-restart. Accessed via the process-global [`services`] singleton.
#[derive(Default)]
pub struct RunningServiceTable {
    services: HashMap<String, ServiceHandle>,
}

impl RunningServiceTable {
    pub fn insert(&mut self, name: String, handle: ServiceHandle) {
        self.services.insert(name, handle);
    }
    pub fn get_mut(&mut self, name: &str) -> Option<&mut ServiceHandle> {
        self.services.get_mut(name)
    }
    pub fn remove(&mut self, name: &str) -> Option<ServiceHandle> {
        self.services.remove(name)
    }
    pub fn names(&self) -> Vec<String> {
        self.services.keys().cloned().collect()
    }
    /// Kill and reap every running service (ADR 0021: dropped on process exit).
    pub fn kill_all(&mut self) {
        for (_, mut h) in self.services.drain() {
            match &mut h.service {
                Service::Process(child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Service::Container(name) => {
                    let _ = std::process::Command::new("docker").args(["rm", "-f", name]).output();
                }
            }
        }
    }
}

impl Drop for RunningServiceTable {
    fn drop(&mut self) {
        self.kill_all();
    }
}

/// Process-global running-service table (services live for the process lifetime).
pub fn services() -> &'static std::sync::Mutex<RunningServiceTable> {
    static SERVICES: std::sync::OnceLock<std::sync::Mutex<RunningServiceTable>> =
        std::sync::OnceLock::new();
    SERVICES.get_or_init(|| std::sync::Mutex::new(RunningServiceTable::default()))
}

/// Kill all persistent services (call on shutdown, ADR 0021).
pub fn shutdown_all() {
    if let Ok(mut t) = services().lock() {
        t.kill_all();
    }
}
