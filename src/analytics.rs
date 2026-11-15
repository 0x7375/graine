use crate::{
    config::{Config, tracker_is_visible, tracker_label},
    model::{
        ActivityEvent, AnalyticsPeriod, AnalyticsState, LeaderboardEntry, Torrent,
        TorrentAnalytics, TrackerAnalytics, TrackerHistoryPoint, TrackerHistoryTotals,
        TransferTotals,
    },
    util::{ANALYTICS_VERSION, DAY_IN_SEC, now_unix, period_starts},
};
use std::collections::{BTreeMap, HashMap, HashSet};

fn add_tracker_totals(
    trackers: &mut HashMap<String, TrackerAnalytics>,
    tracker: &str,
    is_cross_seed: bool,
    uploaded: u64,
    downloaded: u64,
) {
    let totals = trackers.entry(tracker.to_string()).or_default();
    totals.total.add(uploaded, downloaded);
    if is_cross_seed {
        totals.cross_seed.add(uploaded, downloaded);
    } else {
        totals.regular.add(uploaded, downloaded);
    }
}

fn counter_delta(current: u64, previous: u64) -> u64 {
    // qBittorrent can reset a counter when a torrent is rechecked/re-added. Count the new
    // counter value as fresh activity instead of allowing subtraction to underflow.
    current.checked_sub(previous).unwrap_or(current)
}

/// Update all-time counters and return observed deltas. Persistence is owned by the SQLite
/// layer so dry runs can calculate a preview without modifying stored state. All-time counters
/// are seeded from qBittorrent's lifetime counters on first observation. Period rankings begin
/// at installation; old lifetime traffic is not falsely attributed to the current week/month/year.
pub(crate) fn track_analytics(
    torrents: &[Torrent],
    mut state: AnalyticsState,
) -> (AnalyticsState, Vec<ActivityEvent>) {
    let now = now_unix();
    if state.tracking_started_at == 0 {
        state.tracking_started_at = now;
    }
    state.version = ANALYTICS_VERSION;
    let mut events = Vec::new();

    for torrent in torrents {
        let tracker = crate::config::tracker_host(&torrent.tracker);
        let is_cross_seed = torrent.is_cross();

        if let Some(previous) = state.torrents.get_mut(&torrent.hash) {
            let previous_uploaded = previous.last_uploaded_counter;
            let previous_downloaded = previous.last_downloaded_counter;
            let uploaded_delta = counter_delta(torrent.uploaded, previous_uploaded);
            let downloaded_delta = counter_delta(torrent.downloaded, previous_downloaded);

            previous.name = torrent.name.clone();
            previous.tracker = tracker.clone();
            previous.category = torrent.category.clone();
            previous.added_on = torrent.added_on;
            previous.last_seen_at = now;
            previous.last_uploaded_counter = torrent.uploaded;
            previous.last_downloaded_counter = torrent.downloaded;
            previous.all_time_uploaded_bytes = previous
                .all_time_uploaded_bytes
                .saturating_add(uploaded_delta);
            previous.all_time_downloaded_bytes = previous
                .all_time_downloaded_bytes
                .saturating_add(downloaded_delta);

            add_tracker_totals(
                &mut state.trackers,
                &tracker,
                is_cross_seed,
                uploaded_delta,
                downloaded_delta,
            );

            if uploaded_delta > 0 || downloaded_delta > 0 {
                events.push(ActivityEvent {
                    event_id: format!(
                        "{}:{}:{}:{}:{}",
                        torrent.hash,
                        previous_uploaded,
                        torrent.uploaded,
                        previous_downloaded,
                        torrent.downloaded
                    ),
                    timestamp: now,
                    hash: torrent.hash.clone(),
                    name: torrent.name.clone(),
                    tracker,
                    category: torrent.category.clone(),
                    is_cross_seed,
                    uploaded_bytes: uploaded_delta,
                    downloaded_bytes: downloaded_delta,
                });
            }
        } else {
            let added_during_tracking =
                torrent.added_on >= state.tracking_started_at && torrent.added_on <= now;
            let period_uploaded = if added_during_tracking {
                torrent.uploaded
            } else {
                0
            };
            let period_downloaded = if added_during_tracking {
                torrent.downloaded
            } else {
                0
            };

            state.torrents.insert(
                torrent.hash.clone(),
                TorrentAnalytics {
                    name: torrent.name.clone(),
                    tracker: tracker.clone(),
                    category: torrent.category.clone(),
                    added_on: torrent.added_on,
                    first_seen_at: now,
                    last_seen_at: now,
                    all_time_uploaded_bytes: torrent.uploaded,
                    all_time_downloaded_bytes: torrent.downloaded,
                    last_uploaded_counter: torrent.uploaded,
                    last_downloaded_counter: torrent.downloaded,
                },
            );
            add_tracker_totals(
                &mut state.trackers,
                &tracker,
                is_cross_seed,
                torrent.uploaded,
                torrent.downloaded,
            );

            if period_uploaded > 0 || period_downloaded > 0 {
                events.push(ActivityEvent {
                    event_id: format!(
                        "{}:0:{}:0:{}",
                        torrent.hash, torrent.uploaded, torrent.downloaded
                    ),
                    timestamp: now,
                    hash: torrent.hash.clone(),
                    name: torrent.name.clone(),
                    tracker,
                    category: torrent.category.clone(),
                    is_cross_seed,
                    uploaded_bytes: period_uploaded,
                    downloaded_bytes: period_downloaded,
                });
            }
        }
    }



    (state, events)
}

fn event_identity(event: &ActivityEvent) -> String {
    if event.event_id.is_empty() {
        format!(
            "legacy:{}:{}:{}:{}",
            event.timestamp, event.hash, event.uploaded_bytes, event.downloaded_bytes
        )
    } else {
        event.event_id.clone()
    }
}

fn unique_events(events: &[ActivityEvent]) -> Vec<&ActivityEvent> {
    let mut seen = HashSet::new();
    events
        .iter()
        .filter(|event| seen.insert(event_identity(event)))
        .collect()
}

fn subtract_totals(current: &TransferTotals, observed: &TransferTotals) -> TransferTotals {
    TransferTotals {
        uploaded_bytes: current
            .uploaded_bytes
            .saturating_sub(observed.uploaded_bytes),
        downloaded_bytes: current
            .downloaded_bytes
            .saturating_sub(observed.downloaded_bytes),
    }
}

fn all_time_baseline(
    state: &AnalyticsState,
    events: &[ActivityEvent],
    cfg: &Config,
) -> BTreeMap<String, TrackerHistoryTotals> {
    // The analytics state contains lifetime totals. Subtract all timestamped deltas to recover
    // the lifetime-counter baseline that existed when Graine first observed each tracker.
    let mut observed_by_host: HashMap<String, TrackerAnalytics> = HashMap::new();
    for event in unique_events(events) {
        add_tracker_totals(
            &mut observed_by_host,
            &event.tracker,
            event.is_cross_seed,
            event.uploaded_bytes,
            event.downloaded_bytes,
        );
    }

    let mut baseline = BTreeMap::new();
    for (host, totals) in &state.trackers {
        if !tracker_is_visible(cfg, host) {
            continue;
        }

        let observed = observed_by_host.get(host).cloned().unwrap_or_default();
        let regular = subtract_totals(&totals.regular, &observed.regular);
        let cross_seed = subtract_totals(&totals.cross_seed, &observed.cross_seed);
        let label = tracker_label(cfg, host);
        let entry = baseline
            .entry(label)
            .or_insert_with(TrackerHistoryTotals::default);
        entry.add(regular.uploaded_bytes, regular.downloaded_bytes, false);
        entry.add(cross_seed.uploaded_bytes, cross_seed.downloaded_bytes, true);
    }

    baseline
}

fn tracker_history(
    start: u64,
    now: u64,
    baseline: BTreeMap<String, TrackerHistoryTotals>,
    events: &[ActivityEvent],
    cfg: &Config,
) -> Vec<TrackerHistoryPoint> {
    // One point per day keeps the graph readable without losing the observed transfer totals.
    let mut daily: BTreeMap<u64, BTreeMap<String, TrackerHistoryTotals>> = BTreeMap::new();
    for event in unique_events(events) {
        if event.timestamp < start
            || event.timestamp > now
            || !tracker_is_visible(cfg, &event.tracker)
        {
            continue;
        }

        let day = (event.timestamp / DAY_IN_SEC) * DAY_IN_SEC;
        // Place the daily aggregate at the end of its day (or now for the current day),
        // so the initial point remains the period baseline rather than including same-day traffic.
        let timestamp = day.saturating_add(DAY_IN_SEC).min(now).max(start);
        daily
            .entry(timestamp)
            .or_default()
            .entry(tracker_label(cfg, &event.tracker))
            .or_default()
            .add(
                event.uploaded_bytes,
                event.downloaded_bytes,
                event.is_cross_seed,
            );
    }

    let mut cumulative = baseline;
    let mut points = vec![TrackerHistoryPoint {
        timestamp: start,
        trackers: cumulative.clone(),
    }];

    for (timestamp, deltas) in daily {
        for (tracker, delta) in deltas {
            let totals = cumulative.entry(tracker).or_default();
            totals.total_net_bytes += delta.total_net_bytes;
            totals.regular_net_bytes += delta.regular_net_bytes;
            totals.cross_seed_net_bytes += delta.cross_seed_net_bytes;
        }

        if timestamp <= start {
            points[0].trackers = cumulative.clone();
        } else {
            points.push(TrackerHistoryPoint {
                timestamp,
                trackers: cumulative.clone(),
            });
        }
    }

    if now > points.last().map_or(start, |point| point.timestamp) {
        points.push(TrackerHistoryPoint {
            timestamp: now,
            trackers: cumulative,
        });
    }

    points
}

fn all_time_period(
    state: &AnalyticsState,
    events: &[ActivityEvent],
    now: u64,
    cfg: &Config,
) -> AnalyticsPeriod {
    let torrents = state
        .torrents
        .iter()
        .filter(|(_, torrent)| tracker_is_visible(cfg, &torrent.tracker))
        .map(|(hash, torrent)| LeaderboardEntry {
            hash: hash.clone(),
            name: torrent.name.clone(),
            tracker: tracker_label(cfg, &torrent.tracker),
            category: torrent.category.clone(),
            is_cross_seed: torrent.category.ends_with(".cross"),
            uploaded_bytes: torrent.all_time_uploaded_bytes,
            downloaded_bytes: torrent.all_time_downloaded_bytes,
            timestamp: torrent.last_seen_at,
        })
        .collect();

    let start = if state.tracking_started_at == 0 {
        now
    } else {
        state.tracking_started_at
    };
    let baseline = all_time_baseline(state, events, cfg);

    AnalyticsPeriod {
        torrents,
        tracker_history: tracker_history(start, now, baseline, events, cfg),
    }
}

fn activity_period(
    events: &[ActivityEvent],
    start: u64,
    now: u64,
    cfg: &Config,
) -> AnalyticsPeriod {
    let mut torrents: HashMap<String, LeaderboardEntry> = HashMap::new();

    for event in unique_events(events) {
        if event.timestamp < start
            || event.timestamp > now
            || !tracker_is_visible(cfg, &event.tracker)
        {
            continue;
        }

        let torrent = torrents
            .entry(event.hash.clone())
            .or_insert_with(|| LeaderboardEntry {
                hash: event.hash.clone(),
                name: event.name.clone(),
                tracker: tracker_label(cfg, &event.tracker),
                category: event.category.clone(),
                is_cross_seed: event.is_cross_seed,
                uploaded_bytes: 0,
                downloaded_bytes: 0,
                timestamp: event.timestamp,
            });
        torrent.name = event.name.clone();
        torrent.tracker = tracker_label(cfg, &event.tracker);
        torrent.category = event.category.clone();
        torrent.is_cross_seed = event.is_cross_seed;
        torrent.uploaded_bytes = torrent.uploaded_bytes.saturating_add(event.uploaded_bytes);
        torrent.downloaded_bytes = torrent
            .downloaded_bytes
            .saturating_add(event.downloaded_bytes);
        torrent.timestamp = torrent.timestamp.max(event.timestamp);
    }

    AnalyticsPeriod {
        torrents: torrents.into_values().collect(),
        tracker_history: tracker_history(start, now, BTreeMap::new(), events, cfg),
    }
}

pub(crate) fn build_periods(
    state: &AnalyticsState,
    stored_events: &[ActivityEvent],
    current_events: &[ActivityEvent],
    now: u64,
    cfg: &Config,
) -> HashMap<String, AnalyticsPeriod> {
    let mut events = stored_events.to_vec();
    // In --dry mode current deltas are not persisted, but should appear in the preview.
    events.extend_from_slice(current_events);
    let (week, month, year) = period_starts(now);

    HashMap::from([
        ("all".to_string(), all_time_period(state, &events, now, cfg)),
        ("year".to_string(), activity_period(&events, year, now, cfg)),
        (
            "month".to_string(),
            activity_period(&events, month, now, cfg),
        ),
        ("week".to_string(), activity_period(&events, week, now, cfg)),
    ])
}

