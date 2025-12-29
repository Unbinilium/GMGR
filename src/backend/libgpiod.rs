use log::warn;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{JoinHandle, yield_now};
use std::time::Duration;

use libgpiod::{chip::Chip, line, line::EventClock, request};
use parking_lot::{FairMutex, RwLock as PLRwLock, RwLockUpgradableReadGuard};
use rustc_hash::FxHashMap;

use crate::config::{EdgeDetect, PinConfig};
use crate::error::AppError;
use crate::gpio::{EdgeEvent, EventHandler, GpioBackend, GpioState, PinSettings};

const LIBGPIOD_BACKEND_EVENT_BUFFER_CAPACITY: usize = 64;
const LIBGPIOD_BACKEND_EVENT_WAIT_TIMEOUT_MS: Duration = Duration::from_millis(10);

pub struct LibgpiodBackend {
    pins: PLRwLock<FxHashMap<u32, RwLock<PinHandle>>>, // keyed by pin id
}

struct PinHandle {
    line: u32,
    settings: PinSettings,
    gpiod_handle: Arc<FairMutex<GpiodHandle>>,
    listener: Option<EdgeListener>, // drop in reverse order
}

impl PinHandle {
    fn new(
        line: u32,
        settings: PinSettings,
        gpiod_handle: Arc<FairMutex<GpiodHandle>>,
        listener: Option<EdgeListener>,
    ) -> Self {
        Self {
            line,
            settings,
            gpiod_handle,
            listener,
        }
    }
}

struct GpiodHandle {
    request: request::Request,
}

impl GpiodHandle {
    fn new(chip: &str, line_cfg: &line::Config) -> Result<Self, AppError> {
        let chip = Self::open_chip(chip)?;
        let request = Self::request_lines(&chip, line_cfg)?;
        Ok(Self { request })
    }

    fn open_chip(path: &str) -> Result<Chip, AppError> {
        let p = PathBuf::from(path);
        Chip::open(&p).map_err(|e| AppError::Gpio(format!("open chip {path}: {e}")))
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
}

struct EdgeListener {
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EdgeListener {
    fn new(
        pin_id: u32,
        gpiod_handle: Arc<FairMutex<GpiodHandle>>,
        handler: EventHandler,
    ) -> Result<Self, AppError> {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancel.clone();
        let mut buffer = request::Buffer::new(LIBGPIOD_BACKEND_EVENT_BUFFER_CAPACITY)
            .map_err(|e| AppError::Gpio(format!("event buffer: {e}")))?;

        let handle = std::thread::spawn(move || {
            while !cancel_flag.load(Ordering::Relaxed) {
                let hdl = gpiod_handle.lock();
                let req = &hdl.request;

                let has_event =
                    match req.wait_edge_events(Some(LIBGPIOD_BACKEND_EVENT_WAIT_TIMEOUT_MS)) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("wait edge events error for pin {pin_id}: {e}");
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
                        warn!("read edge events error for pin {pin_id}: {e}");
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

                    handler.dispatch(EdgeEvent {
                        pin_id,
                        edge: edge_kind,
                        timestamp_ms: evt.timestamp().as_millis() as u64,
                    });
                }
            }
        });

        Ok(Self {
            cancel,
            handle: Some(handle),
        })
    }
}

impl Drop for EdgeListener {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl LibgpiodBackend {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            pins: PLRwLock::new(FxHashMap::default()),
        })
    }

    fn validate_pin_settings(settings: &PinSettings) -> Result<(), AppError> {
        match settings.state {
            GpioState::Error => Err(AppError::InvalidState(
                "cannot set pin to error state".into(),
            )),
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
                match settings.edge {
                    EdgeDetect::None => {
                        if settings.debounce_ms != 0 {
                            return Err(AppError::InvalidState(
                                "debouncing requires edge detection to be enabled".into(),
                            ));
                        }
                    }
                    _ => {
                        if !settings.state.is_edge_detectable() {
                            return Err(AppError::InvalidState(
                                "edge detection requires an input-capable state".into(),
                            ));
                        }
                    }
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
                    .map_err(|e| AppError::Gpio(format!("set direction: {e}")))?;
                ls.set_drive(line::Drive::PushPull)
                    .map_err(|e| AppError::Gpio(format!("set drive: {e}")))?;
            }
            GpioState::OpenDrain => {
                ls.set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("set direction: {e}")))?;
                ls.set_drive(line::Drive::OpenDrain)
                    .map_err(|e| AppError::Gpio(format!("set drive: {e}")))?;
            }
            GpioState::OpenSource => {
                ls.set_direction(line::Direction::Output)
                    .map_err(|e| AppError::Gpio(format!("set direction: {e}")))?;
                ls.set_drive(line::Drive::OpenSource)
                    .map_err(|e| AppError::Gpio(format!("set drive: {e}")))?;
            }
            GpioState::Floating => {
                ls.set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("set direction: {e}")))?;
                ls.set_bias(None)
                    .map_err(|e| AppError::Gpio(format!("set bias: {e}")))?;
            }
            GpioState::PullUp => {
                ls.set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("set direction: {e}")))?;
                ls.set_bias(Some(line::Bias::PullUp))
                    .map_err(|e| AppError::Gpio(format!("set bias: {e}")))?;
            }
            GpioState::PullDown => {
                ls.set_direction(line::Direction::Input)
                    .map_err(|e| AppError::Gpio(format!("set direction: {e}")))?;
                ls.set_bias(Some(line::Bias::PullDown))
                    .map_err(|e| AppError::Gpio(format!("set bias: {e}")))?;
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
                .map_err(|e| AppError::Gpio(format!("set edge detection: {e}")))?;
            ls.set_event_clock(EventClock::Realtime)
                .map_err(|e| AppError::Gpio(format!("set event clock: {e}")))?;
            ls.set_debounce_period(Duration::from_millis(settings.debounce_ms));
        }

        Ok(ls)
    }

    fn make_line_config(offset: u32, settings: line::Settings) -> Result<line::Config, AppError> {
        let mut cfg =
            line::Config::new().map_err(|e| AppError::Gpio(format!("line config: {e}")))?;
        cfg.add_line_settings(&[offset], settings)
            .map_err(|e| AppError::Gpio(format!("line config add settings: {e}")))?;
        Ok(cfg)
    }
}

impl GpioBackend for LibgpiodBackend {
    fn get_settings(&self, pin_id: u32) -> Result<PinSettings, AppError> {
        let pins = self.pins.read();

        match pins.get(&pin_id) {
            None => Ok(PinSettings::default()),
            Some(handle_lock) => {
                let handle = handle_lock
                    .read()
                    .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;
                Ok(handle.settings.clone())
            }
        }
    }

    fn set_settings(
        &self,
        pin_id: u32,
        pin: &PinConfig,
        settings: &PinSettings,
        event_handler: Option<EventHandler>,
    ) -> Result<(), AppError> {
        let get_listener = |edge: EdgeDetect,
                            pin_id: u32,
                            gpiod_handle: &Arc<FairMutex<GpiodHandle>>,
                            handler: Option<EventHandler>|
         -> Result<Option<EdgeListener>, AppError> {
            if edge != EdgeDetect::None
                && let Some(handler) = handler
            {
                let listener = EdgeListener::new(pin_id, gpiod_handle.clone(), handler)?;
                Ok(Some(listener))
            } else {
                Ok(None)
            }
        };

        Self::validate_pin_settings(settings)?;

        let pins = self.pins.upgradable_read();

        // fast path for disabling pin
        if settings.state == GpioState::Disabled {
            if pins.get(&pin_id).is_some() {
                let _ = RwLockUpgradableReadGuard::upgrade(pins).remove(&pin_id);
            }
            return Ok(());
        }

        match pins.get(&pin_id) {
            Some(handle) => {
                let mut handle = handle
                    .write()
                    .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

                // drop listener if disabling edge detection before reconfiguring lines
                if settings.edge == EdgeDetect::None
                    && let Some(listener) = handle.listener.take()
                {
                    drop(listener);
                }

                let line_settings = Self::make_line_settings(settings)?;
                let line_cfg = Self::make_line_config(handle.line, line_settings)?;

                handle
                    .gpiod_handle
                    .lock()
                    .request
                    .reconfigure_lines(&line_cfg)
                    .map_err(|e| AppError::Gpio(format!("reconfigure lines: {e}")))?;

                if handle.listener.is_none() {
                    handle.listener =
                        get_listener(settings.edge, pin_id, &handle.gpiod_handle, event_handler)?;
                }

                handle.settings = settings.clone();
            }
            None => {
                // since upgradable read lock is exclusive held by this thread, it safe to pre-allocate
                // new pin handle without double locking
                let line_settings = Self::make_line_settings(settings)?;
                let line_cfg = Self::make_line_config(pin.line, line_settings)?;

                let gpiod_handle =
                    Arc::new(FairMutex::new(GpiodHandle::new(&pin.chip, &line_cfg)?));
                let listener = get_listener(settings.edge, pin_id, &gpiod_handle, event_handler)?;

                let handle = RwLock::new(PinHandle::new(
                    pin.line,
                    settings.clone(),
                    gpiod_handle,
                    listener,
                ));

                let mut pins = RwLockUpgradableReadGuard::upgrade(pins);
                pins.insert(pin_id, handle);
            }
        }

        Ok(())
    }

    fn read_value(&self, pin_id: u32) -> Result<u8, AppError> {
        let pins = self.pins.read();
        let handle_lock = pins
            .get(&pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let handle = handle_lock
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        let value = handle
            .gpiod_handle
            .lock()
            .request
            .value(handle.line)
            .map_err(|e| AppError::Gpio(format!("get value: {e}")))?;
        Ok(match value {
            line::Value::InActive => 0,
            line::Value::Active => 1,
        })
    }

    fn write_value(&self, pin_id: u32, value: u8) -> Result<(), AppError> {
        let pins = self.pins.read();
        let handle_lock = pins
            .get(&pin_id)
            .ok_or_else(|| AppError::InvalidState("pin not configured, set state first".into()))?;
        let handle = handle_lock
            .read()
            .map_err(|e| AppError::Gpio(format!("lock poisoned: {e}")))?;

        if !handle.settings.state.is_writable() {
            return Err(AppError::InvalidState(
                "pin must be in output mode to set value".into(),
            ));
        }

        let offset = handle.line;

        handle
            .gpiod_handle
            .lock()
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
