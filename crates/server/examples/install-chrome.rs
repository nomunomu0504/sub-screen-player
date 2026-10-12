//! Downloads headless Chrome into a folder as `ssp web --install` does, and prints the path of
//! the program. CI runs the web page tests with it: the browser people get by default.
//!
//!     cargo run -p ssp-server --example install-chrome -- <folder>

use std::path::PathBuf;

use anyhow::{Context, anyhow};
use ssp_server::web::Web;

fn main() -> anyhow::Result<()> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: install-chrome <folder>")?;
    let (program, version) = Web::with_dir(None, dir).install().map_err(|e| anyhow!(e))?;
    eprintln!("headless Chrome {version}");
    println!("{}", program.display());
    Ok(())
}
