use serde::Deserialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static PERSIST_JSONL_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    pub proxy: ProxyConfig,
    pub monitor: MonitorConfig,
    pub bark: BarkConfig,
    pub logging: LoggingConfig,
    pub web: WebConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            proxy: ProxyConfig::default(),
            monitor: MonitorConfig::default(),
            bark: BarkConfig::default(),
            logging: LoggingConfig::default(),
            web: WebConfig::default(),
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct ProxyConfig {
    pub url: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self { url: String::new() }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct MonitorConfig {
    pub interval_secs: u64,
    pub max_fail_count: u32,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            interval_secs: 30,
            max_fail_count: 5,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct BarkConfig {
    pub urls: Vec<String>,
}

impl Default for BarkConfig {
    fn default() -> Self {
        Self { urls: Vec::new() }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct LoggingConfig {
    pub persist_jsonl: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            persist_jsonl: false,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct WebConfig {
    pub enabled: bool,
    pub port: u16,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 32141,
        }
    }
}

fn parse_bool_env(
    name: &str,
    value: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("invalid boolean for {}: {}", name, value).into()),
    }
}

fn parse_env_number<T>(
    name: &str,
    value: &str,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .trim()
        .parse::<T>()
        .map_err(|e| format!("invalid number for {}: {} ({})", name, value, e).into())
}

fn parse_bark_urls(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn apply_env_overrides(
    config: &mut AppConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(value) = std::env::var("BOCHK_PROXY_URL") {
        config.proxy.url = value;
    }

    if let Ok(value) = std::env::var("BOCHK_MONITOR_INTERVAL_SECS") {
        config.monitor.interval_secs = parse_env_number("BOCHK_MONITOR_INTERVAL_SECS", &value)?;
    }

    if let Ok(value) = std::env::var("BOCHK_MONITOR_MAX_FAIL_COUNT") {
        config.monitor.max_fail_count = parse_env_number("BOCHK_MONITOR_MAX_FAIL_COUNT", &value)?;
    }

    if let Ok(value) = std::env::var("BOCHK_BARK_URLS") {
        config.bark.urls = parse_bark_urls(&value);
    }

    if let Ok(value) = std::env::var("BOCHK_LOGGING_PERSIST_JSONL") {
        config.logging.persist_jsonl = parse_bool_env("BOCHK_LOGGING_PERSIST_JSONL", &value)?;
    }

    if let Ok(value) = std::env::var("BOCHK_WEB_ENABLED") {
        config.web.enabled = parse_bool_env("BOCHK_WEB_ENABLED", &value)?;
    }

    if let Ok(value) = std::env::var("BOCHK_WEB_PORT") {
        config.web.port = parse_env_number("BOCHK_WEB_PORT", &value)?;
    }

    Ok(())
}

pub fn set_persist_jsonl_enabled(enabled: bool) {
    PERSIST_JSONL_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn persist_jsonl_enabled() -> bool {
    PERSIST_JSONL_ENABLED.load(Ordering::Relaxed)
}

fn find_base_dir(start: &PathBuf) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.to_path_buf();
        if candidate.join("data").join("config").exists()
            || candidate.join("Cargo.toml").exists()
            || candidate.join("AGENTS.md").exists()
        {
            return Some(candidate);
        }
    }
    None
}

pub fn base_dir() -> PathBuf {
    if let Ok(dir) = std::env::current_dir() {
        if let Some(found) = find_base_dir(&dir) {
            return found;
        }
    }

    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        if let Some(found) = find_base_dir(&dir) {
            return found;
        }
    }

    PathBuf::from(".")
}

pub fn config_path() -> PathBuf {
    base_dir().join("data").join("config").join("app.toml")
}

pub fn data_file_dir() -> PathBuf {
    let dir = base_dir().join("data").join("file");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn log_dir() -> PathBuf {
    let dir = base_dir().join("data").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn load_config_file_only() -> Result<Option<AppConfig>, Box<dyn std::error::Error + Send + Sync>>
{
    let path = config_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let config = toml::from_str(&content)?;
    Ok(Some(config))
}

pub fn env_bark_urls_override() -> Option<Vec<String>> {
    std::env::var("BOCHK_BARK_URLS")
        .ok()
        .map(|value| parse_bark_urls(&value))
}

pub fn load_config() -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let config_path = config_path();
    let mut config = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        toml::from_str(&content)?
    } else {
        AppConfig::default()
    };
    apply_env_overrides(&mut config)?;
    Ok(config)
}
