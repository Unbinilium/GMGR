use std::collections::{HashMap, hash_map::Entry};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::yield_now;
use std::time::Duration;

use futures::executor::block_on;
use libgpiod::{chip::Chip, line, line::EventClock, request};
use parking_lot::FairMutex;
use tokio::task::JoinHandle;

use crate::config::{EdgeDetect, PinConfig};
use crate::error::AppError;
use crate::gpio::{EdgeCallback, EdgeEvent, GpioBackend, GpioState, PinSettings};

const LIBGPIOD_BACKEND_EVENT_BUFFER_CAPACITY: usize = 16;
const LIBGPIOD_BACKEND_EVENT_WAIT_TIMEOUT_MS: Duration = Duration::from_millis(100);

pub struct LibgpiodBackend {
    pins: RwLock<HashMap<String, Mutex<PinHandle>>>, // keyed by pin id
}

struct PinHandle {
    line: u32,
    settings: PinSettings,
    chip: Arc<Mutex<Chip>>,
    request: Arc<FairMutex<request::Request>>,
    listener: Option<EdgeListener>,
}

#[allow(dead_code)]
struct EdgeListener {
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<Result<(), AppError>>,
    callback: EdgeCallback,
    // Keep strong refs so chip outlives request while listener runs
    chip: Arc<Mutex<Chip>>,
    request: Arc<FairMutex<request::Request>>,
}

impl LibgpiodBackend {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            pins: RwLock::new(HashMap::new()),
        })
    }

    fn validate_pin_settings(settings: &PinSettings) -> Result<(), AppError> {
        match settings.state {
            GpioState::Error => {
                return Err(AppError::InvalidState(
                    "cannot set pin to error state".into(),
                ));
            }
            GpioState::Disabled => {
                if settings.edge != EdgeDetect::None {
                    return Err(AppError::InvalidState(
                        "cannot set edge detection on disabled pin".into(),
                    ));
                }
                if settings.debounce_ms != 0 {
                    return Err(AppError::InvalidState(
                        "cannot set debounce on disabled pin".into(),
                    ));
                }
                Ok(())
            }
            _ => {
                if !settings.state.is_edge_detectable() && settings.edge != EdgeDetect::None {
                    return Err(AppError::InvalidState(
                        "edge detection requires an input-capable state".into(),
                    ));
                }
                if settings.edge == EdgeDetect::None && settings.debounce_ms != 0 {
                    return Err(AppError::InvalidState(
                        "debounce requires edge detection to be enabled".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn make_line_settings(settings: &PinSettings) -> Result<line::Settings, AppError> {
        let mut ls =
            line::Settings::new().map_err(|e| AppError::Gpio(format!("libgpiod settings: {e}")))?;

        match settings.state {
            GpioState::Error | GpioState::Disabled => {
                return Err(AppError::InvalidState(
                    "cannot create settings for error or disabled state".into(),
                ));
            }
            GpioState::PushPull => {
                ls.set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                ls.set_drive(line::Drive::PushPull)
                    .map_err(|e| AppError::Gpio(format!("libgpiod drive: {e}")))?;
            }
            GpioState::OpenDrain => {
                ls.set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                ls.set_drive(line::Drive::OpenDrain)
                    .map_err(|e| AppError::Gpio(format!("libgpiod drive: {e}")))?;
            }
            GpioState::OpenSource => {
                ls.set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                ls.set_drive(line::Drive::OpenSource)
                    .map_err(|e| AppError::Gpio(format!("libgpiod drive: {e}")))?;
            }
            GpioState::Floating => {
                ls.set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                ls.set_bias(None)
                    .map_err(|e| AppError::Gpio(format!("libgpiod bias: {e}")))?;
            }
            GpioState::PullUp => {
                ls.set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                ls.set_bias(Some(line::Bias::PullUp))
                    .map_err(|e| AppError::Gpio(format!("libgpiod bias: {e}")))?;
            }
            GpioState::PullDown => {
                ls.set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("libgpiod dir: {e}")))?;
                ls.set_bias(Some(line::Bias::PullDown))
                    .map_err(|e| AppError::Gpio(format!("libgpiod bias: {e}")))?;
            }
        }

        if settings.edge != EdgeDetect::None && settings.state.is_edge_detectable() {
            let edge = match settings.edge {
                EdgeDetect::None => None,
                EdgeDetect::Rising => Some(line::Edge::Rising),
                EdgeDetect::Falling => Some(line::Edge::Falling),
                EdgeDetect::Both => Some(line::Edge::Both),
            };
            ls.set_edge_detection(edge)
                .map_err(|e| AppError::Gpio(format!("libgpiod edge: {e}")))?;
            ls.set_event_clock(EventClock::Realtime)
                .map_err(|e| AppError::Gpio(format!("libgpiod clock: {e}")))?;
            ls.set_debounce_period(Duration::from_millis(settings.debounce_ms));
        }

        Ok(ls)
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
            .set_consumer(env!("CARGO_PKG_NAME"))
            .map_err(|e| AppError::Gpio(format!("request consumer: {e}")))?;
        chip.request_lines(Some(&req_cfg), line_cfg)
            .map_err(|e| AppError::Gpio(format!("request lines: {e}")))
    }

    fn spawn_edge_listener(
        pin_id: String,
        chip: Arc<Mutex<Chip>>,
        request: Arc<FairMutex<request::Request>>,
        callback: EdgeCallback,
    ) -> Result<EdgeListener, AppError> {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancel.clone();
        let pin_for_thread = pin_id.clone();
        let req_for_thread = request.clone();
        let cb_for_thread = callback.clone();

        let handle = tokio::task::spawn_blocking(move || {
            let mut buffer = request::Buffer::new(LIBGPIOD_BACKEND_EVENT_BUFFER_CAPACITY)
                .map_err(|e| AppError::Gpio(format!("event buffer: {e}")))?;

            while !cancel_flag.load(Ordering::Relaxed) {
                let req = match req_for_thread.try_lock() {
                    Some(r) => r,
                    None => {
                        yield_now();
                        continue;
                    }
                };

                let has_event =
                    match req.wait_edge_events(Some(LIBGPIOD_BACKEND_EVENT_WAIT_TIMEOUT_MS)) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!(
                                "edge listener: wait_edge_events error for pin {}: {e}",
                                pin_for_thread
                            );
                            yield_now();
                            continue;
                        }
                    };

                if !has_event {
                    continue;
                }

                let events = match req.read_edge_events(&mut buffer) {
                    Ok(evts) => evts,
                    Err(e) => {
                        eprintln!(
                            "edge listener: read_edge_events error for pin {}: {e}",
                            pin_for_thread
                        );
                        yield_now();
                        continue;
                    }
                };

                for evt in events {
                    let evt = match evt {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    let edge_kind = match evt.event_type() {
                        Ok(line::EdgeKind::Rising) => EdgeDetect::Rising,
                        Ok(line::EdgeKind::Falling) => EdgeDetect::Falling,
                        Err(_) => continue,
                    };

                    cb_for_thread(EdgeEvent {
                        pin_id: pin_for_thread.clone(),
                        edge: edge_kind,
                        timestamp_ms: evt.timestamp().as_millis() as u64,
                    });
                }
            }

            Ok::<(), AppError>(())
        });

        Ok(EdgeListener {
            cancel,
            handle,
            callback,
            chip,
            request,
        })
    }

    fn stop_edge_listener(listener: EdgeListener) {
        listener.cancel.store(true, Ordering::Relaxed);
        let handle = listener.handle;
        let _ = block_on(handle);
    }
}

impl GpioBackend for LibgpiodBackend {
    fn get_settings(&self, pin_id: &str) -> Result<PinSettings, AppError> {
        let pins = self
            .pins
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if let Some(handle_lock) = pins.get(pin_id) {
            let handle = handle_lock
                .lock()
                .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
            Ok(handle.settings.clone())
        } else {
            Ok(PinSettings::default())
        }
    }

    fn set_settings(
        &self,
        pin_id: &str,
        pin: &PinConfig,
        settings: &PinSettings,
        event_callback: Option<EdgeCallback>,
    ) -> Result<(), AppError> {
        Self::validate_pin_settings(settings)?;

        let mut pins = self
            .pins
            .write()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if settings.edge == EdgeDetect::None {
            if let Some(entry) = pins.get_mut(pin_id) {
                let mut handle = entry
                    .lock()
                    .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

                if let Some(listener) = handle.listener.take() {
                    Self::stop_edge_listener(listener);
                }
            }
        }

        if settings.state == GpioState::Disabled {
            if let Some(entry) = pins.remove(pin_id) {
                drop(entry);
            }
            return Ok(());
        }

        let get_listener = |edge: EdgeDetect,
                            pin_id: &str,
                            chip: Arc<Mutex<Chip>>,
                            request: Arc<FairMutex<request::Request>>,
                            callback: Option<EdgeCallback>|
         -> Result<Option<EdgeListener>, AppError> {
            if edge != EdgeDetect::None && callback.is_some() {
                let listener = Self::spawn_edge_listener(
                    pin_id.to_string(),
                    chip,
                    request,
                    callback.unwrap(),
                )?;
                Ok(Some(listener))
            } else {
                Ok(None)
            }
        };

        match pins.entry(pin_id.to_string()) {
            Entry::Occupied(mut entry) => {
                let mut handle = entry
                    .get_mut()
                    .lock()
                    .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

                let line_settings = Self::make_line_settings(settings)?;
                let line_cfg = Self::make_line_config(handle.line, line_settings)?;
                handle
                    .request
                    .lock()
                    .reconfigure_lines(&line_cfg)
                    .map_err(|e| AppError::Gpio(format!("reconfigure lines: {e}")))?;
                if handle.listener.is_none() {
                    handle.listener = get_listener(
                        settings.edge,
                        pin_id,
                        handle.chip.clone(),
                        handle.request.clone(),
                        event_callback,
                    )?;
                }

                handle.settings = settings.clone();
            }
            Entry::Vacant(entry) => {
                let line_settings = Self::make_line_settings(settings)?;
                let line_cfg = Self::make_line_config(pin.line, line_settings)?;
                let chip = Arc::new(Mutex::new(Self::open_chip(&pin.chip)?));
                let request = {
                    let guard = chip
                        .lock()
                        .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
                    Arc::new(FairMutex::new(Self::request_lines(&*guard, &line_cfg)?))
                };
                let listener = get_listener(
                    settings.edge,
                    pin_id,
                    chip.clone(),
                    request.clone(),
                    event_callback,
                )?;

                entry.insert(Mutex::new(PinHandle {
                    line: pin.line,
                    settings: settings.clone(),
                    chip,
                    request,
                    listener,
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
            .lock()
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
        let handle = handle_lock
            .lock()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if !handle.settings.state.is_writable() {
            return Err(AppError::InvalidState(
                "pin must be in output mode to set value".to_string(),
            ));
        }

        let offset = handle.line;

        handle
            .request
            .lock()
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
