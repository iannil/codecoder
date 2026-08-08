// CLI help/skill 输出（spec 2026-08-07 §2.2）：ccda/ccli/ccweb 共用。
// 默认纯文本 markdown，`--json` 输出结构化 JSON，便于 LLM agent 解析。
use serde_json::Value;
use std::path::Path;

/// 单个技能/模式/子命令的条目。
#[derive(Debug, Clone, Copy)]
pub struct SkillEntry {
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static [&'static str],
    /// 参数/输出结构说明（可含 JSON 片段）。
    pub schema: Option<&'static str>,
    /// 可直接复用的模板。
    pub template: Option<&'static str>,
}

/// 单个二进制的帮助规格。
pub struct HelpSpec {
    pub binary: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub usage: &'static [&'static str],
    pub config_note: &'static str,
    pub skills: &'static [SkillEntry],
}

/// 一次帮助请求的解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum HelpRequest {
    Help { json: bool },
    Skill { name: String, json: bool },
}

/// 扫描参数找 `--help`/`-h`、`--skill <name>`/`-s <name>`、`--json`。
/// 返回 None 表示无帮助请求（正常运行）。
pub fn parse_help_request(args: &[String]) -> Option<HelpRequest> {
    let mut json = false;
    let mut skill: Option<String> = None;
    let mut i = 0;
    let mut help = false;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--help" | "-h" => help = true,
            "--skill" | "-s" => {
                if i + 1 < args.len() {
                    skill = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(name) = skill {
        return Some(HelpRequest::Skill { name, json });
    }
    if help {
        return Some(HelpRequest::Help { json });
    }
    None
}

/// 渲染纯文本帮助（markdown）。
pub fn render_help(spec: &HelpSpec) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {} — {}\n\n{}\n\n", spec.title, spec.description, spec.description));
    s.push_str("## USAGE\n\n");
    for u in spec.usage {
        s.push_str(&format!("```\n{u}\n```\n"));
    }
    s.push_str("\n## CONFIGURATION\n\n");
    s.push_str(spec.config_note);
    s.push_str("\n\n## SKILLS\n\n");
    for sk in spec.skills {
        s.push_str(&format!("- **`{}`** — {}\n", sk.name, sk.description));
    }
    s.push_str("\n查看某技能详情：`");
    s.push_str(spec.binary);
    s.push_str(" --skill <name>`；结构化输出追加 `--json`。\n");
    s
}

fn find_skill<'a>(spec: &'a HelpSpec, name: &str) -> Option<&'a SkillEntry> {
    spec.skills.iter().find(|s| s.name == name)
}

/// 渲染单个技能详情。先查内置技能表；查不到则读仓库 `skills/<name>.md`。
pub fn render_skill(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<String> {
    if let Some(sk) = find_skill(spec, name) {
        return Some(render_skill_entry(name, sk));
    }
    let f = skills_dir.join(format!("{name}.md"));
    std::fs::read_to_string(&f).ok()
}

fn render_skill_entry(name: &str, sk: &SkillEntry) -> String {
    let mut s = format!("# Skill: {name}\n\n{}\n\n## Usage\n\n", sk.description);
    for u in sk.usage {
        s.push_str(&format!("```\n{u}\n```\n"));
    }
    if let Some(sch) = sk.schema {
        s.push_str(&format!("\n## Schema\n\n```json\n{sch}\n```\n"));
    }
    if let Some(t) = sk.template {
        s.push_str(&format!("\n## Template\n\n```\n{t}\n```\n"));
    }
    s
}

fn skill_to_value(name: &str, sk: &SkillEntry) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), Value::String(name.into()));
    m.insert("description".into(), Value::String(sk.description.into()));
    m.insert("usage".into(), Value::Array(sk.usage.iter().map(|u| Value::String((*u).into())).collect()));
    if let Some(sch) = sk.schema {
        m.insert("schema".into(), Value::String(sch.into()));
    }
    if let Some(t) = sk.template {
        m.insert("template".into(), Value::String(t.into()));
    }
    Value::Object(m)
}

/// 完整帮助的 JSON 结构。
pub fn help_json(spec: &HelpSpec) -> Value {
    let skills: Vec<Value> = spec.skills.iter().map(|sk| skill_to_value(sk.name, sk)).collect();
    serde_json::json!({
        "binary": spec.binary,
        "title": spec.title,
        "description": spec.description,
        "usage": spec.usage,
        "config": spec.config_note,
        "skills": skills,
    })
}

/// 单个技能的 JSON。查不到内置技能则读仓库 `skills/<name>.md` 原文。
pub fn skill_json(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<Value> {
    if let Some(sk) = find_skill(spec, name) {
        return Some(skill_to_value(name, sk));
    }
    let f = skills_dir.join(format!("{name}.md"));
    let content = std::fs::read_to_string(&f).ok()?;
    Some(serde_json::json!({ "name": name, "source": "skills/", "content": content }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HelpSpec {
        HelpSpec {
            binary: "ccda",
            title: "CodeCoder daemon",
            description: "Autonomous AI agent daemon",
            usage: &["ccda [FLAGS]", "ccda --skill <name>"],
            config_note: "Config: $HOME/.codecoder/codecoder.json then <root>/.codecoder/codecoder.json",
            skills: &[SkillEntry {
                name: "daemon",
                description: "Run the daemon",
                usage: &["ccda"],
                schema: None,
                template: Some("CODECODER_ROOT=/path ccda"),
            }],
        }
    }

    #[test]
    fn parse_help_request_recognizes_help_and_json() {
        assert!(matches!(parse_help_request(&["--help".into()]), Some(HelpRequest::Help{json:false})));
        assert!(matches!(parse_help_request(&["-h".into(), "--json".into()]), Some(HelpRequest::Help{json:true})));
        assert!(matches!(parse_help_request(&["--skill".into(), "daemon".into()]), Some(HelpRequest::Skill{name, json:false}) if name=="daemon"));
        assert!(matches!(parse_help_request(&["--skill".into(), "daemon".into(), "--json".into()]), Some(HelpRequest::Skill{json:true,..})));
        assert!(parse_help_request(&["--port".into(), "9876".into()]).is_none());
    }

    #[test]
    fn render_skill_missing_returns_none() {
        let s = spec();
        assert!(render_skill(&s, "missing", Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn render_help_contains_binary_and_skills() {
        let text = render_help(&spec());
        assert!(text.contains("ccda"));
        assert!(text.contains("daemon"));
        assert!(text.contains("--skill"));
    }

    #[test]
    fn json_output_is_structurally_valid() {
        let j = help_json(&spec());
        assert_eq!(j["binary"], "ccda");
        assert_eq!(j["skills"][0]["name"], "daemon");
        let sj = skill_json(&spec(), "daemon", Path::new("/nonexistent")).unwrap();
        assert_eq!(sj["template"], "CODECODER_ROOT=/path ccda");
    }
}