//! HTTP and SSE dashboard API for audiorouter.
//!
//! This crate intentionally depends on `audiorouter-core` instead of the CLI/TUI
//! crate so the web dashboard, dev server proxy target, and future non-TUI
//! frontends share the same config/device/validation contract.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use audiorouter_core::api_types::{
    ConfigLoadResponse, ConfigPreviewResponse, ConfigSaveRequest, ConfigSaveResponse,
    ConfigStatusResponse, ValidateResponse, dashboard_config_status, read_dashboard_config,
    stringify_dashboard_config, validate_dashboard_config,
};
use audiorouter_core::{ConfigFileWatcher, DevicePoller, DevicesResponse, list_audio_devices};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::{RwLock, broadcast};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct DashboardState {
    config_path: PathBuf,
    runtime_snapshot: Arc<RwLock<RuntimeSnapshot>>,
    event_tx: broadcast::Sender<DashboardEvent>,
    config_version: Arc<AtomicU64>,
    device_version: Arc<AtomicU64>,
    runtime_version: Arc<AtomicU64>,
}

impl DashboardState {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            config_path: config_path.into(),
            runtime_snapshot: Arc::new(RwLock::new(RuntimeSnapshot::default())),
            event_tx,
            config_version: Arc::new(AtomicU64::new(0)),
            device_version: Arc::new(AtomicU64::new(0)),
            runtime_version: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub async fn set_runtime_snapshot(&self, snapshot: RuntimeSnapshot) {
        *self.runtime_snapshot.write().await = snapshot;
        self.emit_runtime_changed();
    }

    pub fn emit_config_changed(&self) {
        let version = self.config_version.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.event_tx.send(DashboardEvent::ConfigChanged {
            version,
            path: self.config_path.display().to_string(),
        });
    }

    pub fn emit_devices_changed(&self, events: Vec<String>) {
        let version = self.device_version.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .event_tx
            .send(DashboardEvent::DevicesChanged { version, events });
    }

    pub fn emit_runtime_changed(&self) {
        let version = self.runtime_version.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .event_tx
            .send(DashboardEvent::RuntimeChanged { version });
    }

    pub fn emit_log(&self, level: impl Into<String>, message: impl Into<String>) {
        let _ = self.event_tx.send(DashboardEvent::Log {
            level: level.into(),
            message: message.into(),
            timestamp: chrono_like_timestamp(),
        });
    }

    /// Spawn a background task that polls CoreAudio every 2 seconds and emits
    /// `DevicesChanged` events when device connections, channel counts, or
    /// defaults change.
    ///
    /// Uses the shared `DevicePoller` from `audiorouter-core` — the same
    /// primitive the TUI uses — so device-change detection logic is not
    /// duplicated.
    ///
    /// The returned `JoinHandle` can be awaited or aborted; the watcher lives
    /// until the runtime is shut down.
    pub fn spawn_device_watcher(&self) -> tokio::task::JoinHandle<()> {
        let state = self.clone();
        tokio::spawn(async move {
            let mut poller = DevicePoller::new(std::time::Duration::from_secs(2));
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Some(events) = poller.poll() {
                    tracing::info!("device change detected: {}", events.join("; "));
                    state.emit_devices_changed(events);
                }
            }
        })
    }

    /// Spawn a background task that watches the config file for external edits
    /// and emits `ConfigChanged` SSE events when the file changes on disk.
    ///
    /// Uses the shared `ConfigFileWatcher` from `audiorouter-core` — the same
    /// primitive the TUI uses for hot-reload — so config-change detection logic
    /// is not duplicated.
    pub fn spawn_config_watcher(&self) -> tokio::task::JoinHandle<()> {
        let state = self.clone();
        let config_path = self.config_path.clone();
        tokio::spawn(async move {
            let watcher = ConfigFileWatcher::new(&config_path);
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if watcher.poll() {
                    tracing::info!("config file changed on disk");
                    state.emit_config_changed();
                }
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub state: RuntimeState,
    pub disabled_route_indices: Vec<usize>,
    pub unavailable_inputs: Vec<String>,
    pub unavailable_outputs: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    #[default]
    Starting,
    Running,
    Stopped,
    FatalError,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    ConfigChanged {
        version: u64,
        path: String,
    },
    ConfigSaved {
        version: u64,
    },
    DevicesChanged {
        version: u64,
        events: Vec<String>,
    },
    RuntimeChanged {
        version: u64,
    },
    Log {
        level: String,
        message: String,
        timestamp: String,
    },
}

impl DashboardEvent {
    fn event_name(&self) -> &'static str {
        match self {
            Self::ConfigChanged { .. } => "config_changed",
            Self::ConfigSaved { .. } => "config_saved",
            Self::DevicesChanged { .. } => "devices_changed",
            Self::RuntimeChanged { .. } => "runtime_changed",
            Self::Log { .. } => "log",
        }
    }
}

pub fn api_router(state: DashboardState) -> Router {
    Router::new()
        .route("/config", get(get_config).put(put_config))
        .route("/config/preview", post(post_config_preview))
        .route("/config/status", post(post_config_status))
        .route("/validate", post(post_validate))
        .route("/devices", get(get_devices))
        .route("/runtime", get(get_runtime))
        .route("/events", get(get_events))
        .with_state(state)
}

pub fn dashboard_router(state: DashboardState, dist_dir: impl Into<PathBuf>) -> Router {
    Router::new()
        .nest("/api", api_router(state))
        .fallback_service(ServeDir::new(dist_dir.into()).append_index_html_on_directories(true))
}

pub struct DashboardHandle {
    pub local_addr: SocketAddr,
    pub state: DashboardState,
}

impl DashboardHandle {
    pub fn url(&self) -> String {
        format!("http://{}", self.local_addr)
    }
}

async fn get_config(
    State(state): State<DashboardState>,
) -> Result<Json<ConfigLoadResponse>, ApiError> {
    let (config, raw) = read_dashboard_config(state.config_path())?;
    Ok(Json(ConfigLoadResponse {
        config,
        raw,
        path: state.config_path().display().to_string(),
    }))
}

async fn put_config(
    State(state): State<DashboardState>,
    Json(req): Json<ConfigSaveRequest>,
) -> Result<Json<ConfigSaveResponse>, ApiError> {
    let validation = validate_dashboard_config(req.config.clone());
    if !validation.errors.is_empty() {
        return Ok(Json(ConfigSaveResponse {
            ok: false,
            raw: String::new(),
            errors: validation.errors,
        }));
    }

    let raw = stringify_dashboard_config(&req.config)?;
    std::fs::write(state.config_path(), &raw)?;
    let version = state.config_version.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = state.event_tx.send(DashboardEvent::ConfigSaved { version });

    Ok(Json(ConfigSaveResponse {
        ok: true,
        raw,
        errors: Vec::new(),
    }))
}

async fn post_config_preview(
    Json(req): Json<ConfigSaveRequest>,
) -> Result<Json<ConfigPreviewResponse>, ApiError> {
    Ok(Json(ConfigPreviewResponse {
        raw: stringify_dashboard_config(&req.config)?,
    }))
}

async fn post_config_status(Json(req): Json<ConfigSaveRequest>) -> Json<ConfigStatusResponse> {
    Json(dashboard_config_status(req.config))
}

async fn post_validate(Json(req): Json<ConfigSaveRequest>) -> Json<ValidateResponse> {
    Json(validate_dashboard_config(req.config))
}

async fn get_devices() -> Result<Json<DevicesResponse>, ApiError> {
    Ok(Json(list_audio_devices()?))
}

async fn get_runtime(State(state): State<DashboardState>) -> Json<RuntimeSnapshot> {
    Json(state.runtime_snapshot.read().await.clone())
}

async fn get_events(
    State(state): State<DashboardState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|event| async move {
        let event = event.ok()?;
        let name = event.event_name();
        let data = serde_json::to_string(&event).ok()?;
        Some(Ok(Event::default().event(name).data(data)))
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Debug)]
pub struct ApiError(anyhow::Error);

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        Self(anyhow::Error::new(error))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let message = self.0.to_string();
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response()
    }
}

fn chrono_like_timestamp() -> String {
    // Avoid adding a time crate just for log event DTOs; consumers treat this as
    // an opaque display timestamp until the CLI wires real structured logs in.
    format!("{:?}", std::time::SystemTime::now())
}
