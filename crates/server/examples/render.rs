//! Draws a built-in screen into a PNG file, without a display or the daemon.
//!
//!     cargo run -p ssp-server --example render -- clock clock.png
//!     cargo run -p ssp-server --example render -- dashboard dashboard.png --config my.toml
//!     cargo run -p ssp-server --example render -- dashboard dashboard.png --seconds 60
//!     cargo run -p ssp-server --example render -- dashboard metrics.png --metrics metrics.json
//!     cargo run -p ssp-server --example render -- clock notify.png --notify "Build finished"
//!     cargo run -p ssp-server --example render -- layout layout.png --config layout.toml
//!
//! The screen is drawn at the D92's size (1920x462) with the `[clock]`, `[dashboard]` and
//! `[layout]` sections of the given config file, or the defaults. Useful for trying a look or making documentation.
//! The dashboard shows this computer's figures, sampled once a second for `--seconds` (default
//! 8) so the graphs have something to show; 60 fills them. `--metrics` takes a JSON object of
//! `{"<id>": <body of PUT /api/v1/metrics/<id>>}` for `metric:<id>` panels. `--notify` draws a
//! notification over the screen; `--detail`, `--color` and `--style` (`banner` or `full`) set it
//! up as `ssp notify` does.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use ssp_core::Frame;
use ssp_server::config::Config;
use ssp_server::metrics::{MetricUpdate, Metrics};
use ssp_server::notify::{self, Notification, Overlay, Style};
use ssp_server::sources::Content;

fn main() -> anyhow::Result<()> {
    const USAGE: &str = "usage: render <clock|dashboard|layout> <out.png> [--config <config.toml>] \
                         [--seconds <n>] [--metrics <metrics.json>] [--notify <text> \
                         [--detail <text>] [--color <color>] [--style banner|full]]";
    let mut args = std::env::args().skip(1);
    let (Some(screen), Some(out)) = (args.next(), args.next()) else {
        bail!(USAGE);
    };
    let (mut config, mut seconds, metrics) = (Config::default(), 8, Metrics::default());
    let mut notification: Option<Notification> = None;
    let mut detail = None;
    let mut color = notify::DEFAULT_COLOR.to_owned();
    let mut style = Style::Banner;
    while let Some(flag) = args.next() {
        let value = args.next().context(USAGE)?;
        match flag.as_str() {
            "--config" => config = Config::load(value.as_ref())?,
            "--seconds" => seconds = value.parse().context("--seconds takes a number")?,
            "--metrics" => {
                let text = std::fs::read_to_string(&value)
                    .with_context(|| format!("cannot read {value}"))?;
                let updates: std::collections::BTreeMap<String, MetricUpdate> =
                    serde_json::from_str(&text).with_context(|| format!("cannot parse {value}"))?;
                for (id, update) in updates {
                    metrics.set(&id, update).map_err(anyhow::Error::msg)?;
                }
            }
            "--notify" => {
                notification = Some(Notification {
                    text: value,
                    detail: None,
                    style: Style::Banner,
                    color: [0; 3],
                    duration: None,
                    wake: false,
                })
            }
            "--detail" => detail = Some(value),
            "--color" => color = value,
            "--style" => {
                style = match value.as_str() {
                    "banner" => Style::Banner,
                    "full" => Style::Full,
                    _ => bail!("--style takes banner or full"),
                }
            }
            _ => bail!(USAGE),
        }
    }
    let notification = match notification {
        Some(n) => Some(Notification {
            detail,
            style,
            color: notify::parse_color(&color).context("unknown --color")?,
            ..n
        }),
        None => None,
    };
    let out = PathBuf::from(out);
    let (content, samples) = match screen.as_str() {
        "clock" => (Content::Clock(config.clock), 1),
        "dashboard" => (
            Content::Dashboard(config.dashboard, config.clock, metrics),
            seconds.max(1),
        ),
        // Zones draw on threads of their own; give them a few rounds.
        "layout" => (
            Content::layout(&config.layout, &config, &metrics).map_err(anyhow::Error::msg)?,
            seconds.max(3),
        ),
        other => bail!("unknown screen {other:?} (try clock, dashboard or layout)"),
    };
    let mut source = content.source().context("this screen draws nothing")?;
    let mut frame = Frame::blank(1920, 462);
    for i in 0..samples {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        source.render(&mut frame);
    }
    if let Some(notification) = notification {
        notification.validate().map_err(anyhow::Error::msg)?;
        let overlay = Overlay::new();
        overlay.show(notification);
        frame = overlay.pass(frame);
    }
    frame
        .into_image()
        .save(&out)
        .with_context(|| format!("cannot write {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
