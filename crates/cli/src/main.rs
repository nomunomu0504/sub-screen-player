//! `ssp`: the sub-screen-player command line. `ssp serve` runs the daemon; the other
//! commands talk to it over its HTTP API.

/// `println!` for what a command prints: when standard output is closed early
/// (`ssp devices | head -1`), the program ends quietly instead of panicking.
macro_rules! say {
    ($($arg:tt)*) => {
        $crate::write_stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}

mod claude;
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
    NotifyRequest, NotifyStyle, PowerRequest, ScheduleView, WebRequest,
};
use ssp_server::config::{FitName, LayoutConfig, Widget, ZoneConfig};

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
    /// Show a message over the screen for a while, then go back to what was shown
    Notify {
        /// The message (at most 80 characters)
        #[arg(required_unless_present_any = ["stdin", "dismiss"])]
        text: Option<String>,
        /// A smaller line under the message (at most 120 characters)
        #[arg(long)]
        detail: Option<String>,
        /// How long it stays, in seconds [default: 10]
        #[arg(long = "for", value_name = "SECONDS",
              value_parser = clap::value_parser!(u64).range(1..=86400))]
        seconds: Option<u64>,
        /// Keep it until dismissed or replaced
        #[arg(long, conflicts_with = "seconds")]
        sticky: bool,
        /// Where to draw it
        #[arg(long, value_enum, default_value_t = NotifyStyleArg::Banner)]
        style: NotifyStyleArg,
        /// red, orange, yellow, green, blue, gray or #rrggbb
        #[arg(long, default_value = "blue")]
        color: String,
        /// Switch the screen on if it is off, and off again after
        #[arg(long)]
        wake: bool,
        /// Read the message from standard input: the first line, the rest as the detail. JSON
        /// from a Claude Code hook gives its message, project and last reply. Long texts are
        /// shortened; TEXT and --detail win
        #[arg(long)]
        stdin: bool,
        /// End the notification being shown
        #[arg(long, conflicts_with_all = ["text", "stdin", "detail", "seconds", "sticky", "wake"])]
        dismiss: bool,
        /// Do nothing, quietly, when the daemon is not running or no display is connected
        /// (for hooks)
        #[arg(long)]
        if_running: bool,
    },
    /// Claude Code: notifications on the display when it needs you or is done
    ClaudeCode {
        #[command(subcommand)]
        action: ClaudeCodeCmd,
    },
    /// Show several things side by side: dashboard panels, a picture or video, a web page
    Layout {
        /// Zones from left to right: clock, cpu, memory, network, disk, claude-code,
        /// metric:<id>, dashboard, image:<FILE>, video:<FILE>, web:<URL or FILE>, nothing
        #[arg(required = true, num_args = 1..)]
        zones: Vec<String>,
        /// Widths in percent, comma-separated, e.g. 30,45,25 or 30,,25; zones without one share
        /// the rest
        #[arg(long, value_delimiter = ',')]
        widths: Vec<String>,
        /// Pixels between zones
        #[arg(long, default_value_t = 0)]
        gap: u32,
        /// How pictures and videos are fitted into their zones
        #[arg(long, value_enum, default_value_t = FitArg::Contain)]
        fit: FitArg,
    },
    /// Show the schedule of the config ([[schedule]]), or pause and resume it
    Schedule {
        #[command(subcommand)]
        action: Option<ScheduleCmd>,
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
    /// Have the running daemon read the config file again and apply it
    Reload,
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

#[derive(Subcommand)]
enum ClaudeCodeCmd {
    /// Show the hooks that notify on the display, or add them to Claude Code's settings
    Hooks {
        /// Add them to Claude Code's settings.json (the old file is kept as settings.json.bak)
        #[arg(long)]
        install: bool,
        /// Remove the hooks added by --install
        #[arg(long, conflicts_with = "install")]
        uninstall: bool,
    },
}

#[derive(Subcommand)]
enum ScheduleCmd {
    /// Show the last and the next entries (the default)
    Status,
    /// Stop changing the screen at set times, until `resume`
    Pause,
    /// Apply what the schedule says now, and change at set times again
    Resume,
}

#[derive(Clone, Copy, ValueEnum)]
enum NotifyStyleArg {
    /// A band across the bottom third
    Banner,
    /// The whole panel
    Full,
}

impl From<NotifyStyleArg> for NotifyStyle {
    fn from(style: NotifyStyleArg) -> Self {
        match style {
            NotifyStyleArg::Banner => Self::Banner,
            NotifyStyleArg::Full => Self::Full,
        }
    }
}

/// Text and detail of a notification from standard input, each shortened to what the daemon
/// takes; `text` and `detail` from the command line win.
///
/// Plain text gives its first non-empty line and the other lines joined. A JSON object, as
/// Claude Code passes to hooks, gives its `message` (or `title`), and as the detail the folder
/// of `cwd` and the first line of `last_assistant_message`.
fn message_from(
    input: &str,
    text: Option<String>,
    detail: Option<String>,
) -> Result<(String, Option<String>)> {
    let (found_text, found_detail) = match serde_json::from_str(input.trim()) {
        Ok(serde_json::Value::Object(fields)) => {
            let field = |name: &str| {
                fields
                    .get(name)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            };
            let project = field("cwd")
                .and_then(|cwd| std::path::Path::new(cwd).file_name())
                .and_then(|name| name.to_str());
            let said = field("last_assistant_message")
                .and_then(|m| m.lines().map(str::trim).find(|l| !l.is_empty()));
            let detail = [project, said].into_iter().flatten().collect::<Vec<_>>();
            (
                field("message").or(field("title")).map(str::to_owned),
                detail.join(" · "),
            )
        }
        _ => {
            let mut lines = input.lines().map(str::trim).filter(|l| !l.is_empty());
            let first = lines.next().map(str::to_owned);
            (first, lines.collect::<Vec<_>>().join(" "))
        }
    };
    let Some(text) = text.or(found_text) else {
        anyhow::bail!("no message on standard input");
    };
    let detail = detail.or((!found_detail.is_empty()).then_some(found_detail));
    Ok((
        shorten(&text, ssp_server::notify::MAX_TEXT),
        detail.map(|d| shorten(&d, ssp_server::notify::MAX_DETAIL)),
    ))
}

/// `text` cut to `max` characters, ending with an ellipsis if it was longer.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
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
            // Read again, with the same options, when the config is reloaded.
            let path = config_path.clone();
            let load = move || -> Result<Config> {
                let mut config = Config::load(&path)?;
                if let Some(listen) = listen {
                    config.listen = listen;
                }
                if !drivers.is_empty() {
                    config.drivers.enable = drivers.clone();
                }
                Ok(config)
            };
            let config = load()?;
            init_logging(log_file.as_deref())?;
            tracing::info!(config = %config_path.display(), "starting sub-screen-player {}", env!("CARGO_PKG_VERSION"));
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(ssp_server::run(
                config,
                Some(std::sync::Arc::new(load)),
                shutdown_signal,
            ))
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
        Cmd::Notify {
            text,
            detail,
            seconds,
            sticky,
            style,
            color,
            wake,
            stdin,
            dismiss,
            if_running,
        } => {
            let client = cli_client(&cli.url, &cli.token, &config_path)?;
            let path = format!("{display}/notify");
            let quiet = |result: Result<()>| match result {
                Err(err) if if_running && client::is_unavailable(&err) => Ok(()),
                other => other,
            };
            if dismiss {
                return quiet(client.delete(&path));
            }
            let (text, detail) = if stdin {
                let mut input = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
                message_from(&input, text, detail)?
            } else {
                (text.expect("required by clap"), detail)
            };
            quiet(client.post_json(
                &path,
                &NotifyRequest {
                    text,
                    detail,
                    seconds,
                    sticky,
                    style: Some(style.into()),
                    color: Some(color),
                    wake,
                },
            ))
        }
        Cmd::ClaudeCode {
            action: ClaudeCodeCmd::Hooks { install, uninstall },
        } => {
            // Not resolved through links: a package manager's link stays valid after updates.
            let program = std::env::current_exe().context("cannot find this program")?;
            let path = claude::settings_path()?;
            if !install && !uninstall {
                say!(
                    "Add these hooks to {} (or run `ssp claude-code hooks --install`):\n\n{}",
                    path.display(),
                    claude::snippet(&program)
                );
                return Ok(());
            }
            let mut settings = claude::read(&path)?;
            let changed = if install {
                claude::add(&mut settings, &program)
            } else {
                claude::remove(&mut settings)
            };
            if !changed {
                say!("Nothing to change in {}.", path.display());
                return Ok(());
            }
            claude::write(&path, &settings)?;
            let (what, then) = if install {
                ("Added the hooks to", "New Claude Code sessions use them.")
            } else {
                (
                    "Removed the hooks from",
                    "New Claude Code sessions do without them.",
                )
            };
            say!(
                "{what} {} (the old file is settings.json.bak).\n{then}",
                path.display()
            );
            Ok(())
        }
        Cmd::Layout {
            zones,
            widths,
            gap,
            fit,
        } => {
            if widths.len() > zones.len() {
                anyhow::bail!("more --widths than zones");
            }
            let widths = widths
                .iter()
                .map(|w| percent(w))
                .collect::<Result<Vec<_>>>()?;
            let zones = zones
                .iter()
                .enumerate()
                .map(|(i, zone)| zone_config(zone, widths.get(i).copied().flatten(), fit))
                .collect::<Result<Vec<_>>>()?;
            let layout = LayoutConfig {
                zones,
                gap,
                ..LayoutConfig::default()
            };
            cli_client(&cli.url, &cli.token, &config_path)?
                .post_json(&format!("{display}/layout"), &layout)
        }
        Cmd::Schedule { action } => {
            let client = cli_client(&cli.url, &cli.token, &config_path)?;
            match action.unwrap_or(ScheduleCmd::Status) {
                ScheduleCmd::Status => {
                    print_schedule(&client.schedule()?, "");
                    Ok(())
                }
                ScheduleCmd::Pause => client.post_empty("/schedule/pause"),
                ScheduleCmd::Resume => client.post_empty("/schedule/resume"),
            }
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
            ConfigCmd::Reload => reload_config(&cli.url, &cli.token, &config_path),
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
/// `ssp config reload`: has the daemon apply its config again, then waits until it answers with
/// it. The daemon is reached as by other commands (the config file, `--url`, `--token`); after
/// changing `listen` or `token` in the file, `--url` and `--token` give the running daemon's.
fn reload_config(url: &Option<String>, token: &Option<String>, config_path: &Path) -> Result<()> {
    let before = cli_client(url, token, config_path)?;
    let started = match before.health() {
        Ok(health) => health.started,
        Err(err) if client::is_unreachable(&err) || client::is_unauthorized(&err) => {
            anyhow::bail!(
                "{err:#}\nIf you changed `listen` or `token` in the config file, give the running \
                 daemon's with --url and --token, e.g. \
                 `ssp --url http://127.0.0.1:{} config reload`.",
                ssp_server::config::DEFAULT_PORT
            );
        }
        Err(err) => return Err(err),
    };
    let reloaded = before.reload()?;
    // The new daemon: on the host used so far, at the new port, with the token of the edited
    // file or the one given. Or, if it went back to the config before, where it was.
    let port = reloaded
        .listen
        .parse::<std::net::SocketAddr>()
        .map(|a| a.port())
        .context("the daemon answered an invalid address")?;
    let base = with_port(before.base(), port);
    let mut tokens = vec![token.clone()];
    if let Ok(config) = Config::load(config_path) {
        tokens.push(config.token);
    }
    tokens.dedup();
    let mut candidates: Vec<Client> = tokens.into_iter().map(|t| Client::new(&base, t)).collect();
    candidates.push(before);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        for client in &candidates {
            let Ok(health) = client.health() else {
                continue;
            };
            if health.started == started {
                continue;
            }
            if let Some(error) = health.reload_error {
                anyhow::bail!(
                    "the daemon could not start with the new config and went back to the one \
                     before: {error}"
                );
            }
            say!(
                "Reloaded the config; the daemon answers at {}.",
                client.base()
            );
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!(
                "the daemon did not answer within 30 seconds after reloading its config; see its log"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// `base` (`http://host:port`) with another port; as it is when it names none.
fn with_port(base: &str, port: u16) -> String {
    match base.rsplit_once(':') {
        Some((host, old)) if !old.is_empty() && old.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{host}:{port}")
        }
        _ => base.to_owned(),
    }
}

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

/// Prints the schedule, each line after `indent`.
fn print_schedule(schedule: &ScheduleView, indent: &str) {
    if schedule.entries == 0 {
        say!("{indent}No schedule: add [[schedule]] entries to the config.");
        return;
    }
    let paused = if schedule.paused {
        " (paused: `ssp schedule resume`)"
    } else {
        ""
    };
    say!("{indent}schedule  {} entries{paused}", schedule.entries);
    for (name, event) in [("last", &schedule.last), ("next", &schedule.next)] {
        if let Some(event) = event {
            // "2026-10-09T19:00:00+09:00" -> "10-09 19:00"
            let at = event.at.get(5..16).unwrap_or(&event.at).replace('T', " ");
            say!("{indent}  {name}    {at}  {}", event.does.join("; "));
        }
    }
}

fn status(client: &Client) -> Result<()> {
    let health = client.health()?;
    say!("Daemon {} at {}", health.version, client.base());
    // Daemons before 0.5.0 have no schedule.
    if let Ok(schedule) = client.schedule()
        && schedule.entries > 0
    {
        print_schedule(&schedule, "");
    }
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
        if let Some(n) = &d.notification {
            let left = n
                .seconds_left
                .map_or_else(|| "until dismissed".to_owned(), |s| format!("{s} s left"));
            say!("  notice    {:?} ({left})", n.text);
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

/// A width of `--widths` as a share: `"30"` is 0.3, `""` none.
fn percent(width: &str) -> Result<Option<f64>> {
    if width.trim().is_empty() {
        return Ok(None);
    }
    match width.trim().parse::<f64>() {
        Ok(w) if w > 0.0 && w <= 100.0 => Ok(Some(w / 100.0)),
        _ => anyhow::bail!("--widths are percent, more than 0 and at most 100, not {width:?}"),
    }
}

/// A zone of `ssp layout`: a panel name, or `image:`, `video:` or `web:` with a file or URL.
/// Files are passed to the daemon as absolute paths.
fn zone_config(zone: &str, width: Option<f64>, fit: FitArg) -> Result<ZoneConfig> {
    let mut config = ZoneConfig {
        show: zone.to_owned(),
        width,
        image: None,
        url: None,
        reload: None,
        fit: None,
    };
    match zone.split_once(':') {
        Some(("image" | "video", file)) => {
            let path =
                std::fs::canonicalize(file).with_context(|| format!("cannot find {file}"))?;
            config.show = "image".into();
            config.image = Some(path);
            config.fit = Some(fit.into());
        }
        Some(("web", page)) => {
            config.show = "web".into();
            config.url = Some(page_url(page)?);
        }
        _ => {}
    }
    Ok(config)
}

impl From<FitArg> for FitName {
    fn from(fit: FitArg) -> Self {
        match fit {
            FitArg::Contain => Self::Contain,
            FitArg::Cover => Self::Cover,
            FitArg::Stretch => Self::Stretch,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_daemon_to_its_new_port() {
        assert_eq!(
            with_port("http://127.0.0.1:7920", 7922),
            "http://127.0.0.1:7922"
        );
        assert_eq!(with_port("http://[::1]:7920", 80), "http://[::1]:80");
        assert_eq!(
            with_port("https://ssp.example", 7922),
            "https://ssp.example"
        );
    }

    #[test]
    fn reads_a_notification_from_standard_input() {
        let read = |input: &str| message_from(input, None, None);
        let (text, detail) = read("\n  Claude needs your permission  \n\nto use Bash\n").unwrap();
        assert_eq!(text, "Claude needs your permission");
        assert_eq!(detail.as_deref(), Some("to use Bash"));
        let (text, detail) = read("done").unwrap();
        assert_eq!((text.as_str(), detail), ("done", None));
        assert!(read(" \n").is_err());
        // The command line wins.
        let given = message_from(
            "from stdin\nmore",
            Some("given".into()),
            Some("also".into()),
        );
        let (text, detail) = given.unwrap();
        assert_eq!((text.as_str(), detail.as_deref()), ("given", Some("also")));

        let long = "x".repeat(200);
        let (text, detail) = read(&format!("{long}\n{long}")).unwrap();
        assert_eq!(text.chars().count(), ssp_server::notify::MAX_TEXT);
        assert!(text.ends_with('…'));
        let detail = detail.unwrap();
        assert_eq!(detail.chars().count(), ssp_server::notify::MAX_DETAIL);
    }

    #[test]
    fn reads_layout_widths() {
        assert_eq!(percent("30").unwrap(), Some(0.3));
        assert_eq!(percent("").unwrap(), None);
        assert!(percent("0").is_err() && percent("120").is_err() && percent("x").is_err());
    }

    #[test]
    fn reads_layout_zones() {
        let panel = zone_config("metric:ci", Some(0.25), FitArg::Contain).unwrap();
        assert_eq!(
            (panel.show.as_str(), panel.width, panel.image),
            ("metric:ci", Some(0.25), None)
        );
        let web = zone_config("web:https://example.com/a", None, FitArg::Contain).unwrap();
        assert_eq!(
            (web.show.as_str(), web.url.as_deref()),
            ("web", Some("https://example.com/a"))
        );
        let here = std::env::current_dir().unwrap();
        let video = zone_config("video:.", None, FitArg::Cover).unwrap();
        assert_eq!(video.show, "image");
        assert_eq!(
            video.image.as_deref(),
            Some(here.canonicalize().unwrap().as_path())
        );
        assert_eq!(video.fit, Some(FitName::Cover));
        assert!(zone_config("image:/no/such/file.png", None, FitArg::Contain).is_err());
    }

    #[test]
    fn reads_what_claude_code_passes_to_hooks() {
        let notification = r#"{"session_id": "abc", "cwd": "/Users/me/projects/shop",
            "hook_event_name": "Notification", "message": "Claude needs your permission to use Bash",
            "title": "Permission needed", "notification_type": "permission_prompt"}"#;
        let (text, detail) = message_from(notification, None, None).unwrap();
        assert_eq!(text, "Claude needs your permission to use Bash");
        assert_eq!(detail.as_deref(), Some("shop"));

        let stop = r#"{"cwd": "/Users/me/projects/shop", "hook_event_name": "Stop",
            "last_assistant_message": "\nI've finished the refactoring.\nDetails follow."}"#;
        let (text, detail) = message_from(stop, Some("Claude Code is done".into()), None).unwrap();
        assert_eq!(text, "Claude Code is done");
        assert_eq!(
            detail.as_deref(),
            Some("shop · I've finished the refactoring.")
        );
        assert!(
            message_from(stop, None, None).is_err(),
            "Stop has no message"
        );
    }
}
