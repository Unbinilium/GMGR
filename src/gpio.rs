use crate::config::{AppConfig, GpioCapability, PinConfig};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

#[cfg(feature = "hardware-gpio")]
use libgpiod::{chip::Chip, line, request};
#[cfg(feature = "hardware-gpio")]
use std::collections::hash_map::Entry;
#[cfg(feature = "hardware-gpio")]
use std::path::PathBuf;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinDescriptor {
    pub id: String,
    pub name: String,
    pub chip: String,
    pub line: u32,
    pub capabilities: Vec<GpioCapability>,
    pub state: GpioState,
}

pub trait GpioBackend: Send + Sync {
    fn get_state(&self, pin_id: &str) -> Result<GpioState, AppError>;
    fn set_state(&self, pin_id: &str, pin: &PinConfig, state: GpioState) -> Result<(), AppError>;
    fn read_value(&self, pin_id: &str) -> Result<u8, AppError>;
    fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError>;
}

#[derive(Default)]
pub struct MockGpioBackend {
    pins: RwLock<HashMap<String, Mutex<MockPinState>>>, // keyed by pin id
}

#[derive(Debug, Clone)]
struct MockPinState {
    state: GpioState,
    value: u8,
}

impl GpioBackend for MockGpioBackend {
    fn get_state(&self, pin_id: &str) -> Result<GpioState, AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if let Some(pin_lock) = pins.get(pin_id) {
            let pin = pin_lock
                .lock()
                .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
            Ok(pin.state)
        } else {
            Ok(GpioState::Disabled)
        }
    }

    fn set_state(&self, pin_id: &str, _pin: &PinConfig, state: GpioState) -> Result<(), AppError> {
        let mut pins = self
            .pins
            .write()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        let entry = pins.entry(pin_id.to_string()).or_insert_with(|| {
            Mutex::new(MockPinState {
                state: GpioState::Disabled,
                value: 0,
            })
        });

        let mut pin = entry
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        pin.state = state;
        if state == GpioState::Disabled {
            pin.value = 0;
        }
        Ok(())
    }

    fn read_value(&self, pin_id: &str) -> Result<u8, AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let entry = pins
            .get(pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let pin = entry
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if pin.state == GpioState::Disabled {
            return Err(AppError::InvalidState(
                "pin is disabled and cannot be read".to_string(),
            ));
        }
        Ok(pin.value)
    }

    fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let entry = pins
            .get(pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let mut pin = entry
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if !pin.state.is_writable() {
            return Err(AppError::InvalidState(
                "pin must be in output mode to set value".into(),
            ));
        }

        pin.value = value;
        Ok(())
    }
}

#[cfg(feature = "hardware-gpio")]
pub struct LibgpiodBackend {
    pins: RwLock<HashMap<String, Mutex<PinHandle>>>, // keyed by pin id
}

#[cfg(feature = "hardware-gpio")]
struct PinHandle {
    line: u32,
    state: GpioState,
    _chip: Chip,
    request: request::Request,
}

#[cfg(feature = "hardware-gpio")]
impl LibgpiodBackend {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            pins: RwLock::new(HashMap::new()),
        })
    }

    fn settings_for_state(state: GpioState) -> Result<line::Settings, AppError> {
        let mut settings =
            line::Settings::new().map_err(|e| AppError::Gpio(format!("libgpiod settings: {e}")))?;

        match state {
            GpioState::Error | GpioState::Disabled => {
                return Err(AppError::InvalidState(
                    "cannot create settings for error or disabled state".into(),
                ));
            }
            GpioState::PushPull => {
                settings
                    .set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                settings
                    .set_drive(line::Drive::PushPull)
                    .map_err(|e| AppError::Gpio(format!("libgpiod drive: {e}")))?;
            }
            GpioState::OpenDrain => {
                settings
                    .set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                settings
                    .set_drive(line::Drive::OpenDrain)
                    .map_err(|e| AppError::Gpio(format!("libgpiod drive: {e}")))?;
            }
            GpioState::OpenSource => {
                settings
                    .set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                settings
                    .set_drive(line::Drive::OpenSource)
                    .map_err(|e| AppError::Gpio(format!("libgpiod drive: {e}")))?;
            }
            GpioState::Floating => {
                settings
                    .set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                settings
                    .set_bias(None)
                    .map_err(|e| AppError::Gpio(format!("libgpiod bias: {e}")))?;
            }
            GpioState::PullUp => {
                settings
                    .set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                settings
                    .set_bias(Some(line::Bias::PullUp))
                    .map_err(|e| AppError::Gpio(format!("libgpiod bias: {e}")))?;
            }
            GpioState::PullDown => {
                settings
                    .set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                settings
                    .set_bias(Some(line::Bias::PullDown))
                    .map_err(|e| AppError::Gpio(format!("libgpiod bias: {e}")))?;
            }
        }

        Ok(settings)
    }

    fn open_chip(path: &str) -> Result<Chip, AppError> {
        let p = PathBuf::from(path);
        Chip::open(&p).map_err(|e| AppError::Gpio(format!("open chip {path}: {e}")))
    }

    fn make_line_config(offset: u32, settings: line::Settings) -> Result<line::Config, AppError> {
        let mut cfg =
            line::Config::new().map_err(|e| AppError::Gpio(format!("line config: {e}")))?;
        cfg.add_line_settings(&[offset], settings)
            .map_err(|e| AppError::Gpio(format!("line config add settings: {e}")))?;
        Ok(cfg)
    }

    fn request_lines(chip: &Chip, line_cfg: &line::Config) -> Result<request::Request, AppError> {
        let mut req_cfg =
            request::Config::new().map_err(|e| AppError::Gpio(format!("request config: {e}")))?;
        req_cfg
            .set_consumer("gmgr")
            .map_err(|e| AppError::Gpio(format!("request consumer: {e}")))?;
        chip.request_lines(Some(&req_cfg), line_cfg)
            .map_err(|e| AppError::Gpio(format!("request lines: {e}")))
    }
}

#[cfg(feature = "hardware-gpio")]
impl GpioBackend for LibgpiodBackend {
    fn get_state(&self, pin_id: &str) -> Result<GpioState, AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if let Some(handle_lock) = pins.get(pin_id) {
            let handle = handle_lock
                .lock()
                .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
            Ok(handle.state)
        } else {
            Ok(GpioState::Disabled)
        }
    }

    fn set_state(&self, pin_id: &str, pin: &PinConfig, state: GpioState) -> Result<(), AppError> {
        let mut pins = self
            .pins
            .write()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if state == GpioState::Disabled {
            pins.remove(pin_id);
            return Ok(());
        }

        match pins.entry(pin_id.to_string()) {
            Entry::Occupied(mut entry) => {
                let handle_lock = entry.get_mut();
                let mut handle = handle_lock
                    .lock()
                    .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

                if handle.state == state {
                    return Ok(());
                }

                let settings = Self::settings_for_state(state)?;
                let line_cfg = Self::make_line_config(handle.line, settings)?;
                handle
                    .request
                    .reconfigure_lines(&line_cfg)
                    .map_err(|e| AppError::Gpio(format!("reconfigure lines: {e}")))?;
                handle.state = state;
            }
            Entry::Vacant(entry) => {
                let settings = Self::settings_for_state(state)?;
                let line_cfg = Self::make_line_config(pin.line, settings)?;
                let chip = Self::open_chip(&pin.chip)?;
                let request = Self::request_lines(&chip, &line_cfg)?;
                entry.insert(Mutex::new(PinHandle {
                    line: pin.line,
                    state,
                    _chip: chip,
                    request,
                }));
            }
        }
        Ok(())
    }

    fn read_value(&self, pin_id: &str) -> Result<u8, AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let handle_lock = pins
            .get(pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let handle = handle_lock
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let value = handle
            .request
            .value(handle.line)
            .map_err(|e| AppError::Gpio(format!("get value: {e}")))?;

        Ok(match value {
            line::Value::InActive => 0,
            line::Value::Active => 1,
        })
    }

    fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
        let handle_lock = pins
            .get(pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let mut handle = handle_lock
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if !handle.state.is_writable() {
            return Err(AppError::InvalidState(
                "pin must be in output mode to set value".to_string(),
            ));
        }

        let offset = handle.line;

        handle
            .request
            .set_value(
                offset,
                match value {
                    0 => line::Value::InActive,
                    1 => line::Value::Active,
                    _ => line::Value::InActive,
                },
            )
            .map_err(|e| AppError::Gpio(format!("set value: {e}")))?;
        Ok(())
    }
}

pub struct GpioManager {
    backend: Arc<dyn GpioBackend>,
    config: Arc<AppConfig>,
}

impl GpioManager {
    pub fn new(config: Arc<AppConfig>, backend: Arc<dyn GpioBackend>) -> Self {
        Self { backend, config }
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

    pub async fn list_pins(&self) -> Vec<PinDescriptor> {
        self.config
            .gpios
            .iter()
            .map(|(id, cfg)| PinDescriptor {
                id: id.clone(),
                name: cfg.name.clone(),
                chip: cfg.chip.clone(),
                line: cfg.line,
                capabilities: cfg.capabilities.clone(),
                state: self.backend.get_state(id).unwrap_or(GpioState::Error),
            })
            .collect()
    }

    pub async fn get_pin_info(&self, pin_id: &str) -> Result<PinDescriptor, AppError> {
        let cfg = self.pin_config(pin_id)?.clone();
        Ok(PinDescriptor {
            id: pin_id.to_string(),
            name: cfg.name,
            chip: cfg.chip,
            line: cfg.line,
            capabilities: cfg.capabilities,
            state: self.backend.get_state(pin_id).unwrap_or(GpioState::Error),
        })
    }

    pub async fn get_state(&self, pin_id: &str) -> Result<GpioState, AppError> {
        self.backend
            .get_state(pin_id)
            .map_err(|_| AppError::NotFoundPin(pin_id.to_string()))
    }

    pub async fn set_state(&self, pin_id: &str, state: GpioState) -> Result<(), AppError> {
        let cfg = self.pin_config(pin_id)?;
        if !Self::capability_matches(state, &cfg.capabilities) {
            return Err(AppError::InvalidState(format!(
                "state {state:?} not supported by pin {pin_id}"
            )));
        }
        self.backend.set_state(pin_id, cfg, state)?;
        Ok(())
    }

    pub async fn read_value(&self, pin_id: &str) -> Result<u8, AppError> {
        let value = self.backend.read_value(pin_id)?;
        Ok(value)
    }

    pub async fn write_value(&self, pin_id: &str, value: u8) -> Result<(), AppError> {
        if value > 1 {
            return Err(AppError::InvalidValue("value must be 0 or 1".to_string()));
        }
        self.backend.write_value(pin_id, value)?;
        Ok(())
    }
}
