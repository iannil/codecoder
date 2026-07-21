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

/// 监督一个 Persistent Capability 的运行状态。`gave_up` = 该服务已退出并被标记
/// 为 Failed（ADR 0021：不自动重启）。
pub struct SupervisedService {
    pub manifest: CapabilityManifest,
    pub child: Option<std::process::Child>,
    /// 已退出并标记为 Failed（不再重启）。停留在此处对 agent 可见。
    pub gave_up: bool,
}

/// Persistent Capability 监督树（first-class citizen #3 的 daemon 级形态）：
/// daemon 启动时扫描 capabilities/ 起 Persistent+Shell 条目并 spawn。崩溃的服务
/// 被标记 Failed 并保持可见——**不自动重启**（ADR 0021：静默重启会掩盖 bug）。
/// daemon 退出时 shutdown_all。
pub struct Supervisor {
    pub root: std::path::PathBuf,
    pub states: std::collections::HashMap<String, SupervisedService>,
}

impl Supervisor {
    pub fn start_all(root: &std::path::Path) -> anyhow::Result<Self> {
        let mut sup = Self { root: root.to_path_buf(), states: Default::default() };
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
            SupervisedService { manifest: man, child: Some(child), gave_up: false },
        );
        Ok(())
    }

    /// 周期调用：检查每个已起服务的子进程，若已退出则标记 Failed 并保持可见——
    /// **不重启**（ADR 0021）。返回本周期内发生的事件行（人类可读）。
    pub fn supervise(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        for (name, s) in self.states.iter_mut() {
            if s.gave_up { continue; }
            let exited = match s.child.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            if !exited { continue; }
            // ADR 0021: a crashed Persistent capability is marked Failed and left
            // visible for the agent to decide. Auto-restart is deliberately absent
            // — a silent restart would mask bugs.
            s.gave_up = true;
            s.child = None;
            events.push(format!(
                "capability '{name}' exited; marked Failed (not auto-restarted, ADR 0021)"
            ));
        }
        events
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
    fn supervisor_marks_crashed_persistent_failed_without_restart() {
        let dir = std::env::temp_dir().join(format!("cc_supervisor_{}", std::process::id()));
        let capdir = dir.join("capabilities/flaky");
        std::fs::create_dir_all(&capdir).unwrap();

        // Create a marker file path at an absolute location we control
        let marker_path = dir.join("respawn_cwd.txt");

        // 一个会立即退出的脚本（模拟崩溃），并在退出前写入当前工作目录到标记文件
        // 使用追加模式，这样每次运行（包括重启）都会添加一行
        let script = if cfg!(windows) {
            format!("cd >> \"{}\" & exit 1", marker_path.display())
        } else {
            format!("#!/bin/sh\npwd >> \"{}\"\nexit 1\n", marker_path.display())
        };

        std::fs::write(dir.join("capabilities/flaky/entry.sh"), script).unwrap();
        std::fs::write(
            dir.join("capabilities/flaky/manifest.json"),
            r#"{"name":"flaky","description":"crashes","environment":"shell","lifecycle":"persistent","entry":"sh entry.sh"}"#,
        ).unwrap();
        std::fs::create_dir_all(dir.join("capabilities")).unwrap(); // 确保目录

        let mut sup = Supervisor::start_all(&dir).unwrap();
        // Wait for the initial spawn to run and exit (writes the marker, exits 1).
        let start = Instant::now();
        loop {
            if std::fs::read_to_string(&marker_path).is_ok() || start.elapsed().as_secs() > 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Let the exit register, then supervise detects it and marks Failed.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let events = sup.supervise();

        let s = sup.states.get("flaky").expect("flaky supervised");
        assert!(s.gave_up, "crashed capability should be marked Failed (gave_up)");
        assert!(
            events.iter().any(|e| e.contains("marked Failed") && e.contains("not auto-restarted")),
            "should emit a no-restart event: {:?}", events
        );

        // ADR 0021: NO auto-restart → the marker must have exactly ONE line
        // (the initial spawn). A second line would mean a respawn happened.
        let marker_content = std::fs::read_to_string(&marker_path)
            .expect("initial spawn should have written the marker");
        let lines: Vec<&str> = marker_content.lines().collect();
        assert_eq!(lines.len(), 1, "should NOT respawn (ADR 0021 no auto-restart): got {} lines", lines.len());

        // The single spawn ran in the capability directory, not daemon cwd.
        let expected_cwd = dir.join("capabilities/flaky").canonicalize().expect("canonicalize expected path");
        let actual_cwd = std::path::PathBuf::from(lines[0].trim());
        let actual_cwd = actual_cwd.canonicalize().unwrap_or(actual_cwd);
        assert_eq!(actual_cwd, expected_cwd,
                   "capability should run in its own directory, not daemon cwd");

        sup.shutdown_all();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
