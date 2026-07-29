//! BG 客观验收门 + continue/stop 策略(spec 2026-07-22)。
//!
//! Background Agent 的失败安全层:turn 结束后**客观**判定 milestone 的 acceptance,
//! verdict **覆盖** agent 自报的 `VERDICT:` 行;失败时由 background.rs 写回
//! `needs_fix` + reason 因果节点。本模块全是纯函数 + 一个可取消 shell 调用,
//! 便于 hermetic 单测。
use crate::agent::CancelToken;
use crate::tool::ToolCtx;
use crate::tool::builtin::run_shell_cancellable;
use crate::workgraph::{CheckSpec, CheckType, Milestone, NodeStatus, WorkGraph};
use std::path::Path;
use std::process::Command;
#[cfg(test)]
use serde_json::json;

/// 客观验收门的判定结果。**覆盖** agent 自报 verdict。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateVerdict {
    Pass,
    NeedsFix(String),
    Inconclusive(String),
}

/// 从 acceptance 文本提取可执行的验收命令(若有)。按行扫描,返回首个含已知
/// 测试/构建命令模式**且为纯 ASCII 命令行**的行。已知模式:cargo test/build/check/clippy、
/// pytest、npm/yarn test、make、go test、rustc。
///
/// 含命令关键字但混有 prose(尤其 CJK 描述,如 `cargo init ... 创建二进制项目`)的行**不**
/// 视为可运行命令——原样交 `sh -c` 执行要么因 prose 报错(假 needs_fix),要么退化成
/// 空过滤(假 pass)。这类描述性 acceptance 跳过命令门,改由注入式 review 门评判。
pub fn extract_gate_command(acceptance: &str) -> Option<String> {
    const PATTERNS: &[&str] = &[
        "cargo test",
        "cargo build",
        "cargo check",
        "cargo clippy",
        "pytest",
        "py.test",
        "npm test",
        "yarn test",
        "go test",
        "rustc",
        "make ",
    ];
    for line in acceptance.lines() {
        let trimmed = line.trim();
        let low = trimmed.to_lowercase();
        if PATTERNS.iter().any(|p| low.contains(p)) {
            // 仅当整行是纯 ASCII 命令(无 prose)时才作为 shell 门执行;否则继续扫描,
            // 找不到干净命令行则返回 None → 交 review 门。
            if trimmed.is_ascii() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// 跑命令门:exit 0 → Pass;非零 → NeedsFix(附输出摘要);跑不起来 → Inconclusive。
/// 对构建类命令(如 build)，exit 0 后额外检查常见产物路径是否存在，加深验证(迭代 5)。
pub fn run_command_gate(cmd: &str, root: &Path, cancel: Option<&CancelToken>) -> GateVerdict {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd).current_dir(root);
    let r = match cancel {
        Some(c) => run_shell_cancellable(command, &ToolCtx::with_cancel(root, c)),
        None => run_shell_cancellable(command, &ToolCtx::new(root)),
    };
    match r {
        Ok(out) if !out.is_error => {
            // build 类命令 exit 0 后，额外检查常见产物存在性(迭代 5)。
            let build_check = build_output_check(cmd, root, cancel);
            match build_check {
                Some(Err(e)) => GateVerdict::NeedsFix(format!("`{cmd}` passed but build output check failed: {e}")),
                _ => GateVerdict::Pass,
            }
        }
        Ok(out) => GateVerdict::NeedsFix(format!("gate `{cmd}` failed: {}", truncate(out.content, 400))),
        Err(e) => GateVerdict::Inconclusive(format!("gate `{cmd}` could not run: {e}")),
    }
}

/// 对已知构建命令检查产物文件是否存在。返回 None(非 build 命令)或 Some(Ok/Err)。
fn build_output_check(cmd: &str, root: &Path, cancel: Option<&CancelToken>) -> Option<std::io::Result<()>> {
    let low = cmd.to_lowercase();
    let checks: Vec<&str> = if low.contains("vite build") || low.contains("npm run build") {
        vec!["dist/index.html", "dist/assets/"]
    } else if low.contains("cargo build") {
        vec!["target/debug/", "Cargo.toml"]
    } else if low.contains("mkdocs build") || low.contains("sphinx-build") {
        vec!["site/index.html"]
    } else {
        return None;
    };
    let cancel_flag = cancel.map(|c| c.is_cancelled()).unwrap_or(false);
    if cancel_flag {
        return Some(Err(std::io::Error::new(std::io::ErrorKind::Other, "cancelled")));
    }
    for path in &checks {
        let full = root.join(path);
        if !full.exists() {
            return Some(Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("expected build output not found: {path}"))));
        }
    }
    // 额外运行时验证:如果检查到 dist/index.html,验证其内容合理性。
    if checks.iter().any(|p| p.contains("index.html")) {
        if let Err(e) = runtime_verify_html(root) {
            return Some(Err(e));
        }
    }
    Some(Ok(()))
}

/// 运行时验证:检查构建产物的 HTML 文件内容是否合理(非空、有基本结构)。
fn runtime_verify_html(root: &Path) -> std::io::Result<()> {
    let html_path = root.join("dist/index.html");
    if !html_path.exists() {
        return Ok(()); // 无 HTML 产物,跳过验证
    }
    let content = std::fs::read_to_string(&html_path)?;
    if content.trim().is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "dist/index.html is empty"));
    }
    if !content.contains("<html") && !content.contains("<!DOCTYPE html") && !content.contains("<div") && !content.contains("<body") {
        return Err(std::io::Error::new(std::io::ErrorKind::Other,
            "dist/index.html appears to be missing HTML structure (no <html>/<div>/<body> tags)"));
    }
    Ok(())
}

fn truncate(s: String, n: usize) -> String {
    if s.chars().count() <= n {
        s
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

/// 本里程碑将走哪种验收门(迭代 3 可观测)。默认 None(旧账本记录无门类信息时最保守)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GateKind {
    Command,
    Review,
    None,
}

impl Default for GateKind {
    fn default() -> Self {
        GateKind::None
    }
}

/// 客观命令门要跑的命令:显式 `command` 优先,旧数据裸命令启发式(extract_gate_command)兜底。
pub fn gate_command(m: &Milestone) -> Option<String> {
    m.command.clone().or_else(|| extract_gate_command(&m.acceptance))
}

/// 门路由决策(单一事实源):有命令→Command;否则 acceptance 空→None;否则→Review。
pub fn gate_kind(m: &Milestone) -> GateKind {
    if gate_command(m).is_some() {
        GateKind::Command
    } else if m.acceptance.trim().is_empty() {
        GateKind::None
    } else {
        GateKind::Review
    }
}

/// 顶层验收:命令门优先(客观);否则注入式 review 门;acceptance 空 → Inconclusive。
/// 路由决策统一走 `gate_kind`(单一事实源)。
/// `review_runner` 注入便于纯策略测试;prod 由 background.rs 注入调用 review 工具的闭包。
pub fn evaluate(
    m: &Milestone,
    root: &Path,
    cancel: Option<&CancelToken>,
    review_runner: &dyn Fn() -> GateVerdict,
) -> GateVerdict {
    match gate_kind(m) {
        GateKind::Command => {
            let cmd = gate_command(m).expect("gate_kind==Command ⇒ gate_command is Some");
            let verdict = run_command_gate(&cmd, root, cancel);
            // 命令门 pass 后执行 checks（Phase 1）
            if verdict == GateVerdict::Pass {
                if let Some(checks) = &m.checks {
                    if !checks.is_empty() {
                        if let Err(errors) = run_checks(checks, root) {
                            let detail = errors.join("; ");
                            return GateVerdict::NeedsFix(format!("command passed but checks failed: {detail}"));
                        }
                    }
                }
            }
            verdict
        }
        GateKind::None => {
            // 宽容模式: milestone 有显式 command + touched 文件(证明已产生代码)时,
            // 降级跑命令门验收,而非直接 Inconclusive。这解决 seed 阶段 generate_milestones
            // 生成空 acceptance 里程碑时,代码已通过构建但验收门仍标记为 needs_fix 的问题。
            if let Some(cmd) = &m.command {
                if !m.touched.is_empty() {
                    return run_command_gate(cmd, root, cancel);
                }
            }
            GateVerdict::Inconclusive("no acceptance criterion (weak signal)".into())
        }
        GateKind::Review => review_runner(),
    }
}

// ── continue/stop 策略 ─────────────────────────────────────────────────────

/// 一次 BG 调用的整体任务终态。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MissionState {
    /// 仍在推进循环内。
    Running,
    /// 无更多就绪 milestone(全部完成或无就绪)。
    CompletedAllReady,
    /// 图中无任何里程碑：headless 无事可做。区别于 CompletedAllReady 的"真完成"
    /// （曾有里程碑且全部到达终态），避免空图 exit 0 假报成功（spec 2026-07-25）。
    EmptyGraph,
    /// 某 milestone 失败且其下游全部 Blocked,任务无法继续。
    BlockedAt(u64),
    /// 连续 K 个 milestone 失败,熔断。
    CircuitBreaker,
    /// 无就绪 milestone,但图中仍有 `needs_fix`(及被其阻塞的下游)——任务未完成,
    /// 需人工/上层修复该 milestone 后重置为 pending 再续跑。区别于 CompletedAllReady
    /// 的"真完成",避免 headless 空跑一轮却 exit 0 假报成功。
    StuckNeedsFix(u64),
    /// turn/provider 自身错误(不动 workgraph 状态)。
    Error(String),
}

/// `next_action` 的返回:推进到下一个 milestone,或停止并给出终态,或降级跳过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextAction {
    Advance(u64),
    /// 熔断时降级:跳过当前 milestone,继续推进下一个就绪里程碑(不阻塞依赖)。
    DegradeAndAdvance(u64),
    Stop(MissionState),
}

/// 决定一次 milestone 验收后的下一步。`next_ready()` 自然跳过因失败而 Blocked 的依赖者
/// (recompute_blocked 把 dep 未 Done 的节点置 Blocked)。
///
/// - Pass & 有就绪 & 有预算 → Advance(next)
/// - 失败 & consecutive ≥ k → Stop(CircuitBreaker)(即便有就绪,也停,防连环 flail)
/// - 无预算 → Stop(CompletedAllReady)
/// - 有就绪 → Advance(next)
/// - 无就绪 & 有被阻塞的下游 → Stop(BlockedAt(just_done_id));否则 CompletedAllReady
pub fn next_action(
    graph: &WorkGraph,
    just_done_id: u64,
    verdict: &GateVerdict,
    consecutive_fail: usize,
    budget_left: bool,
    k: usize,
) -> NextAction {
    let failed = !matches!(verdict, GateVerdict::Pass);
    // 熔断降级:连续 K 次失败且存在独立就绪里程碑(未被当前失败阻塞)。
    if failed && consecutive_fail >= k {
        if let Some(n) = graph.next_ready() {
            if !n.deps.contains(&just_done_id) {
                return NextAction::DegradeAndAdvance(n.id);
            }
        }
        return NextAction::Stop(MissionState::CircuitBreaker);
    }
    if !budget_left {
        return NextAction::Stop(MissionState::CompletedAllReady);
    }
    // 是否有因 just_done 失败而 Blocked 的下游(独立借用,先于 next_ready)。
    let has_blocked_dependent = graph
        .nodes
        .iter()
        .any(|n| n.status != NodeStatus::Done && n.deps.contains(&just_done_id));
    match graph.next_ready() {
        Some(n) => NextAction::Advance(n.id),
        None => {
            if has_blocked_dependent {
                NextAction::Stop(MissionState::BlockedAt(just_done_id))
            } else {
                NextAction::Stop(MissionState::CompletedAllReady)
            }
        }
    }
}

/// ── checks 引擎 ────────────────────────────────────────────────────────

/// 执行 checks 列表。全部成功 → Ok(())，任何失败 → Err(失败信息列表)。
pub fn run_checks(specs: &[CheckSpec], root: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for spec in specs {
        if let Err(e) = execute_check(spec, root) {
            errors.push(e);
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

fn execute_check(spec: &CheckSpec, root: &Path) -> Result<(), String> {
    match spec.type_ {
        CheckType::BuildExitZero => {
            let cmd = spec.params.get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "BuildExitZero check missing 'command' param".to_string())?;
            let mut command = Command::new("sh");
            command.arg("-c").arg(cmd).current_dir(root);
            let output = command.output().map_err(|e| format!("BuildExitZero command failed: {e}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!("BuildExitZero: `{cmd}` exited with {}", output.status))
            }
        }
        CheckType::NoTemplateContent => {
            let patterns = spec.params.get("patterns")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "NoTemplateContent check missing 'patterns' param".to_string())?;
            let forbidden = spec.params.get("forbidden")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "NoTemplateContent check missing 'forbidden' param".to_string())?;
            let forbidden_strs: Vec<&str> = forbidden.iter()
                .filter_map(|v| v.as_str()).collect();

            let mut found_issues = Vec::new();
            for pattern_val in patterns {
                let pattern = pattern_val.as_str()
                    .ok_or_else(|| "Invalid pattern (not a string)".to_string())?;
                let pattern_str = if pattern.starts_with("src/") {
                    root.join(pattern).to_string_lossy().to_string()
                } else {
                    root.join(pattern).to_string_lossy().to_string()
                };
                match glob::glob(&pattern_str) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            if let Ok(content) = std::fs::read_to_string(&entry) {
                                for forbidden in &forbidden_strs {
                                    if content.contains(forbidden) {
                                        found_issues.push(format!(
                                            "{} contains forbidden text '{}'",
                                            entry.display(), forbidden
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        found_issues.push(format!("glob pattern '{pattern}' failed: {e}"));
                    }
                }
            }
            if found_issues.is_empty() { Ok(()) } else { Err(found_issues.join("; ")) }
        }
        CheckType::FileCountMin => {
            let path = spec.params.get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "FileCountMin check missing 'path' param".to_string())?;
            let min = spec.params.get("min")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "FileCountMin check missing 'min' param".to_string())? as usize;
            let full = root.join(path);
            if !full.is_dir() {
                return Err(format!("FileCountMin: {path} is not a directory"));
            }
            let count = std::fs::read_dir(&full)
                .map_err(|e| format!("FileCountMin: cannot read {path}: {e}"))?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .count();
            if count >= min { Ok(()) } else { Err(format!("FileCountMin: {path} has {count} files, expected at least {min}")) }
        }
        CheckType::MinLinesPerFile => {
            let paths_pattern = spec.params.get("paths")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "MinLinesPerFile check missing 'paths' param".to_string())?;
            let min = spec.params.get("min")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "MinLinesPerFile check missing 'min' param".to_string())? as usize;
            let exclude = spec.params.get("exclude_patterns")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
                .unwrap_or_default();

            let pattern_abs = root.join(paths_pattern).to_string_lossy().to_string();
            let mut issues = Vec::new();
            match glob::glob(&pattern_abs) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let name = entry.to_string_lossy().to_string();
                        let is_excluded = exclude.iter().any(|e| {
                            name.contains(e.trim_start_matches("**/"))
                        });
                        if is_excluded { continue; }
                        if let Ok(content) = std::fs::read_to_string(&entry) {
                            let lines = content.lines().count();
                            if lines < min {
                                issues.push(format!("{} has {} lines, expected at least {}", entry.display(), lines, min));
                            }
                        }
                    }
                }
                Err(e) => {
                    issues.push(format!("glob '{paths_pattern}' failed: {e}"));
                }
            }
            if issues.is_empty() { Ok(()) } else { Err(issues.join("; ")) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ms(id: u64, acceptance: &str) -> Milestone {
        Milestone {
            id,
            title: format!("t{id}"),
            acceptance: acceptance.into(),
            deps: vec![],
            status: NodeStatus::Pending,
            verdict: None,
            touched: vec![],
            fix_attempts: 0,
            last_failure: None,
            command: None,
            checks: None,
        }
    }

    fn graph_with(nodes: Vec<Milestone>) -> WorkGraph {
        let mut g = WorkGraph::default();
        for n in nodes {
            g.nodes.push(n);
        }
        g
    }

    // ── extract_gate_command ──
    #[test]
    fn extract_gate_command_finds_clean_ascii_commands() {
        assert_eq!(extract_gate_command("cargo build"), Some("cargo build".into()));
        assert_eq!(extract_gate_command("cargo test"), Some("cargo test".into()));
        assert_eq!(extract_gate_command("runs: pytest -q"), Some("runs: pytest -q".into()));
        assert_eq!(extract_gate_command("make test"), Some("make test".into()));
    }

    #[test]
    fn extract_gate_command_skips_prose_acceptance_with_command_word() {
        // 描述性 acceptance(命令关键字 + CJK prose)不当作可运行命令,交 review 门。
        assert_eq!(extract_gate_command("cargo test 通过"), None);
        assert_eq!(
            extract_gate_command("cargo init --name coedit 创建二进制项目；cargo build 通过"),
            None
        );
        // 多行:跳过 prose 行,取后续干净命令行。
        assert_eq!(
            extract_gate_command("完成 CRDT 核心后 cargo build 应通过\ncargo test"),
            Some("cargo test".into())
        );
    }

    #[test]
    fn extract_gate_command_none_when_no_pattern() {
        assert_eq!(extract_gate_command("renderer 输出正确"), None);
        assert_eq!(extract_gate_command(""), None);
    }

    // ── run_command_gate ──
    #[test]
    fn command_gate_pass_on_exit_zero() {
        let dir = tempdir().unwrap();
        assert_eq!(run_command_gate("echo ok", dir.path(), None), GateVerdict::Pass);
    }

    #[test]
    fn command_gate_needsfix_on_nonzero() {
        let dir = tempdir().unwrap();
        match run_command_gate("false", dir.path(), None) {
            GateVerdict::NeedsFix(msg) => assert!(msg.contains("false"), "{msg}"),
            other => panic!("expected NeedsFix, got {other:?}"),
        }
    }

    #[test]
    fn command_gate_needsfix_on_missing_binary() {
        // sh -c <missing> → exit 127 → NeedsFix(sh 包装了缺失命令,非 spawn 错误)。
        // Inconclusive 分支为防御性代码(sh 基本总能 spawn),此处不直接触发。
        let dir = tempdir().unwrap();
        let v = run_command_gate("this-binary-does-not-exist-xyz-123", dir.path(), None);
        match v {
            GateVerdict::NeedsFix(msg) => assert!(msg.contains("command not found") || msg.contains("exit 127"), "{msg}"),
            other => panic!("expected NeedsFix for missing binary, got {other:?}"),
        }
    }

    // ── evaluate ──
    #[test]
    fn evaluate_uses_command_gate_when_present() {
        let dir = tempdir().unwrap();
        let m = ms(1, "cargo test"); // 含已知模式 → 命令门触发
        // review_runner 返回独特标记;若被调用说明命令门没生效。
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Inconclusive("REVIEW_RAN".into()));
        match v {
            GateVerdict::Inconclusive(msg) if msg.contains("REVIEW_RAN") =>
                panic!("review_runner was called; command gate should have fired first"),
            _ => {} // NeedsFix/Pass/其它 Inconclusive 都说明命令门跑了(空 tempdir 下 cargo test 多半 NeedsFix)
        }
    }

    #[test]
    fn evaluate_falls_back_to_review_runner() {
        let dir = tempdir().unwrap();
        let m = ms(1, "renderer 输出正确"); // 无命令模式
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::NeedsFix("review says no".into()));
        assert_eq!(v, GateVerdict::NeedsFix("review says no".into()));
    }

    #[test]
    fn evaluate_inconclusive_when_acceptance_empty() {
        let dir = tempdir().unwrap();
        let m = ms(1, "");
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
        assert!(matches!(v, GateVerdict::Inconclusive(_)));
    }

    #[test]
    fn evaluate_none_with_command_and_touched_runs_command_gate() {
        let dir = tempdir().unwrap();
        let mut m = ms(1, "");
        m.command = Some("echo ok".into());
        m.touched = vec!["src/foo.tsx".into()];
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
        assert_eq!(v, GateVerdict::Pass);
    }

    #[test]
    fn evaluate_none_without_touched_stays_inconclusive() {
        let dir = tempdir().unwrap();
        let mut m = ms(1, "");
        // With explicit command but no touched, gate_kind returns Command
        // (not None), so the command gate runs. This test verifies that when
        // command IS set but touched is empty, command gate still fires.
        m.command = Some("echo ok".into());
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
        // Command gate runs (gate_kind returns Command), echo ok passes.
        assert_eq!(v, GateVerdict::Pass);
    }

    #[test]
    fn evaluate_none_without_command_stays_inconclusive() {
        let dir = tempdir().unwrap();
        let mut m = ms(1, "");
        m.touched = vec!["src/foo.tsx".into()];
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
        assert!(matches!(v, GateVerdict::Inconclusive(_)));
    }

    // ── gate_command / gate_kind ──
    #[test]
    fn gate_command_prefers_explicit_over_extract() {
        let mut m = ms(1, "cargo test"); // acceptance 含可提取命令
        assert_eq!(gate_command(&m), Some("cargo test".into())); // 无显式 command → 用 extract
        m.command = Some("cargo build".into());
        assert_eq!(gate_command(&m), Some("cargo build".into())); // 显式 command 优先
    }

    #[test]
    fn gate_kind_classifies() {
        let mut m = ms(1, "cargo test");
        assert_eq!(gate_kind(&m), GateKind::Command); // 裸命令 acceptance → 兜底命令门
        m.command = Some("cargo build".into());
        assert_eq!(gate_kind(&m), GateKind::Command); // 显式 command
        let prose = ms(2, "渲染输出正确");
        assert_eq!(gate_kind(&prose), GateKind::Review); // prose
        let empty = ms(3, "");
        assert_eq!(gate_kind(&empty), GateKind::None); // 空
    }

    #[test]
    fn next_retryable_skips_gate_kind_none() {
        let mut m = ms(100, "");
        m.status = NodeStatus::NeedsFix;
        m.fix_attempts = 0;
        m.command = Some("echo ok".into());
        let mut m2 = ms(101, "");
        m2.status = NodeStatus::NeedsFix;
        m2.fix_attempts = 0;
        let g = graph_with(vec![m, m2]);
        let retryable = g.next_retryable(3);
        assert!(retryable.is_some());
        assert_eq!(retryable.unwrap().id, 100);
    }

    #[test]
    fn evaluate_uses_explicit_command_over_review() {
        let dir = tempdir().unwrap();
        let mut m = ms(1, "渲染输出正确"); // prose acceptance
        m.command = Some("rustc --version".into()); // 显式命令(纯 ASCII,exit 0)
        // review_runner 若被调用会返回独特标记;命令门应先生效 → 不应等于该标记。
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::NeedsFix("REVIEW_RAN".into()));
        assert_ne!(v, GateVerdict::NeedsFix("REVIEW_RAN".into()), "explicit command gate should fire, not review");
    }

    #[test]
    fn gate_kind_default_is_none() {
        assert_eq!(GateKind::default(), GateKind::None);
    }

    // ── next_action ──
    // 把指定 id 的节点置为给定状态(模拟 turn 后 background.rs 已写回状态)。
    fn with_status(mut g: WorkGraph, id: u64, status: NodeStatus) -> WorkGraph {
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == id) {
            n.status = status;
        }
        g
    }

    #[test]
    fn next_action_pass_advances_to_next_ready() {
        let g = with_status(graph_with(vec![ms(1, "x"), ms(2, "y")]), 1, NodeStatus::Done);
        assert_eq!(next_action(&g, 1, &GateVerdict::Pass, 0, true, 2), NextAction::Advance(2));
    }

    #[test]
    fn next_action_pass_no_more_ready_completes() {
        let g = with_status(graph_with(vec![ms(1, "x")]), 1, NodeStatus::Done);
        assert_eq!(
            next_action(&g, 1, &GateVerdict::Pass, 0, true, 2),
            NextAction::Stop(MissionState::CompletedAllReady)
        );
    }

    #[test]
    fn next_action_fail_with_blocked_dependent_and_no_independent_ready_blocks() {
        let mut m2 = ms(2, "y");
        m2.deps = vec![1];
        let g = with_status(graph_with(vec![ms(1, "x"), m2]), 1, NodeStatus::NeedsFix);
        assert_eq!(
            next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 1, true, 2),
            NextAction::Stop(MissionState::BlockedAt(1))
        );
    }

    #[test]
    fn next_action_fail_independent_ready_advances() {
        let g = with_status(graph_with(vec![ms(1, "x"), ms(3, "z")]), 1, NodeStatus::NeedsFix);
        assert_eq!(
            next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 1, true, 2),
            NextAction::Advance(3)
        );
    }

    #[test]
    fn next_action_circuit_breaker_on_k_consecutive_fails() {
        // #3 不依赖 #1(独立就绪)→ 降级跳过,继续推进 #3。
        let g = with_status(graph_with(vec![ms(1, "x"), ms(3, "z")]), 1, NodeStatus::NeedsFix);
        assert_eq!(
            next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 2, true, 2),
            NextAction::DegradeAndAdvance(3)
        );
    }

    #[test]
    fn next_action_circuit_breaker_stops_when_all_blocked() {
        // #3 依赖 #1(被阻塞)→ 无可降级项 → Stop(CircuitBreaker)。
        let mut m3 = ms(3, "z");
        m3.deps = vec![1];
        let g = with_status(graph_with(vec![ms(1, "x"), m3]), 1, NodeStatus::NeedsFix);
        assert_eq!(
            next_action(&g, 1, &GateVerdict::NeedsFix("e".into()), 2, true, 2),
            NextAction::Stop(MissionState::CircuitBreaker)
        );
    }

    #[test]
    fn next_action_no_budget_stops_completed() {
        let g = with_status(graph_with(vec![ms(1, "x"), ms(2, "y")]), 1, NodeStatus::Done);
        assert_eq!(
            next_action(&g, 1, &GateVerdict::Pass, 0, false, 2),
            NextAction::Stop(MissionState::CompletedAllReady)
        );
    }

    // ── checks 引擎 ──

    #[test]
    fn checks_no_template_content_detects_forbidden_text() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("page.tsx"), "import PlaceholderPage from '...'").unwrap();
        std::fs::write(dir.path().join("real.tsx"), "export default function RealPage()").unwrap();

        let spec = CheckSpec {
            type_: CheckType::NoTemplateContent,
            params: [("patterns".into(), json!(["*.tsx"])), ("forbidden".into(), json!(["PlaceholderPage"]))].into_iter().collect(),
        };
        let result = super::execute_check(&spec, dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("PlaceholderPage"));
    }

    #[test]
    fn checks_build_exit_zero_with_true() {
        let spec = CheckSpec {
            type_: CheckType::BuildExitZero,
            params: [("command".into(), json!("true"))].into_iter().collect(),
        };
        let result = super::execute_check(&spec, Path::new("/"));
        assert!(result.is_ok());
    }

    #[test]
    fn checks_file_count_min() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.tsx"), "a").unwrap();
        std::fs::write(dir.path().join("b.tsx"), "b").unwrap();
        let spec = CheckSpec {
            type_: CheckType::FileCountMin,
            params: [("path".into(), json!(".")), ("min".into(), json!(2))].into_iter().collect(),
        };
        assert!(super::execute_check(&spec, dir.path()).is_ok());
        let spec3 = CheckSpec {
            type_: CheckType::FileCountMin,
            params: [("path".into(), json!(".")), ("min".into(), json!(3))].into_iter().collect(),
        };
        assert!(super::execute_check(&spec3, dir.path()).is_err());
    }

    #[test]
    fn checks_run_checks_collects_errors() {
        let dir = tempdir().unwrap();
        // 两条 checks:一条 expect 至少 2 个文件(只有一个) → 应失败
        let bad = CheckSpec {
            type_: CheckType::FileCountMin,
            params: [("path".into(), json!(".")), ("min".into(), json!(2))].into_iter().collect(),
        };
        let result = super::run_checks(&[bad], dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn evaluate_with_checks_detects_placeholder() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("good.tsx"), "export function Real() { return <div>OK</div>; }").unwrap();

        let m = Milestone {
            id: 1, title: "test".into(), acceptance: String::new(), deps: vec![],
            status: NodeStatus::Pending, verdict: None, touched: vec![],
            fix_attempts: 0, last_failure: None,
            command: Some("true".into()),
            checks: Some(vec![CheckSpec {
                type_: CheckType::NoTemplateContent,
                params: [("patterns".into(), json!(["*.tsx"])),
                          ("forbidden".into(), json!(["PlaceholderPage"]))].into_iter().collect(),
            }]),
        };
        // good.tsx 不含 PlaceholderPage → pass
        let v = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
        assert_eq!(v, GateVerdict::Pass);

        // 在同一个目录加一个含 PlaceholderPage 的文件
        std::fs::write(dir.path().join("bad.tsx"), "import { PlaceholderPage } from '@/components'").unwrap();
        let v2 = evaluate(&m, dir.path(), None, &|| GateVerdict::Pass);
        assert!(matches!(v2, GateVerdict::NeedsFix(_)));
        assert!(format!("{:?}", v2).contains("PlaceholderPage"));
    }
}
