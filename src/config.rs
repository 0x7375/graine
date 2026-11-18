use serde::Deserialize;
use std::path::PathBuf;

fn default_username() -> String {
    "admin".to_string()
}

fn default_high() -> f64 {
    0.85
}

fn default_low() -> f64 {
    0.80
}

fn default_grace() -> u64 {
    7
}

fn default_hide_unconfigured_trackers() -> bool {
    true
}

fn default_interval_seconds() -> u64 {
    60 * 60
}

fn default_management_enabled() -> bool {
    true
}

fn default_listen() -> String {
    "127.0.0.1:3000".to_string()
}

fn default_database_path() -> PathBuf {
    PathBuf::from("/var/lib/graine/graine.sqlite3")
}

fn default_legacy_data_dir() -> PathBuf {
    PathBuf::from("/var/lib/graine")
}

/// A logical private-tracker group. Multiple announce hosts can share one dashboard identity.
#[derive(Clone, Deserialize)]
pub(crate) struct TrackerGroup {
    pub(crate) name: String,
    pub(crate) hosts: Vec<String>,
    pub(crate) priority: i32,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) trackers: Vec<TrackerGroup>,
    #[serde(default = "default_hide_unconfigured_trackers")]
    pub(crate) hide_unconfigured_trackers: bool,
    pub(crate) url: String,
    /// qBittorrent WebUI login name.
    #[serde(default = "default_username")]
    pub(crate) username: String,
    /// Filesystem containing downloads and the media library.
    pub(crate) disk_path: String,
    /// Start evicting above this fraction of filesystem usage.
    #[serde(default = "default_high")]
    pub(crate) high_watermark: f64,
    /// Stop evicting once filesystem usage reaches this fraction.
    #[serde(default = "default_low")]
    pub(crate) low_watermark: f64,
    /// One grace period shared by every managed torrent category.
    #[serde(default = "default_grace")]
    pub(crate) grace_days: u64,
    /// qBittorrent categories managed by Graine. A category's `.cross` variant is included too.
    pub(crate) managed_categories: Vec<String>,

    /// Management cycle period. Defaults to one hour.
    #[serde(default = "default_interval_seconds")]
    pub(crate) interval_seconds: u64,
    /// Axum bind address.
    #[serde(default = "default_listen")]
    pub(crate) listen: String,
    /// SQLite database path.
    #[serde(default = "default_database_path")]
    pub(crate) database_path: PathBuf,
    /// Directory containing analytics.json and the legacy JSONL files to import once.
    #[serde(default = "default_legacy_data_dir")]
    pub(crate) legacy_data_dir: PathBuf,
    /// Path to a file containing the qBittorrent password. Can be overridden by --password-file.
    #[serde(default)]
    pub(crate) password_file: Option<PathBuf>,
    /// Enable the periodic management worker. Disable this for dashboard-only development.
    #[serde(default = "default_management_enabled")]
    pub(crate) management_enabled: bool,
    /// Optional local qBittorrent snapshot for offline development and tests.
    /// Fixture mode always forces dry-run and refuses torrent deletion.
    #[serde(default)]
    pub(crate) fixture_path: Option<PathBuf>,
    /// Run management in preview mode. This does not persist analytics/history or delete torrents.
    #[serde(default)]
    pub(crate) dry_run: bool,
    /// Directory containing Vite's production build. In development Vite serves the frontend.
    #[serde(default = "default_web_dir")]
    pub(crate) web_dir: PathBuf,
}

fn default_web_dir() -> PathBuf {
    PathBuf::from("web/dist")
}

impl Config {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.low_watermark >= self.high_watermark
            || !(0.0..=1.0).contains(&self.low_watermark)
            || !(0.0..=1.0).contains(&self.high_watermark)
        {
            return Err("need 0 <= low_watermark < high_watermark <= 1".to_string());
        }

        if self.managed_categories.is_empty() {
            return Err("managed_categories must contain at least one qBittorrent category".to_string());
        }

        if self
            .managed_categories
            .iter()
            .any(|category| category.trim().is_empty())
        {
            return Err("managed_categories cannot contain empty names".to_string());
        }

        if self.interval_seconds == 0 {
            return Err("interval_seconds must be greater than zero".to_string());
        }

        if self.listen.trim().is_empty() {
            return Err("listen cannot be empty".to_string());
        }

        if self.username.trim().is_empty() {
            return Err("username cannot be empty".to_string());
        }

        if self.url.trim().is_empty() {
            return Err("url cannot be empty".to_string());
        }

        if self.disk_path.trim().is_empty() {
            return Err("disk_path cannot be empty".to_string());
        }

        Ok(())
    }
}

pub(crate) fn base_category(category: &str) -> &str {
    category.strip_suffix(".cross").unwrap_or(category)
}

pub(crate) fn is_managed_category(cfg: &Config, category: &str) -> bool {
    let base = base_category(category);
    cfg.managed_categories
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(base))
}

pub(crate) fn tracker_host(tracker: &str) -> String {
    let without_scheme = tracker
        .split_once("://")
        .map_or(tracker, |(_, rest)| rest);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority.split(':').next().unwrap_or(authority)
    };

    if host.is_empty() {
        "unknown".to_string()
    } else {
        host.to_ascii_lowercase()
    }
}

pub(crate) fn tracker_group<'a>(cfg: &'a Config, tracker: &str) -> Option<&'a TrackerGroup> {
    let host = tracker_host(tracker);

    cfg.trackers.iter().find(|group| {
        group
            .hosts
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&host))
    })
}

pub(crate) fn tracker_label(cfg: &Config, tracker: &str) -> String {
    tracker_group(cfg, tracker)
        .map_or_else(|| tracker_host(tracker), |group| group.name.clone())
}

pub(crate) fn tracker_priority(cfg: &Config, tracker: &str) -> i32 {
    tracker_group(cfg, tracker).map_or(0, |group| group.priority)
}

pub(crate) fn tracker_is_visible(cfg: &Config, tracker: &str) -> bool {
    !cfg.hide_unconfigured_trackers || tracker_group(cfg, tracker).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_can_be_configured() {
        let config: Config = toml::from_str(
            r#"
            url = "http://localhost:8080"
            username = "qb-user"
            disk_path = "/data"
            managed_categories = ["radarr"]
            "#,
        )
        .unwrap();
        assert_eq!(config.username, "qb-user");
    }

    #[test]
    fn management_interval_defaults_to_one_hour() {
        let config: Config = toml::from_str(
            r#"
            url = "http://localhost:8080"
            disk_path = "/data/shared"
            managed_categories = ["radarr"]
            "#,
        )
        .expect("minimal config should deserialize with defaults");

        assert_eq!(config.interval_seconds, 3600);
        assert!(config.management_enabled);
        assert_eq!(config.username, "admin");
    }
}
