//! `ssp selftest`: drives every connected display through a fixed sequence and reports what
//! worked. It talks to the hardware directly, so the daemon must not be running.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use image::{Rgb, RgbImage};
use serde::Serialize;
use ssp_core::{Found, Frame, Presenter, PresenterOptions, StopAction, hid};
use ssp_server::text::{TextStyle, builtin_font};

use crate::client::{self, Client};

/// Settings of one run.
pub struct Options {
    /// Only test the display with this id (`default`: all of them).
    pub display: String,
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

pub fn run(client: &Client, options: &Options) -> Result<bool> {
    match client.health() {
        Err(err) if client::is_unreachable(&err) => {}
        _ => bail!(
            "the daemon is running at {} and holds the displays.\n\
             Stop it first (Ctrl-C, or `ssp service uninstall`), then run the selftest again.",
            client.base()
        ),
    }

    let registry = ssp_server::drivers::registry();
    let found: Vec<Found<'_>> = registry
        .scan()?
        .into_iter()
        .filter(|f| options.display == "default" || candidate_id(f) == options.display)
        .collect();

    let mut report = Report {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        passed: false,
        displays: Vec::new(),
    };
    if found.is_empty() {
        if !options.json {
            println!("No supported display found.");
            println!("{}", no_display_hint());
        }
        print_report(&report, options.json)?;
        return Ok(false);
    }
    for f in &found {
        if !options.json {
            println!("Testing {} ({}) ...", f.driver.name(), candidate_id(f));
        }
        report.displays.push(test_display(f, options));
    }
    report.passed = report
        .displays
        .iter()
        .all(|d| d.checks.iter().all(|c| c.status != Status::Fail));
    print_report(&report, options.json)?;
    Ok(report.passed)
}

fn candidate_id(found: &Found<'_>) -> String {
    match found.candidate.serial.as_str() {
        "" => found.driver.id().to_string(),
        serial => format!("{}-{serial}", found.driver.id()),
    }
}

fn no_display_hint() -> &'static str {
    if cfg!(target_os = "linux") {
        "Check the cable, and that the udev rule from contrib/linux is installed (then replug)."
    } else {
        "Check the cable, and that no other program (such as the vendor app) uses the display."
    }
}

fn test_display(found: &Found<'_>, options: &Options) -> DisplayReport {
    let mut report = DisplayReport {
        id: candidate_id(found),
        driver: found.driver.id(),
        model: found.driver.name().to_string(),
        serial: found.candidate.serial.clone(),
        firmware: None,
        checks: Vec::new(),
    };

    // Open.
    let started = Instant::now();
    let display = match found.driver.open(&found.candidate) {
        Ok(display) => display,
        Err(err) => {
            let mut detail = err.to_string();
            if cfg!(target_os = "linux") && detail.to_lowercase().contains("permission") {
                detail.push_str(" (install the udev rule from contrib/linux and replug)");
            }
            report.checks.push(Check::new("open", Status::Fail, detail));
            return report;
        }
    };
    let info = display.info().clone();
    report.model = info.model.clone();
    report.firmware = info.firmware.clone();
    report.checks.push(Check::new(
        "open",
        Status::Pass,
        format!(
            "{} ms, firmware {}",
            started.elapsed().as_millis(),
            info.firmware.as_deref().unwrap_or("not reported")
        ),
    ));

    let caps = info.capabilities.clone();
    let (width, height) = (info.panel.width, info.panel.height);
    let presenter = Presenter::spawn(
        display,
        PresenterOptions {
            max_fps: caps.max_fps,
            skip_duplicates: false,
            ..Default::default()
        },
    );
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
        &presenter,
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
    let still_there = hid::enumerate()
        .map(|list| list.iter().any(|c| c.path == found.candidate.path))
        .unwrap_or(false);
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
    if caps.power && presenter.is_running() {
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
    } else {
        checks.push(Check::new("power", Status::Skip, "not supported"));
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
    let _ = presenter.stop(StopAction::Leave);
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
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!(
        "\nssp selftest {} ({} {})",
        report.version, report.os, report.arch
    );
    for d in &report.displays {
        println!(
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
            println!("  {status}  {:<12} {}", c.name, c.detail);
        }
    }
    println!("\nResult: {}", if report.passed { "PASS" } else { "FAIL" });
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
}
