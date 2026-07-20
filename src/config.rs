// Runtime configuration from CODECODER_* env vars (see README.md env table).
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: Option<String>,
    pub model: String,
    pub api_base: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub root: PathBuf,
    pub github_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        Config {
            api_key: env("CODECODER_API_KEY"),
            model: env("CODECODER_MODEL").unwrap_or_else(|| "gpt-4o".into()),
            api_base: env("CODECODER_API_BASE")
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            max_tokens: env("CODECODER_MAX_TOKENS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4096),
            temperature: env("CODECODER_TEMPERATURE")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.7),
            root: env("CODECODER_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            github_token: env("GITHUB_TOKEN"),
        }
    }
}
