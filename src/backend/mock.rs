use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::config::{EdgeDetect, PinConfig};
use crate::error::AppError;
use crate::gpio::{EdgeEvent, EventHandler, GpioBackend, GpioState, PinSettings};

#[derive(Default)]
pub struct MockGpioBackend {
    pins: RwLock<HashMap<String, Mutex<MockPinState>>>, // keyed by pin id
}

#[derive(Clone)]
struct MockPinState {
    settings: PinSettings,
    value: u8,
    handler: Option<EventHandler>,
    last_event: Option<Instant>,
}

impl GpioBackend for MockGpioBackend {
    fn get_settings(&self, pin_id: &str) -> Result<PinSettings, AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if let Some(pin_lock) = pins.get(pin_id) {
            let pin = pin_lock
                .lock()
                .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
            Ok(pin.settings.clone())
        } else {
            Ok(PinSettings::default())
        }
    }

    fn set_settings(
        &self,
        pin_id: &str,
        _pin: &PinConfig,
        settings: &PinSettings,
        event_handler: Option<EventHandler>,
    ) -> Result<(), AppError> {
        let mut pins = self
            .pins
            .write()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        let entry = pins.entry(pin_id.to_string()).or_insert_with(|| {
            Mutex::new(MockPinState {
                settings: PinSettings::default(),
                value: 0,
                handler: None,
                last_event: None,
            })
        });

        let mut pin = entry
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        pin.settings = settings.clone();
        if settings.state == GpioState::Disabled {
            pin.value = 0;
            pin.handler = None;
        } else if settings.edge != EdgeDetect::None {
            pin.handler = event_handler;
            pin.last_event = None;
        } else {
            pin.handler = None;
        }

        Ok(())
    }

    fn read_value(&self, pin_id: &str) -> Result<u8, AppError> {
        let mut pins = self
            .pins
            .write()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let entry = pins
            .get_mut(pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let pin = entry
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if pin.settings.state == GpioState::Disabled {
            return Err(AppError::InvalidState(
                "pin is disabled and cannot be read".to_string(),
            ));
        }
        Ok(pin.value)
    }

    fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError> {
        let mut pins = self
            .pins
            .write()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let entry = pins
            .get_mut(pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let mut pin = entry
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if !pin.settings.state.is_writable() {
            return Err(AppError::InvalidState(
                "pin must be in output mode to set value".into(),
            ));
        }

        let old = pin.value;
        pin.value = value;

        if let Some(edge_kind) = match (old, value) {
            (0, 1) => Some(EdgeDetect::Rising),
            (1, 0) => Some(EdgeDetect::Falling),
            _ => None,
        } {
            if edge_matches(pin.settings.edge, edge_kind) {
                let now = Instant::now();
                let debounce = pin.settings.debounce_ms;
                let allow = pin
                    .last_event
                    .map(|t| now.duration_since(t).as_millis() >= debounce as u128)
                    .unwrap_or(true);
                if allow {
                    pin.last_event = Some(now);
                    if let Some(h) = &pin.handler {
                        h.dispatch(EdgeEvent {
                            pin_id: pin_id.to_string(),
                            edge: edge_kind,
                            timestamp_ms: epoch_millis(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

fn edge_matches(configured: EdgeDetect, observed: EdgeDetect) -> bool {
    match configured {
        EdgeDetect::None => false,
        EdgeDetect::Rising => observed == EdgeDetect::Rising,
        EdgeDetect::Falling => observed == EdgeDetect::Falling,
        EdgeDetect::Both => matches!(observed, EdgeDetect::Rising | EdgeDetect::Falling),
    }
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
