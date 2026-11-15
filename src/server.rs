use crate::{config::Config, db::Database, engine::run_management_cycle};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Response, StatusCode, header::{CACHE_CONTROL, CONTENT_TYPE}},
    response::IntoResponse,
    routing::get,
};
use serde_json::json;
use std::time::Duration;
use tokio::{
    net::TcpListener,
    task,
    time::{MissedTickBehavior, interval},
};
use tower_http::{services::ServeDir, trace::TraceLayer};

pub(crate) async fn run(config: Config, database: Database, force_dry: bool) -> Result<()> {
    let listener = TcpListener::bind(&config.listen)
        .await
        .with_context(|| format!("cannot bind Axum server to {}", config.listen))?;

    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/dashboard", get(dashboard))
        .with_state(database.clone());

    let index_file = config.web_dir.join("index.html");
    let static_files = ServeDir::new(&config.web_dir)
        .not_found_service(tower_http::services::ServeFile::new(index_file));
    let app = api
        .fallback_service(static_files)
        .layer(TraceLayer::new_for_http());

    let interval_seconds = config.interval_seconds;
    if config.management_enabled {
        let worker_config = config.clone();
        let worker_database = database.clone();
        task::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_seconds));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Tokio's interval ticks immediately on its first tick, so the first management run
            // happens at startup; subsequent runs are separated by the configured interval.
            loop {
                ticker.tick().await;
                let run_config = worker_config.clone();
                let run_database = worker_database.clone();
                let result = task::spawn_blocking(move || {
                    let password = if run_config.fixture_path.is_some() {
                        None
                    } else {
                        Some(crate::load_password(&run_config)?)
                    };
                    run_management_cycle(&run_config, &run_database, password.as_deref(), force_dry)
                })
                .await;

                match result {
                    Ok(Ok(data)) => {
                        tracing::info!(
                            timestamp = data.generated_at,
                            mode = %data.run.mode,
                            result = %data.run.result,
                            "management cycle finished"
                        );
                    }
                    Ok(Err(error)) => tracing::error!(error = %error, "management cycle failed"),
                    Err(error) => tracing::error!(error = %error, "management worker task failed"),
                }
            }
        });
    } else {
        tracing::info!("periodic management worker disabled; serving stored dashboard only");
    }

    tracing::info!(
        address = %listener.local_addr()?,
        interval_seconds,
        database = %database_location(&config),
        "graine web server started"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Axum server failed")?;

    Ok(())
}

fn database_location(config: &Config) -> String {
    config.database_path.display().to_string()
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn dashboard(State(database): State<Database>) -> Response<Body> {
    match task::spawn_blocking(move || database.dashboard_json()).await {
        Ok(Ok(Some(payload))) => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/json; charset=utf-8")
            .header(CACHE_CONTROL, "no-store")
            .body(Body::from(payload))
            .expect("static response headers are valid"),
        Ok(Ok(None)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "dashboard data is not available until the first successful management run" })),
        )
            .into_response(),
        Ok(Err(error)) => {
            tracing::error!(error = %error, "cannot read dashboard data");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "cannot read dashboard data" })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "dashboard database task failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "dashboard database task failed" })),
            )
                .into_response()
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        )
        .expect("failed to install SIGTERM handler");
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
