use serde::Deserialize;

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

/// A logical private-tracker group. Multiple announce hosts can share one dashboard identity.
#[derive(Clone, Deserialize)]
pub(crate) struct TrackerGroup {
    pub(crate) name: String,
    pub(crate) hosts: Vec<String>,
    pub(crate) priority: i32,
}

#[derive(Deserialize)]
pub(crate) struct Config {
    #[serde(default)]
    pub(crate) trackers: Vec<TrackerGroup>,
    #[serde(default = "default_hide_unconfigured_trackers")]
    pub(crate) hide_unconfigured_trackers: bool,
    pub(crate) url: String,
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
