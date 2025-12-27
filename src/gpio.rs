use crate::config::{AppConfig, EdgeDetect, GpioCapability, PinConfig};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::broadcast;

pub type GpioManager<B> = GenericGpioManager<B>;

pub type GpioState = GpioCapability;

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

pub struct EventCallbackHandler {
    event_tx: broadcast::Sender<EdgeEvent>,
    event_history: RwLock<HashMap<u32, VecDeque<EdgeEvent>>>,
    event_history_capacity: usize,
}

impl EventCallbackHandler {
    pub fn new(
        event_tx: broadcast::Sender<EdgeEvent>,
        event_history: RwLock<HashMap<u32, VecDeque<EdgeEvent>>>,
        event_history_capacity: usize,
    ) -> Self {
        Self {
            event_tx,
            event_history,
            event_history_capacity,
        }
    }

    pub fn dispatch(&self, event: EdgeEvent) {
        {
            let mut map = self.event_history.write();
            let deque: &mut VecDeque<EdgeEvent> = map.entry(event.pin_id.clone()).or_default();
            while deque.len() >= self.event_history_capacity {
                deque.pop_front();
            }
            deque.push_back(event.clone());
        }
        let _ = self.event_tx.send(event);
    }
}

pub type EventHandler = Arc<EventCallbackHandler>;

#[derive(Debug, Clone, Serialize)]
pub struct EdgeEvent {
    pub pin_id: u32,
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
    fn get_settings(&self, pin_id: u32) -> Result<PinSettings, AppError>;

    fn set_settings(
        &self,
        pin_id: u32,
        pin: &PinConfig,
        settings: &PinSettings,
        event_callback: Option<EventHandler>,
    ) -> Result<(), AppError>;

    fn read_value(&self, pin_id: u32) -> Result<u8, AppError>;

    fn write_value(&self, pin_id: u32, value: u8) -> Result<(), AppError>;
}

pub struct GenericGpioManager<B: GpioBackend> {
    config: Arc<AppConfig>,
    backend: Arc<B>,
    event_handler: EventHandler,
}

impl<B: GpioBackend> GenericGpioManager<B> {
    pub fn new(config: Arc<AppConfig>, backend: Arc<B>) -> Self {
        let (event_tx, _) = broadcast::channel(config.broadcast_capacity);
        let mut history = HashMap::new();
        for id in config.gpios.keys() {
            history.insert(id.clone(), VecDeque::new());
        }
        let event_handler = Arc::new(EventCallbackHandler::new(
            event_tx,
            RwLock::new(history),
            config.event_history_capacity,
        ));
        Self {
            config,
            backend,
            event_handler,
        }
    }

    fn pin_config(&self, pin_id: u32) -> Result<&PinConfig, AppError> {
        self.config
            .gpios
            .get(&pin_id)
            .ok_or_else(|| AppError::NotFoundPin(pin_id.to_string()))
    }

    fn capability_matches(state: GpioState, caps: &HashSet<GpioState>) -> bool {
        match state {
            GpioState::Error => false,
            GpioState::Disabled => true,
            _ => match state {
                GpioState::Error => false,
                GpioState::Disabled => true,
                _ => caps.contains(&state),
            },
        }
    }

    pub async fn list_pins(&self) -> HashMap<u32, PinDescriptor> {
        self.config
            .gpios
            .iter()
            .map(|(id, cfg)| {
                let settings = self.backend.get_settings(*id).unwrap_or_default();
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

    pub async fn get_pin_descriptor(&self, pin_id: u32) -> Result<PinDescriptor, AppError> {
        let cfg = self.pin_config(pin_id)?.clone();
        let settings = self.backend.get_settings(pin_id).unwrap_or_default();
        Ok(PinDescriptor {
            info: cfg,
            settings,
        })
    }

    pub async fn get_pin_info(&self, pin_id: u32) -> Result<PinConfig, AppError> {
        self.pin_config(pin_id).cloned()
    }

    pub async fn get_pin_settings(&self, pin_id: u32) -> Result<PinSettings, AppError> {
        self.pin_config(pin_id)?;
        self.backend.get_settings(pin_id)
    }

    pub async fn set_pin_settings(
        &self,
        pin_id: u32,
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
            Some(self.event_handler.clone())
        } else {
            None
        };

        self.backend.set_settings(pin_id, cfg, &settings, callback)
    }

    pub async fn read_value(&self, pin_id: u32) -> Result<u8, AppError> {
        let value = self.backend.read_value(pin_id)?;
        Ok(value)
    }

    pub async fn write_value(&self, pin_id: u32, value: u8) -> Result<(), AppError> {
        if value > 1 {
            return Err(AppError::InvalidValue("value must be 0 or 1".into()));
        }
        self.pin_config(pin_id)?;
        self.backend.write_value(pin_id, value)?;
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<EdgeEvent> {
        self.event_handler.event_tx.subscribe()
    }

    pub async fn get_events(
        &self,
        pin_id: u32,
        limit: Option<usize>,
    ) -> Result<Vec<EdgeEvent>, AppError> {
        self.pin_config(pin_id)?;
        let map = self.event_handler.event_history.read();
        Ok(map
            .get(&pin_id)
            .map(|d| {
                let events: Vec<EdgeEvent> = if let Some(lim) = limit {
                    d.iter().rev().take(lim).cloned().collect()
                } else {
                    d.iter().cloned().collect()
                };
                events.into_iter().rev().collect()
            })
            .unwrap_or_default())
    }

    pub async fn get_last_event(&self, pin_id: u32) -> Result<Option<EdgeEvent>, AppError> {
        self.pin_config(pin_id)?;
        let map = self.event_handler.event_history.read();
        Ok(map.get(&pin_id).and_then(|d| d.back().cloned()))
    }
}
