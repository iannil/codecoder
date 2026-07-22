// Built-in tools (ADR 0018): read_file (Permission::None) and run_command (Ask),
// plus the self-evolution dispatchers use_skill and run_capability (ADR 0020/0021).
use super::{Tool, ToolCtx, ToolOutput};
use crate::capability::{CapabilityManifest, Environment, Lifecycle};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read the contents of a file, relative to the project root."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string", "description": "File path relative to root." } },
            "required": ["path"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolOutput::err("missing required arg: path"));
        }
        let full = ctx.root.join(path);
        match std::fs::read_to_string(&full) {
            Ok(contents) => Ok(ToolOutput::ok(contents)),
            Err(e) => Ok(ToolOutput::err(format!("cannot read {}: {e}", full.display()))),
        }
    }
}

pub struct RunCommand;

impl RunCommand {
    /// 复合命令(含 shell 控制运算符)?保守偏向安全:引号内的 `>`/`<` 会误判
    /// 为复合(过度弹窗),但**绝不漏判**(ADR 0036)。
    fn is_compound(cmd: &str) -> bool {
        ["&&", "||", ";", "|", "`", "$(", "<", ">"]
            .iter()
            .any(|op| cmd.contains(op))
            || cmd.trim_end().ends_with('&')
    }

    /// Permission key(ADR 0018)。简单命令按命令类(`run_command:git`);
    /// **复合命令按整条命令串**(`run_command:cd X && rm`),使其不可经良性
    /// 前缀预授权(ADR 0036,P5 加固)。
    fn key_for(cmd: &str) -> String {
        if Self::is_compound(cmd) {
            format!("run_command:{cmd}")
        } else {
            let head = cmd.split_whitespace().next().unwrap_or("");
            format!("run_command:{head}")
        }
    }
}

impl Tool for RunCommand {
    fn name(&self) -> &str {
        "run_command"
    }
    fn description(&self) -> &str {
        "Run a shell command in the project root and return its combined output."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "cmd": { "type": "string", "description": "The shell command line." } },
            "required": ["cmd"]
        })
    }
    fn permission(&self, args: &Value, _root: &Path) -> Permission {
        let cmd = args.get("cmd").and_then(Value::as_str).unwrap_or_default();
        Permission::Ask { key: Self::key_for(cmd) }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let cmd = args.get("cmd").and_then(Value::as_str).unwrap_or_default();
        if cmd.is_empty() {
            return Ok(ToolOutput::err("missing required arg: cmd"));
        }
        let mut command = Command::new("sh");
        command.arg("-c").arg(cmd).current_dir(ctx.root);
        run_shell_cancellable(command, ctx)
    }
}

/// Run a shell `Command` to completion, killing the child if the turn is
/// cancelled mid-run (ADR 0016). Spawns (not `.output()`) and drains stdout/stderr
/// on their own threads so a chatty command can't fill the pipe buffer and
/// deadlock the poll loop. Shared by `run_command` and shell Capabilities.
pub(crate) fn run_shell_cancellable(mut command: Command, ctx: &ToolCtx) -> anyhow::Result<ToolOutput> {
    if ctx.is_cancelled() {
        return Ok(ToolOutput::err("cancelled"));
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b);
        b
    });
    let err_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });

    // Poll for exit while watching the cancel token; kill the child on cancel.
    let status = loop {
        if ctx.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait()? {
            Some(status) => break Some(status),
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    let stdout_buf = out_reader.join().unwrap_or_default();
    let stderr_buf = err_reader.join().unwrap_or_default();

    let status = match status {
        Some(status) => status,
        None => return Ok(ToolOutput::err("cancelled")),
    };
    let mut buf = String::from_utf8_lossy(&stdout_buf).into_owned();
    if !stderr_buf.is_empty() {
        buf.push_str(&String::from_utf8_lossy(&stderr_buf));
    }
    let is_error = !status.success();
    if is_error {
        buf = format!("exit {}: {buf}", status.code().unwrap_or(-1));
    }
    Ok(ToolOutput { content: buf, is_error, session_meta_mark: None })
}

pub struct ListDirectory;

impl Tool for ListDirectory {
    fn name(&self) -> &str {
        "list_directory"
    }
    fn description(&self) -> &str {
        "List the entries of a directory, relative to the project root."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "path": { "type": "string", "description": "Directory path relative to root (default '.')." } }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let full = ctx.root.join(rel);
        let mut names = Vec::new();
        match std::fs::read_dir(&full) {
            Ok(entries) => {
                for e in entries.flatten() {
                    let mut name = e.file_name().to_string_lossy().into_owned();
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        name.push('/');
                    }
                    names.push(name);
                }
            }
            Err(e) => return Ok(ToolOutput::err(format!("cannot list {}: {e}", full.display()))),
        }
        names.sort();
        Ok(ToolOutput::ok(names.join("\n")))
    }
}

pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write content to a file (creating parent directories), relative to root."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "write_file".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
        let content = args.get("content").and_then(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolOutput::err("missing required arg: path"));
        }
        let full = ctx.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::write(&full, content) {
            Ok(()) => Ok(ToolOutput::ok(format!("wrote {} bytes to {}", content.len(), path))),
            Err(e) => Ok(ToolOutput::err(format!("cannot write {}: {e}", full.display()))),
        }
    }
}

pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &str {
        "edit_file"
    }
    fn description(&self) -> &str {
        "Replace exact text in a file. `old` must occur at least once."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old": { "type": "string", "description": "Exact text to replace." },
                "new": { "type": "string", "description": "Replacement text." }
            },
            "required": ["path", "old", "new"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "edit_file".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
        let old = args.get("old").and_then(Value::as_str).unwrap_or_default();
        let new = args.get("new").and_then(Value::as_str).unwrap_or_default();
        if path.is_empty() || old.is_empty() {
            return Ok(ToolOutput::err("missing required arg: path/old"));
        }
        let full = ctx.root.join(path);
        let contents = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::err(format!("cannot read {}: {e}", full.display()))),
        };
        let count = contents.matches(old).count();
        if count == 0 {
            return Ok(ToolOutput::err("`old` text not found; no change made"));
        }
        let updated = contents.replace(old, new);
        std::fs::write(&full, updated)?;
        Ok(ToolOutput::ok(format!("replaced {count} occurrence(s) in {path}")))
    }
}

pub struct UseSkill;

impl Tool for UseSkill {
    fn name(&self) -> &str {
        "use_skill"
    }
    fn description(&self) -> &str {
        "Activate a Skill (or a [draft] Prompt) by name, returning its full instructions to follow."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "Skill or draft-prompt name (see the catalog)." } },
            "required": ["name"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None // pure knowledge injection, never executes (ADR 0020)
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            return Ok(ToolOutput::err("missing required arg: name"));
        }
        // Matured Skill wins; fall back to a prompts/ draft of the same name (ADR 0025).
        let skill = ctx.root.join("skills").join(format!("{name}.md"));
        let draft = ctx.root.join("prompts").join(format!("{name}.md"));
        match std::fs::read_to_string(&skill).or_else(|_| std::fs::read_to_string(&draft)) {
            Ok(text) => Ok(ToolOutput::ok(text)),
            Err(_) => Ok(ToolOutput::err(format!("no such skill or draft: {name}"))),
        }
    }
}

pub struct RunCapability;

impl RunCapability {
    fn manifest(root: &Path, name: &str) -> Option<CapabilityManifest> {
        let path = root.join("capabilities").join(name).join("manifest.json");
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

impl Tool for RunCapability {
    fn name(&self) -> &str {
        "run_capability"
    }
    fn description(&self) -> &str {
        "Execute a Capability by name in its declared environment."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Capability name (see the catalog)." },
                "args": { "type": "object", "description": "Arguments passed via CODECODER_CAPABILITY_ARGS (JSON)." }
            },
            "required": ["name"]
        })
    }
    fn permission(&self, args: &Value, root: &Path) -> Permission {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        // Key by name + environment (ADR 0018/0022). Unknown → @shell (most capped).
        let env = Self::manifest(root, name)
            .map(|m| match m.environment {
                Environment::Shell => "shell",
                Environment::Wasm => "wasm",
                Environment::Docker => "docker",
            })
            .unwrap_or("shell");
        Permission::Ask { key: format!("run_capability:{name}@{env}") }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        let Some(m) = Self::manifest(ctx.root, name) else {
            return Ok(ToolOutput::err(format!("no such capability: {name}")));
        };
        if matches!(m.lifecycle, Lifecycle::Persistent) {
            return run_persistent(name, &m, ctx.root);
        }
        if matches!(m.lifecycle, Lifecycle::OnDemand) {
            return run_ondemand(name, &m, ctx.root);
        }
        match m.environment {
            Environment::Shell => {
                let dir = ctx.root.join("capabilities").join(name);
                let cap_args = args.get("args").cloned().unwrap_or(json!({}));
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg(&m.entry)
                    .current_dir(&dir)
                    .env("CODECODER_CAPABILITY_ARGS", cap_args.to_string());
                run_shell_cancellable(command, ctx)
            }
            Environment::Docker => {
                if m.image.trim().is_empty() {
                    return Ok(ToolOutput::err("docker capability requires an 'image' in its manifest"));
                }
                let dir = ctx.root.join("capabilities").join(name);
                let dir_str = dir.to_string_lossy().into_owned();
                let cap_args = args.get("args").cloned().unwrap_or(json!({}));
                // Isolation per ADR 0021: no network, read-only workspace mount, limits.
                let output = Command::new("docker")
                    .args(["run", "--rm", "--network", "none", "--memory", "512m", "--cpus", "1"])
                    .arg("-e")
                    .arg(format!("CODECODER_CAPABILITY_ARGS={}", cap_args))
                    .arg("-v")
                    .arg(format!("{dir_str}:/work:ro"))
                    .args(["-w", "/work", &m.image, "sh", "-c", &m.entry])
                    .output();
                match output {
                    Ok(out) => {
                        let mut buf = String::from_utf8_lossy(&out.stdout).into_owned();
                        if !out.stderr.is_empty() {
                            buf.push_str(&String::from_utf8_lossy(&out.stderr));
                        }
                        let is_error = !out.status.success();
                        if is_error {
                            buf = format!("exit {}: {buf}", out.status.code().unwrap_or(-1));
                        }
                        Ok(ToolOutput { content: buf, is_error, session_meta_mark: None })
                    }
                    // No silent downgrade to host (ADR 0021): Docker absent → explicit error.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Ok(ToolOutput::err("Docker not available (daemon/CLI missing); refusing to downgrade to host (ADR 0021)"))
                    }
                    Err(e) => Ok(ToolOutput::err(format!("docker run failed: {e}"))),
                }
            }
            Environment::Wasm => {
                if m.entry.trim().is_empty() {
                    return Ok(ToolOutput::err("wasm capability requires 'entry' = a .wasm/.wat file path"));
                }
                let dir = ctx.root.join("capabilities").join(name);
                let cap_args = args.get("args").cloned().unwrap_or(json!({}));
                match crate::tool::wasm::run_wasm(&dir, &m.entry, &cap_args.to_string()) {
                    Ok((out, is_error)) => Ok(ToolOutput { content: out, is_error, session_meta_mark: None }),
                    Err(e) => Ok(ToolOutput::err(format!("wasm run failed: {e}"))),
                }
            }
        }
    }
}

/// Whether a detached Docker container of this name is currently running.
fn container_running(name: &str) -> bool {
    Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Start (or report on) a Persistent capability, tracked in the process-global
/// running-service table (ADR 0021). Shell (child process) and Docker (detached
/// container) are supported. No auto-restart: a service found dead is removed and
/// reported; the agent may call again to restart.
fn run_persistent(name: &str, m: &CapabilityManifest, root: &Path) -> anyhow::Result<ToolOutput> {
    use crate::capability::{Service, ServiceHandle};
    if matches!(m.environment, Environment::Wasm) {
        return Ok(ToolOutput::err("persistent services are not supported for the wasm environment"));
    }
    let table = crate::capability::services();
    let mut table = table.lock().unwrap();

    // Is it already tracked, and alive?
    let existing = if let Some(h) = table.get_mut(name) {
        let alive = match &mut h.service {
            Service::Process(child) => matches!(child.try_wait(), Ok(None)),
            Service::Container(cname) => container_running(cname),
        };
        Some((alive, h.address.clone()))
    } else {
        None
    };
    match existing {
        Some((true, addr)) => {
            return Ok(ToolOutput::ok(format!("service '{name}' already running at {addr}")));
        }
        Some((false, addr)) => {
            table.remove(name);
            return Ok(ToolOutput::err(format!(
                "service '{name}' (was at {addr}) had exited; not auto-restarted (ADR 0021) — call again to restart"
            )));
        }
        None => {}
    }

    let dir = root.join("capabilities").join(name);
    let address = if m.address.is_empty() { "(no address declared)".to_string() } else { m.address.clone() };
    let cap_args = json!({}); // persistent services read args from their own config

    match m.environment {
        Environment::Shell => {
            match Command::new("sh").arg("-c").arg(&m.entry).current_dir(&dir).spawn() {
                Ok(child) => {
                    let pid = child.id();
                    table.insert(name.to_string(), ServiceHandle { service: Service::Process(child), address: address.clone() });
                    Ok(ToolOutput::ok(format!("started persistent service '{name}' (pid {pid}) at {address}")))
                }
                Err(e) => Ok(ToolOutput::err(format!("failed to start '{name}': {e}"))),
            }
        }
        Environment::Docker => {
            if m.image.trim().is_empty() {
                return Ok(ToolOutput::err("docker capability requires an 'image' in its manifest"));
            }
            let cname = format!("codecoder-{name}");
            let dir_str = dir.to_string_lossy().into_owned();
            let out = Command::new("docker")
                .args(["run", "-d", "--rm", "--name", &cname, "--network", "none", "--memory", "512m", "--cpus", "1"])
                .arg("-e")
                .arg(format!("CODECODER_CAPABILITY_ARGS={cap_args}"))
                .arg("-v")
                .arg(format!("{dir_str}:/work:ro"))
                .args(["-w", "/work", &m.image, "sh", "-c", &m.entry])
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    table.insert(name.to_string(), ServiceHandle { service: Service::Container(cname.clone()), address: address.clone() });
                    Ok(ToolOutput::ok(format!("started persistent container '{cname}' at {address}")))
                }
                Ok(o) => Ok(ToolOutput::err(format!("docker run failed: {}", String::from_utf8_lossy(&o.stderr)))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(ToolOutput::err("Docker not available; refusing to downgrade (ADR 0021)"))
                }
                Err(e) => Ok(ToolOutput::err(format!("docker run failed: {e}"))),
            }
        }
        Environment::Wasm => unreachable!(),
    }
}

/// Run an OnDemand capability: starts a process, keeps it alive briefly for
/// reuse within the same turn, then auto-reaps. OnDemand services are tracked
/// in the RunningServiceTable and checked for liveness on each call (via
/// `try_wait`); if the prior process has exited, a new one is spawned.
/// For Wasm, OnDemand behaves as OneShot (no reuse — wasmtime modules are
/// ephemeral and cheaper to re-instantiate).
fn run_ondemand(name: &str, m: &CapabilityManifest, root: &Path) -> anyhow::Result<ToolOutput> {
    use crate::capability::{Service, ServiceHandle, services};
    let table = services();
    let mut table = table.lock().unwrap();

    // Check for an existing warm handle.
    let fresh = if let Some(h) = table.get_mut(name) {
        match &mut h.service {
            Service::Process(child) => match child.try_wait() {
                Ok(None) => {
                    return Ok(ToolOutput::ok(format!(
                        "ondemand '{}' still warm at {}", name, h.address
                    )));
                }
                _ => {
                    table.remove(name);
                    true
                }
            },
            Service::Container(_) => {
                table.remove(name);
                true
            }
        }
    } else {
        true
    };

    if !fresh {
        return Ok(ToolOutput::ok(format!("ondemand '{}' already running", name)));
    }

    let dir = root.join("capabilities").join(name);
    let address = if m.address.is_empty() { format!("ondemand:{name}") } else { m.address.clone() };

    match m.environment {
        Environment::Shell => {
            match Command::new("sh").arg("-c").arg(&m.entry).current_dir(&dir).spawn() {
                Ok(child) => {
                    let pid = child.id();
                    table.insert(name.to_string(), ServiceHandle {
                        service: Service::Process(child),
                        address: address.clone(),
                    });
                    Ok(ToolOutput::ok(format!(
                        "started ondemand '{}' (pid {}) at {}", name, pid, address
                    )))
                }
                Err(e) => Ok(ToolOutput::err(format!("failed to start ondemand '{}': {}", name, e))),
            }
        }
        Environment::Docker => {
            if m.image.trim().is_empty() {
                return Ok(ToolOutput::err("docker ondemand requires an 'image' in manifest"));
            }
            let cname = format!("codecoder-ondemand-{name}");
            let dir_str = dir.to_string_lossy().into_owned();
            let out = Command::new("docker")
                .args(["run", "-d", "--rm", "--name", &cname, "--network", "none", "--memory", "256m", "--cpus", "1"])
                .arg("-v").arg(format!("{dir_str}:/work:ro"))
                .args(["-w", "/work", &m.image, "sh", "-c", &m.entry])
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    table.insert(name.to_string(), ServiceHandle {
                        service: Service::Container(cname.clone()),
                        address: address.clone(),
                    });
                    Ok(ToolOutput::ok(format!("started ondemand container '{}' at {}", cname, address)))
                }
                Ok(o) => Ok(ToolOutput::err(format!("docker run failed: {}", String::from_utf8_lossy(&o.stderr)))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Ok(ToolOutput::err("Docker not available; refusing to downgrade (ADR 0021)"))
                }
                Err(e) => Ok(ToolOutput::err(format!("docker run failed: {e}"))),
            }
        }
        Environment::Wasm => {
            let cap_args = json!({});
            match crate::tool::wasm::run_wasm(&dir, &m.entry, &cap_args.to_string()) {
                Ok((out, is_error)) => Ok(ToolOutput { content: out, is_error, session_meta_mark: None }),
                Err(e) => Ok(ToolOutput::err(format!("wasm run failed: {e}"))),
            }
        }
    }
}

pub struct GenerateSkill;

impl Tool for GenerateSkill {
    fn name(&self) -> &str {
        "generate_skill"
    }
    fn description(&self) -> &str {
        "Author a new Skill (.md, ideally with name/description frontmatter) into skills/. Takes effect on /reload."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "content": { "type": "string", "description": "Full markdown, ideally with --- name/description --- frontmatter." }
            },
            "required": ["name", "content"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        // Authoring only writes a file — gated at write_file level (ADR 0022).
        Permission::Ask { key: "generate_skill".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        let content = args.get("content").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            return Ok(ToolOutput::err("missing required arg: name"));
        }
        let dir = ctx.root.join("skills");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{name}.md")), content)?;
        Ok(ToolOutput::ok(format!("wrote skill '{name}' — run /reload to register it")))
    }
}

pub struct GeneratePrompt;

impl Tool for GeneratePrompt {
    fn name(&self) -> &str {
        "generate_prompt"
    }
    fn description(&self) -> &str {
        "Author a reusable prompt template into prompts/<name>.md."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["name", "content"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "generate_prompt".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        let content = args.get("content").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            return Ok(ToolOutput::err("missing required arg: name"));
        }
        let dir = ctx.root.join("prompts");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{name}.md")), content)?;
        Ok(ToolOutput::ok(format!("wrote prompt template 'prompts/{name}.md'")))
    }
}

pub struct PromotePrompt;

impl Tool for PromotePrompt {
    fn name(&self) -> &str {
        "promote_prompt"
    }
    fn description(&self) -> &str {
        "Promote a draft prompts/<name>.md into a durable Skill at skills/<name>.md, \
         removing the draft. Omit `content` to promote verbatim; pass it to land a \
         refined version. Errors if the Skill already exists. Takes effect on /reload."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Draft prompt name (without .md)." },
                "content": { "type": "string", "description": "Optional refined skill body; omit to promote the draft verbatim." }
            },
            "required": ["name"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        // Crosses prompts/ → skills/ but only writes files — gated at write_file level (ADR 0022/0025).
        Permission::Ask { key: "promote_prompt".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            return Ok(ToolOutput::err("missing required arg: name"));
        }
        let draft = ctx.root.join("prompts").join(format!("{name}.md"));
        let skill = ctx.root.join("skills").join(format!("{name}.md"));

        // The draft must exist — promotion is "this draft earned it", not "create a skill".
        let draft_body = match std::fs::read_to_string(&draft) {
            Ok(t) => t,
            Err(_) => return Ok(ToolOutput::err(format!("no such prompt draft: {name}"))),
        };
        // Refuse to clobber a matured, validated Skill (ADR 0025).
        if skill.exists() {
            return Ok(ToolOutput::err(format!(
                "skill '{name}' already exists; refusing to overwrite — rename or edit it instead"
            )));
        }

        let body = args.get("content").and_then(Value::as_str).unwrap_or(&draft_body);
        std::fs::create_dir_all(ctx.root.join("skills"))?;
        std::fs::write(&skill, body)?;
        // Atomic intent: skill written, draft removed. If removal fails, the promotion
        // still stands; surface it so a stale draft can be cleaned up.
        match std::fs::remove_file(&draft) {
            Ok(()) => Ok(ToolOutput::ok(format!(
                "promoted '{name}': wrote skills/{name}.md, removed draft — run /reload to register it"
            ))),
            Err(e) => Ok(ToolOutput::ok(format!(
                "promoted '{name}' to skills/{name}.md, but could not remove prompts/{name}.md ({e}); delete it manually. Run /reload."
            ))),
        }
    }
}

pub struct GenerateCapability;

impl Tool for GenerateCapability {
    fn name(&self) -> &str {
        "generate_capability"
    }
    fn description(&self) -> &str {
        "Author a new Capability (manifest + optional entry script) into capabilities/. Takes effect on /reload."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "description": { "type": "string" },
                "environment": { "type": "string", "enum": ["shell", "wasm", "docker"] },
                "lifecycle": { "type": "string", "enum": ["one_shot", "on_demand", "persistent"] },
                "entry": { "type": "string", "description": "Command to run (sh -c) in the environment." },
                "script": { "type": "string", "description": "Optional: script body written to run.sh; entry becomes 'sh run.sh'." },
                "image": { "type": "string", "description": "Docker image (required when environment is docker)." },
                "usage": { "type": "string" }
            },
            "required": ["name", "description", "environment", "lifecycle"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::Ask { key: "generate_capability".into() }
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
        let description = args.get("description").and_then(Value::as_str).unwrap_or_default();
        if name.is_empty() {
            return Ok(ToolOutput::err("missing required arg: name"));
        }
        let environment = match args.get("environment").and_then(Value::as_str) {
            Some("shell") => Environment::Shell,
            Some("wasm") => Environment::Wasm,
            Some("docker") => Environment::Docker,
            other => return Ok(ToolOutput::err(format!("invalid environment: {other:?}"))),
        };
        let lifecycle = match args.get("lifecycle").and_then(Value::as_str) {
            Some("one_shot") => Lifecycle::OneShot,
            Some("on_demand") => Lifecycle::OnDemand,
            Some("persistent") => Lifecycle::Persistent,
            other => return Ok(ToolOutput::err(format!("invalid lifecycle: {other:?}"))),
        };

        let dir = ctx.root.join("capabilities").join(name);
        std::fs::create_dir_all(&dir)?;

        // Optional script body → run.sh (chmod +x), and entry points at it.
        let entry = if let Some(script) = args.get("script").and_then(Value::as_str) {
            let script_path = dir.join("run.sh");
            std::fs::write(&script_path, script)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
            }
            "sh run.sh".to_string()
        } else {
            args.get("entry").and_then(Value::as_str).unwrap_or_default().to_string()
        };

        let manifest = CapabilityManifest {
            name: name.to_string(),
            description: description.to_string(),
            environment,
            lifecycle,
            entry,
            image: args.get("image").and_then(Value::as_str).unwrap_or_default().to_string(),
            address: args.get("address").and_then(Value::as_str).unwrap_or_default().to_string(),
            usage: args.get("usage").and_then(Value::as_str).unwrap_or_default().to_string(),
        };
        std::fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&manifest)?)?;
        Ok(ToolOutput::ok(format!("wrote capability '{name}' — run /reload to register it")))
    }
}

/// `agent` is registered for its schema/name, but its execution is intercepted by
/// the AgentLoop (which owns the Provider + event channel needed to spawn a
/// sub-agent). `run` is therefore never called (ADR 0019).
pub struct Agent;

impl Tool for Agent {
    fn name(&self) -> &str {
        "agent"
    }
    fn description(&self) -> &str {
        "Delegate a read-only research sub-task to a sub-agent; returns its findings."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "task": { "type": "string", "description": "The delegated task." } },
            "required": ["task"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None // the sub-agent itself can only use Permission::None tools
    }
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::err("agent tool must be handled by the AgentLoop"))
    }
}

/// `ask_user` and `review` are also intercepted by the AgentLoop (they need the
/// event channel / sub-agent machinery). Registered here for schema/name only.
pub struct AskUser;

impl Tool for AskUser {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn description(&self) -> &str {
        "Ask the user a question and wait for their typed answer."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "question": { "type": "string" } },
            "required": ["question"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None // it IS a prompt; no separate permission needed
    }
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::err("ask_user must be handled by the AgentLoop"))
    }
}

pub struct Review;

impl Tool for Review {
    fn name(&self) -> &str {
        "review"
    }
    fn description(&self) -> &str {
        "Delegate a read-only architecture review of a target (path, or the current changes) to a \
         sub-agent. Returns a structured verdict (pass / needs_fix / rebuild) plus drift signals."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "target": { "type": "string", "description": "Path or description; default: the current diff." } }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::err("review must be handled by the AgentLoop"))
    }
}

pub struct Confirm;

impl Tool for Confirm {
    fn name(&self) -> &str {
        "confirm"
    }
    fn description(&self) -> &str {
        "Ask the user a yes/no question before proceeding; returns \"yes\" or \"no\"."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "prompt": { "type": "string" } },
            "required": ["prompt"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, _args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::err("confirm must be handled by the AgentLoop"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_is_permissionless() {
        assert!(matches!(ReadFile.permission(&json!({}), std::path::Path::new(".")), Permission::None));
    }

    #[test]
    fn run_command_keys_by_command_class() {
        match RunCommand.permission(&json!({ "cmd": "git status --short" }), std::path::Path::new(".")) {
            Permission::Ask { key } => assert_eq!(key, "run_command:git"),
            _ => panic!("expected Ask"),
        }
    }

    #[test]
    fn run_command_compound_keys_by_full_string() {
        // 复合命令(含 shell 运算符)→ 整条命令串 key,不可经良性前缀预授权(ADR 0036)。
        let cases: &[(&str, &str)] = &[
            ("cd X && cargo test", "run_command:cd X && cargo test"),
            ("ls | grep foo", "run_command:ls | grep foo"),
            ("a; b", "run_command:a; b"),
            ("sort &", "run_command:sort &"),
            ("echo `whoami`", "run_command:echo `whoami`"),
            ("tee <(x)", "run_command:tee <(x)"),
        ];
        for (cmd, want) in cases {
            match RunCommand.permission(&json!({ "cmd": cmd }), std::path::Path::new(".")) {
                Permission::Ask { key } => assert_eq!(key, *want, "cmd={cmd:?}"),
                _ => panic!("expected Ask for {cmd:?}"),
            }
        }
    }

    #[test]
    fn run_command_simple_keys_by_first_token() {
        // 简单命令(无运算符)→ 首 token,同既有行为,可预授权。
        let cases: &[(&str, &str)] = &[
            ("echo hi", "run_command:echo"),
            ("cargo test", "run_command:cargo"),
        ];
        for (cmd, want) in cases {
            match RunCommand.permission(&json!({ "cmd": cmd }), std::path::Path::new(".")) {
                Permission::Ask { key } => assert_eq!(key, *want, "cmd={cmd:?}"),
                _ => panic!("expected Ask for {cmd:?}"),
            }
        }
    }

    #[test]
    fn run_command_executes() {
        let root = std::env::temp_dir();
        let mut ctx = ToolCtx::new(&root);
        let out = RunCommand.run(json!({ "cmd": "echo hi" }), &mut ctx).unwrap();
        assert!(!out.is_error);
        assert_eq!(out.content.trim(), "hi");
    }

    #[test]
    fn shell_cancellable_kills_child_when_cancelled_mid_run() {
        let token = crate::agent::CancelToken::default();
        let dir = std::env::temp_dir();
        let ctx = ToolCtx::with_cancel(&dir, &token);
        let t2 = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            t2.cancel();
        });
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 5");
        let start = std::time::Instant::now();
        let out = run_shell_cancellable(command, &ctx).unwrap();
        assert!(out.content.contains("cancelled"), "got: {}", out.content);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "the child must be killed promptly on cancel; took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn shell_cancellable_short_circuits_when_already_cancelled() {
        let token = crate::agent::CancelToken::default();
        token.cancel();
        let dir = std::env::temp_dir();
        let ctx = ToolCtx::with_cancel(&dir, &token);
        let mut command = Command::new("sh");
        command.arg("-c").arg("echo should-not-matter");
        let out = run_shell_cancellable(command, &ctx).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("cancelled"));
    }

    #[test]
    fn write_then_edit_then_read() {
        let dir = std::env::temp_dir().join(format!("cc_tool_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);

        let w = WriteFile
            .run(json!({ "path": "sub/f.txt", "content": "alpha beta" }), &mut ctx)
            .unwrap();
        assert!(!w.is_error, "{}", w.content);

        let e = EditFile
            .run(json!({ "path": "sub/f.txt", "old": "beta", "new": "gamma" }), &mut ctx)
            .unwrap();
        assert!(!e.is_error, "{}", e.content);

        let r = ReadFile.run(json!({ "path": "sub/f.txt" }), &mut ctx).unwrap();
        assert_eq!(r.content, "alpha gamma");

        // edit_file requires `old` to exist.
        let miss = EditFile
            .run(json!({ "path": "sub/f.txt", "old": "nope", "new": "x" }), &mut ctx)
            .unwrap();
        assert!(miss.is_error);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_directory_is_permissionless() {
        assert!(matches!(ListDirectory.permission(&json!({}), std::path::Path::new(".")), Permission::None));
    }

    #[test]
    fn write_file_requires_permission() {
        match WriteFile.permission(&json!({ "path": "x" }), std::path::Path::new(".")) {
            Permission::Ask { key } => assert_eq!(key, "write_file"),
            _ => panic!("expected Ask"),
        }
    }

    #[test]
    fn use_skill_returns_file_contents() {
        let dir = std::env::temp_dir().join(format!("cc_skill_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills/greet.md"), "always say hi").unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = UseSkill.run(json!({ "name": "greet" }), &mut ctx).unwrap();
        assert!(!out.is_error);
        assert_eq!(out.content, "always say hi");
        let miss = UseSkill.run(json!({ "name": "nope" }), &mut ctx).unwrap();
        assert!(miss.is_error);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_then_scan_roundtrip() {
        use crate::registry::Registry;
        let dir = std::env::temp_dir().join(format!("cc_gen_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);

        GenerateSkill
            .run(
                json!({ "name": "greet", "content": "---\nname: greet\ndescription: say hello\n---\nbody" }),
                &mut ctx,
            )
            .unwrap();
        GenerateCapability
            .run(
                json!({
                    "name": "echoer", "description": "echoes", "environment": "shell",
                    "lifecycle": "one_shot", "script": "echo hi"
                }),
                &mut ctx,
            )
            .unwrap();

        // The generated capability ran its script path via the written run.sh.
        let out = RunCapability.run(json!({ "name": "echoer" }), &mut ctx).unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content.trim(), "hi");

        // Both are now discoverable by the Registry (i.e. /reload would register them).
        let reg = Registry::scan(&dir);
        let cat = reg.render_catalog();
        assert!(cat.contains("greet — say hello"), "{cat}");
        assert!(cat.contains("echoer — echoes"), "{cat}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prompt_draft_lifecycle_generate_use_promote() {
        use crate::registry::Registry;
        let dir = std::env::temp_dir().join(format!("cc_prompt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);

        // 1. Author a draft prompt.
        GeneratePrompt
            .run(
                json!({ "name": "tone", "content": "---\nname: tone\ndescription: keep replies terse\n---\nbe terse" }),
                &mut ctx,
            )
            .unwrap();

        // 2. Registry surfaces it under Skills with a [draft] marker (ADR 0025).
        let cat = Registry::scan(&dir).render_catalog();
        assert!(cat.contains("tone — [draft] keep replies terse"), "{cat}");

        // 3. use_skill activates the draft by name (skills/ miss → prompts/ fallback).
        let used = UseSkill.run(json!({ "name": "tone" }), &mut ctx).unwrap();
        assert!(!used.is_error);
        assert!(used.content.contains("be terse"));

        // 4. promote_prompt moves it to skills/ and deletes the draft.
        let promoted = PromotePrompt.run(json!({ "name": "tone" }), &mut ctx).unwrap();
        assert!(!promoted.is_error, "{}", promoted.content);
        assert!(dir.join("skills/tone.md").exists());
        assert!(!dir.join("prompts/tone.md").exists(), "draft should be removed");

        // 5. It is now a matured Skill (no [draft] marker).
        let cat2 = Registry::scan(&dir).render_catalog();
        assert!(cat2.contains("tone — keep replies terse"), "{cat2}");
        assert!(!cat2.contains("[draft]"), "{cat2}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn promote_prompt_errors_on_missing_draft_and_name_collision() {
        let dir = std::env::temp_dir().join(format!("cc_promerr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);

        // No draft → error.
        let miss = PromotePrompt.run(json!({ "name": "ghost" }), &mut ctx).unwrap();
        assert!(miss.is_error);

        // Draft exists AND a same-named Skill already exists → refuse to overwrite.
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("prompts/dup.md"), "draft body").unwrap();
        std::fs::write(dir.join("skills/dup.md"), "matured body").unwrap();
        let clash = PromotePrompt.run(json!({ "name": "dup" }), &mut ctx).unwrap();
        assert!(clash.is_error);
        // The matured Skill is untouched and the draft survives.
        assert_eq!(std::fs::read_to_string(dir.join("skills/dup.md")).unwrap(), "matured body");
        assert!(dir.join("prompts/dup.md").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn promote_prompt_is_gated_at_write_level() {
        match PromotePrompt.permission(&json!({ "name": "x" }), std::path::Path::new(".")) {
            Permission::Ask { key } => assert_eq!(key, "promote_prompt"),
            _ => panic!("expected Ask"),
        }
    }

    #[test]
    fn generate_capability_rejects_bad_environment() {
        let dir = std::env::temp_dir().join(format!("cc_genbad_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = GenerateCapability
            .run(
                json!({ "name": "x", "description": "d", "environment": "nope", "lifecycle": "one_shot" }),
                &mut ctx,
            )
            .unwrap();
        assert!(out.is_error);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn persistent_service_starts_once_then_reports_running() {
        let dir = std::env::temp_dir().join(format!("cc_persist_{}", std::process::id()));
        let capdir = dir.join("capabilities/server");
        std::fs::create_dir_all(&capdir).unwrap();
        std::fs::write(
            capdir.join("manifest.json"),
            r#"{"name":"server","description":"d","environment":"shell","lifecycle":"persistent","entry":"sleep 5","address":"127.0.0.1:9999"}"#,
        )
        .unwrap();
        let mut ctx = ToolCtx::new(&dir);

        let first = RunCapability.run(json!({ "name": "server" }), &mut ctx).unwrap();
        assert!(!first.is_error, "{}", first.content);
        assert!(first.content.contains("started persistent service 'server'"), "{}", first.content);
        assert!(first.content.contains("127.0.0.1:9999"));

        let second = RunCapability.run(json!({ "name": "server" }), &mut ctx).unwrap();
        assert!(second.content.contains("already running"), "{}", second.content);

        // Bound to process lifetime: shutdown kills it.
        crate::capability::shutdown_all();
        assert!(crate::capability::services().lock().unwrap().names().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn docker_capability_requires_image() {
        let dir = std::env::temp_dir().join(format!("cc_dk_{}", std::process::id()));
        let capdir = dir.join("capabilities/box");
        std::fs::create_dir_all(&capdir).unwrap();
        std::fs::write(
            capdir.join("manifest.json"),
            r#"{"name":"box","description":"d","environment":"docker","lifecycle":"one_shot","entry":"echo hi"}"#,
        )
        .unwrap();
        // keys by @docker
        match RunCapability.permission(&json!({ "name": "box" }), &dir) {
            Permission::Ask { key } => assert_eq!(key, "run_capability:box@docker"),
            _ => panic!("expected Ask"),
        }
        let mut ctx = ToolCtx::new(&dir);
        let out = RunCapability.run(json!({ "name": "box" }), &mut ctx).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("image"), "{}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore = "requires a running Docker daemon"]
    fn docker_persistent_service_starts_and_reports_running() {
        let dir = std::env::temp_dir().join(format!("cc_dpers_{}", std::process::id()));
        let capdir = dir.join("capabilities/dpers");
        std::fs::create_dir_all(&capdir).unwrap();
        std::fs::write(
            capdir.join("manifest.json"),
            r#"{"name":"dpers","description":"d","environment":"docker","lifecycle":"persistent","image":"alpine","entry":"sleep 30","address":"127.0.0.1:1234"}"#,
        )
        .unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let first = RunCapability.run(json!({ "name": "dpers" }), &mut ctx).unwrap();
        assert!(!first.is_error, "{}", first.content);
        assert!(first.content.contains("started persistent container"), "{}", first.content);
        let second = RunCapability.run(json!({ "name": "dpers" }), &mut ctx).unwrap();
        assert!(second.content.contains("already running"), "{}", second.content);
        crate::capability::shutdown_all(); // docker rm -f
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[ignore = "requires a running Docker daemon + network to pull alpine"]
    fn docker_capability_runs_container() {
        let dir = std::env::temp_dir().join(format!("cc_dkrun_{}", std::process::id()));
        let capdir = dir.join("capabilities/hello");
        std::fs::create_dir_all(&capdir).unwrap();
        std::fs::write(
            capdir.join("manifest.json"),
            r#"{"name":"hello","description":"d","environment":"docker","lifecycle":"one_shot","image":"alpine","entry":"echo from-container"}"#,
        )
        .unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = RunCapability.run(json!({ "name": "hello" }), &mut ctx).unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("from-container"), "{}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ondemand_shell_starts_and_reports() {
        let dir = std::env::temp_dir().join(format!("cc_ondemand_{}", std::process::id()));
        let capdir = dir.join("capabilities/od");
        std::fs::create_dir_all(&capdir).unwrap();
        std::fs::write(
            capdir.join("manifest.json"),
            r#"{"name":"od","description":"d","environment":"shell","lifecycle":"on_demand","entry":"sleep 5"}"#,
        ).unwrap();
        let mut ctx = ToolCtx::new(&dir);
        let out = RunCapability.run(json!({ "name": "od" }), &mut ctx).unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("ondemand"), "{}", out.content);
        crate::capability::shutdown_all();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_capability_shell_executes_and_keys_by_env() {
        let dir = std::env::temp_dir().join(format!("cc_cap_{}", std::process::id()));
        let capdir = dir.join("capabilities/echoer");
        std::fs::create_dir_all(&capdir).unwrap();
        std::fs::write(
            capdir.join("manifest.json"),
            r#"{"name":"echoer","description":"echo","environment":"shell","lifecycle":"one_shot","entry":"echo from-capability"}"#,
        )
        .unwrap();

        match RunCapability.permission(&json!({ "name": "echoer" }), &dir) {
            Permission::Ask { key } => assert_eq!(key, "run_capability:echoer@shell"),
            _ => panic!("expected Ask"),
        }

        let mut ctx = ToolCtx::new(&dir);
        let out = RunCapability.run(json!({ "name": "echoer" }), &mut ctx).unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(out.content.trim(), "from-capability");

        std::fs::remove_dir_all(&dir).ok();
    }
}
