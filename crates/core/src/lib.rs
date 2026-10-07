//! Device-independent core of sub-screen-player.
//!
//! This crate defines what every supported screen must provide and everything that is the same
//! for all of them:
//!
//! - [`Display`]: the trait a device driver implements (show a frame, set brightness, ...).
//! - [`Driver`] and [`Registry`]: how a driver tells which USB devices it handles.
//! - [`Transport`] and [`hid`]: how bytes reach the device.
//! - [`Frame`] and [`Encoder`]: turning a landscape RGBA image into what the panel expects.
//! - [`Presenter`]: pacing frames to one device from any thread (latest frame wins).
//!
//! Device-specific protocol code never lives here; it belongs in `crates/drivers/<model>`.
//! See `docs/architecture.md` for the full picture.
#![warn(missing_docs)]

mod display;
mod driver;
mod error;
mod frame;
pub mod hid;
mod presenter;
pub mod testing;
mod transport;

pub use display::{
    Capabilities, Display, DisplayInfo, EncodedImage, ImageFormat, PanelSpec, Rotation,
};
pub use driver::{Candidate, Driver, DriverSelection, Found, Registry, UsbMatch};
pub use error::{Error, Result};
pub use frame::{Encoder, Fit, Frame};
pub use presenter::{Presenter, PresenterOptions, PresenterStats, StopAction};
pub use transport::Transport;
