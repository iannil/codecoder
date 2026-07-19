// src/verify/explore.rs
// 自驱动探索状态 (L4 阶段 2)

/// 自愈记录
#[derive(Debug, Clone)]
pub struct HealRecord {
    pub target: String,
    pub diagnosis: String,
    pub applied: bool,
    pub diff: String,
}

/// 探索模式状态
#[derive(Debug, Clone)]
pub struct ExploreState {
    pub checked_skills: Vec<String>,
    pub checked_capabilities: Vec<String>,
    pub healed: Vec<HealRecord>,
    pub failed: Vec<String>,
    pub running: bool,
    pub current_target: Option<String>,
}

impl ExploreState {
    pub fn new() -> Self {
        Self {
            checked_skills: Vec::new(),
            checked_capabilities: Vec::new(),
            healed: Vec::new(),
            failed: Vec::new(),
            running: false,
            current_target: None,
        }
    }

    /// 已检查的总数
    pub fn checked_count(&self) -> usize {
        self.checked_skills.len() + self.checked_capabilities.len()
    }

    /// 失败总数
    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    /// 自愈成功数
    pub fn healed_count(&self) -> usize {
        self.healed.iter().filter(|h| h.applied).count()
    }
}