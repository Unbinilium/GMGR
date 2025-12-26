use crate::config::{AppConfig, EdgeDetect, GpioCapability, PinConfig};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;

#[cfg(feature = "hardware-gpio")]
pub use crate::backend::LibgpiodBackend;
#[cfg(not(feature = "hardware-gpio"))]
pub use crate::backend::MockGpioBackend;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GpioState {
    Error,
    Disabled,
    PushPull,
    OpenDrain,
    OpenSource,
    Floating,
    PullUp,
    PullDown,
}

impl GpioState {
    pub fn is_writable(&self) -> bool {
        matches!(
            self,
            GpioState::PushPull | GpioState::OpenDrain | GpioState::OpenSource
        )
    }

    pub fn is_edge_detectable(&self) -> bool {
        matches!(
            self,
            GpioState::Floating | GpioState::PullUp | GpioState::PullDown
        )
    }
}

pub type EdgeCallback = Arc<dyn Fn(EdgeEvent) + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
pub struct EdgeEvent {
    pub pin_id: String,
    pub edge: EdgeDetect,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinSettings {
    pub state: GpioState,
    pub edge: EdgeDetect,
    pub debounce_ms: u64,
}

impl Default for PinSettings {
    fn default() -> Self {
        Self {
            state: GpioState::Disabled,
            edge: EdgeDetect::None,
            debounce_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinDescriptor {
    pub info: PinConfig,
    pub settings: PinSettings,
}

pub trait GpioBackend: Send + Sync {
    fn get_settings(&self, pin_id: &str) -> Result<PinSettings, AppError>;

    fn set_settings(
        &self,
        pin_id: &str,
        pin: &PinConfig,
        settings: &PinSettings,
        event_callback: Option<EdgeCallback>,
    ) -> Result<(), AppError>;

    fn read_value(&self, pin_id: &str) -> Result<u8, AppError>;

    fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError>;
}

pub struct GpioManager {
    backend: Arc<dyn GpioBackend>,
    config: Arc<AppConfig>,
    event_tx: broadcast::Sender<EdgeEvent>,
    event_history: Arc<RwLock<HashMap<String, VecDeque<EdgeEvent>>>>,
    event_history_capacity: usize,
}

impl GpioManager {
    pub fn new(config: Arc<AppConfig>, backend: Arc<dyn GpioBackend>) -> Self {
        let (event_tx, _) = broadcast::channel(128);
        let mut history = HashMap::new();
        for id in config.gpios.keys() {
            history.insert(id.clone(), VecDeque::new());
        }
        let event_history_capacity = config.event_history_capacity;
        Self {
            backend,
            config,
            event_tx,
            event_history: Arc::new(RwLock::new(history)),
            event_history_capacity,
        }
    }

    fn pin_config(&self, pin_id: &str) -> Result<&PinConfig, AppError> {
        self.config
            .gpios
            .get(pin_id)
            .ok_or_else(|| AppError::NotFoundPin(pin_id.to_string()))
    }

    fn capability_matches(state: GpioState, caps: &[GpioCapability]) -> bool {
        match state {
            GpioState::Error => false,
            GpioState::Disabled => true,
            GpioState::PushPull => caps.contains(&GpioCapability::PushPull),
            GpioState::OpenDrain => caps.contains(&GpioCapability::OpenDrain),
            GpioState::OpenSource => caps.contains(&GpioCapability::OpenSource),
            GpioState::Floating => caps.contains(&GpioCapability::Floating),
            GpioState::PullUp => caps.contains(&GpioCapability::PullUp),
            GpioState::PullDown => caps.contains(&GpioCapability::PullDown),
        }
    }

    fn edge_callback(&self) -> EdgeCallback {
        let tx = self.event_tx.clone();
        let history = self.event_history.clone();
        let capacity = self.event_history_capacity;
        Arc::new(move |event: EdgeEvent| {
            if let Ok(mut map) = history.write() {
                let deque: &mut VecDeque<EdgeEvent> = map.entry(event.pin_id.clone()).or_default();
                while deque.len() >= capacity {
                    deque.pop_front();
                }
                deque.push_back(event.clone());
            }
            let _ = tx.send(event);
        })
    }

    pub async fn list_pins(&self) -> HashMap<String, PinDescriptor> {
        self.config
            .gpios
            .iter()
            .map(|(id, cfg)| {
                let settings = self.backend.get_settings(id).unwrap_or_default();
                (
                    id.clone(),
                    PinDescriptor {
                        info: cfg.clone(),
                        settings,
                    },
                )
            })
            .collect()
    }

    pub async fn get_pin_descriptor(&self, pin_id: &str) -> Result<PinDescriptor, AppError> {
        let cfg = self.pin_config(pin_id)?.clone();
        let settings = self.backend.get_settings(pin_id).unwrap_or_default();
        Ok(PinDescriptor {
            info: cfg,
            settings,
        })
    }

    pub async fn get_pin_info(&self, pin_id: &str) -> Result<PinConfig, AppError> {
        self.pin_config(pin_id).cloned()
    }

    pub async fn get_pin_settings(&self, pin_id: &str) -> Result<PinSettings, AppError> {
        self.pin_config(pin_id)?;
        self.backend.get_settings(pin_id)
    }

    pub async fn set_pin_settings(
        &self,
        pin_id: &str,
        settings: &PinSettings,
    ) -> Result<(), AppError> {
        let cfg = self.pin_config(pin_id)?;

        if !Self::capability_matches(settings.state, &cfg.capabilities) {
            return Err(AppError::InvalidState(format!(
                "state not supported by pin {pin_id}"
            )));
        }

        if settings.edge != EdgeDetect::None && !settings.state.is_edge_detectable() {
            return Err(AppError::InvalidState(
                "edge detection requires an input-capable state".into(),
            ));
        }

        let callback = if settings.edge != EdgeDetect::None {
            Some(self.edge_callback())
        } else {
            None
        };

        self.backend.set_settings(pin_id, cfg, &settings, callback)
    }

    pub async fn read_value(&self, pin_id: &str) -> Result<u8, AppError> {
        let value = self.backend.read_value(pin_id)?;
        Ok(value)
    }

    pub async fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError> {
        if value > 1 {
            return Err(AppError::InvalidValue("value must be 0 or 1".into()));
        }
        self.pin_config(pin_id)?;
        self.backend.write_value(pin_id, value)?;
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<EdgeEvent> {
        self.event_tx.subscribe()
    }

    pub async fn get_events(&self, pin_id: &str) -> Result<Vec<EdgeEvent>, AppError> {
        self.pin_config(pin_id)?;
        let map = self
            .event_history
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        Ok(map
            .get(pin_id)
            .map(|d| d.iter().cloned().collect())
            .unwrap_or_else(Vec::new))
    }

    pub async fn get_last_event(&self, pin_id: &str) -> Result<Option<EdgeEvent>, AppError> {
        self.pin_config(pin_id)?;
        let map = self
            .event_history
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        Ok(map.get(pin_id).and_then(|d| d.back().cloned()))
    }
}
