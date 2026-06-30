//! HTTP and SSE dashboard API for audiorouter.
//!
//! This crate intentionally depends on `audiorouter-core` instead of the CLI/TUI
//! crate so the web dashboard, dev server proxy target, and future non-TUI
//! frontends share the same config/device/validation contract.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use audiorouter_core::api_types::{
    ConfigLoadResponse, ConfigPreviewResponse, ConfigSaveRequest, ConfigSaveResponse,
    ConfigStatusResponse, ValidateResponse, dashboard_config_status, read_dashboard_config,
    stringify_dashboard_config, validate_dashboard_config,
};
use audiorouter_core::{
    ConfigFileWatcher, DevicePoller, DevicesResponse, RuntimeSnapshot, list_audio_devices,
};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use include_dir::{Dir, include_dir};
use serde::Serialize;
use tokio::sync::{RwLock, broadcast};
use tokio_stream::wrappers::BroadcastStream;

// Embedded frontend produced by `pnpm build` (see build.rs). Lives in OUT_DIR
// (cargo-managed) so it survives build.rs cache skips between cargo runs.
static DIST_DIR: Dir<'static> = include_dir!("$OUT_DIR/dist");

#[derive(Clone)]
pub struct DashboardState {
    config_path: PathBuf,
    runtime_snapshot: Arc<RwLock<RuntimeSnapshot>>,
    event_tx: broadcast::Sender<DashboardEvent>,
    config_version: Arc<AtomicU64>,
    device_version: Arc<AtomicU64>,
    runtime_version: Arc<AtomicU64>,
    last_dashboard_written_config: Arc<Mutex<Option<String>>>,
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
            last_dashboard_written_config: Arc::new(Mutex::new(None)),
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

    fn remember_dashboard_written_config(&self, raw: String) {
        if let Ok(mut last_written) = self.last_dashboard_written_config.lock() {
            *last_written = Some(raw);
        }
    }

    fn forget_dashboard_written_config_if_matches(&self, raw: &str) {
        if let Ok(mut last_written) = self.last_dashboard_written_config.lock()
            && last_written.as_deref() == Some(raw)
        {
            *last_written = None;
        }
    }

    fn should_emit_config_changed_after_file_event(&self) -> bool {
        let Ok(current_raw) = std::fs::read_to_string(&self.config_path) else {
            // Missing/unreadable config is still an external state change the UI should surface.
            return true;
        };

        let Ok(last_written) = self.last_dashboard_written_config.lock() else {
            return true;
        };

        last_written.as_deref() != Some(current_raw.as_str())
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
                    let summary = events.join("; ");
                    tracing::info!("device change detected: {}", summary);
                    state.emit_log("info", format!("device change: {}", summary));
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
                    if state.should_emit_config_changed_after_file_event() {
                        tracing::info!("config file changed on disk");
                        state.emit_log("info", "config file changed on disk");
                        state.emit_config_changed();
                    } else {
                        tracing::debug!("ignored config file event from dashboard save");
                    }
                }
            }
        })
    }
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

/// Serve the embedded frontend + API on an already-bound listener.
///
/// Convenience wrapper around [`dashboard_router`] so callers (e.g. the
/// `audiorouter dashboard` subcommand) don't need to depend on `axum` directly.
pub async fn serve(listener: tokio::net::TcpListener, state: DashboardState) -> anyhow::Result<()> {
    let router = dashboard_router(state);
    axum::serve(listener, router).await?;
    Ok(())
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

/// Build a router that serves the dashboard API under `/api/*` and serves the
/// embedded frontend dist (compiled in via `include_dir!`) for everything else.
///
/// This composes [`api_router`] — the API-only router — and layers static-file
/// hosting on top via a fallback handler that reads from the embedded `Dir`.
pub fn dashboard_router(state: DashboardState) -> Router {
    Router::new()
        .nest("/api", api_router(state))
        .fallback(serve_embedded)
}

/// Serve a file from the embedded `DIST_DIR`. Unknown paths fall back to
/// `index.html` so client-side routing works (SPA convention).
async fn serve_embedded(req: Request) -> Response {
    let rel = req.uri().path().trim_start_matches('/');
    if let Some(file) = DIST_DIR.get_file(rel) {
        return embedded_response(file.path(), file.contents());
    }
    if (rel.is_empty() || !looks_like_asset(rel))
        && let Some(file) = DIST_DIR.get_file("index.html")
    {
        return embedded_response(file.path(), file.contents());
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not found" })),
    )
        .into_response()
}

// ponytail: naive heuristic — anything with an extension is an asset request;
// extensionless paths are treated as SPA routes and get index.html.
fn looks_like_asset(path: &str) -> bool {
    std::path::Path::new(path).extension().is_some()
}

fn embedded_response(path: &std::path::Path, body: &[u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from(body.to_vec()));
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    response
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
    state.remember_dashboard_written_config(raw.clone());
    if let Err(error) = std::fs::write(state.config_path(), &raw) {
        state.forget_dashboard_written_config_if_matches(&raw);
        return Err(error.into());
    }
    state.emit_log(
        "info",
        format!("config saved to {}", state.config_path().display()),
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_config_path(name: &str) -> PathBuf {
        let unique = format!(
            "audiorouter-dashboard-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        );
        std::env::temp_dir().join(unique).join("config.toml")
    }

    #[test]
    fn dashboard_written_config_does_not_emit_config_changed() {
        let config_path = unique_config_path("internal-save");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let state = DashboardState::new(&config_path);
        let raw = "[[routes]]\nfrom = 'mic'\nto = 'speakers'\n".to_string();

        state.remember_dashboard_written_config(raw.clone());
        std::fs::write(&config_path, raw).unwrap();

        assert!(!state.should_emit_config_changed_after_file_event());
    }

    #[test]
    fn external_config_change_after_dashboard_save_emits_config_changed() {
        let config_path = unique_config_path("external-save");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let state = DashboardState::new(&config_path);

        state.remember_dashboard_written_config("dashboard version".to_string());
        std::fs::write(&config_path, "external version").unwrap();

        assert!(state.should_emit_config_changed_after_file_event());
    }
}
