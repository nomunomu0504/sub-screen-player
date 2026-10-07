//! `ssp`: the sub-screen-player command line. `ssp serve` runs the daemon; the other
//! commands talk to it over its HTTP API.

mod client;
mod service;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ssp_server::Config;
use ssp_server::api::types::{BrightnessRequest, ClockRequest, DisplayView, PowerRequest};

use client::Client;

#[derive(Parser)]
#[command(
    name = "ssp",
    version,
    about = "Drive small USB sub-displays from your computer"
)]
struct Cli {
    /// Config file [default: the platform's config directory]
    #[arg(long, env = "SSP_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// URL of the daemon [default: from the config file]
    #[arg(long, env = "SSP_URL", global = true)]
    url: Option<String>,

    /// API token [default: from the config file]
    #[arg(long, env = "SSP_TOKEN", global = true, hide_env_values = true)]
    token: Option<String>,

    /// Display to act on (see `ssp devices`) [default: the first connected one]
    #[arg(long, short, global = true, default_value = "default")]
    display: String,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon in the foreground
    Serve {
        /// Address to listen on, overriding the config file
        #[arg(long)]
        listen: Option<std::net::SocketAddr>,
        /// Append logs to this file instead of printing them
        #[arg(long)]
        log_file: Option<PathBuf>,
        /// Start in the background without a console window (Windows autostart)
        #[arg(long, hide = true)]
        detach: bool,
    },
    /// List displays
    Devices {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show the daemon's state and frame counters
    Status,
    /// Show an image file (PNG, JPEG, GIF or WebP)
    Show {
        /// The image
        path: PathBuf,
        /// How to fit an image whose aspect ratio differs from the panel
        #[arg(long, value_enum, default_value_t = FitArg::Contain)]
        fit: FitArg,
        /// Also store it on the device so it stays after power loss (writes flash memory)
        #[arg(long)]
        persist: bool,
    },
    /// Show the built-in clock
    Clock {
        /// Hide seconds
        #[arg(long)]
        no_seconds: bool,
        /// strftime-style format of the time, e.g. "%H:%M"
        #[arg(long)]
        format: Option<String>,
        /// strftime-style format of the date line ("" hides it)
        #[arg(long)]
        date_format: Option<String>,
    },
    /// Set the backlight
    Brightness {
        /// Percent, 0-100
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    /// Switch the screen on
    On,
    /// Switch the screen off
    Off,
    /// Blank the screen
    Clear,
    /// Stop the clock or image; the screen keeps its last picture
    Stop,
    /// Start the daemon automatically at login
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Inspect or create the config file
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Register and start the daemon (launchd / systemd --user / Windows Run key)
    Install,
    /// Stop and unregister the daemon
    Uninstall,
    /// Show whether the daemon is registered
    Status,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the config file's path
    Path,
    /// Write a commented config file with the defaults
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Print the effective configuration
    Show,
}

#[derive(Clone, Copy, ValueEnum)]
enum FitArg {
    Contain,
    Cover,
    Stretch,
}

impl FitArg {
    fn name(self) -> &'static str {
        match self {
            Self::Contain => "contain",
            Self::Cover => "cover",
            Self::Stretch => "stretch",
        }
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let config_path = match &cli.config {
        Some(path) => path.clone(),
        None => default_config_path()?,
    };
    let display = format!("/displays/{}", cli.display);
    match cli.command {
        Cmd::Serve {
            listen,
            log_file,
            detach,
        } => {
            if detach {
                return detach_daemon();
            }
            let mut config = Config::load(&config_path)?;
            if let Some(listen) = listen {
                config.listen = listen;
            }
            init_logging(log_file.as_deref())?;
            tracing::info!(config = %config_path.display(), "starting sub-screen-player {}", env!("CARGO_PKG_VERSION"));
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(ssp_server::serve(config, shutdown_signal()))
        }
        Cmd::Devices { json } => devices(&cli_client(&cli.url, &cli.token, &config_path)?, json),
        Cmd::Status => status(&cli_client(&cli.url, &cli.token, &config_path)?),
        Cmd::Show { path, fit, persist } => {
            let bytes = client::read_file(&path)?;
            let query = format!("?fit={}&persist={persist}", fit.name());
            cli_client(&cli.url, &cli.token, &config_path)?
                .post_bytes(&format!("{display}/image{query}"), &bytes)
        }
        Cmd::Clock {
            no_seconds,
            format,
            date_format,
        } => {
            let request = ClockRequest {
                seconds: no_seconds.then_some(false),
                time_format: format,
                date_format,
                ..ClockRequest::default()
            };
            cli_client(&cli.url, &cli.token, &config_path)?
                .post_json(&format!("{display}/clock"), &request)
        }
        Cmd::Brightness { percent } => cli_client(&cli.url, &cli.token, &config_path)?.post_json(
            &format!("{display}/brightness"),
            &BrightnessRequest { percent },
        ),
        Cmd::On | Cmd::Off => cli_client(&cli.url, &cli.token, &config_path)?.post_json(
            &format!("{display}/power"),
            &PowerRequest {
                on: matches!(cli.command, Cmd::On),
            },
        ),
        Cmd::Clear => {
            cli_client(&cli.url, &cli.token, &config_path)?.post_empty(&format!("{display}/clear"))
        }
        Cmd::Stop => {
            cli_client(&cli.url, &cli.token, &config_path)?.post_empty(&format!("{display}/stop"))
        }
        Cmd::Service { action } => {
            let message = match action {
                ServiceCmd::Install => service::install(&config_path)?,
                ServiceCmd::Uninstall => service::uninstall()?,
                ServiceCmd::Status => service::status()?,
            };
            println!("{message}");
            Ok(())
        }
        Cmd::Config { action } => match action {
            ConfigCmd::Path => {
                println!("{}", config_path.display());
                Ok(())
            }
            ConfigCmd::Init { force } => init_config(&config_path, force),
            ConfigCmd::Show => {
                print!("{}", toml::to_string_pretty(&Config::load(&config_path)?)?);
                Ok(())
            }
        },
    }
}

fn default_config_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "sub-screen-player")
        .context("cannot determine the config directory")?;
    Ok(dirs.config_dir().join("config.toml"))
}

fn init_config(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, ssp_server::config::TEMPLATE)
        .with_context(|| format!("cannot write {}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}

/// A client for the daemon, using the config file for anything not given on the command line.
fn cli_client(url: &Option<String>, token: &Option<String>, config_path: &Path) -> Result<Client> {
    let config = Config::load(config_path)?;
    let url = url.clone().unwrap_or_else(|| {
        let mut addr = config.listen;
        if addr.ip().is_unspecified() {
            addr.set_ip(match addr.ip() {
                IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
            });
        }
        format!("http://{addr}")
    });
    Ok(Client::new(&url, token.clone().or(config.token)))
}

fn devices(client: &Client, json: bool) -> Result<()> {
    let displays = match client.displays() {
        Ok(displays) => displays,
        Err(err) if client::is_unreachable(&err) => return local_scan(client),
        Err(err) => return Err(err),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&displays)?);
        return Ok(());
    }
    if displays.is_empty() {
        println!("No displays found.");
        return Ok(());
    }
    print_table(&displays);
    Ok(())
}

fn print_table(displays: &[DisplayView]) {
    let rows: Vec<[String; 5]> = displays
        .iter()
        .map(|d| {
            [
                d.id.clone(),
                d.model.clone(),
                format!("{}x{}", d.width, d.height),
                d.content.clone(),
                if d.connected {
                    "connected".into()
                } else {
                    "unplugged".into()
                },
            ]
        })
        .collect();
    let header = ["ID", "MODEL", "SIZE", "CONTENT", "STATE"].map(String::from);
    let widths: Vec<usize> = (0..5)
        .map(|i| {
            rows.iter()
                .chain([&header])
                .map(|r| r[i].len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in [&header].into_iter().chain(&rows) {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

/// Without a daemon, at least show which devices are plugged in.
fn local_scan(client: &Client) -> Result<()> {
    println!(
        "The daemon is not running at {} (start it with `ssp serve`).",
        client.base()
    );
    let registry = ssp_server::drivers::registry();
    let found = registry.scan().context("cannot list USB devices")?;
    if found.is_empty() {
        println!("No supported displays are plugged in.");
    }
    for f in found {
        let c = &f.candidate;
        println!(
            "Plugged in: {} ({:04x}:{:04x}, serial {:?})",
            f.driver.name(),
            c.vendor_id,
            c.product_id,
            c.serial
        );
    }
    Ok(())
}

fn status(client: &Client) -> Result<()> {
    let health = client.health()?;
    println!("Daemon {} at {}", health.version, client.base());
    for d in client.displays()? {
        let state = if d.connected {
            "connected"
        } else {
            "unplugged"
        };
        println!("\n{}  {} ({state}, showing {})", d.id, d.model, d.content);
        if let Some(fw) = &d.firmware {
            println!("  firmware  {fw}");
        }
        if let Some(s) = &d.stats {
            println!(
                "  frames    {} shown, {} dropped, {} unchanged, {} received",
                s.shown, s.dropped, s.duplicates, s.submitted
            );
            println!(
                "  last      {:.1} ms encode, {:.1} ms send, {} bytes",
                s.last_encode_ms, s.last_send_ms, s.last_bytes
            );
        }
    }
    Ok(())
}

fn init_logging(log_file: Option<&Path>) -> Result<()> {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("SSP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match log_file {
        Some(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            builder
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init();
        }
        None => builder.with_writer(std::io::stderr).init(),
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Restarts `ssp serve` without `--detach` as a background process without a console.
fn detach_daemon() -> Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<_> = std::env::args_os()
        .skip(1)
        .filter(|a| a != "--detach")
        .collect();
    let mut command = std::process::Command::new(exe);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .context("cannot start the daemon in the background")?;
    Ok(())
}
