use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::exit,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const DAY_IN_SEC: u64 = 60 * 60 * 24;
pub(crate) const GIB: u64 = 1024 * 1024 * 1024;
pub(crate) const ANALYTICS_VERSION: u32 = 2;
pub(crate) const ANALYTICS_FILE: &str = "analytics.json";
pub(crate) const ACTIVITY_FILE: &str = "activity.jsonl";
pub(crate) const HISTORY_FILE: &str = "history.jsonl";
pub(crate) const DELETIONS_FILE: &str = "deletions.jsonl";
pub(crate) const DASHBOARD_FILE: &str = "/var/www/graine/index.html";
pub(crate) const USAGE: &str = "Usage: QBIT_PW=<password> graine <config.toml> [--dry]";

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert days since 1970-01-01 to a Gregorian UTC date.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month as i64 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn period_starts(now: u64) -> (u64, u64, u64) {
    let days = (now / DAY_IN_SEC) as i64;
    let (year, month, _) = civil_from_days(days);
    let year_start = days_from_civil(year, 1, 1) as u64 * DAY_IN_SEC;
    let month_start = days_from_civil(year, month, 1) as u64 * DAY_IN_SEC;
    let week_start = now - (((days as u64 + 4) % 7) * DAY_IN_SEC) - (now % DAY_IN_SEC);
    (week_start, month_start, year_start)
}

pub(crate) fn atomic_write(path: &str, contents: &[u8]) {
    let temporary = format!("{path}.tmp");
    fs::write(&temporary, contents).unwrap_or_else(|e| {
        eprintln!("Cannot write {temporary}: {e}");
        exit(1);
    });
    fs::rename(&temporary, path).unwrap_or_else(|e| {
        eprintln!("Cannot replace {path}: {e}");
        exit(1);
    });
}

pub(crate) fn append_jsonl<T: Serialize>(path: &str, records: &[T]) {
    if records.is_empty() {
        return;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|e| {
            eprintln!("Cannot open {path}: {e}");
            exit(1);
        });

    for record in records {
        serde_json::to_writer(&mut file, record).unwrap_or_else(|e| {
            eprintln!("Cannot serialize a record for {path}: {e}");
            exit(1);
        });
        file.write_all(b"\n").unwrap_or_else(|e| {
            eprintln!("Cannot append to {path}: {e}");
            exit(1);
        });
    }

    file.sync_data().unwrap_or_else(|e| {
        eprintln!("Cannot sync {path}: {e}");
        exit(1);
    });
}

pub(crate) fn read_jsonl<T: DeserializeOwned>(path: &str) -> Vec<T> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("Cannot read {path}: {e}");
            exit(1);
        }
    };

    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.trim().is_empty() {
                return None;
            }
            match serde_json::from_str(line) {
                Ok(record) => Some(record),
                Err(e) => {
                    // A process killed midway through the final append may leave one partial line.
                    eprintln!(
                        "WARN: ignoring invalid JSON in {path}, line {}: {e}",
                        index + 1
                    );
                    None
                }
            }
        })
        .collect()
}
