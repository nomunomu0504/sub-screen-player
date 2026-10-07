//! The drivers built into the daemon.
//!
//! Adding a device: create `crates/drivers/<model>`, add it to the workspace and to this
//! crate's dependencies, then register its driver below. See `docs/adding-a-device.md`.

use ssp_core::Registry;

/// A registry with every built-in driver.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(ssp_driver_d92::D92Driver);
    registry
}
