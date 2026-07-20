// Daemon (ADR 待补): 长驻后台进程，管理 N 个 AgentLoop，对外暴露 Unix socket。
// 本文件随 Task 2 起逐步填充真实逻辑；当前仅提供可空跑的骨架。
use crate::config::Config;

/// 长驻 daemon。`run()` 当前为 stub，Task 2 起接入 socket + session 管理。
pub struct Daemon {
    #[allow(dead_code)]
    cfg: Config,
}

impl Daemon {
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }

    /// Task 1: stub。Task 2 起监听 Unix socket、accept 连接、分发请求。
    pub fn run(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg_with_temp_root() -> Config {
        let dir = std::env::temp_dir().join(format!("cc_daemon_stub_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Config {
            api_key: None,
            model: "gpt-4o".into(),
            api_base: "https://api.openai.com/v1".into(),
            max_tokens: 4096,
            temperature: 0.7,
            root: dir,
            github_token: None,
        }
    }

    #[test]
    fn daemon_stub_runs_and_returns_ok() {
        let d = Daemon::new(cfg_with_temp_root());
        let res = d.run();
        assert!(res.is_ok());
        // 清理：Daemon 还不持有 root 之外的状态；删掉临时根。
        let _ = std::fs::remove_dir_all(&d.cfg.root);
    }
}
