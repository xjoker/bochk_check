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
        Self {
            url: String::new(),
        }
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

fn parse_bool_env(name: &str, value: &str) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("环境变量 {} 的布尔值无效: {}", name, value).into()),
    }
}

fn parse_env_number<T>(name: &str, value: &str) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .trim()
        .parse::<T>()
        .map_err(|e| format!("环境变量 {} 的数值无效: {} ({})", name, value, e).into())
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
        config.bark.urls = value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToString::to_string)
            .collect();
    }

    if let Ok(value) = std::env::var("BOCHK_LOGGING_PERSIST_JSONL") {
        config.logging.persist_jsonl =
            parse_bool_env("BOCHK_LOGGING_PERSIST_JSONL", &value)?;
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

pub fn base_dir() -> PathBuf {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(ref dir) = exe {
        if dir.join("config.toml").exists() {
            return dir.clone();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn load_config() -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let config_path = base_dir().join("config.toml");
    let mut config = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        toml::from_str(&content)?
    } else {
        AppConfig::default()
    };
    apply_env_overrides(&mut config)?;
    Ok(config)
}
