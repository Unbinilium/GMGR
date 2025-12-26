use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HttpConfig {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub timeout: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GpioCapability {
    PushPull,
    OpenDrain,
    OpenSource,
    Floating,
    PullUp,
    PullDown,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeDetect {
    None,
    Rising,
    Falling,
    Both,
}

impl Default for EdgeDetect {
    fn default() -> Self {
        EdgeDetect::None
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PinConfig {
    pub name: String,
    pub chip: String,
    pub line: u32,
    pub capabilities: Vec<GpioCapability>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub http: HttpConfig,
    pub gpios: HashMap<String, PinConfig>,
}

impl AppConfig {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, AppError> {
        let contents = fs::read_to_string(&path)
            .map_err(|e| AppError::Config(format!("failed to read config: {e}")))?;
        serde_json::from_str(&contents)
            .map_err(|e| AppError::Config(format!("invalid config json: {e}")))
    }
}
