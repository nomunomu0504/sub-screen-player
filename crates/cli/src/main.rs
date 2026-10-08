//! `ssp`: the sub-screen-player command line. `ssp serve` runs the daemon; the other
//! commands talk to it over its HTTP API.

/// `println!` for what a command prints: when standard output is closed early
/// (`ssp devices | head -1`), the program ends quietly instead of panicking.
macro_rules! say {
    ($($arg:tt)*) => {
        $crate::write_stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}

mod client;
mod selftest;
mod service;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use ssp_server::Config;
use ssp_server::api::types::{
    BrightnessRequest, ClockRequest, DashboardRequest, DisplayView, MetricUpdate, MetricView,
    PowerRequest, WebRequest,
};
use ssp_server::config::Widget;

use client::Client;

#[derive(Parser)]
#[command(
    name = "ssp",
    version,
    about = "Drive small USB sub-displays from your computer",
    after_help = "Guide with examples: https://github.com/nomunomu0504/sub-screen-player/blob/main/docs/cli.md"
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

    /// Display to act on, as listed by `ssp devices`; `default` is the first connected one
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
        /// Use only this driver (repeatable; overrides `[drivers] enable` in the config)
        #[arg(long = "driver", value_name = "ID")]
        drivers: Vec<String>,
    },
    /// List displays
    Devices {
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Only list displays of this driver (repeatable), e.g. `--driver d92`
        #[arg(long = "driver", value_name = "ID")]
        drivers: Vec<String>,
    },
    /// Show the daemon's state and frame counters
    Status,
    /// Check that connected displays work (displays the daemon may use are skipped)
    Selftest {
        /// Only test displays of this driver (repeatable); also enables experimental drivers
        #[arg(long = "driver", value_name = "ID")]
        drivers: Vec<String>,
        /// Frames to send in the streaming check
        #[arg(long, default_value_t = 180)]
        frames: u32,
        /// Seconds to stay idle in the keep-alive check
        #[arg(long, default_value_t = 15)]
        hold: u64,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show an image (PNG, JPEG, GIF, WebP) or a video (with ffmpeg); both loop if they move
    Show {
        /// The image or video file
        path: PathBuf,
        /// How to fit an image whose aspect ratio differs from the panel
        #[arg(long, value_enum, default_value_t = FitArg::Contain)]
        fit: FitArg,
        /// Also store the image on the device so it stays after power loss (writes flash memory)
        #[arg(long)]
        persist: bool,
    },
    /// Show a web page or a local HTML file, drawn by headless Chrome
    Web {
        /// An http(s) URL, or an HTML file
        #[arg(required_unless_present = "install")]
        page: Option<String>,
        /// Reload the page every this many seconds
        #[arg(long, value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
        reload: Option<u64>,
        /// Download headless Chrome without asking, if it is not there yet
        #[arg(long, short)]
        yes: bool,
        /// Only download headless Chrome (about 100 MB)
        #[arg(long)]
        install: bool,
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
        /// Names for %a and %A, from Sunday, e.g. "日,月,火,水,木,金,土"
        #[arg(long, value_delimiter = ',')]
        weekdays: Option<Vec<String>>,
    },
    /// Show the built-in dashboard: the time, CPU, memory, network, disk and your own metrics
    Dashboard {
        /// Panels from left to right, comma-separated: clock, cpu, memory, network, disk or
        /// metric:<id>, e.g. "clock,metric:ci,cpu"
        #[arg(long, value_delimiter = ',')]
        widgets: Option<Vec<Widget>>,
    },
    /// Send figures to the dashboard's `metric:<id>` panels
    Metric {
        #[command(subcommand)]
        action: MetricCmd,
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
    /// Stop the clock, dashboard or image; the screen keeps its last picture
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

#[derive(Subcommand)]
enum MetricCmd {
    /// Create or update a metric
    Set {
        /// 1-32 of a-z, 0-9 and -; shown by the `metric:<id>` panel
        id: String,
        /// A number, also added to the graph; `-` reads it from standard input
        #[arg(long, allow_hyphen_values = true, conflicts_with = "text")]
        value: Option<String>,
        /// A short text shown instead of a number, e.g. "passing"
        #[arg(long)]
        text: Option<String>,
        /// Name shown above the value [default: the id]
        #[arg(long)]
        label: Option<String>,
        /// Shown small after the value, e.g. "%" or "failed"
        #[arg(long)]
        unit: Option<String>,
        /// The line under the value
        #[arg(long)]
        detail: Option<String>,
        /// Top of the graph [default: the largest recent value]
        #[arg(long)]
        max: Option<f64>,
        /// Seconds until the value is shown as stale [default: 300]
        #[arg(long)]
        ttl: Option<u64>,
        /// Replace the graph with these values, oldest first, comma-separated
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        series: Option<Vec<f64>>,
    },
    /// List the metrics the daemon has
    List {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a metric
    Rm {
        /// The metric's id
        id: String,
    },
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

/// Writes to standard output; see [`say!`].
fn write_stdout(text: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Err(err) = std::io::stdout().lock().write_fmt(text) {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        panic!("failed printing to stdout: {err}");
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
            drivers,
        } => {
            if detach {
                return detach_daemon();
            }
            let mut config = Config::load(&config_path)?;
            if let Some(listen) = listen {
                config.listen = listen;
            }
            if !drivers.is_empty() {
                config.drivers.enable = drivers;
            }
            init_logging(log_file.as_deref())?;
            tracing::info!(config = %config_path.display(), "starting sub-screen-player {}", env!("CARGO_PKG_VERSION"));
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(ssp_server::serve(config, shutdown_signal()))
        }
        Cmd::Devices { json, drivers } => devices(
            &cli_client(&cli.url, &cli.token, &config_path)?,
            json,
            &drivers,
        ),
        Cmd::Status => status(&cli_client(&cli.url, &cli.token, &config_path)?),
        Cmd::Selftest {
            drivers,
            frames,
            hold,
            json,
        } => {
            let options = selftest::Options {
                display: cli.display.clone(),
                drivers,
                frames,
                hold: std::time::Duration::from_secs(hold),
                json,
            };
            if !selftest::run(&cli_client(&cli.url, &cli.token, &config_path)?, &options)? {
                std::process::exit(1);
            }
            Ok(())
        }
        Cmd::Show { path, fit, persist } => {
            if persist && looks_like_a_video(&path) {
                anyhow::bail!("--persist stores a picture on the display; it does not take videos");
            }
            let query = format!("?fit={}&persist={persist}", fit.name());
            cli_client(&cli.url, &cli.token, &config_path)?
                .post_file(&format!("{display}/image{query}"), &path)
        }
        Cmd::Web {
            page,
            reload,
            yes,
            install,
        } => {
            let client = cli_client(&cli.url, &cli.token, &config_path)?;
            ensure_chrome(&client, yes || install)?;
            let Some(page) = page else {
                return Ok(());
            };
            let request = WebRequest {
                url: page_url(&page)?,
                reload,
            };
            client.post_json(&format!("{display}/web"), &request)
        }
        Cmd::Clock {
            no_seconds,
            format,
            date_format,
            weekdays,
        } => {
            let request = ClockRequest {
                seconds: no_seconds.then_some(false),
                time_format: format,
                date_format,
                weekdays,
                ..ClockRequest::default()
            };
            cli_client(&cli.url, &cli.token, &config_path)?
                .post_json(&format!("{display}/clock"), &request)
        }
        Cmd::Dashboard { widgets } => {
            let request = DashboardRequest {
                widgets,
                ..DashboardRequest::default()
            };
            cli_client(&cli.url, &cli.token, &config_path)?
                .post_json(&format!("{display}/dashboard"), &request)
        }
        Cmd::Metric { action } => metric(&cli_client(&cli.url, &cli.token, &config_path)?, action),
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
            say!("{message}");
            Ok(())
        }
        Cmd::Config { action } => match action {
            ConfigCmd::Path => {
                say!("{}", config_path.display());
                Ok(())
            }
            ConfigCmd::Init { force } => init_config(&config_path, force),
            ConfigCmd::Show => {
                say!(
                    "{}",
                    toml::to_string_pretty(&Config::load(&config_path)?)?.trim_end()
                );
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
    say!("Wrote {}", path.display());
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

fn devices(client: &Client, json: bool, drivers: &[String]) -> Result<()> {
    // Rejects driver ids that do not exist.
    ssp_server::drivers::registry(&ssp_core::DriverSelection {
        only: drivers.to_vec(),
        exclude: Vec::new(),
    })?;
    let mut displays = match client.displays() {
        Ok(displays) => displays,
        Err(err) if client::is_unreachable(&err) => return local_scan(client, drivers),
        Err(err) => return Err(err),
    };
    if !drivers.is_empty() {
        displays.retain(|d| drivers.contains(&d.driver));
    }
    if json {
        say!("{}", serde_json::to_string_pretty(&displays)?);
        return Ok(());
    }
    if displays.is_empty() {
        say!("No displays found.");
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
        say!("{}", line.join("  ").trim_end());
    }
}

/// Without a daemon, at least show which devices are plugged in.
fn local_scan(client: &Client, drivers: &[String]) -> Result<()> {
    say!(
        "The daemon is not running at {} (start it with `ssp serve`).",
        client.base()
    );
    let selection = ssp_core::DriverSelection {
        only: drivers.to_vec(),
        exclude: Vec::new(),
    };
    let registry = ssp_server::drivers::registry(&selection)?;
    let found = registry.scan().context("cannot list USB devices")?;
    if found.is_empty() {
        say!("No supported displays are plugged in.");
    }
    for f in found {
        let c = &f.candidate;
        say!(
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
    say!("Daemon {} at {}", health.version, client.base());
    for d in client.displays()? {
        let state = if d.connected {
            "connected"
        } else {
            "unplugged"
        };
        say!("\n{}  {} ({state}, showing {})", d.id, d.model, d.content);
        if let Some(fw) = &d.firmware {
            say!("  firmware  {fw}");
        }
        if let Some(s) = &d.stats {
            say!(
                "  frames    {} shown ({} in parts), {} dropped, {} unchanged, {} received",
                s.shown,
                s.partial,
                s.dropped,
                s.duplicates,
                s.submitted
            );
            say!(
                "  last      {:.1} ms encode, {:.1} ms send, {} bytes, quality {}",
                s.last_encode_ms,
                s.last_send_ms,
                s.last_bytes,
                s.quality
            );
        }
    }
    Ok(())
}

/// Makes sure the daemon has a browser for web pages, downloading one (after asking, unless
/// `yes`) if needed.
fn ensure_chrome(client: &Client, yes: bool) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    let chrome = client.chrome()?;
    if chrome.installed {
        return Ok(());
    }
    if chrome.configured {
        anyhow::bail!("[web] chrome in the daemon's config points to a missing program");
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "web pages need headless Chrome ({}): run `ssp web --install` first, or add --yes",
                ssp_server::web::DOWNLOAD_SIZE
            );
        }
        eprint!(
            "Web pages are drawn by headless Chrome, which is not installed yet.\n\
             Download it ({}, from Google's Chrome for Testing) into {}? [y/N] ",
            ssp_server::web::DOWNLOAD_SIZE,
            chrome.dir
        );
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            anyhow::bail!("not downloaded");
        }
    }
    eprintln!("Downloading headless Chrome...");
    let installed = client.install_chrome()?;
    eprintln!(
        "Installed headless Chrome {}",
        installed.version.as_deref().unwrap_or("")
    );
    Ok(())
}

/// A URL as it is, or a file as a `file://` URL.
fn page_url(page: &str) -> Result<String> {
    if let Ok(url) = url::Url::parse(page)
        && matches!(url.scheme(), "http" | "https" | "file")
    {
        return Ok(url.into());
    }
    let path = std::fs::canonicalize(page)
        .with_context(|| format!("{page} is neither a URL nor a file"))?;
    let url = url::Url::from_file_path(&path)
        .map_err(|()| anyhow::anyhow!("cannot turn {} into a URL", path.display()))?;
    Ok(url.into())
}

/// Whether the file starts like a video (the daemon decides for good).
fn looks_like_a_video(path: &Path) -> bool {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(256).read_to_end(&mut head))
        .is_ok_and(|_| {
            ssp_server::sources::video::is_video(&head) && image::guess_format(&head).is_err()
        })
}

fn metric(client: &Client, action: MetricCmd) -> Result<()> {
    match action {
        MetricCmd::Set {
            id,
            value,
            text,
            label,
            unit,
            detail,
            max,
            ttl,
            series,
        } => {
            let value = value.map(|v| parse_value(&v)).transpose()?;
            let update = MetricUpdate {
                label,
                value,
                text,
                unit,
                detail,
                max,
                ttl,
                series,
            };
            client.put_json(&format!("/metrics/{id}"), &update)
        }
        MetricCmd::List { json } => {
            let metrics = client.metrics()?;
            if json {
                say!("{}", serde_json::to_string_pretty(&metrics)?);
            } else if metrics.is_empty() {
                say!("No metrics yet. Send one with `ssp metric set <id> --value <n>`.");
            } else {
                print_metrics(&metrics);
            }
            Ok(())
        }
        MetricCmd::Rm { id } => client.delete(&format!("/metrics/{id}")),
    }
}

/// A `--value`: a number, or `-` for a number on standard input.
fn parse_value(value: &str) -> Result<f64> {
    let text = if value == "-" {
        std::io::read_to_string(std::io::stdin()).context("cannot read standard input")?
    } else {
        value.to_owned()
    };
    text.trim()
        .parse()
        .with_context(|| format!("--value must be a number, not {:?}", text.trim()))
}

fn print_metrics(metrics: &[MetricView]) {
    let rows: Vec<[String; 4]> = metrics
        .iter()
        .map(|m| {
            let value = match (&m.text, m.value) {
                (Some(text), _) => text.clone(),
                (None, Some(value)) => format!("{value} {}", m.unit).trim_end().to_owned(),
                (None, None) => "-".into(),
            };
            let age = match m.age {
                s if s < 120 => format!("{s} s ago"),
                s if s < 2 * 3600 => format!("{} min ago", s / 60),
                s => format!("{} h ago", s / 3600),
            };
            let state = if m.stale { "  (stale)" } else { "" };
            [
                m.id.clone(),
                m.label.clone(),
                value,
                format!("{age}{state}"),
            ]
        })
        .collect();
    let header = ["ID", "LABEL", "VALUE", "UPDATED"].map(String::from);
    let widths: Vec<usize> = (0..4)
        .map(|i| {
            rows.iter()
                .chain([&header])
                .map(|r| r[i].chars().count())
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
        say!("{}", line.join("  ").trim_end());
    }
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
