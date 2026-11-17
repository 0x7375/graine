use crate::{
    model::{
        ActivityEvent, AnalyticsState, DashboardData, DeletionBatch, HistorySnapshot,
        ManagementRun, RunSummary,
    },
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use std::{
    fs,
    path::PathBuf,
    time::Duration,
};

const ANALYTICS_KEY: &str = "analytics_state";
const RUN_HISTORY_BACKFILL_KEY: &str = "management_run_history_backfill_v1";

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RunFilter {
    All,
    Deletions,
    Errors,
}

impl RunFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Deletions => "deletions",
            Self::Errors => "errors",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Database {
    path: PathBuf,
}

impl Database {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn initialize(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create database directory {}", parent.display()))?;
        }

        let mut connection = self.connect()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_state (
                key TEXT PRIMARY KEY,
                json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS activity_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS activity_events_timestamp
                ON activity_events(timestamp);
            CREATE TABLE IF NOT EXISTS history_snapshots (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS history_snapshots_timestamp
                ON history_snapshots(timestamp);
            CREATE TABLE IF NOT EXISTS deletion_batches (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                batch_id TEXT NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS deletion_batches_timestamp
                ON deletion_batches(timestamp);
            CREATE TABLE IF NOT EXISTS leaderboard_baselines (
                period TEXT NOT NULL,
                period_start INTEGER NOT NULL,
                captured_at INTEGER NOT NULL,
                rankings_json TEXT NOT NULL,
                PRIMARY KEY (period, period_start)
            );
            CREATE TABLE IF NOT EXISTS management_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                duration_ms INTEGER,
                trigger TEXT NOT NULL,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                summary_json TEXT,
                snapshot_json TEXT,
                deletion_batch_json TEXT,
                error TEXT
            );
            CREATE INDEX IF NOT EXISTS management_runs_started_at ON management_runs(started_at);
            CREATE TABLE IF NOT EXISTS latest_dashboard (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                generated_at INTEGER NOT NULL,
                payload TEXT NOT NULL
            );
            ",
        )?;

        self.backfill_management_runs(&mut connection)?;
        // The old process is gone after a restart; do not leave an eternal "running" entry.
        connection.execute(
            "UPDATE management_runs SET status = 'interrupted' WHERE status = 'running'", [],
        )?;
        Ok(())
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)
            .with_context(|| format!("cannot open SQLite database {}", self.path.display()))?;
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(connection)
    }

    // Pre-existing SQLite snapshots provide storage figures but not timing, trigger, or
    // the number of torrents scanned. Show them as historical, not completed live runs.
    // Match eviction batches to their disk snapshots; do not lose old deletion details.
    fn backfill_management_runs(&self, connection: &mut Connection) -> Result<()> {
        if connection.query_row(
            "SELECT 1 FROM metadata WHERE key = ?1", [RUN_HISTORY_BACKFILL_KEY],
            |_| Ok(()),
        ).optional()?.is_some() {
            return Ok(());
        }
        let snapshots: Vec<HistorySnapshot> = {
            let mut statement = connection.prepare("SELECT payload FROM history_snapshots ORDER BY seq")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|row| {
                let payload = row?;
                Ok(serde_json::from_str::<HistorySnapshot>(&payload)?)
            }).collect::<Result<_, anyhow::Error>>()?
        };
        let batches: Vec<DeletionBatch> = {
            let mut statement = connection.prepare("SELECT payload FROM deletion_batches ORDER BY seq")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|row| {
                let payload = row?;
                Ok(serde_json::from_str::<DeletionBatch>(&payload)?)
            }).collect::<Result<_, anyhow::Error>>()?
        };
        let mut associated = vec![false; batches.len()];
        let tx = connection.transaction()?;
        for snapshot in &snapshots {
            // Historical disk snapshots have no reliable completion timestamp.
            let batch = batches.iter().enumerate().find(|(index, batch)| {
                !associated[*index] && batch.timestamp == snapshot.timestamp
            });
            let batch_json = if let Some((index, batch)) = batch {
                associated[index] = true;
                Some(serde_json::to_string(batch)?)
            } else {
                None
            };
            tx.execute(
                "INSERT INTO management_runs(started_at, trigger, mode, status, snapshot_json, deletion_batch_json)
                 VALUES (?1, 'archive', 'historical', 'historical', ?2, ?3)",
                params![snapshot.timestamp, serde_json::to_string(snapshot)?, batch_json],
            )?;
        }
        for (index, batch) in batches.iter().enumerate() {
            if !associated[index] {
                tx.execute(
                    "INSERT INTO management_runs(started_at, trigger, mode, status, deletion_batch_json)
                     VALUES (?1, 'archive', 'historical', 'historical', ?2)",
                    params![batch.timestamp, serde_json::to_string(batch)?],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO metadata(key, value) VALUES (?1, 'complete')",
            [RUN_HISTORY_BACKFILL_KEY],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn begin_management_run(&self, trigger: &str, mode: &str, timestamp: u64) -> Result<i64> {
        let connection = self.connect()?;
        connection.execute(
            "INSERT INTO management_runs(started_at, trigger, mode, status)
             VALUES (?1, ?2, ?3, 'running')",
            params![timestamp, trigger, mode],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub(crate) fn finish_management_run(
        &self,
        id: i64,
        duration_ms: u64,
        result: &Result<DashboardData>,
    ) -> Result<()> {
        let connection = self.connect()?;
        let finished = crate::util::now_unix();
        match result {
            Ok(dashboard) => {
                let summary = &dashboard.run;
                let snapshot = dashboard.history.last();
                let batch = summary.deletion_batch_id.as_ref().and_then(|batch_id| {
                    dashboard.deletions.iter().rev().find(|item| &item.batch_id == batch_id)
                });
                connection.execute(
                    "UPDATE management_runs SET finished_at = ?1, duration_ms = ?2, status = ?3,
                     summary_json = ?4, snapshot_json = ?5, deletion_batch_json = ?6 WHERE id = ?7",
                    params![finished, duration_ms, &summary.result,
                        serde_json::to_string(summary)?,
                        snapshot.map(serde_json::to_string).transpose()?,
                        batch.map(serde_json::to_string).transpose()?, id],
                )?;
            }
            Err(error) => {
                connection.execute(
                    "UPDATE management_runs SET finished_at = ?1, duration_ms = ?2,
                     status = 'error', error = ?3 WHERE id = ?4",
                    params![finished, duration_ms, error.to_string(), id],
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn list_management_runs(&self, before_id: Option<i64>, limit: usize, filter: RunFilter) -> Result<Vec<ManagementRun>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, started_at, finished_at, duration_ms, trigger, mode, status,
                    summary_json, snapshot_json, deletion_batch_json, error
             FROM management_runs
             WHERE id < ?1
               AND (?3 = 'all'
                    OR (?3 = 'deletions' AND deletion_batch_json IS NOT NULL)
                    OR (?3 = 'errors' AND status IN ('error', 'interrupted', 'over_limit')))
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![before_id.unwrap_or(i64::MAX), limit as i64, filter.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?, row.get::<_, u64>(1)?, row.get::<_, Option<u64>>(2)?,
                row.get::<_, Option<u64>>(3)?, row.get::<_, String>(4)?,
                row.get::<_, String>(5)?, row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?, row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?, row.get::<_, Option<String>>(10)?,
            ))
        })?;
        let mut output = Vec::new();
        for row in rows {
            let (id, started_at, finished_at, duration_ms, trigger, mode, status,
                summary_json, snapshot_json, batch_json, error) = row?;
            let summary: Option<RunSummary> = summary_json.map(|v| serde_json::from_str(&v)).transpose()?;
            let snapshot: Option<HistorySnapshot> = snapshot_json.map(|v| serde_json::from_str(&v)).transpose()?;
            let deletion_batch: Option<DeletionBatch> = batch_json.map(|v| serde_json::from_str(&v)).transpose()?;
            output.push(ManagementRun {
                id, started_at, finished_at, duration_ms, trigger, mode, status,
                summary, snapshot, deletion_batch, error,
            });
        }
        Ok(output)
    }

    pub(crate) fn load_analytics(&self) -> Result<AnalyticsState> {
        let connection = self.connect()?;
        let json: Option<String> = connection
            .query_row(
                "SELECT json FROM app_state WHERE key = ?1",
                [ANALYTICS_KEY],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value).context("cannot decode analytics state"))
            .transpose()
            .map(|value| value.unwrap_or_default())
    }

    pub(crate) fn load_activity(&self) -> Result<Vec<ActivityEvent>> {
        self.load_records(
            "SELECT payload FROM activity_events ORDER BY seq",
            "activity event",
        )
    }

    pub(crate) fn load_history(&self) -> Result<Vec<HistorySnapshot>> {
        self.load_records(
            "SELECT payload FROM history_snapshots ORDER BY seq",
            "history snapshot",
        )
    }

    pub(crate) fn load_deletions(&self) -> Result<Vec<DeletionBatch>> {
        self.load_records(
            "SELECT payload FROM deletion_batches ORDER BY seq",
            "deletion batch",
        )
    }

    fn load_records<T: DeserializeOwned>(&self, query: &str, label: &str) -> Result<Vec<T>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(query)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut records = Vec::new();
        for row in rows {
            let json = row?;
            records.push(
                serde_json::from_str(&json)
                    .with_context(|| format!("cannot decode stored {label}"))?,
            );
        }
        Ok(records)
    }

    pub(crate) fn persist_analytics(
        &self,
        analytics: &AnalyticsState,
        activity: &[ActivityEvent],
        is_dry: bool,
    ) -> Result<()> {
        if is_dry {
            return Ok(());
        }

        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO app_state(key, json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            params![ANALYTICS_KEY, serde_json::to_string(analytics)?],
        )?;

        for event in activity {
            let event_id = if event.event_id.is_empty() {
                format!(
                    "legacy:{}:{}:{}:{}",
                    event.timestamp, event.hash, event.uploaded_bytes, event.downloaded_bytes
                )
            } else {
                event.event_id.clone()
            };
            transaction.execute(
                "INSERT OR IGNORE INTO activity_events(event_id, timestamp, payload) VALUES (?1, ?2, ?3)",
                params![event_id, event.timestamp, serde_json::to_string(event)?],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn leaderboard_baseline(
        &self, period: &str, start: u64,
    ) -> Result<Option<(u64, std::collections::HashMap<String, i32>)>> {
        let connection = self.connect()?;
        let stored: Option<(u64, String)> = connection
            .query_row(
                "SELECT captured_at, rankings_json FROM leaderboard_baselines WHERE period = ?1 AND period_start = ?2",
                params![period, start],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        stored
            .map(|(captured, json)| {
                Ok((captured, serde_json::from_str(&json).context("invalid stored leaderboard baseline")?))
            })
            .transpose()
    }

    pub(crate) fn save_run(
        &self,
        snapshot: &HistorySnapshot,
        deletion_batch: Option<&DeletionBatch>,
        dashboard: &DashboardData,
        is_dry: bool,
        new_baselines: &[(String, u64, std::collections::HashMap<String, i32>)],
    ) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;

        if !is_dry {
            for (period, start, ranks) in new_baselines {
                transaction.execute(
                    "INSERT OR IGNORE INTO leaderboard_baselines(period, period_start, captured_at, rankings_json) VALUES (?1, ?2, ?3, ?4)",
                    params![period, start, dashboard.generated_at, serde_json::to_string(ranks)?],
                )?;
            }
            transaction.execute(
                "INSERT INTO history_snapshots(timestamp, payload) VALUES (?1, ?2)",
                params![snapshot.timestamp, serde_json::to_string(snapshot)?],
            )?;

            if let Some(batch) = deletion_batch {
                transaction.execute(
                    "INSERT OR IGNORE INTO deletion_batches(batch_id, timestamp, payload) VALUES (?1, ?2, ?3)",
                    params![batch.batch_id, batch.timestamp, serde_json::to_string(batch)?],
                )?;
            }
        }

        transaction.execute(
            "INSERT INTO latest_dashboard(id, generated_at, payload) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET generated_at = excluded.generated_at, payload = excluded.payload",
            params![dashboard.generated_at, serde_json::to_string(dashboard)?],
        )?;
        transaction.commit()?;
        Ok(())
    }


    pub(crate) fn dashboard_json(&self) -> Result<Option<String>> {
        let connection = self.connect()?;
        Ok(connection
            .query_row(
                "SELECT payload FROM latest_dashboard WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn test_directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("graine-db-test-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should be created");
        path
    }

    #[test]
    fn preserves_existing_sqlite_snapshots_as_historical_runs() {
        let root = test_directory();
        let db = Database::new(root.join("graine.sqlite3"));
        db.initialize().unwrap();
        let connection = db.connect().unwrap();
        let snapshot = json!({
            "timestamp": 1794677280, "disk_used_bytes": 500,
            "disk_total_bytes": 1000, "disk_used_percent": 50.0,
            "projected_used_bytes": 450, "projected_used_percent": 45.0,
            "high_watermark_percent": 85.0, "low_watermark_percent": 82.0,
            "reclaimable_bytes": 300, "protected_bytes": 100,
            "estimated_reclaimed_bytes": 50, "actual_reclaimed_bytes": 50
        });
        connection.execute(
            "INSERT INTO history_snapshots(timestamp, payload) VALUES (?1, ?2)",
            params![1794677280_i64, snapshot.to_string()],
        ).unwrap();
        connection.execute("DELETE FROM metadata WHERE key = ?1", [RUN_HISTORY_BACKFILL_KEY]).unwrap();
        drop(connection);
        db.initialize().unwrap();
        let runs = db.list_management_runs(None, 30, RunFilter::All).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "historical");
        db.initialize().unwrap();
        assert_eq!(db.list_management_runs(None, 30, RunFilter::All).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn records_failed_and_interrupted_runs_with_pagination() {
        let root = test_directory();
        let db = Database::new(root.join("runs.sqlite3"));
        db.initialize().unwrap();
        let id = db.begin_management_run("manual", "dry", 101).unwrap();
        let failure: Result<DashboardData> = Err(anyhow::anyhow!("simulated login failure"));
        db.finish_management_run(id, 1234, &failure).unwrap();
        let running = db.begin_management_run("scheduled", "live", 102).unwrap();
        let rows = db.list_management_runs(None, 20, RunFilter::All).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, running);
        assert_eq!(rows[0].status, "running");
        assert_eq!(rows[1].error.as_deref(), Some("simulated login failure"));
        assert_eq!(rows[1].duration_ms, Some(1234));
        assert_eq!(db.list_management_runs(Some(running), 1, RunFilter::All).unwrap().len(), 1);
        db.initialize().unwrap();
        assert_eq!(db.list_management_runs(None, 1, RunFilter::All).unwrap()[0].status, "interrupted");
        fs::remove_dir_all(root).unwrap();
    }


    #[test]
    fn history_filters_are_applied_before_limit() {
        let root = test_directory();
        let db = Database::new(root.join("filters.sqlite3"));
        db.initialize().unwrap();
        let conn = db.connect().unwrap();
        for id in 1..=35 {
            let (status, batch) = if id == 2 {
                ("ok", Some(r#"{"batch_id":"b", "timestamp":2,"estimated_reclaimed_bytes":0,"observed_reclaimed_bytes":null,"clusters":[]}"#))
            } else if id == 3 {
                ("error", None)
            } else {
                ("ok", None)
            };
            conn.execute(
                "INSERT INTO management_runs(started_at, trigger, mode, status, deletion_batch_json) VALUES (?1, 'scheduled', 'live', ?2, ?3)",
                params![id, status, batch],
            ).unwrap();
        }
        // The relevant runs are older than the first unfiltered page.
        assert_eq!(db.list_management_runs(None, 1, RunFilter::Deletions).unwrap()[0].id, 2);
        assert_eq!(db.list_management_runs(None, 1, RunFilter::Errors).unwrap()[0].id, 3);
        fs::remove_dir_all(root).unwrap();
    }
}
