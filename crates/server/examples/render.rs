//! Draws a built-in screen into a PNG file, without a display or the daemon.
//!
//!     cargo run -p ssp-server --example render -- clock clock.png
//!     cargo run -p ssp-server --example render -- clock clock.png --config my.toml
//!
//! The screen is drawn at the D92's size (1920x462) with the `[clock]` (and other) sections of
//! the given config file, or the defaults. Useful for trying a look or making documentation.

use std::path::PathBuf;

use anyhow::{Context, bail};
use ssp_core::Frame;
use ssp_server::config::Config;
use ssp_server::sources::Content;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (screen, out, config) = match args.as_slice() {
        [screen, out] => (screen, PathBuf::from(out), Config::default()),
        [screen, out, flag, path] if flag == "--config" => {
            let config = Config::load(path.as_ref())?;
            (screen, PathBuf::from(out), config)
        }
        _ => bail!("usage: render <clock> <out.png> [--config <config.toml>]"),
    };
    let content = match screen.as_str() {
        "clock" => Content::Clock(config.clock),
        other => bail!("unknown screen {other:?} (try clock)"),
    };
    let mut source = content.source().context("this screen draws nothing")?;
    let mut frame = Frame::blank(1920, 462);
    source.render(&mut frame);
    frame
        .into_image()
        .save(&out)
        .with_context(|| format!("cannot write {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
