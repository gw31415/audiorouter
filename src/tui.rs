//! Full-screen TUI using ratatui: node-graph routing visualization.
//!
//! Layout:
//!
//! ```text
//! ┌──────────────── audiorouter ──────────────────────────┐
//! │ Status bar: sample rate · buffer · uptime · routes    │
//! ├────────────────────────────────────────────────────────┤
//! │  ┌────────┐     ┌────────┐     ┌────────┐              │
//! │  │🎤 mic  │────▶│🔄 mix │────▶│🔊 out  │              │
//! │  │2ch in  │     │2+2ch   │     │2ch out │              │
//! │  │▂▅▇▅▂░░│     │▂▅█▅▂░░│     │▂▅█▅▂░░│              │
//! │  ╰────────╯     ╰────────╯     ╰────────╯              │
//! │  ┌────────┐          ┌────────┐                         │
//! │  │🎤 vt4in│─────────▶│🔊 BH   │                         │
//! │  ╰────────╯          ╰────────╯                         │
//! ├────────────────────────────────────────────────────────┤
//! │ Log / warnings                                        │
//! ├────────────────────────────────────────────────────────┤
//! │ [q]quit [r]reload [^L]reset peaks [↑↓]scroll [Esc]quit │
//! └────────────────────────────────────────────────────────┘
//! ```
//!
//! Device positions are computed by a topological layered layout
//! (see `graph` module), not a fixed Input/Both/Output grid.

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use unicode_width::UnicodeWidthStr;

use crate::audio::{AudioEngine, ConfigWatcher, EngineState};
use crate::validate::ValidatedConfig;

const TICK_RATE: Duration = Duration::from_millis(50); // 20 fps UI refresh
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(1);

// ── Semantic color palette for routing graph ──────────────────────────────
/// Node border (available device).
const COLOR_BORDER: Color = Color::Cyan;
/// Signal source: capture channels (▲) and from-side channel labels.
const COLOR_IN: Color = Color::Green;
/// Signal destination: playback channels (▼) and to-side channel labels.
const COLOR_OUT: Color = Color::Magenta;
/// Route path line and arrowhead.
const COLOR_ROUTE: Color = Color::LightBlue;
/// Gain value and limiter indicator.
const COLOR_GAIN: Color = Color::Yellow;
/// Clip / overload indicator.
const COLOR_CLIP: Color = Color::Red;

/// Run the TUI event loop over an audio engine until the user quits.
///
/// # Errors
///
/// Returns an error if terminal setup fails or a fatal audio error occurs.
pub fn run(
    mut engine: AudioEngine,
    config_path: &std::path::Path,
    warnings: &[String],
) -> Result<(), crate::error::AppError> {
    let watcher = ConfigWatcher::new(config_path);

    // Terminal setup
    enable_raw_mode().map_err(term_err)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(term_err)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(term_err)?;

    let start_time = Instant::now();
    let mut loop_state = LoopState {
        log_lines: warnings.to_vec(),
        reload_pending: None,
        reload_message: None,
        last_tick: Instant::now(),
        last_device_poll: Instant::now(),
        show_inactive_devices: false,
        show_missing_devices: true,
        config_path: config_path.to_path_buf(),
    };

    // Result stored here so we can restore the terminal before returning.
    let result = run_loop(
        &mut terminal,
        &mut engine,
        &watcher,
        start_time,
        &mut loop_state,
    );

    // Restore terminal
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

/// Mutable state carried across TUI loop iterations.
struct LoopState {
    log_lines: Vec<String>,
    reload_pending: Option<Instant>,
    reload_message: Option<String>,
    last_tick: Instant,
    last_device_poll: Instant,
    show_inactive_devices: bool,
    show_missing_devices: bool,
    config_path: std::path::PathBuf,
}

const LOG_PANEL_HEIGHT: u16 = 7;
const LOG_VISIBLE_LINES: u16 = LOG_PANEL_HEIGHT.saturating_sub(2);

/// Inner loop — separated so terminal restoration always runs.
fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    engine: &mut AudioEngine,
    watcher: &ConfigWatcher,
    start_time: Instant,
    st: &mut LoopState,
) -> Result<(), crate::error::AppError> {
    let mut scroll: u16 = 0;

    loop {
        // Poll for terminal events (keyboard input).
        let timeout = TICK_RATE
            .checked_sub(st.last_tick.elapsed())
            .unwrap_or(Duration::from_millis(0));
        if event::poll(timeout).map_err(term_err)?
            && let Event::Key(key) = event::read().map_err(term_err)?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    engine.stop();
                    break;
                }
                KeyCode::Char('r') => {
                    // Manual reload trigger.
                    st.reload_pending = Some(Instant::now());
                }
                KeyCode::Char('l')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    // Reset all peak-hold / clip indicators.
                    engine.meter_bank().reset_all_peaks();
                    st.log_lines.push(format!(
                        "[{}] peak-hold / clip reset",
                        timestamp(start_time)
                    ));
                }
                KeyCode::Char('h') => {
                    // Toggle visibility of devices not participating in any route.
                    st.show_inactive_devices = !st.show_inactive_devices;
                    st.log_lines.push(format!(
                        "[{}] inactive devices {}",
                        timestamp(start_time),
                        if st.show_inactive_devices {
                            "shown"
                        } else {
                            "hidden"
                        },
                    ));
                }
                KeyCode::Char('H') => {
                    // Toggle visibility of devices disabled by missing hardware.
                    st.show_missing_devices = !st.show_missing_devices;
                    st.log_lines.push(format!(
                        "[{}] missing devices {}",
                        timestamp(start_time),
                        if st.show_missing_devices {
                            "shown"
                        } else {
                            "hidden"
                        },
                    ));
                }
                KeyCode::Char('c')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    engine.stop();
                    break;
                }
                KeyCode::Up => {
                    scroll = clamp_log_scroll(scroll.saturating_add(1), st.log_lines.len());
                }
                KeyCode::PageUp => {
                    scroll = clamp_log_scroll(
                        scroll.saturating_add(LOG_VISIBLE_LINES),
                        st.log_lines.len(),
                    );
                }
                KeyCode::Down => {
                    scroll = scroll.saturating_sub(1);
                }
                KeyCode::PageDown => {
                    scroll = scroll.saturating_sub(LOG_VISIBLE_LINES);
                }
                _ => {}
            }
        }

        if st.last_tick.elapsed() >= TICK_RATE {
            st.last_tick = Instant::now();
        }

        scroll = clamp_log_scroll(scroll, st.log_lines.len());

        // Check for config changes.
        if watcher.poll() {
            st.reload_pending = Some(Instant::now());
            st.log_lines.push(format!(
                "[{}] config file changed — preparing to reload",
                timestamp(start_time)
            ));
        }

        // Execute debounced reload.
        if let Some(t) = st.reload_pending
            && t.elapsed() >= RELOAD_DEBOUNCE
        {
            st.reload_pending = None;
            match engine.reload() {
                Ok(()) => {
                    st.reload_message = None;
                    st.log_lines
                        .push(format!("[{}] hot-reload succeeded", timestamp(start_time)));
                }
                Err(e) => {
                    st.reload_message = Some(format!("reload failed: {e}"));
                    st.log_lines
                        .push(format!("[{}] reload failed: {e}", timestamp(start_time)));
                }
            }
        }

        // Drain tracing output into the TUI log panel. In interactive run mode,
        // the tracing subscriber writes to an in-memory buffer instead of
        // stderr so it cannot overlap the ratatui alternate screen.
        for line in crate::log_buffer::drain() {
            st.log_lines
                .push(format!("[{}] {line}", timestamp(start_time)));
        }

        // Poll physical device connectivity while running. Missing-device
        // warnings are startup-only; runtime changes are logged as concise
        // connected/disconnected events.
        if st.last_device_poll.elapsed() >= DEVICE_POLL_INTERVAL {
            st.last_device_poll = Instant::now();
            match engine.refresh_devices() {
                Ok(events) => {
                    for event in events {
                        st.log_lines
                            .push(format!("[{}] {event}", timestamp(start_time)));
                    }
                }
                Err(e) => {
                    st.log_lines.push(format!(
                        "[{}] device refresh failed: {e}",
                        timestamp(start_time)
                    ));
                }
            }
        }

        // Check engine state.
        match engine.state() {
            EngineState::FatalError => {
                st.log_lines.push(format!(
                    "[{}] fatal audio error — exiting",
                    timestamp(start_time)
                ));
                draw(
                    terminal,
                    engine,
                    start_time,
                    &st.log_lines,
                    &st.reload_message,
                    scroll,
                    st.show_inactive_devices,
                    st.show_missing_devices,
                    &st.config_path,
                )?;
                std::thread::sleep(Duration::from_secs(2));
                return Err(crate::error::AppError::runtime("fatal audio stream error"));
            }
            EngineState::Stopped => {
                break;
            }
            EngineState::Running => {}
        }

        draw(
            terminal,
            engine,
            start_time,
            &st.log_lines,
            &st.reload_message,
            scroll,
            st.show_inactive_devices,
            st.show_missing_devices,
            &st.config_path,
        )?;
    }

    Ok(())
}

fn max_log_scroll(total_lines: usize) -> u16 {
    total_lines
        .saturating_sub(LOG_VISIBLE_LINES as usize)
        .min(u16::MAX as usize) as u16
}

fn clamp_log_scroll(scroll: u16, total_lines: usize) -> u16 {
    scroll.min(max_log_scroll(total_lines))
}

/// Render one frame.
fn draw(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    engine: &AudioEngine,
    start_time: Instant,
    log_lines: &[String],
    reload_message: &Option<String>,
    scroll: u16,
    show_inactive: bool,
    show_missing: bool,
    config_path: &std::path::Path,
) -> Result<(), crate::error::AppError> {
    terminal
        .draw(|f| {
            let plan = engine.plan();
            let meter_bank = engine.meter_bank();
            let resolved = engine.resolved();

            // ── Top-level layout ──────────────────────────────────────
            let area = f.area();

            // Compact status bar for small terminals.
            if area.height < 16 {
                draw_compact(f, area, engine, start_time, plan, resolved);
                return;
            }

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),                // status/title line
                    Constraint::Min(16),                  // routing graph (node graph)
                    Constraint::Length(LOG_PANEL_HEIGHT), // log
                    Constraint::Length(1),                // help
                ])
                .split(area);

            draw_status_bar(
                f,
                chunks[0],
                plan,
                resolved,
                start_time,
                reload_message,
                config_path,
            );
            draw_routing_graph(
                f,
                chunks[1],
                plan,
                resolved,
                meter_bank,
                show_inactive,
                show_missing,
            );
            draw_log(f, chunks[2], log_lines, scroll);
            draw_help(f, chunks[3]);
        })
        .map_err(term_err)?;

    Ok(())
}

// ── Status bar ─────────────────────────────────────────────────────────────

const APP_VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

fn draw_status_bar(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
    start_time: Instant,
    _reload_message: &Option<String>,
    config_path: &std::path::Path,
) {
    let elapsed = start_time.elapsed();
    let mins = elapsed.as_secs() / 60;
    let secs = elapsed.as_secs() % 60;

    let app_label = format!("audiorouter v{APP_VERSION}");
    let stats = format!(
        "  \u{2502}  \u{1f3b5} {}kHz  \u{2502}  \u{1f39a} buf {}  \u{2502}  \u{23f1} {}m{:02}s  \u{2502}  \u{1f517} {}/{} ",
        plan.config.engine.sample_rate / 1000,
        plan.config.engine.buffer_size,
        mins,
        secs,
        resolved.active_route_count(plan),
        plan.routes.len(),
    );

    let label_w = app_label.width() as u16;
    let stats_w = stats.width() as u16;

    f.buffer_mut().set_string(
        area.x,
        area.y,
        &app_label,
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
    );
    f.buffer_mut().set_string(
        area.x + label_w,
        area.y,
        &stats,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    // Config path: right-aligned, home directory abbreviated to ~.
    let path_str = abbreviate_home(config_path);
    let path_w = path_str.width() as u16;
    let used = label_w + stats_w;
    if area.width > used + path_w {
        f.buffer_mut().set_string(
            area.x + area.width - path_w,
            area.y,
            &path_str,
            Style::default().fg(Color::DarkGray),
        );
    }
}

fn abbreviate_home(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

// ── Compact fallback for tiny terminals ─────────────────────────────────────

fn draw_compact(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    engine: &AudioEngine,
    start_time: Instant,
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
) {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "audiorouter · {}/{} routes · {} Hz · ↑{}s",
            resolved.active_route_count(plan),
            plan.routes.len(),
            plan.config.engine.sample_rate,
            start_time.elapsed().as_secs()
        ),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];

    for (i, route) in plan.routes.iter().enumerate() {
        let meter = engine.meter_bank().get(&route.from, 1);
        let level = meter.map(|m| m.snapshot().peak).unwrap_or(0.0);
        let bar_len = ((level * 10.0) as usize).min(10);
        let bar: String = "█".repeat(bar_len);
        let prefix = if resolved.route_enabled(i) {
            ""
        } else {
            "OFF "
        };
        lines.push(Line::from(format!(
            "{}{} → {} {:>6.1}dB {}",
            prefix, route.from, route.to, route.gain_db, bar
        )));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, area);
}

// ── Routing graph ──────────────────────────────────────────────────────────

fn draw_routing_graph(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
    meter_bank: &crate::meter::MeterBank,
    show_inactive: bool,
    show_missing: bool,
) {
    // Build title showing current toggle/filter state with icons.
    let mut title_parts = vec!["\u{1f500} Routing Graph".to_string()]; // 🔀
    if !show_inactive {
        title_parts.push("\u{1f648} inactive hidden".to_string()); // 🙈
    }
    if !show_missing {
        title_parts.push("\u{1f6ab} missing hidden".to_string()); // 🚫
    }
    let title = format!(" {} ", title_parts.join("  \u{2502}  "));

    let block = Block::default()
        .borders(Borders::ALL)
        .title_top(title.bold());
    f.render_widget(&block, area);
    let inner = block.inner(area);
    if inner.height < 5 || inner.width < 20 {
        return;
    }

    // If showing inactive devices, reserve the bottom rows for them.
    let inactive = crate::graph::inactive_device_names(plan);
    let inactive_area_height = if show_inactive && !inactive.is_empty() {
        // One line for the separator label + one line per device, clamped.
        let h = (inactive.len() as u16 + 1).min(inner.height / 3);
        h
    } else {
        0
    };
    let graph_area = Rect {
        height: inner.height.saturating_sub(inactive_area_height),
        ..inner
    };

    if plan.routes.is_empty() {
        let msg =
            Paragraph::new("No routes to display").style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, graph_area);
        return;
    }

    // ── Determine which devices to exclude from the layout ────────
    // Missing devices (hardware not connected) are shown by default.
    // When hidden, cascade: devices that lose all surviving routes
    // (because every route touched a missing device) are also hidden.
    let missing = resolved.missing_device_aliases();
    let exclude: std::collections::HashSet<String> = if show_missing {
        std::collections::HashSet::new()
    } else {
        crate::graph::cascade_hidden(plan, &missing)
    };

    // ── Compute topological layered layout ────────────────────────
    // Devices are placed in layers derived from the route graph, not in
    // fixed Input/Both/Output columns. See `graph::compute_layout`.
    let layout = crate::graph::compute_layout(plan, &exclude);
    if layout.is_empty() {
        return;
    }

    let max_layer = layout.iter().map(|n| n.layer).max().unwrap_or(0);
    let max_row = layout.iter().map(|n| n.row).max().unwrap_or(0);

    // ── Compute node grid positions ───────────────────────────────
    // Keep nodes compact and independent of screen size. Instead of stretching
    // routes to the full panel width, compute the minimum comfortable route gap
    // from the labels drawn on the path, then center the whole graph.
    const NODE_W: u16 = 24;
    const NODE_H: u16 = 6;
    const ROW_GAP: u16 = 1;
    const MIN_PANEL_PAD: u16 = 2;
    const MIN_ROUTE_GAP: u16 = 10;
    const MAX_ROUTE_GAP: u16 = 24;

    let node_w = NODE_W.min(graph_area.width.saturating_sub(2)).max(12);
    let node_h = NODE_H.min(graph_area.height.saturating_sub(1)).max(5);
    let layer_count = max_layer as u16 + 1;
    let row_count = max_row as u16 + 1;

    let required_route_gap = plan
        .routes
        .iter()
        .enumerate()
        .filter(|(_, r)| !exclude.contains(&r.from) && !exclude.contains(&r.to))
        .map(|(i, r)| {
            let src = channel_label(&r.from_channels).width() as u16;
            let dst = channel_label(&r.to_channels).width() as u16;
            let gain = if !resolved.route_enabled(i) {
                3 // "OFF"
            } else if r.mute {
                1 // "X"
            } else if r.gain_db == 0.0 {
                6 // "──────"
            } else {
                format!("{:+.1}dB", r.gain_db).width() as u16
            };
            src + dst + gain + 6
        })
        .max()
        .unwrap_or(MIN_ROUTE_GAP)
        .clamp(MIN_ROUTE_GAP, MAX_ROUTE_GAP);

    let available_w = graph_area.width.saturating_sub(MIN_PANEL_PAD * 2);
    let natural_graph_w = layer_count * node_w + max_layer as u16 * required_route_gap;
    let graph_w = natural_graph_w.min(available_w).max(node_w);
    let route_gap = if max_layer == 0 {
        0
    } else {
        graph_w
            .saturating_sub(layer_count * node_w)
            .checked_div(max_layer as u16)
            .unwrap_or(0)
    };
    let graph_left = graph_area.x + (graph_area.width.saturating_sub(graph_w) / 2);
    let col_x = |layer: usize| -> u16 { graph_left + layer as u16 * (node_w + route_gap) };

    let row_spacing = node_h + ROW_GAP;
    let natural_graph_h = row_count * node_h + max_row as u16 * ROW_GAP;
    let graph_h = natural_graph_h.min(graph_area.height).max(node_h);
    let graph_top = graph_area.y + (graph_area.height.saturating_sub(graph_h) / 2);

    let mut nodes: Vec<NodeInfo> = Vec::new();

    for placed in &layout {
        nodes.push(NodeInfo {
            alias: placed.alias.clone(),
            x: col_x(placed.layer),
            y: graph_top + placed.row as u16 * row_spacing,
        });
    }

    // ── Draw edges first (so nodes overlap them) ──────────────────
    for (route_index, route) in plan.routes.iter().enumerate() {
        if let (Some(src), Some(dst)) = (
            nodes.iter().find(|n| n.alias == route.from),
            nodes.iter().find(|n| n.alias == route.to),
        ) {
            let src_right = src.x + node_w;
            let src_mid_y = src.y + node_h / 2;
            let dst_left = dst.x;
            let dst_mid_y = dst.y + node_h / 2;
            // Midpoint between source right edge and destination left edge.
            let mid_x = (src_right + dst_left) / 2;
            draw_edge(
                f,
                src_right,
                src_mid_y,
                dst_left,
                dst_mid_y,
                route,
                !resolved.route_enabled(route_index),
                mid_x,
            );
        }
    }

    // ── Draw device nodes ─────────────────────────────────────────
    for node in &nodes {
        let dev = plan.device_by_name(&node.alias).unwrap();
        draw_device_node(
            f, node.x, node.y, node_w, node_h, node, dev, plan, resolved, meter_bank,
        );
    }

    // ── Draw inactive (non-routing) devices at the bottom ─────────
    if show_inactive && !inactive.is_empty() {
        draw_inactive_devices(f, inner, graph_area, &inactive, plan);
    }
}

/// Draw non-routing devices in a compact list at the bottom of the routing
/// graph panel. These devices are configured but don't participate in any
/// route — shown only when the user toggles them with `d`.
fn draw_inactive_devices(
    f: &mut ratatui::Frame<'_>,
    inner: Rect,
    graph_area: Rect,
    inactive: &[String],
    plan: &ValidatedConfig,
) {
    let label = " Inactive (no routes) ";
    let separator_y = graph_area.y + graph_area.height;
    if separator_y >= inner.y + inner.height {
        return;
    }

    f.buffer_mut().set_string(
        inner.x,
        separator_y,
        label,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let devices_per_line = (inner.width as usize / 24).max(1);
    for (i, alias) in inactive.iter().enumerate() {
        let row = i / devices_per_line;
        let col = i % devices_per_line;
        let y = separator_y + 1 + row as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let dev = plan.device_by_name(alias);
        let detail = match dev {
            Some(d) => format!("⊙ {} ({})", alias, d.device),
            None => format!("⊙ {}", alias),
        };

        let x = inner.x + (col as u16) * 24;
        f.buffer_mut()
            .set_string(x, y, &detail, Style::default().fg(Color::DarkGray));
    }
}

#[derive(Clone)]
struct NodeInfo {
    alias: String,
    x: u16,
    y: u16,
}

/// Draw a smoothstep-style edge between two nodes using Unicode box-drawing chars.
#[allow(clippy::too_many_arguments)]
fn draw_edge(
    f: &mut ratatui::Frame<'_>,
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
    route: &crate::validate::ValidatedRoute,
    disabled: bool,
    mid_x: u16,
) {
    let dim_route = disabled || route.mute;
    let route_style = if dim_route {
        Style::default().fg(COLOR_ROUTE).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(COLOR_ROUTE)
    };
    let line_h = if disabled { "┄" } else { "─" };
    let line_v = if disabled { "┆" } else { "│" };

    let gain_label = if disabled {
        "OFF".to_string()
    } else if route.mute {
        "X".to_string()
    } else if route.gain_db == 0.0 {
        "──────".to_string()
    } else {
        format!("{:+.1}dB", route.gain_db)
    };

    // Draw horizontal segments + mid connection.
    // We draw a simplified smoothstep: right from source, then vertical, then right to target.
    if y1 == y2 {
        // Same row — straight line.
        for x in x1..x2 {
            f.buffer_mut().set_string(x, y1, line_h, route_style);
        }
    } else {
        // Step path: right from source → corner → vertical → corner → right to target.
        let half1 = mid_x;
        let half2 = mid_x + 1;

        // Horizontal from source to mid.
        for x in x1..half1 {
            f.buffer_mut().set_string(x, y1, line_h, route_style);
        }
        // Corner at source side.
        let corner1 = if y2 > y1 { "┌" } else { "└" };
        f.buffer_mut().set_string(half1, y1, corner1, route_style);

        // Vertical segment.
        let (vy_start, vy_end) = if y2 > y1 { (y1 + 1, y2) } else { (y2 + 1, y1) };
        for y in vy_start..vy_end {
            f.buffer_mut().set_string(half1, y, line_v, route_style);
        }
        // Corner at target side.
        let corner2 = if y2 > y1 { "┐" } else { "┘" };
        f.buffer_mut().set_string(half1, y2, corner2, route_style);

        // Horizontal from mid to target.
        for x in half2..x2 {
            f.buffer_mut().set_string(x, y2, line_h, route_style);
        }
    }

    // Arrowhead at destination to make direction explicit.
    if x2 > x1 {
        let arrow = if disabled { "▷" } else { "▶" };
        f.buffer_mut()
            .set_string(x2.saturating_sub(1), y2, arrow, route_style);
    }

    // Place channel and gain labels along the edge with explicit ─ gaps.
    //
    // Same-row layout (left → right):
    //   ─[src_ch]─[ gain]─[dst_ch]─▶
    //
    // Cross-row layout: src_ch on source segment, dst_ch on target segment,
    // gain on the lower row alongside whichever channel label shares it.
    let src_channels = channel_label(&route.from_channels);
    let dst_channels = channel_label(&route.to_channels);
    let inactive = route.mute || disabled;
    let src_ch_style = if inactive {
        Style::default().fg(COLOR_IN).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(COLOR_IN).add_modifier(Modifier::BOLD)
    };
    let dst_ch_style = if inactive {
        Style::default().fg(COLOR_OUT).add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(COLOR_OUT).add_modifier(Modifier::BOLD)
    };
    let gain_style = if dim_route || route.gain_db == 0.0 {
        route_style // blend: dim when inactive, or 0dB dashes flow into line
    } else {
        Style::default().fg(COLOR_GAIN)
    };
    // Leading space breaks the line visually before a numeric value.
    // Omit it when the label itself is dashes — they should flow into the line.
    let gain_text = if route.gain_db == 0.0 && !disabled && !route.mute {
        gain_label.clone()
    } else {
        format!(" {}", gain_label)
    };
    let gain_w = gain_text.width() as u16;
    let src_w = src_channels.width() as u16;
    let dst_w = dst_channels.width() as u16;

    if y1 == y2 {
        // Same row: place labels sequentially with 1-cell ─ gaps.
        //   ─ [src_w] ─ [gain_w] ─ [dst_w] ─ ▶
        let row = y1;
        let src_x = x1.saturating_add(1);
        let gain_x = src_x.saturating_add(src_w + 1);
        let dst_x = gain_x.saturating_add(gain_w + 1);
        let arrow_x = x2.saturating_sub(1);

        // Always draw src (it's closest to the source node).
        f.buffer_mut()
            .set_string(src_x, row, &src_channels, src_ch_style);

        // Draw gain + dst only if they fit before the arrow with a ─ gap.
        if dst_x.saturating_add(dst_w) < arrow_x {
            f.buffer_mut()
                .set_string(gain_x, row, &gain_text, gain_style);
            f.buffer_mut()
                .set_string(dst_x, row, &dst_channels, dst_ch_style);
        } else if gain_x.saturating_add(gain_w) < arrow_x {
            // Not enough room for dst — draw gain only.
            f.buffer_mut()
                .set_string(gain_x, row, &gain_text, gain_style);
        }
    } else {
        // Cross-row: src on source horizontal segment, dst on target segment.
        let src_x = x1.saturating_add(1);
        f.buffer_mut()
            .set_string(src_x, y1, &src_channels, src_ch_style);

        let dst_x = x2.saturating_sub(dst_w + 2);
        f.buffer_mut()
            .set_string(dst_x, y2, &dst_channels, dst_ch_style);

        // Gain shares the lower row with one of the channel labels.
        let gain_row = y1.min(y2);
        let (gain_x, gain_ok) = if gain_row == y1 {
            // Gain after src on source segment (must end before the bend).
            let gx = src_x.saturating_add(src_w + 1);
            (gx, gx.saturating_add(gain_w) <= mid_x)
        } else {
            // Gain before dst on target segment (must start after the bend).
            let gx = dst_x.saturating_sub(gain_w + 1);
            (gx, gx > mid_x.saturating_add(1))
        };
        if gain_ok {
            f.buffer_mut()
                .set_string(gain_x, gain_row, &gain_text, gain_style);
        }
    }
}

fn channel_label(channels: &[usize]) -> String {
    channels
        .iter()
        .map(|ch| ch.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Draw a compact device node: name line + spectrum bar.
#[allow(clippy::too_many_arguments)]
fn draw_device_node(
    f: &mut ratatui::Frame<'_>,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    node: &NodeInfo,
    dev: &crate::validate::ResolvedDeviceRole,
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
    meter_bank: &crate::meter::MeterBank,
) {
    let w = w.max(12);

    let missing_input = resolved.unavailable_inputs.contains(&node.alias);
    let missing_output = resolved.unavailable_outputs.contains(&node.alias);
    let unavailable = missing_input || missing_output;

    let title_style = if unavailable {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    // Content is drawn FIRST and border LAST so the border is never overwritten.
    let h = h.max(5);

    // ── Content (drawn into inner area) ───────────────────────────────
    let inner_x = x + 1;
    let inner_w = w.saturating_sub(2);

    // Line 1: icon + alias.
    let max_name = (inner_w as usize).saturating_sub(2);
    let name_display = truncate_display(&node.alias, max_name);
    let title = truncate_display(&format!("⊙ {}", name_display), inner_w as usize);
    f.buffer_mut()
        .set_string(inner_x, y + 1, title, title_style);

    // Right-aligned indicators on the title line (drawn last to overwrite title text).
    // 🧱 (limiter active) at second-from-right slot, ⚡ (clip) at rightmost slot.
    if dev.limiter {
        let style = if unavailable {
            Style::default().fg(COLOR_GAIN).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(COLOR_GAIN)
        };
        f.buffer_mut()
            .set_string(x + w.saturating_sub(5), y + 1, "🧱", style);
    }
    if let (false, Some(meter)) = (unavailable, meter_bank.get(&node.alias, 1)) {
        let snap = meter.snapshot();
        if snap.clipped {
            f.buffer_mut().set_string(
                x + w.saturating_sub(3),
                y + 1,
                "⚡",
                Style::default().fg(COLOR_CLIP),
            );
        }
    }

    // Inner lines: spectrum bars or missing-device message.
    let spectrum_rows = h.saturating_sub(3).max(1);
    if unavailable {
        let missing_label = if missing_input && missing_output {
            "device missing: input + output"
        } else if missing_input {
            "device missing: input"
        } else {
            "device missing: output"
        };
        f.buffer_mut().set_string(
            inner_x,
            y + 2,
            truncate_display(missing_label, inner_w as usize),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM),
        );
    } else if let Some(meter) = meter_bank.get(&node.alias, 1) {
        let snap = meter.snapshot();
        draw_spectrum(f, inner_x, y + 2, inner_w, spectrum_rows, &snap.bands);
    }

    // ── Border (drawn LAST so it's always intact) ─────────────────────
    let border_style = if unavailable {
        Style::default()
            .fg(COLOR_BORDER)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(COLOR_BORDER)
    };
    let top_bottom = "─".repeat(inner_w as usize);
    f.buffer_mut()
        .set_string(x, y, format!("╭{}╮", top_bottom), border_style);
    f.buffer_mut()
        .set_string(x, y + h - 1, format!("╰{}╯", top_bottom), border_style);
    for ry in 1..h.saturating_sub(1) {
        f.buffer_mut().set_string(x, y + ry, "│", border_style);
        f.buffer_mut()
            .set_string(x + w - 1, y + ry, "│", border_style);
    }

    // ── Channel info overlaid on top border ───────────────────────────
    // ▲ = audio leaving the device (capture/up), ▼ = audio entering the device (playback/down).
    // Format: "▲routed/total" so both utilisation and capacity are visible at a glance.
    {
        let phys = resolved.devices.get(&node.alias);
        // Count unique channels actually routed to/from this device across
        // all active (non-disabled) routes — not the max channel index.
        let active_input_channels: std::collections::HashSet<usize> = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(i, r)| resolved.route_enabled(*i) && r.from == node.alias)
            .flat_map(|(_, r)| r.from_channels.iter().copied())
            .collect();
        let active_output_channels: std::collections::HashSet<usize> = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(i, r)| resolved.route_enabled(*i) && r.to == node.alias)
            .flat_map(|(_, r)| r.to_channels.iter().copied())
            .collect();
        let ch_in = active_input_channels.len();
        let ch_out = active_output_channels.len();
        let total_in = phys.map(|d| d.max_input_channels as usize).unwrap_or(0);
        let total_out = phys.map(|d| d.max_output_channels as usize).unwrap_or(0);

        // total=0: omit entirely. used=0 but total>0: show dimmed. used>0: show colored.
        let up_str = if total_in > 0 {
            format!("▲{}/{}", ch_in, total_in)
        } else if ch_in > 0 {
            format!("▲{}", ch_in)
        } else {
            String::new()
        };
        let down_str = if total_out > 0 {
            format!("▼{}/{}", ch_out, total_out)
        } else if ch_out > 0 {
            format!("▼{}", ch_out)
        } else {
            String::new()
        };

        let up_style = if unavailable || ch_in == 0 {
            Style::default().fg(COLOR_IN).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(COLOR_IN).add_modifier(Modifier::BOLD)
        };
        let down_style = if unavailable || ch_out == 0 {
            Style::default().fg(COLOR_OUT).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(COLOR_OUT).add_modifier(Modifier::BOLD)
        };

        let mut pos = x + 1;
        if !up_str.is_empty() {
            f.buffer_mut().set_string(pos, y, &up_str, up_style);
            pos += up_str.width() as u16;
        }
        if !up_str.is_empty() && !down_str.is_empty() {
            f.buffer_mut().set_string(pos, y, " ", border_style);
            pos += 1;
        }
        if !down_str.is_empty() {
            f.buffer_mut().set_string(pos, y, &down_str, down_style);
        }
    }
}

/// Draw a compact EQ-style spectrum using Unicode Braille cells.
///
/// Each Braille cell packs 2 horizontal samples × 4 vertical dots, so this is
/// roughly half the width and a quarter-to-third of the height of block bars.
fn draw_spectrum(f: &mut ratatui::Frame<'_>, x: u16, y: u16, w: u16, rows: u16, bands: &[f32]) {
    if bands.is_empty() || w == 0 || rows == 0 {
        return;
    }

    let cols = w as usize;
    let band_count = bands.len();
    let total_dot_rows = rows as usize * 4;

    for col in 0..cols {
        let left_idx = ((col * 2) as f32 / (cols * 2) as f32 * band_count as f32) as usize;
        let right_idx = (((col * 2 + 1) as f32 / (cols * 2) as f32) * band_count as f32) as usize;
        let left = bands[left_idx.min(band_count - 1)].clamp(0.0, 1.0);
        let right = bands[right_idx.min(band_count - 1)].clamp(0.0, 1.0);
        let color = Style::default().fg(spectrum_color(left.max(right)));
        let left_level = (left * total_dot_rows as f32).round() as usize;
        let right_level = (right * total_dot_rows as f32).round() as usize;

        for row in 0..rows as usize {
            let mut mask = 0u8;
            for dot_row in 0..4usize {
                let global_row = row * 4 + dot_row;
                let filled_from_bottom = total_dot_rows.saturating_sub(global_row);
                if left_level >= filled_from_bottom {
                    mask |= braille_dot_mask(false, dot_row);
                }
                if right_level >= filled_from_bottom {
                    mask |= braille_dot_mask(true, dot_row);
                }
            }
            let ch = char::from_u32(0x2800 + mask as u32).unwrap_or(' ');
            f.buffer_mut()
                .set_string(x + col as u16, y + row as u16, ch.to_string(), color);
        }
    }
}

fn braille_dot_mask(right_column: bool, dot_row: usize) -> u8 {
    match (right_column, dot_row) {
        (false, 0) => 0x01,
        (false, 1) => 0x02,
        (false, 2) => 0x04,
        (false, _) => 0x40,
        (true, 0) => 0x08,
        (true, 1) => 0x10,
        (true, 2) => 0x20,
        (true, _) => 0x80,
    }
}

/// Map a magnitude [0,1] to a spectrum colour (green → yellow → red).
fn spectrum_color(val: f32) -> Color {
    if val > 0.85 {
        Color::Red
    } else if val > 0.65 {
        Color::LightRed
    } else if val > 0.45 {
        Color::Yellow
    } else if val > 0.25 {
        Color::LightGreen
    } else {
        Color::Green
    }
}

// ── Log panel ──────────────────────────────────────────────────────────────

fn draw_log(f: &mut ratatui::Frame<'_>, area: Rect, log_lines: &[String], scroll: u16) {
    let block = Block::default().borders(Borders::ALL).title(" Log ");
    f.render_widget(&block, area);

    let inner = block.inner(area);
    let visible = inner.height as usize;
    let total = log_lines.len();

    let scroll = clamp_log_scroll(scroll, total) as usize;
    let start = if total > visible {
        total.saturating_sub(visible)
    } else {
        0
    };
    let start = start.saturating_sub(scroll);
    let end = (start + visible).min(total);

    let lines: Vec<Line<'_>> = log_lines[start..end]
        .iter()
        .map(|s| log_line_with_icon(s))
        .collect();

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn log_line_with_icon(s: &str) -> Line<'_> {
    let (icon, style) = log_icon_style(s);
    Line::from(vec![
        Span::styled(format!("{icon} "), style.add_modifier(Modifier::BOLD)),
        Span::styled(s.to_string(), style),
    ])
}

fn log_icon_style(s: &str) -> (&'static str, Style) {
    let lower = s.to_ascii_lowercase();

    if contains_log_level(s, "ERROR")
        || lower.contains("failed")
        || lower.contains("error")
        || lower.contains("fatal")
    {
        ("✖", Style::default().fg(Color::Red))
    } else if contains_log_level(s, "WARN") || lower.contains("warning") {
        ("⚠", Style::default().fg(Color::Yellow))
    } else if contains_log_level(s, "INFO")
        || lower.contains("connected")
        || lower.contains("succeeded")
    {
        ("●", Style::default().fg(Color::Cyan))
    } else if contains_log_level(s, "DEBUG") {
        ("◆", Style::default().fg(Color::Magenta))
    } else if contains_log_level(s, "TRACE") {
        ("◇", Style::default().fg(Color::DarkGray))
    } else if lower.contains("reload") || lower.contains("changed") {
        ("↻", Style::default().fg(Color::Yellow))
    } else {
        ("·", Style::default().fg(Color::DarkGray))
    }
}

fn contains_log_level(s: &str, level: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphabetic())
        .any(|word| word == level)
}

// ── Help bar ───────────────────────────────────────────────────────────────

fn draw_help(f: &mut ratatui::Frame<'_>, area: Rect) {
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let help = Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled("[q]", key),
        Span::raw(" quit  "),
        Span::styled("[r]", key),
        Span::raw(" reload  "),
        Span::styled("[^L]", key),
        Span::raw(" reset peaks  "),
        Span::styled("[h]", key),
        Span::raw(" toggle inactive  "),
        Span::styled("[H]", key),
        Span::raw(" toggle missing  "),
        Span::styled("[↑↓]", key),
        Span::raw(" scroll log  "),
        Span::styled("[Esc]", key),
        Span::raw(" quit  "),
    ]));
    f.render_widget(help, area);
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn truncate_display(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    const ELLIPSIS: &str = "…"; // 1 display column
    let full_width: usize = s
        .chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    if full_width <= max_width {
        return s.to_string();
    }
    let mut col = 0usize;
    let mut out = String::new();
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w + 1 > max_width {
            break;
        }
        out.push(ch);
        col += w;
    }
    out.push_str(ELLIPSIS);
    out
}

/// Timestamp since start, in MM:SS format.
fn timestamp(start: Instant) -> String {
    let s = start.elapsed().as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// Map std::io::Error to AppError.
fn term_err(e: std::io::Error) -> crate::error::AppError {
    crate::error::AppError::runtime(format!("terminal error: {e}"))
}
