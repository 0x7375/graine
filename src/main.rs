mod analytics;
mod config;
mod dashboard;
mod db;
mod engine;
mod model;
mod qbittorrent;
mod server;
mod storage;
mod util;

use anyhow::{Context, Result, bail};
use config::Config;
use db::Database;
use std::{env, fs, path::PathBuf};
use tracing_subscriber::EnvFilter;

const DEFAULT_CONFIG_PATH: &str = "/etc/graine/config.toml";

struct Cli {
    config_path: PathBuf,
    password_file: Option<PathBuf>,
    once: bool,
    dry: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = parse_args()?;
    let config_text = fs::read_to_string(&cli.config_path)
        .with_context(|| format!("cannot read config {}", cli.config_path.display()))?;
    let mut config: Config = toml::from_str(&config_text)
        .with_context(|| format!("invalid TOML config {}", cli.config_path.display()))?;
    if let Some(password_file) = cli.password_file {
        config.password_file = Some(password_file);
    }
    config.validate().map_err(anyhow::Error::msg)?;

    let database = Database::new(
        config.database_path.clone(),
        config.legacy_data_dir.clone(),
    );
    database.initialize()?;
    engine::seed_dashboard_from_legacy(&config, &database)?;

    if cli.once {
        let password = if config.fixture_path.is_some() {
            None
        } else {
            Some(load_password(&config)?)
        };
        let dashboard = engine::run_management_cycle(&config, &database, password.as_deref(), cli.dry)?;
        if dashboard.run.result == "over_limit" {
            std::process::exit(2);
        }
        return Ok(());
    }

    server::run(config, database, cli.dry).await
}

pub(crate) fn load_password(config: &Config) -> Result<String> {
    let password = if let Some(path) = &config.password_file {
        fs::read_to_string(path)
            .with_context(|| format!("cannot read qBittorrent password file {}", path.display()))?
    } else {
        env::var("QBIT_PW").context(
            "qBittorrent password is missing; set password_file in config, pass --password-file, or set QBIT_PW",
        )?
    };
    let password = password.trim_end_matches(|character| character == '\r' || character == '\n').to_string();
    if password.is_empty() {
        bail!("qBittorrent password is empty");
    }
    Ok(password)
}

fn parse_args() -> Result<Cli> {
    let mut cli = Cli {
        config_path: PathBuf::from(DEFAULT_CONFIG_PATH),
        password_file: None,
        once: false,
        dry: false,
    };
    let mut args = env::args().skip(1);
    let mut positional_config_seen = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" => {
                cli.config_path = PathBuf::from(
                    args.next()
                        .context("--config requires a path")?,
                );
            }
            "--password-file" => {
                cli.password_file = Some(PathBuf::from(
                    args.next()
                        .context("--password-file requires a path")?,
                ));
            }
            "--once" => cli.once = true,
            "--dry" => cli.dry = true,
            "--help" | "-h" => {
                println!(
                    "Usage: graine [--config PATH] [--password-file PATH] [--once] [--dry]\n\n\
                     Defaults to {DEFAULT_CONFIG_PATH}. Without --once, starts the web server\n\
                     and runs the management cycle immediately, then every interval_seconds.\n\
                     --dry forces scheduled cycles to preview without deleting or persisting state.\n\
                     Configure fixture_path to run against a local read-only qBittorrent snapshot."
                );
                std::process::exit(0);
            }
            value if value.starts_with('-') => bail!("unknown option: {value}"),
            value if !positional_config_seen => {
                // Preserve the previous `graine config.toml [--dry]` invocation as a convenience.
                cli.config_path = PathBuf::from(value);
                positional_config_seen = true;
            }
            value => bail!("unexpected argument: {value}"),
        }
    }

    Ok(cli)
}
