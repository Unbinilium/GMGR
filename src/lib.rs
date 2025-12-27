mod backend;
mod config;
mod error;
mod gpio;
mod routes;

pub use config::{AppConfig, EdgeDetect, GpioCapability, HttpConfig, PinConfig};
pub use error::AppError;
pub use gpio::{
    EdgeEvent, EventHandler, GpioBackend, GpioManager, GpioState, PinDescriptor, PinSettings,
};
pub use routes::AppState;

#[cfg(feature = "hardware-gpio")]
pub use backend::LibgpiodBackend;
pub use backend::MockGpioBackend;
