//! Engine actor: runs AudioEngine on its own thread, exposes shared read-only
//! state and a command channel to other threads (TUI, HTTP server).

use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

use audiorouter_core::monitor::{ConfigFileWatcher, DevicePoller};

use crate::audio::{AudioEngine, EngineState};
use crate::meter::MeterBank;
use crate::{RuntimeSnapshot, devices::ResolvedAudioDevices, validate::ValidatedConfig};

// ─── Shared view ───────────────────────────────────────────────────────────

/// Read-only snapshot of engine state, updated by the engine thread.
pub struct EngineView {
    pub plan: Arc<ValidatedConfig>,
    pub resolved: Arc<ResolvedAudioDevices>,
    pub meter_bank: Arc<MeterBank>,
    pub snapshot: RuntimeSnapshot,
}

// ─── Commands ──────────────────────────────────────────────────────────────

pub enum EngineCmd {
    Reload,
    Stop,
    ResetPeaks,
}

// ─── Handle ────────────────────────────────────────────────────────────────

pub struct EngineHandle {
    pub shared: Arc<RwLock<EngineView>>,
    pub cmd_tx: mpsc::SyncSender<EngineCmd>,
    pub log_rx: mpsc::Receiver<String>,
}

// ─── Spawn ─────────────────────────────────────────────────────────────────

pub fn spawn_engine_actor(engine: AudioEngine) -> (EngineHandle, std::thread::JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::sync_channel::<EngineCmd>(16);
    let (log_tx, log_rx) = mpsc::channel::<String>();

    let initial_snapshot = engine.runtime_snapshot();
    let shared = Arc::new(RwLock::new(EngineView {
        plan: Arc::new(engine.plan().clone()),
        resolved: Arc::new(engine.resolved().clone()),
        meter_bank: engine.meter_bank().clone(),
        snapshot: initial_snapshot,
    }));
    let shared_for_thread = shared.clone();

    let thread = std::thread::spawn(move || {
        engine_loop(engine, cmd_rx, log_tx, shared_for_thread);
    });

    (
        EngineHandle {
            shared,
            cmd_tx,
            log_rx,
        },
        thread,
    )
}

// ─── Engine loop ───────────────────────────────────────────────────────────

fn engine_loop(
    mut engine: AudioEngine,
    cmd_rx: mpsc::Receiver<EngineCmd>,
    log_tx: mpsc::Sender<String>,
    shared: Arc<RwLock<EngineView>>,
) {
    let config_watcher = ConfigFileWatcher::new(engine.config_path());
    let mut device_poller = DevicePoller::new(Duration::from_secs(1));
    let mut reload_deadline: Option<Instant> = None;

    loop {
        // Process pending commands (non-blocking).
        loop {
            match cmd_rx.try_recv() {
                Ok(EngineCmd::Stop) => {
                    engine.stop();
                    update_shared(&engine, &shared);
                    return;
                }
                Ok(EngineCmd::Reload) => {
                    // Debounce: delay 100ms from first request.
                    if reload_deadline.is_none() {
                        reload_deadline = Some(Instant::now() + Duration::from_millis(100));
                    }
                }
                Ok(EngineCmd::ResetPeaks) => {
                    engine.meter_bank().reset_all_peaks();
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    engine.stop();
                    return;
                }
            }
        }

        // Config file changed on disk?
        if config_watcher.poll() {
            let _ = log_tx.send("config file changed on disk".to_string());
            if reload_deadline.is_none() {
                reload_deadline = Some(Instant::now() + Duration::from_millis(500));
            }
        }

        // Execute pending reload if debounce window has passed.
        if reload_deadline.is_some_and(|d| Instant::now() >= d) {
            reload_deadline = None;
            match engine.reload() {
                Ok(()) => {
                    let _ = log_tx.send("config reloaded".to_string());
                }
                Err(e) => {
                    let _ = log_tx.send(format!("reload error: {}", e.message));
                }
            }
            update_shared(&engine, &shared);
        }

        // Device connectivity polling.
        if let Some(events) = device_poller.poll() {
            for event in &events {
                let _ = log_tx.send(event.clone());
            }
            // refresh_devices handles its own rebuild; update shared state after.
            match engine.refresh_devices() {
                Ok(_) => {}
                Err(e) => {
                    let _ = log_tx.send(format!("device refresh error: {}", e.message));
                }
            }
            update_shared(&engine, &shared);
        }

        // Check for fatal/stop state.
        match engine.state() {
            EngineState::Running => {}
            EngineState::FatalError | EngineState::Stopped => {
                update_shared(&engine, &shared);
                return;
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

fn update_shared(engine: &AudioEngine, shared: &Arc<RwLock<EngineView>>) {
    let snapshot = engine.runtime_snapshot();
    if let Ok(mut view) = shared.write() {
        view.plan = Arc::new(engine.plan().clone());
        view.resolved = Arc::new(engine.resolved().clone());
        view.meter_bank = engine.meter_bank().clone();
        view.snapshot = snapshot;
    }
}
