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

/// 监督一个 Persistent Capability 的运行状态：记录重启次数与窗口。
pub struct SupervisedService {
    pub manifest: CapabilityManifest,
    pub child: Option<std::process::Child>,
    pub restart_count: u32,
    pub first_restart: Option<std::time::Instant>,
    /// 达到上限后放弃重启（已死）。
    pub gave_up: bool,
}

/// Persistent Capability 监督树（first-class citizen #3 的 daemon 级形态）：
/// 扫 capabilities/ 起 Persistent 条目，崩溃自动重启，超过 max_restarts/window_secs
/// 放弃；daemon 退出时 shutdown_all。
pub struct Supervisor {
    pub max_restarts: u32,
    pub window_secs: u64,
    pub states: std::collections::HashMap<String, SupervisedService>,
}

impl Supervisor {
    pub fn start_all(root: &std::path::Path) -> anyhow::Result<Self> {
        let mut sup = Self { max_restarts: 3, window_secs: 60, states: Default::default() };
        let caps = root.join("capabilities");
        let Ok(entries) = std::fs::read_dir(&caps) else { return Ok(sup); };
        for e in entries.flatten() {
            let man = e.path().join("manifest.json");
            let Ok(raw) = std::fs::read_to_string(&man) else { continue; };
            let Ok(m) = serde_json::from_str::<CapabilityManifest>(&raw) else { continue; };
            if m.lifecycle == Lifecycle::Persistent && m.environment == Environment::Shell {
                let _ = sup.start_one(&m.name, root);
            }
        }
        Ok(sup)
    }

    pub fn start_one(&mut self, name: &str, root: &std::path::Path) -> anyhow::Result<()> {
        let man = read_manifest(name, root)?;
        let child = spawn_shell_capability(root, &man)?;
        self.states.insert(
            name.to_string(),
            SupervisedService { manifest: man, child: Some(child), restart_count: 0, first_restart: None, gave_up: false },
        );
        Ok(())
    }

    /// 周期调用：检查每个已起服务的子进程，若已退出则按窗口/上限决定重启或放弃。
    pub fn supervise(&mut self) {
        for (_name, s) in self.states.iter_mut() {
            if s.gave_up { continue; }
            let exited = match s.child.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            if !exited { continue; }
            // 窗口外重置计数（重启滑动窗口）
            let now_inst = std::time::Instant::now();
            if let Some(first) = s.first_restart {
                if now_inst.duration_since(first).as_secs() >= self.window_secs {
                    s.restart_count = 0;
                    s.first_restart = None;
                }
            }
            if s.restart_count >= self.max_restarts {
                s.gave_up = true;
                s.child = None;
                continue;
            }
            s.restart_count += 1;
            if s.first_restart.is_none() { s.first_restart = Some(now_inst); }
            // 重启
            if let Ok(c) = spawn_shell_capability(&std::path::PathBuf::from("."), &s.manifest) {
                s.child = Some(c);
            } else {
                s.child = None;
            }
        }
    }

    pub fn shutdown_all(&mut self) {
        for (_, s) in self.states.iter_mut() {
            if let Some(mut c) = s.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

fn read_manifest(name: &str, root: &std::path::Path) -> anyhow::Result<CapabilityManifest> {
    let raw = std::fs::read_to_string(root.join("capabilities").join(name).join("manifest.json"))?;
    Ok(serde_json::from_str(&raw)?)
}

fn spawn_shell_capability(root: &std::path::Path, m: &CapabilityManifest) -> anyhow::Result<std::process::Child> {
    let cap_dir = root.join("capabilities").join(&m.name);
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(&m.entry);
    cmd.current_dir(cap_dir);
    Ok(cmd.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn supervisor_restarts_crashed_persistent_until_cap() {
        let dir = std::env::temp_dir().join(format!("cc_supervisor_{}", std::process::id()));
        let capdir = dir.join("capabilities/flaky");
        std::fs::create_dir_all(&capdir).unwrap();
        // 一个会立即退出的脚本（模拟崩溃）。Shell 环境。
        let script = if cfg!(windows) { "exit 1" } else { "#!/bin/sh\nexit 1\n" };
        std::fs::write(dir.join("capabilities/flaky/entry.sh"), script).unwrap();
        std::fs::write(
            dir.join("capabilities/flaky/manifest.json"),
            r#"{"name":"flaky","description":"crashes","environment":"shell","lifecycle":"persistent","entry":"sh entry.sh"}"#,
        ).unwrap();
        std::fs::create_dir_all(dir.join("capabilities")).unwrap(); // 确保目录

        let mut sup = Supervisor::start_all(&dir).unwrap();
        // 反复 supervise 直到放弃或超时
        let start = Instant::now();
        loop {
            sup.supervise();
            if start.elapsed().as_secs() > 2 { break; } // 测试保护
            let name = "flaky";
            if let Some(s) = sup.states.get(name) {
                if s.gave_up { break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let s = sup.states.get("flaky").expect("flaky supervised");
        assert!(s.restart_count >= 1, "should have restarted at least once");
        assert!(s.gave_up, "should give up after max_restarts");
        sup.shutdown_all();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
