use crate::{config::Config, db::{Database, RunFilter}, engine::run_management_cycle, util::now_unix};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{Response, StatusCode, header::{CACHE_CONTROL, CONTENT_TYPE}},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{sync::Arc, time::{Duration, Instant}};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore},
    task,
    time::{MissedTickBehavior, interval},
};
use tower_http::{services::ServeDir, trace::TraceLayer};

#[derive(Clone, Serialize)]
struct RunStatus {
    running: bool,
    started_at: Option<u64>,
    finished_at: Option<u64>,
    completed_cycles: u64,
    last_error: Option<String>,
}

#[derive(Clone)]
struct AppState {
    config: Config,
    database: Database,
    force_dry: bool,
    // A single permit protects qBittorrent / SQLite against overlapping manual and timed runs.
    runner: Arc<Semaphore>,
    status: Arc<Mutex<RunStatus>>,
}

pub(crate) async fn run(config: Config, database: Database, force_dry: bool) -> Result<()> {
    let state = AppState {
        config,
        database,
        force_dry,
        runner: Arc::new(Semaphore::new(1)),
        status: Arc::new(Mutex::new(RunStatus {
            running: false,
            started_at: None,
            finished_at: None,
            completed_cycles: 0,
            last_error: None,
        })),
    };

    let listener = TcpListener::bind(&state.config.listen)
        .await
        .with_context(|| format!("cannot bind Axum server to {}", state.config.listen))?;

    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/dashboard", get(dashboard))
        .route("/api/run/status", get(run_status))
        .route("/api/run", post(manual_run))
        .route("/api/runs", get(run_history))
        .with_state(state.clone());

    let index_file = state.config.web_dir.join("index.html");
    let static_files = ServeDir::new(&state.config.web_dir)
        .not_found_service(tower_http::services::ServeFile::new(index_file));
    let app = api
        .fallback_service(static_files)
        .layer(TraceLayer::new_for_http());

    let interval_seconds = state.config.interval_seconds;
    if state.config.management_enabled {
        let worker = state.clone();
        task::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_seconds));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                // If a manual cycle is already running, skip this scheduled tick.
                if !start_cycle(&worker, "scheduled").await {
                    tracing::info!("scheduled management cycle skipped: already running");
                }
            }
        });
    } else {
        tracing::info!("periodic management worker disabled; serving stored dashboard only");
    }

    tracing::info!(
        address = %listener.local_addr()?,
        interval_seconds,
        database = %state.config.database_path.display(),
        "graine web server started"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Axum server failed")?;
    Ok(())
}

// Returns false without starting another run if one is already in progress.
async fn start_cycle(state: &AppState, trigger: &'static str) -> bool {
    let Ok(permit) = state.runner.clone().try_acquire_owned() else {
        return false;
    };
    {
        let mut status = state.status.lock().await;
        status.running = true;
        status.started_at = Some(now_unix());
        status.last_error = None;
    }

    let state = state.clone();
    task::spawn(async move {
        let config = state.config.clone();
        let database = state.database.clone();
        let force_dry = state.force_dry;
        let result = task::spawn_blocking(move || {
            let mode = if config.fixture_path.is_some() { "fixture" }
                else if config.dry_run || force_dry { "dry" } else { "live" };
            let started = Instant::now();
            let record_id = database.begin_management_run(trigger, mode, now_unix())?;
            let outcome = (|| {
                let password = if config.fixture_path.is_some() {
                    None
                } else {
                    Some(crate::load_password(&config)?)
                };
                run_management_cycle(&config, &database, password.as_deref(), force_dry)
            })();
            if let Err(error) = database.finish_management_run(
                record_id, started.elapsed().as_millis() as u64, &outcome,
            ) {
                tracing::error!(error = %error, "cannot record management cycle result");
            }
            outcome
        }).await;

        let mut status = state.status.lock().await;
        status.running = false;
        status.finished_at = Some(now_unix());
        status.completed_cycles += 1;
        match result {
            Ok(Ok(data)) => {
                tracing::info!(timestamp = data.generated_at, mode = %data.run.mode,
                    result = %data.run.result, "management cycle finished");
            }
            Ok(Err(error)) => {
                tracing::error!(error = %error, "management cycle failed");
                status.last_error = Some("Management cycle failed; check server logs".into());
            }
            Err(error) => {
                tracing::error!(error = %error, "management worker task failed");
                status.last_error = Some("Management worker failed; check server logs".into());
            }
        }
        drop(status);
        drop(permit);
    });
    true
}

// Manual runs use the same single-worker guard as scheduled executions.
// Deploy behind a trusted network or reverse proxy: this endpoint can trigger evictions.
async fn manual_run(State(state): State<AppState>) -> impl IntoResponse {
    if !start_cycle(&state, "manual").await {
        return (StatusCode::CONFLICT, Json(json!({"error": "management already running"}))).into_response();
    }
    (StatusCode::ACCEPTED, Json(json!({"status": "started"}))).into_response()
}

async fn run_status(State(state): State<AppState>) -> impl IntoResponse {
    let status = state.status.lock().await.clone();
    ([(CACHE_CONTROL, "no-store")], Json(status))
}


#[derive(Deserialize)]
struct RunsQuery {
    before: Option<i64>,
    limit: Option<usize>,
    filter: Option<RunFilter>,
}

async fn run_history(
    State(state): State<AppState>, Query(query): Query<RunsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    match task::spawn_blocking(move || {
        state.database.list_management_runs(query.before, limit, query.filter.unwrap_or(RunFilter::All))
    }).await {
        Ok(Ok(runs)) => (StatusCode::OK, Json(json!(runs))).into_response(),
        Ok(Err(error)) => {
            tracing::error!(error = %error, "cannot load management history");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "cannot load execution history" }))).into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "history task failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "execution history task failed" }))).into_response()
        }
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn dashboard(State(state): State<AppState>) -> Response<Body> {
    match task::spawn_blocking(move || state.database.dashboard_json()).await {
        Ok(Ok(Some(payload))) => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .header(CACHE_CONTROL, "no-store")
            .body(Body::from(payload))
            .expect("static response headers are valid"),
        Ok(Ok(None)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "dashboard data is not available until the first successful management run" })),
        ).into_response(),
        Ok(Err(error)) => {
            tracing::error!(error = %error, "cannot read dashboard data");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "cannot read dashboard data" }))).into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "dashboard database task failed");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "dashboard database task failed" }))).into_response()
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ).expect("failed to install SIGTERM handler");
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::error!(error = %error, "failed to listen for SIGINT");
                }
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %error, "failed to listen for shutdown signal");
    }
    tracing::info!("shutdown signal received");
}

