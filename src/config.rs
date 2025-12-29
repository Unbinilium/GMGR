use std::{collections::HashSet, fs, path::Path};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HttpConfig {
    pub unix_socket: Option<String>,
    pub host: Option<String>,
    pub path: String,
    pub timeout: u64,
}

#[derive(Debug, Hash, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GpioCapability {
    Error,
    Disabled,
    PushPull,
    OpenDrain,
    OpenSource,
    Floating,
    PullUp,
    PullDown,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum EdgeDetect {
    #[default]
    None,
    Rising,
    Falling,
    Both,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PinConfig {
    pub name: String,
    pub chip: String,
    pub line: u32,
    pub capabilities: HashSet<GpioCapability>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
    pub http: HttpConfig,
    pub gpios: FxHashMap<u32, PinConfig>,
    pub broadcast_capacity: usize,
    pub event_history_capacity: usize,
}

impl AppConfig {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, AppError> {
        let contents = fs::read_to_string(&path)
            .map_err(|e| AppError::Config(format!("failed to read config: {e}")))?;
        serde_json::from_str(&contents)
            .map_err(|e| AppError::Config(format!("invalid config json: {e}")))
    }
}
