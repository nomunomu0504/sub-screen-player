# Adding a device

[日本語](adding-a-device.ja.md)

A driver teaches sub-screen-player one family of displays. Everything else (finding the
device, pacing frames, encoding, keep-alives, the API, the CLI) is shared, so a driver is
usually a few hundred lines. Use `crates/drivers/d92` as the reference.

## 1. Learn how the device talks

Collect, and write down in `docs/devices/<model>.md` (and its Japanese version
`docs/devices/<model>.ja.md`):

- **USB ids** (vendor:product) and the interface class. On macOS: `ioreg -p IOUSB -l`;
  Linux: `lsusb -v`; Windows: Device Manager → Details → Hardware Ids.
- **Panel size and orientation**: the size callers should draw (landscape), and whether
  images must be rotated before sending.
- **Image format**: JPEG, raw RGB565, ... and any size limit.
- **Commands**: how to show a frame, and if available brightness, power, clear, keep-alive.
- **Timing**: how fast frames can go, and whether the device needs keep-alives.

If there is no documentation, capture the vendor app's traffic. Wireshark with USBPcap works
on x86 Windows. On machines where that is not possible (e.g. Apple Silicon), running the
vendor app in a VMware VM with `usb.analyzer.enable = "TRUE"` logs the USB traffic in
`vmware.log`. See the [reverse engineering etiquette](../CONTRIBUTING.md#reverse-engineering-etiquette).

Today `ssp-core` supports **HID** devices. If your display uses USB bulk transfers or a
serial port, open an issue first: it needs a new `Transport` in `crates/core`, which other
drivers will share.

## 2. Create the crate

```text
crates/drivers/<model>/
├── Cargo.toml
└── src/
    ├── lib.rs        Driver + Display implementation
    └── protocol.rs   pure functions that build reports
```

`Cargo.toml`:

```toml
[package]
name = "ssp-driver-<model>"
description = "sub-screen-player driver for the <Vendor Model>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true

[dependencies]
ssp-core.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

## 3. Write the protocol

`protocol.rs` holds constants (USB ids, panel size, report length) and functions that turn
commands and images into bytes. Keep it free of I/O so it can be tested exhaustively.

Test it against known-good bytes. For example, the D92 driver checks the exact header that a
14547-byte JPEG gets:

```rust
#[test]
fn live_frame_header_is_exact() {
    let out = live_frame(&vec![0xAB; 14547]);
    assert_eq!(&out[..13], &[0x43, 0x52, 0x54, 0, 0, 0x44, 0x52, 0x41, 0, 0, 0x38, 0xf3, 0xb1]);
}
```

If a command is known to break the device, write a test that the driver never sends it.

## 4. Implement `Display`

```rust
pub struct MyDisplay {
    transport: Box<dyn Transport>,
    info: DisplayInfo,
}

impl MyDisplay {
    /// Takes a `Transport` (not a HID handle) so tests can pass a `RecordingTransport`.
    pub fn open(transport: Box<dyn Transport>, serial: &str) -> Result<Self> { ... }
}

impl Display for MyDisplay {
    fn info(&self) -> &DisplayInfo { &self.info }
    fn show(&mut self, image: &EncodedImage) -> Result<()> { ... }
    // Implement only what the device supports; the rest keeps the defaults.
}
```

Fill in `DisplayInfo` honestly:

- `driver`: a short, stable id (`"d92"`). It becomes part of display ids and log lines.
- `panel`: the landscape size, the `Rotation` needed on the wire and the `ImageFormat`.
- `capabilities`: set `max_fps` to what you measured, `keep_alive_interval` if the device
  needs one, and `max_image_bytes` to what the device accepts.

Return `Error::Transport` or `Error::Disconnected` when the device is gone; the daemon then
reopens it automatically. Do not retry or sleep in loops inside the driver.

## 5. Implement `Driver`

```rust
pub struct MyDriver;

impl Driver for MyDriver {
    fn id(&self) -> &'static str { "mymodel" }
    fn name(&self) -> &'static str { "Vendor Model" }
    fn usb_matches(&self) -> &'static [UsbMatch] {
        &[UsbMatch { vendor_id: 0x1234, product_id: 0x5678, usage_page: Some(0xFF00) }]
    }
    fn open(&self, candidate: &Candidate) -> Result<Box<dyn Display>> {
        let transport = ssp_core::hid::open(candidate)?;
        Ok(Box::new(MyDisplay::open(Box::new(transport), &candidate.serial)?))
    }
}
```

Match as narrowly as you can (add the usage page if the device has several HID interfaces),
so the driver never claims somebody's keyboard.

While the driver is being developed, mark it experimental. It is then only used when it is
named (`--driver mymodel`, or `enable` in the `[drivers]` section of the config), so a
half-finished driver never takes over a display on its own, not even if it gets released:

```rust
    fn experimental(&self) -> bool {
        true // remove once it is verified on hardware
    }
```

## 6. Register it

1. Root `Cargo.toml`: add the crate to `members` and to `[workspace.dependencies]`.
2. `crates/server/Cargo.toml`: add `ssp-driver-<model>.workspace = true`.
3. `crates/server/src/drivers.rs`: `registry.register(ssp_driver_<model>::MyDriver);`
4. `contrib/linux/70-sub-screen-player.rules`: add a line with your vendor and product id.
5. `README.md` and `README.ja.md`: add the device to the "Supported displays" table.
6. `docs/devices/<model>.md` and `docs/devices/<model>.ja.md`: the protocol notes from step 1.

## 7. Test on hardware

`mise run ci` must pass. Then run the automatic check on the device and paste its output
into your pull request:

```sh
ssp selftest --driver mymodel  # every check should PASS
```

A running daemon only blocks the test if it uses your driver. To keep another display running
meanwhile, start the daemon with just its driver, e.g. `ssp serve --driver d92`.

Then try it by hand and look at the screen:

```sh
mise run serve                 # the clock should appear
ssp devices                    # your display is listed
ssp brightness 30 && ssp brightness 100
ssp off && ssp on
ssp show some-photo.jpg --fit cover   # check the orientation
ssp status                     # check frames shown / dropped and send time
```

To measure the frame rate, stream frames over the WebSocket API (see [api.md](api.md)) and
watch `ssp status`. Unplug and replug the display while the clock runs: it should come back
on its own.
