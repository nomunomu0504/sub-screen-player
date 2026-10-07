//! The drivers built into the daemon.
//!
//! Adding a device: create `crates/drivers/<model>`, add it to the workspace and to this
//! crate's dependencies, then register its driver below. See `docs/adding-a-device.md`.

use ssp_core::{DriverSelection, Registry};

/// A registry with every built-in driver, of which `selection` is enabled.
/// Fails if the selection names a driver that does not exist.
pub fn registry(selection: &DriverSelection) -> ssp_core::Result<Registry> {
    let mut registry = Registry::new();
    registry.register(ssp_driver_d92::D92Driver);
    registry.select(selection)?;
    Ok(registry)
}
