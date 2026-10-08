//! Draws a built-in screen into a PNG file, without a display or the daemon.
//!
//!     cargo run -p ssp-server --example render -- clock clock.png
//!     cargo run -p ssp-server --example render -- dashboard dashboard.png --config my.toml
//!     cargo run -p ssp-server --example render -- dashboard dashboard.png --seconds 60
//!
//! The screen is drawn at the D92's size (1920x462) with the `[clock]` and `[dashboard]` sections
//! of the given config file, or the defaults. Useful for trying a look or making documentation.
//! The dashboard shows this computer's figures, sampled once a second for `--seconds` (default
//! 8) so the graphs have something to show; 60 fills them.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, bail};
use ssp_core::Frame;
use ssp_server::config::Config;
use ssp_server::sources::Content;

fn main() -> anyhow::Result<()> {
    const USAGE: &str =
        "usage: render <clock|dashboard> <out.png> [--config <config.toml>] [--seconds <n>]";
    let mut args = std::env::args().skip(1);
    let (Some(screen), Some(out)) = (args.next(), args.next()) else {
        bail!(USAGE);
    };
    let (mut config, mut seconds) = (Config::default(), 8);
    while let Some(flag) = args.next() {
        let value = args.next().context(USAGE)?;
        match flag.as_str() {
            "--config" => config = Config::load(value.as_ref())?,
            "--seconds" => seconds = value.parse().context("--seconds takes a number")?,
            _ => bail!(USAGE),
        }
    }
    let out = PathBuf::from(out);
    let (content, samples) = match screen.as_str() {
        "clock" => (Content::Clock(config.clock), 1),
        "dashboard" => (
            Content::Dashboard(config.dashboard, config.clock),
            seconds.max(1),
        ),
        other => bail!("unknown screen {other:?} (try clock or dashboard)"),
    };
    let mut source = content.source().context("this screen draws nothing")?;
    let mut frame = Frame::blank(1920, 462);
    for i in 0..samples {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        source.render(&mut frame);
    }
    frame
        .into_image()
        .save(&out)
        .with_context(|| format!("cannot write {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
