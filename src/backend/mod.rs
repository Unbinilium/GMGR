#[cfg(feature = "hardware-gpio")]
pub(crate) mod libgpiod;
pub(crate) mod mock;

#[cfg(feature = "hardware-gpio")]
pub use libgpiod::LibgpiodBackend;
pub use mock::MockGpioBackend;
