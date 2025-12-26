#[cfg(feature = "hardware-gpio")]
pub mod libgpiod;
pub mod mock;

#[cfg(feature = "hardware-gpio")]
pub use libgpiod::LibgpiodBackend;
pub use mock::MockGpioBackend;
