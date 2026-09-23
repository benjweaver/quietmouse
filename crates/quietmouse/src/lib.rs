//! quietmouse: offline, telemetry-free settings for Logitech HID++ mice and receivers.
//!
//! Two binaries share this library: `quietmouse`, the command line, and
//! `quietmoused`, the windowless background agent.

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
compile_error!("quietmouse supports macOS, Windows and Linux");

mod cli;
mod config;
mod connect;
mod daemon;
mod gesture;
mod hid;
mod inject;
mod keys;
mod passkey;
mod permissions;
mod service;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

use crate::config::Config;

#[derive(Parser)]
#[command(
    name = "quietmouse",
    version,
    about = "Offline, telemetry-free settings for Logitech mice and receivers"
)]
struct Cli {
    /// More detail; -vv shows every HID++ report.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List receivers and connected devices.
    List,
    /// Show a device's settings, buttons and features.
    Info {
        /// Part of the device name; defaults to the first device found.
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Set the pointer resolution.
    Dpi {
        value: u16,
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Print button, gesture and wheel events while diverting the named controls.
    Events {
        #[arg(short, long)]
        device: Option<String>,
        /// Controls to divert, e.g. `gesture,back`; `thumbwheel` diverts the thumb wheel.
        #[arg(long, value_delimiter = ',', default_value = "gesture")]
        divert: Vec<String>,
    },
    /// Pair a device with a Bolt, Unifying, Lightspeed or Nano receiver.
    Pair {
        /// Part of the receiver's kind or USB product id, e.g. `unifying` or `c52b`;
        /// needed only when more than one receiver is plugged in.
        #[arg(short, long)]
        receiver: Option<String>,
        /// How long the receiver looks for the device.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u8).range(1..))]
        seconds: u8,
    },
    /// Remove a device's pairing from a receiver.
    Unpair {
        /// Part of the device's name, or its slot number as `quietmouse list` shows it.
        device: String,
        /// Part of the receiver's kind or USB product id, e.g. `unifying` or `c52b`.
        #[arg(short, long)]
        receiver: Option<String>,
        /// Don't ask before unpairing a device the config doesn't set up.
        #[arg(short, long)]
        yes: bool,
    },
    /// Apply the config and handle buttons and gestures in this terminal until stopped.
    Run {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Start the background agent (quietmoused), which runs without a window.
    Start,
    /// Stop the background agent or a running `quietmouse run`, handing buttons back first.
    Stop,
    /// Start the background agent now and whenever you log in (per user, no admin rights).
    Autostart {
        #[arg(value_enum)]
        state: Switch,
    },
    /// Show the config path, write an example config, or check one.
    Config {
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Write an example config if none exists yet.
        #[arg(long)]
        init: bool,
        /// Check the config for mistakes.
        #[arg(long, conflicts_with = "init")]
        check: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    On,
    Off,
}

/// Log filter showing `level` and above from quietmouse, and warnings from everything else.
fn log_filter(level: &str) -> String {
    format!("warn,quietmouse={level},hidpp={level}")
}

/// Entry point of the `quietmouse` command.
pub fn cli_main() -> ExitCode {
    let cli = Cli::parse();
    let level = match (cli.verbose, &cli.command) {
        (2.., _) => "trace",
        (1, _) => "debug",
        (0, Command::Run { .. } | Command::Events { .. }) => "info",
        (0, _) => "warn",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_filter(level)))
        .format_target(false)
        .init();

    let result = match cli.command {
        Command::List => cli::list(),
        Command::Info { device } => cli::info(device.as_deref()),
        Command::Dpi { value, device } => cli::set_dpi(value, device.as_deref()),
        Command::Events { device, divert } => cli::events(device.as_deref(), &divert),
        Command::Pair { receiver, seconds } => cli::pair(receiver.as_deref(), seconds),
        Command::Unpair { device, receiver, yes } => cli::unpair(&device, receiver.as_deref(), yes),
        Command::Run { config } => run(config),
        Command::Start => service::start(),
        Command::Stop => service::stop(),
        Command::Autostart { state } => service::autostart(matches!(state, Switch::On)),
        Command::Config { config, init, check } => cli::config(config, init, check),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Entry point of `quietmoused`: the daemon with the default config, logging to a file.
pub fn agent_main() -> ExitCode {
    // Without a console there's nowhere to report not finding anywhere to log.
    let Ok(log) = service::open_log() else {
        return ExitCode::FAILURE;
    };
    // Warnings and errors only: the agent has no window, so its log is for what
    // went wrong. `quietmouse run -v` is the way to watch it working.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_filter("warn")))
        .format_target(false)
        .target(env_logger::Target::Pipe(Box::new(log)))
        .init();
    match run(None) {
        Ok(()) => ExitCode::SUCCESS,
        // Started twice, say at log-in and by hand: the running instance carries on,
        // and a success here stops launchd or systemd from retrying.
        Err(error) if error.is::<service::AlreadyRunning>() => {
            log::info!("{error}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            log::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Option<PathBuf>) -> anyhow::Result<()> {
    let path = Config::resolve_path(config)?;
    if !path.exists() {
        anyhow::bail!(
            "no config at {}; create one with `quietmouse config --init`",
            path.display()
        );
    }
    let config = Config::load(&path)?;
    log::info!("using {}", path.display());
    daemon::run(config)
}
