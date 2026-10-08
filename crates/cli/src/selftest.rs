//! `ssp selftest`: drives connected displays through a fixed sequence and reports what worked.
//! It talks to the hardware directly. Displays whose driver a running daemon uses are skipped,
//! because the daemon may open them at any moment; stop the daemon, or start it with `--driver`
//! for other drivers only, to test them.

use std::collections::HashMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use image::{Rgb, RgbImage};
use serde::Serialize;
use ssp_core::{
    DisplayInfo, DriverSelection, Found, Frame, Presenter, PresenterOptions, Registry, StopAction,
    hid,
};
use ssp_server::text::{TextStyle, builtin_font};

use crate::client::{self, Client};

/// How long to keep trying to open a display that is restarting.
const OPEN_RETRY: Duration = Duration::from_secs(15);

/// Settings of one run.
pub struct Options {
    /// Only test the display with this id (`default`: all of them).
    pub display: String,
    /// Only test displays of these drivers (empty: all non-experimental drivers).
    pub drivers: Vec<String>,
    /// Frames sent in the streaming check.
    pub frames: u32,
    /// How long to stay idle in the keep-alive check.
    pub hold: Duration,
    /// Print JSON instead of text.
    pub json: bool,
}

#[derive(Serialize)]
struct Report {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    passed: bool,
    displays: Vec<DisplayReport>,
    skipped: Vec<Skipped>,
}

#[derive(Serialize)]
struct DisplayReport {
    id: String,
    driver: &'static str,
    model: String,
    serial: String,
    firmware: Option<String>,
    checks: Vec<Check>,
}

#[derive(Serialize)]
struct Skipped {
    id: String,
    reason: String,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

#[derive(Serialize)]
struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

/// A display found on USB, with an id numbered the way the daemon numbers them.
struct Target<'a> {
    id: String,
    found: Found<'a>,
}

/// An opened display, kept alive by its presenter until the run ends.
struct Opened {
    presenter: Presenter,
    info: DisplayInfo,
    path: CString,
    took: Duration,
}

pub fn run(client: &Client, options: &Options) -> Result<bool> {
    let selection = DriverSelection {
        only: options.drivers.clone(),
        exclude: Vec::new(),
    };
    let registry = ssp_server::drivers::registry(&selection)?;
    let daemon = daemon_drivers(client)?;

    let mut report = Report {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        passed: false,
        displays: Vec::new(),
        skipped: Vec::new(),
    };
    let mut targets = Vec::new();
    for target in number(registry.scan()?) {
        if options.display != "default" && target.id != options.display {
            continue;
        }
        let driver = target.found.driver.id();
        if daemon
            .as_ref()
            .is_some_and(|used| used.iter().any(|d| d == driver))
        {
            report.skipped.push(Skipped {
                reason: format!("the running daemon uses the {driver} driver"),
                id: target.id,
            });
        } else {
            targets.push(target);
        }
    }

    if targets.is_empty() {
        if !options.json {
            if report.skipped.is_empty() {
                say!("No supported display found.");
                say!("{}", no_display_hint());
            } else {
                say!(
                    "The running daemon may use every matching display. Stop it, or run it \
                     with only the drivers it should keep (`ssp serve --driver <id>`)."
                );
            }
        }
        print_report(&report, options.json)?;
        return Ok(false);
    }

    // Open every display before testing the first one. Displays such as the D92 restart a few
    // seconds after keep-alives stop (e.g. right after the daemon was stopped); an opened
    // display gets keep-alives while it waits for its turn.
    let opened: Vec<(Target<'_>, Result<Opened, String>)> = targets
        .into_iter()
        .map(|target| {
            let opened = open(&registry, &target);
            (target, opened)
        })
        .collect();
    let on_usb = |path: &CString| {
        hid::enumerate()
            .map(|list| list.iter().any(|c| &c.path == path))
            .unwrap_or(false)
    };
    for (target, opened) in &opened {
        if !options.json {
            say!("Testing {} ({}) ...", target.found.driver.name(), target.id);
        }
        report
            .displays
            .push(test_display(target, opened, options, &on_usb));
    }
    for (_, opened) in opened {
        if let Ok(opened) = opened {
            let _ = opened.presenter.stop(StopAction::Leave);
        }
    }

    report.passed = report
        .displays
        .iter()
        .all(|d| d.checks.iter().all(|c| c.status != Status::Fail));
    print_report(&report, options.json)?;
    Ok(report.passed)
}

/// Drivers a running daemon uses, or `None` if no daemon is running.
fn daemon_drivers(client: &Client) -> Result<Option<Vec<String>>> {
    match client.health() {
        Err(err) if client::is_unreachable(&err) => Ok(None),
        Err(err) => Err(err.context("cannot ask the running daemon which displays it uses")),
        Ok(health) => match health.drivers {
            Some(drivers) => Ok(Some(drivers)),
            None => bail!(
                "the daemon at {} is too old to say which displays it uses.\n\
                 Stop it (Ctrl-C, or `ssp service uninstall`), then run the selftest again.",
                client.base()
            ),
        },
    }
}

fn base_id(found: &Found<'_>) -> String {
    match found.candidate.serial.as_str() {
        "" => found.driver.id().to_string(),
        serial => format!("{}-{serial}", found.driver.id()),
    }
}

/// Gives displays that would share an id the suffixes `-2`, `-3`, ... like the daemon does.
fn number(found: Vec<Found<'_>>) -> Vec<Target<'_>> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    found
        .into_iter()
        .map(|found| {
            let base = base_id(&found);
            let count = seen.entry(base.clone()).or_insert(0);
            *count += 1;
            let id = if *count == 1 {
                base
            } else {
                format!("{base}-{count}")
            };
            Target { id, found }
        })
        .collect()
}

fn no_display_hint() -> &'static str {
    if cfg!(target_os = "linux") {
        "Check the cable, and that the udev rule from contrib/linux is installed (then replug)."
    } else {
        "Check the cable, and that no other program (such as the vendor app) uses the display."
    }
}

/// Opens a display and starts its presenter. A display that is restarting shows up again under
/// a new path, so failures are retried for a while, looking the display up by its serial number.
fn open(registry: &Registry, target: &Target<'_>) -> Result<Opened, String> {
    let driver = target.found.driver;
    let mut candidate = target.found.candidate.clone();
    let started = Instant::now();
    loop {
        match driver.open(&candidate) {
            Ok(display) => {
                let info = display.info().clone();
                let presenter = Presenter::spawn(
                    display,
                    PresenterOptions {
                        max_fps: info.capabilities.max_fps,
                        skip_duplicates: false,
                        ..Default::default()
                    },
                );
                return Ok(Opened {
                    presenter,
                    info,
                    path: candidate.path,
                    took: started.elapsed(),
                });
            }
            Err(err) => {
                let mut detail = err.to_string();
                let permission = detail.to_lowercase().contains("permission");
                if permission || candidate.serial.is_empty() || started.elapsed() > OPEN_RETRY {
                    if cfg!(target_os = "linux") && permission {
                        detail.push_str(" (install the udev rule from contrib/linux and replug)");
                    }
                    return Err(detail);
                }
                std::thread::sleep(Duration::from_secs(1));
                let again = registry.scan().ok().and_then(|found| {
                    found.into_iter().find(|f| {
                        f.driver.id() == driver.id() && f.candidate.serial == candidate.serial
                    })
                });
                if let Some(again) = again {
                    candidate = again.candidate;
                }
            }
        }
    }
}

/// Runs the checks on one opened display. `on_usb` tells whether a device path still exists.
fn test_display(
    target: &Target<'_>,
    opened: &Result<Opened, String>,
    options: &Options,
    on_usb: &dyn Fn(&CString) -> bool,
) -> DisplayReport {
    let mut report = DisplayReport {
        id: target.id.clone(),
        driver: target.found.driver.id(),
        model: target.found.driver.name().to_string(),
        serial: target.found.candidate.serial.clone(),
        firmware: None,
        checks: Vec::new(),
    };
    let opened = match opened {
        Ok(opened) => opened,
        Err(detail) => {
            report
                .checks
                .push(Check::new("open", Status::Fail, detail.clone()));
            return report;
        }
    };
    let info = &opened.info;
    report.model = info.model.clone();
    report.firmware = info.firmware.clone();
    report.checks.push(Check::new(
        "open",
        Status::Pass,
        format!(
            "{} ms, firmware {}",
            opened.took.as_millis(),
            info.firmware.as_deref().unwrap_or("not reported")
        ),
    ));

    let presenter = &opened.presenter;
    let caps = info.capabilities.clone();
    let (width, height) = (info.panel.width, info.panel.height);
    let checks = &mut report.checks;
    let step =
        |n: u32, label: &str| test_pattern(width, height, &format!("ssp selftest {n}/5: {label}"));

    // Commands.
    let mut done = Vec::new();
    let mut failed = None;
    if caps.power {
        match presenter.wake() {
            Ok(()) => done.push("wake"),
            Err(err) => failed = Some(format!("wake: {err}")),
        }
    }
    if caps.brightness && failed.is_none() {
        match presenter.set_brightness(100) {
            Ok(()) => done.push("brightness 100%"),
            Err(err) => failed = Some(format!("brightness: {err}")),
        }
    }
    checks.push(match failed {
        Some(err) => Check::new("commands", Status::Fail, err),
        None if done.is_empty() => Check::new("commands", Status::Skip, "none supported"),
        None => Check::new("commands", Status::Pass, done.join(", ")),
    });

    // A still image.
    let before = presenter.stats().shown;
    let started = Instant::now();
    let shown = presenter
        .submit(step(1, "still image"))
        .map_err(|e| e.to_string())
        .and_then(|()| wait_for(|| presenter.stats().shown > before, Duration::from_secs(5)));
    checks.push(match shown {
        Ok(()) => Check::new(
            "still image",
            Status::Pass,
            format!("shown after {} ms", started.elapsed().as_millis()),
        ),
        Err(err) => Check::new("still image", Status::Fail, err),
    });

    // Streaming.
    checks.push(stream(
        presenter,
        width,
        height,
        options.frames,
        caps.max_fps,
    ));

    // Keep-alive: stay idle longer than devices usually wait before giving up on the host.
    let _ = presenter.submit(test_pattern(
        width,
        height,
        &format!("ssp selftest 3/5: idle for {} s", options.hold.as_secs()),
    ));
    std::thread::sleep(options.hold);
    let still_there = on_usb(&opened.path);
    checks.push(match (presenter.is_running(), still_there) {
        (true, true) => Check::new(
            "keep-alive",
            Status::Pass,
            format!("still connected after {} s idle", options.hold.as_secs()),
        ),
        (false, _) => Check::new(
            "keep-alive",
            Status::Fail,
            format!("display lost: {}", presenter.failure().unwrap_or_default()),
        ),
        (true, false) => Check::new(
            "keep-alive",
            Status::Fail,
            "the display disappeared from USB (it may have restarted)",
        ),
    });

    // Power.
    if !caps.power {
        checks.push(Check::new("power", Status::Skip, "not supported"));
    } else if !presenter.is_running() {
        checks.push(Check::new(
            "power",
            Status::Skip,
            "not run: the display was lost",
        ));
    } else {
        let _ = presenter.submit(step(4, "screen off for 2 s"));
        std::thread::sleep(Duration::from_millis(500));
        let result = presenter.sleep().and_then(|()| {
            std::thread::sleep(Duration::from_secs(2));
            presenter.wake()
        });
        checks.push(match result {
            Ok(()) => Check::new("power", Status::Pass, "off and on again"),
            Err(err) => Check::new("power", Status::Fail, err.to_string()),
        });
    }

    // Leave the result on the screen.
    let passed = checks.iter().all(|c| c.status != Status::Fail);
    let verdict = if passed { "PASS" } else { "FAIL" };
    let _ = presenter.submit(test_pattern(
        width,
        height,
        &format!("ssp selftest 5/5: {verdict}"),
    ));
    let _ = wait_for(
        || presenter.stats().submitted == presenter.stats().shown,
        Duration::from_secs(3),
    );
    report
}

fn stream(presenter: &Presenter, width: u32, height: u32, frames: u32, max_fps: u32) -> Check {
    let interval = Duration::from_secs(1) / max_fps.max(1);
    let before = presenter.stats();
    let started = Instant::now();
    for n in 0..frames {
        let mut frame = test_pattern(
            width,
            height,
            &format!("ssp selftest 2/5: frame {}/{frames}", n + 1),
        );
        let x = (u64::from(n) * u64::from(width) / u64::from(max_fps.max(1) * 2)) as u32 % width;
        for y in 0..height {
            for dx in 0..width.min(40) {
                frame
                    .image_mut()
                    .put_pixel((x + dx) % width, y, Rgb([255, 255, 255]));
            }
        }
        if let Err(err) = presenter.submit(frame) {
            return Check::new("streaming", Status::Fail, err.to_string());
        }
        let next = started + interval * (n + 1);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let settled = wait_for(
        || {
            let s = presenter.stats();
            s.shown + s.dropped >= before.shown + before.dropped + u64::from(frames)
        },
        Duration::from_secs(10),
    );
    let elapsed = started.elapsed().as_secs_f64();
    let after = presenter.stats();
    let shown = after.shown - before.shown;
    let dropped = after.dropped - before.dropped;
    let fps = shown as f64 / elapsed;
    let detail = format!(
        "{frames} frames: {shown} shown, {dropped} dropped, {fps:.1} fps \
         (encode {:.1} ms, send {:.1} ms, {} KB per frame)",
        after.last_encode.as_secs_f64() * 1000.0,
        after.last_send.as_secs_f64() * 1000.0,
        after.last_bytes / 1024
    );
    let status = match settled {
        Err(err) => return Check::new("streaming", Status::Fail, format!("{err}; {detail}")),
        Ok(()) if shown == 0 => Status::Fail,
        Ok(()) if fps < f64::from(max_fps) * 0.5 => Status::Warn,
        Ok(()) => Status::Pass,
    };
    Check::new("streaming", status, detail)
}

fn wait_for(mut done: impl FnMut() -> bool, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while !done() {
        if Instant::now() > deadline {
            return Err(format!("timed out after {} s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Color bars with a label, so a person watching can tell the steps apart.
fn test_pattern(width: u32, height: u32, label: &str) -> Frame {
    const BARS: [[u8; 3]; 8] = [
        [235, 235, 235],
        [235, 235, 16],
        [16, 235, 235],
        [16, 235, 16],
        [235, 16, 235],
        [235, 16, 16],
        [16, 16, 235],
        [16, 16, 16],
    ];
    let mut image = RgbImage::from_fn(width, height, |x, _| {
        Rgb(BARS[(x * BARS.len() as u32 / width) as usize])
    });
    let band = (height / 3, height * 2 / 3);
    for y in band.0..band.1 {
        for x in 0..width {
            image.put_pixel(x, y, Rgb([0, 0, 0]));
        }
    }
    let style = TextStyle {
        font: builtin_font(),
        px: (band.1 - band.0) as f32 * 0.55,
        tabular: true,
    };
    let text_x = (width as f32 - style.width(label)).max(0.0) / 2.0;
    let baseline = (band.0 + band.1) as f32 / 2.0 + style.digit_height() / 2.0;
    style.draw(&mut image, text_x, baseline, label, [255, 255, 255]);
    Frame::new(image)
}

fn print_report(report: &Report, json: bool) -> Result<()> {
    if json {
        say!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    say!(
        "\nssp selftest {} ({} {})",
        report.version,
        report.os,
        report.arch
    );
    for d in &report.displays {
        say!(
            "\n{}  {}  firmware {}",
            d.id,
            d.model,
            d.firmware.as_deref().unwrap_or("not reported")
        );
        for c in &d.checks {
            let status = match c.status {
                Status::Pass => "PASS",
                Status::Warn => "WARN",
                Status::Fail => "FAIL",
                Status::Skip => "SKIP",
            };
            say!("  {status}  {:<12} {}", c.name, c.detail);
        }
    }
    for skipped in &report.skipped {
        say!("\nSkipped {}: {}", skipped.id, skipped.reason);
    }
    say!("\nResult: {}", if report.passed { "PASS" } else { "FAIL" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_has_panel_size_and_label() {
        let frame = test_pattern(1920, 462, "ssp selftest 1/5: still image");
        assert_eq!((frame.width(), frame.height()), (1920, 462));
        // The label is drawn in white on the black band in the middle.
        let band = (154..308).flat_map(|y| (0..1920).map(move |x| (x, y)));
        assert!(
            band.into_iter()
                .any(|(x, y)| frame.image().get_pixel(x, y).0 == [255, 255, 255])
        );
    }

    use ssp_core::testing::FakeDisplay;
    use ssp_core::{Candidate, Display, Driver, ImageFormat, PanelSpec, Rotation, UsbMatch};

    /// A small panel keeps the tests fast in debug builds.
    fn fake() -> FakeDisplay {
        FakeDisplay::with_panel(PanelSpec {
            width: 192,
            height: 46,
            rotation: Rotation::Clockwise90,
            format: ImageFormat::Jpeg,
        })
        .0
    }

    struct Dummy;

    impl Driver for Dummy {
        fn id(&self) -> &'static str {
            "dummy"
        }
        fn name(&self) -> &'static str {
            "Dummy"
        }
        fn usb_matches(&self) -> &'static [UsbMatch] {
            &[]
        }
        fn open(&self, _: &Candidate) -> ssp_core::Result<Box<dyn Display>> {
            Err(ssp_core::Error::Unsupported("open"))
        }
    }

    fn found(serial: &str) -> Found<'static> {
        Found {
            driver: &Dummy,
            candidate: Candidate {
                path: CString::new(format!("path-{serial}")).unwrap(),
                vendor_id: 1,
                product_id: 2,
                usage_page: 0,
                serial: serial.into(),
                product: String::new(),
            },
        }
    }

    fn options() -> Options {
        Options {
            display: "default".into(),
            drivers: Vec::new(),
            frames: 5,
            hold: Duration::ZERO,
            json: true,
        }
    }

    fn opened(display: FakeDisplay) -> Opened {
        let info = display.info().clone();
        Opened {
            presenter: Presenter::spawn(Box::new(display), PresenterOptions::default()),
            info,
            path: CString::new("fake").unwrap(),
            took: Duration::ZERO,
        }
    }

    fn statuses(report: &DisplayReport) -> Vec<(&'static str, &'static str)> {
        report
            .checks
            .iter()
            .map(|c| {
                let status = match c.status {
                    Status::Pass => "pass",
                    Status::Warn => "warn",
                    Status::Fail => "fail",
                    Status::Skip => "skip",
                };
                (c.name, status)
            })
            .collect()
    }

    #[test]
    fn displays_sharing_an_id_are_numbered() {
        let ids: Vec<String> = number(vec![found(""), found("A1"), found("")])
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, ["dummy", "dummy-A1", "dummy-2"]);
    }

    #[test]
    fn a_working_display_passes_every_check() {
        let target = Target {
            id: "dummy".into(),
            found: found(""),
        };
        let opened = Ok(opened(fake()));
        let report = test_display(&target, &opened, &options(), &|_| true);
        let mut checks = statuses(&report);
        // The frame rate depends on the machine; only a failure would be wrong.
        assert_ne!(checks[3], ("streaming", "fail"));
        checks.remove(3);
        assert_eq!(
            checks,
            [
                ("open", "pass"),
                ("commands", "pass"),
                ("still image", "pass"),
                ("keep-alive", "pass"),
                ("power", "pass"),
            ]
        );
    }

    #[test]
    fn a_lost_display_fails_and_skips_power() {
        let display = fake();
        let unplug = display.unplug_handle();
        let target = Target {
            id: "dummy".into(),
            found: found(""),
        };
        let opened = Ok(opened(display));
        unplug.unplug();
        let report = test_display(&target, &opened, &options(), &|_| false);
        let checks = statuses(&report);
        assert_eq!(checks[1], ("commands", "fail"));
        assert_eq!(checks[4], ("keep-alive", "fail"));
        assert_eq!(checks[5], ("power", "skip"));
        assert_eq!(report.checks[5].detail, "not run: the display was lost");
    }

    #[test]
    fn a_display_that_cannot_be_opened_reports_why() {
        let target = Target {
            id: "dummy".into(),
            found: found(""),
        };
        let report = test_display(&target, &Err("busy".into()), &options(), &|_| true);
        assert_eq!(statuses(&report), [("open", "fail")]);
        assert_eq!(report.checks[0].detail, "busy");
    }
}
