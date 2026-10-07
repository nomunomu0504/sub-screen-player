use std::ffi::CString;

use crate::{Display, Error, Result, hid};

/// A USB interface a driver can handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbMatch {
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
    /// HID usage page of the interface. Ignored on platforms that do not report it.
    pub usage_page: Option<u16>,
}

impl UsbMatch {
    /// Whether `candidate` is this interface.
    pub fn matches(&self, candidate: &Candidate) -> bool {
        candidate.vendor_id == self.vendor_id
            && candidate.product_id == self.product_id
            && match self.usage_page {
                // Some backends report 0 when they do not know the usage page.
                Some(page) => candidate.usage_page == 0 || candidate.usage_page == page,
                None => true,
            }
    }
}

/// A device interface found during a scan, before it is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// OS-specific path used to open the interface.
    pub path: CString,
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
    /// HID usage page (0 if unknown).
    pub usage_page: u16,
    /// USB serial number (may be empty).
    pub serial: String,
    /// USB product string (may be empty).
    pub product: String,
}

/// Knows one family of devices: which USB interfaces belong to it and how to open them.
///
/// Each driver crate in `crates/drivers/<model>` exports one type implementing this trait, and
/// the daemon lists it in `crates/server/src/drivers.rs`.
pub trait Driver: Send + Sync {
    /// Short, stable id used in configs and display ids, e.g. `"d92"`.
    fn id(&self) -> &'static str;

    /// Human-readable name, e.g. `"upHere / MiraBox D92"`.
    fn name(&self) -> &'static str;

    /// USB interfaces this driver handles.
    fn usb_matches(&self) -> &'static [UsbMatch];

    /// Opens a matching interface and prepares the display for use.
    fn open(&self, candidate: &Candidate) -> Result<Box<dyn Display>>;

    /// Whether the driver is still being developed. Experimental drivers are only used when
    /// asked for by id (`--driver <id>` or `[drivers] enable` in the config), so they never
    /// take over a device on their own.
    fn experimental(&self) -> bool {
        false
    }
}

/// Which drivers to use, e.g. from `--driver` options or the `[drivers]` config section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriverSelection {
    /// If not empty, only these drivers are used (experimental ones included).
    pub only: Vec<String>,
    /// Drivers that are never used.
    pub exclude: Vec<String>,
}

impl DriverSelection {
    /// Whether `driver` is used under this selection.
    pub fn allows(&self, driver: &dyn Driver) -> bool {
        let named = |ids: &[String]| ids.iter().any(|id| id == driver.id());
        if named(&self.exclude) {
            false
        } else if self.only.is_empty() {
            !driver.experimental()
        } else {
            named(&self.only)
        }
    }
}

/// The set of drivers the program was built with, and which of them are in use.
#[derive(Default)]
pub struct Registry {
    drivers: Vec<Entry>,
}

struct Entry {
    driver: Box<dyn Driver>,
    enabled: bool,
}

/// A candidate together with the driver that claimed it.
pub struct Found<'a> {
    /// The driver that matches the candidate.
    pub driver: &'a dyn Driver,
    /// The device interface.
    pub candidate: Candidate,
}

impl Registry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a driver. Drivers registered first win when two match the same device.
    /// Experimental drivers start disabled; see [`Registry::select`].
    pub fn register(&mut self, driver: impl Driver + 'static) -> &mut Self {
        let enabled = !driver.experimental();
        self.drivers.push(Entry {
            driver: Box::new(driver),
            enabled,
        });
        self
    }

    /// Enables exactly the drivers `selection` allows. Fails on ids no driver has.
    pub fn select(&mut self, selection: &DriverSelection) -> Result<()> {
        for id in selection.only.iter().chain(&selection.exclude) {
            if !self.drivers.iter().any(|e| e.driver.id() == id) {
                let known: Vec<&str> = self.drivers.iter().map(|e| e.driver.id()).collect();
                return Err(Error::InvalidArgument(format!(
                    "unknown driver {id:?} (available: {})",
                    known.join(", ")
                )));
            }
        }
        for entry in &mut self.drivers {
            entry.enabled = selection.allows(entry.driver.as_ref());
        }
        Ok(())
    }

    /// All registered drivers, in registration order, enabled or not.
    pub fn drivers(&self) -> impl Iterator<Item = &dyn Driver> {
        self.drivers.iter().map(|e| e.driver.as_ref())
    }

    /// The drivers in use, in registration order.
    pub fn enabled(&self) -> impl Iterator<Item = &dyn Driver> {
        self.drivers
            .iter()
            .filter(|e| e.enabled)
            .map(|e| e.driver.as_ref())
    }

    /// Returns the connected devices some driver can handle.
    pub fn scan(&self) -> Result<Vec<Found<'_>>> {
        Ok(self.claim(hid::enumerate()?))
    }

    /// Assigns each candidate to the first enabled driver that matches it.
    pub fn claim(&self, candidates: Vec<Candidate>) -> Vec<Found<'_>> {
        candidates
            .into_iter()
            .filter_map(|candidate| {
                let driver = self
                    .enabled()
                    .find(|d| d.usb_matches().iter().any(|m| m.matches(&candidate)))?;
                Some(Found { driver, candidate })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;

    /// Same device as `Dummy`, but still being developed.
    struct Experimental;

    impl Driver for Experimental {
        fn id(&self) -> &'static str {
            "next"
        }
        fn name(&self) -> &'static str {
            "Next"
        }
        fn usb_matches(&self) -> &'static [UsbMatch] {
            Dummy.usb_matches()
        }
        fn open(&self, _: &Candidate) -> Result<Box<dyn Display>> {
            Err(Error::Unsupported("open"))
        }
        fn experimental(&self) -> bool {
            true
        }
    }

    impl Driver for Dummy {
        fn id(&self) -> &'static str {
            "dummy"
        }
        fn name(&self) -> &'static str {
            "Dummy"
        }
        fn usb_matches(&self) -> &'static [UsbMatch] {
            &[UsbMatch {
                vendor_id: 0x1234,
                product_id: 0x0001,
                usage_page: Some(0xFFA0),
            }]
        }
        fn open(&self, _: &Candidate) -> Result<Box<dyn Display>> {
            Err(Error::Unsupported("open"))
        }
    }

    fn candidate(vendor_id: u16, product_id: u16, usage_page: u16) -> Candidate {
        Candidate {
            path: CString::new("p").unwrap(),
            vendor_id,
            product_id,
            usage_page,
            serial: String::new(),
            product: String::new(),
        }
    }

    #[test]
    fn claims_only_matching_interfaces() {
        let mut registry = Registry::new();
        registry.register(Dummy);
        let found = registry.claim(vec![
            candidate(0x1234, 0x0001, 0xFFA0),
            candidate(0x1234, 0x0001, 0x0001),
            candidate(0x1234, 0x0001, 0),
            candidate(0x1234, 0x0002, 0xFFA0),
        ]);
        let pages: Vec<u16> = found.iter().map(|f| f.candidate.usage_page).collect();
        assert_eq!(pages, [0xFFA0, 0]);
        assert_eq!(found[0].driver.id(), "dummy");
    }

    fn claimed_by(registry: &Registry) -> Vec<&'static str> {
        registry
            .claim(vec![candidate(0x1234, 0x0001, 0xFFA0)])
            .iter()
            .map(|f| f.driver.id())
            .collect()
    }

    fn selection(only: &[&str], exclude: &[&str]) -> DriverSelection {
        DriverSelection {
            only: only.iter().map(ToString::to_string).collect(),
            exclude: exclude.iter().map(ToString::to_string).collect(),
        }
    }

    #[test]
    fn experimental_drivers_are_off_unless_named() {
        let mut registry = Registry::new();
        registry.register(Experimental).register(Dummy);
        assert_eq!(claimed_by(&registry), ["dummy"]);

        registry.select(&selection(&["next"], &[])).unwrap();
        assert_eq!(claimed_by(&registry), ["next"]);

        registry.select(&DriverSelection::default()).unwrap();
        assert_eq!(claimed_by(&registry), ["dummy"]);
    }

    #[test]
    fn excluded_drivers_claim_nothing() {
        let mut registry = Registry::new();
        registry.register(Dummy);
        registry.select(&selection(&[], &["dummy"])).unwrap();
        assert!(claimed_by(&registry).is_empty());
        assert_eq!(registry.drivers().count(), 1);
        assert_eq!(registry.enabled().count(), 0);
    }

    #[test]
    fn unknown_driver_ids_are_rejected() {
        let mut registry = Registry::new();
        registry.register(Dummy);
        let err = registry.select(&selection(&["d93"], &[])).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown driver \"d93\" (available: dummy)"),
            "{err}"
        );
    }
}
