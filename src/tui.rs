//! Full-screen TUI using ratatui: node-graph routing visualization.
//!
//! Layout:
//!
//! ```text
//! ┌──────────────── audiorouter ──────────────────────────┐
//! │ Status bar: sample rate · buffer · uptime · routes    │
//! ├────────────────────────────────────────────────────────┤
//! │   INPUT         BOTH           OUTPUT                  │
//! │  ┌────────┐                   ┌────────┐              │
//! │  │🎤 mic  │──────────────────▶│🔊 BH   │              │
//! │  │BuiltIn│     +3.0dB         │VT-4    │              │
//! │  │2ch in  │                   │2ch out │              │
//! │  │▂▅▇▅▂░░│                   │▂▅█▅▂░░│              │
//! │  ╰────────╯                   ╰────────╯              │
//! │  ┌────────┐    ┌────────┐    ┌────────┐               │
//! │  │🎤 vt4in│───▶│🔄 mix │───▶│🔊 out  │               │
//! │  ╰────────╯    ╰────────╯    ╰────────╯               │
//! ├────────────────────────────────────────────────────────┤
//! │ Log / warnings                                        │
//! ├────────────────────────────────────────────────────────┤
//! │ [q]quit [r]reload [R]reset peaks [↑↓]scroll [Esc]quit │
//! └────────────────────────────────────────────────────────┘
//! ```

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

use crate::audio::{AudioEngine, ConfigWatcher, EngineState};
use crate::validate::ValidatedConfig;

const TICK_RATE: Duration = Duration::from_millis(50); // 20 fps UI refresh
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(1);

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
                KeyCode::Char('R') => {
                    // Reset all peak-hold / clip indicators.
                    engine.meter_bank().reset_all_peaks();
                    st.log_lines.push(format!(
                        "[{}] peak-hold / clip reset",
                        timestamp(start_time)
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

            draw_status_bar(f, chunks[0], plan, resolved, start_time, reload_message);
            draw_routing_graph(f, chunks[1], plan, resolved, meter_bank);
            draw_log(f, chunks[2], log_lines, scroll);
            draw_help(f, chunks[3]);
        })
        .map_err(term_err)?;

    Ok(())
}

// ── Status bar ─────────────────────────────────────────────────────────────

fn draw_status_bar(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
    start_time: Instant,
    _reload_message: &Option<String>,
) {
    let elapsed = start_time.elapsed();
    let mins = elapsed.as_secs() / 60;
    let secs = elapsed.as_secs() % 60;

    let title = format!(
        " audiorouter · {} Hz · buffer {} · ↑{}m{:02}s · {}/{} routes ",
        plan.config.engine.sample_rate,
        plan.config.engine.buffer_size,
        mins,
        secs,
        resolved.active_route_count(plan),
        plan.routes.len(),
    );

    f.buffer_mut().set_string(
        area.x,
        area.y,
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
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
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title_top(" Routing Graph ".bold());
    f.render_widget(&block, area);
    let inner = block.inner(area);
    if inner.height < 5 || inner.width < 20 {
        return;
    }

    // ── Classify devices into input / output / both ───────────────
    let inputs: Vec<&str> = plan
        .devices
        .iter()
        .filter(|d| d.needs_input && !d.needs_output)
        .map(|d| d.name.as_str())
        .collect();
    let outputs: Vec<&str> = plan
        .devices
        .iter()
        .filter(|d| d.needs_output && !d.needs_input)
        .map(|d| d.name.as_str())
        .collect();
    let both: Vec<&str> = plan
        .devices
        .iter()
        .filter(|d| d.needs_input && d.needs_output)
        .map(|d| d.name.as_str())
        .collect();

    if plan.routes.is_empty() {
        let msg =
            Paragraph::new("No routes to display").style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, inner);
        return;
    }

    // ── Compute node grid positions ───────────────────────────────
    // Three columns: input | both | output.
    let col_w = inner.width / 3;
    let col_x = [inner.x, inner.x + col_w, inner.x + col_w * 2];

    // Node size. Terminal cells are taller than they are wide, so approximate
    // a visual 4:3 node by using about 3/8 of the text width as row height.
    let node_w = col_w.saturating_sub(4).max(20);
    let ratio_h = ((node_w as f32) * 3.0 / 8.0).round() as u16;
    let max_nodes_in_column = inputs.len().max(both.len()).max(outputs.len()).max(1) as u16;
    let fit_h = inner
        .height
        .saturating_sub(1)
        .checked_div(max_nodes_in_column)
        .unwrap_or(8)
        .saturating_sub(1);
    let node_h = ratio_h.clamp(8, 18).min(fit_h.max(8));
    let row_spacing = node_h + 1; // node plus a one-line gap

    let mut nodes: Vec<NodeInfo> = Vec::new();

    for (i, &alias) in inputs.iter().enumerate() {
        nodes.push(NodeInfo {
            alias: alias.to_string(),
            role: DeviceRole::Input,
            x: col_x[0] + 2,
            y: inner.y + 1 + (i as u16) * row_spacing,
        });
    }
    for (i, &alias) in both.iter().enumerate() {
        nodes.push(NodeInfo {
            alias: alias.to_string(),
            role: DeviceRole::Both,
            x: col_x[1] + 2,
            y: inner.y + 1 + (i as u16) * row_spacing,
        });
    }
    for (i, &alias) in outputs.iter().enumerate() {
        nodes.push(NodeInfo {
            alias: alias.to_string(),
            role: DeviceRole::Output,
            x: col_x[2] + 2,
            y: inner.y + 1 + (i as u16) * row_spacing,
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
            draw_edge(
                f,
                src_right,
                src_mid_y,
                dst_left,
                dst_mid_y,
                route,
                !resolved.route_enabled(route_index),
                col_x[1] + col_w / 2,
            );
        }
    }

    // ── Draw column headers ───────────────────────────────────────
    let headers = [
        (" INPUT ", Color::Green),
        (" BOTH ", Color::Magenta),
        (" OUTPUT ", Color::Blue),
    ];
    for (i, (label, color)) in headers.iter().enumerate() {
        let header_y = inner.y;
        f.buffer_mut().set_span(
            col_x[i] + col_w / 2 - label.len() as u16 / 2,
            header_y,
            &Span::styled(
                *label,
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            ),
            label.len() as u16,
        );
    }

    // ── Draw device nodes ─────────────────────────────────────────
    for node in &nodes {
        let dev = plan.device_by_name(&node.alias).unwrap();
        draw_device_node(
            f, node.x, node.y, node_w, node_h, node, dev, plan, resolved, meter_bank,
        );
    }
}

#[derive(Clone)]
struct NodeInfo {
    alias: String,
    role: DeviceRole,
    x: u16,
    y: u16,
}

#[derive(Clone, Copy, PartialEq)]
enum DeviceRole {
    Input,
    Output,
    Both,
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
    let color = if disabled || route.mute {
        Color::DarkGray
    } else {
        Color::LightBlue
    };
    let line_h = if disabled { "┄" } else { "─" };
    let line_v = if disabled { "┆" } else { "│" };

    let gain_label = if disabled {
        "OFF".to_string()
    } else if route.mute {
        "X".to_string()
    } else {
        format!("{:+.1}dB", route.gain_db)
    };

    // Draw horizontal segments + mid connection.
    // We draw a simplified smoothstep: right from source, then vertical, then right to target.
    if y1 == y2 {
        // Same row — straight line.
        for x in x1..x2 {
            f.buffer_mut()
                .set_string(x, y1, line_h, Style::default().fg(color));
        }
    } else {
        // Step path: right from source → corner → vertical → corner → right to target.
        let half1 = mid_x;
        let half2 = mid_x + 1;

        // Horizontal from source to mid.
        for x in x1..half1 {
            f.buffer_mut()
                .set_string(x, y1, line_h, Style::default().fg(color));
        }
        // Corner at source side.
        let corner1 = if y2 > y1 { "┌" } else { "└" };
        f.buffer_mut()
            .set_string(half1, y1, corner1, Style::default().fg(color));

        // Vertical segment.
        let (vy_start, vy_end) = if y2 > y1 { (y1 + 1, y2) } else { (y2 + 1, y1) };
        for y in vy_start..vy_end {
            f.buffer_mut()
                .set_string(half1, y, line_v, Style::default().fg(color));
        }
        // Corner at target side.
        let corner2 = if y2 > y1 { "┐" } else { "┘" };
        f.buffer_mut()
            .set_string(half1, y2, corner2, Style::default().fg(color));

        // Horizontal from mid to target.
        for x in half2..x2 {
            f.buffer_mut()
                .set_string(x, y2, line_h, Style::default().fg(color));
        }
    }

    // Channel labels at both endpoints: source = from_channels, target = to_channels.
    // These are drawn on top of the route line so the channel mapping is visible
    // without opening the config.
    let src_channels = channel_label(&route.from_channels);
    let dst_channels = channel_label(&route.to_channels);
    let channel_style = Style::default()
        .fg(if route.mute || disabled {
            Color::DarkGray
        } else {
            Color::Cyan
        })
        .add_modifier(Modifier::BOLD);

    if x2 > x1 + 2 {
        let src_x = x1.saturating_add(1);
        let dst_x = x2.saturating_sub(dst_channels.len() as u16 + 1);
        f.buffer_mut()
            .set_string(src_x, y1, &src_channels, channel_style);
        f.buffer_mut()
            .set_string(dst_x, y2, &dst_channels, channel_style);
    }

    // Gain label at midpoint.
    let label_x = mid_x.saturating_sub(gain_label.len() as u16 / 2);
    f.buffer_mut().set_string(
        label_x,
        y1.min(y2),
        &gain_label,
        Style::default().fg(if disabled {
            Color::DarkGray
        } else {
            Color::Yellow
        }),
    );
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
    let w = w.max(20);

    let missing_input = resolved.unavailable_inputs.contains(&node.alias);
    let missing_output = resolved.unavailable_outputs.contains(&node.alias);
    let unavailable = missing_input || missing_output;

    let (icon, role_color) = match node.role {
        DeviceRole::Input => ("🎤", Color::Green),
        DeviceRole::Output => ("🔊", Color::Blue),
        DeviceRole::Both => ("🔄", Color::Magenta),
    };
    let border_color = if unavailable {
        Color::DarkGray
    } else {
        role_color
    };
    let title_style = if unavailable {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };

    // Content is drawn FIRST and border LAST so the border is never overwritten.
    let h = h.max(8);

    // ── Content (drawn into inner area) ───────────────────────────────
    let inner_x = x + 1;
    let inner_w = w.saturating_sub(2);

    // Line 1: icon + alias + metadata.
    let meta = device_metadata(node, dev, plan, resolved);
    let title_reserved = icon.chars().count() + meta.chars().count() + 3;
    let max_name = inner_w as usize;
    let max_name = max_name.saturating_sub(title_reserved);
    let name_display = truncate_chars(&node.alias, max_name);
    let title = truncate_chars(
        &format!("{} {} {}", icon, name_display, meta),
        inner_w as usize,
    );
    f.buffer_mut()
        .set_string(inner_x, y + 1, title, title_style);

    // Clip indicator.
    if let (false, Some(meter)) = (unavailable, meter_bank.get(&node.alias, 1)) {
        let snap = meter.snapshot();
        if snap.clipped {
            f.buffer_mut().set_string(
                x + w.saturating_sub(3),
                y + 1,
                "⚡",
                Style::default().fg(Color::Red),
            );
        }
    }

    // Remaining inner lines: spectrum bars or missing-device message.
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
            truncate_chars(missing_label, inner_w as usize),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        );
    } else if let Some(meter) = meter_bank.get(&node.alias, 1) {
        let snap = meter.snapshot();
        draw_spectrum(f, inner_x, y + 2, inner_w, spectrum_rows, &snap.bands);
    }

    // ── Border (drawn LAST so it's always intact) ─────────────────────
    let border_style = Style::default().fg(border_color);
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
}

/// Compact metadata shown in each node title.
///
/// Device-level limiter is shown as `LIM`. Route-level mute is summarised as
/// `M0` or `M<n>/<total>` for all routes touching this device.
fn device_metadata(
    node: &NodeInfo,
    dev: &crate::validate::ResolvedDeviceRole,
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
) -> String {
    let mut tags = Vec::new();

    match node.role {
        DeviceRole::Input => tags.push(format!("I{}", dev.required_input_channels)),
        DeviceRole::Output => tags.push(format!("O{}", dev.required_output_channels)),
        DeviceRole::Both => tags.push(format!(
            "I{} O{}",
            dev.required_input_channels, dev.required_output_channels
        )),
    }

    if resolved.unavailable_inputs.contains(&node.alias) {
        tags.push("NO IN".to_string());
    }
    if resolved.unavailable_outputs.contains(&node.alias) {
        tags.push("NO OUT".to_string());
    }

    if dev.limiter {
        tags.push("LIM".to_string());
    }

    let mut route_count = 0usize;
    let mut muted_count = 0usize;
    for route in &plan.routes {
        if route.from == node.alias || route.to == node.alias {
            route_count += 1;
            if route.mute {
                muted_count += 1;
            }
        }
    }

    if route_count > 0 {
        if muted_count == 0 {
            tags.push("M0".to_string());
        } else {
            tags.push(format!("M{muted_count}/{route_count}"));
        }
    }

    format!("[{}]", tags.join(" "))
}

/// Draw a mini EQ-style spectrum: horizontal axis = Hz, vertical = magnitude.
/// Bars grow from the bottom row upward, like a hardware EQ meter.
/// `rows` = number of text rows (each row contributes 8 sub-levels).
fn draw_spectrum(f: &mut ratatui::Frame<'_>, x: u16, y: u16, w: u16, rows: u16, bands: &[f32]) {
    if bands.is_empty() || w < 4 || rows == 0 {
        return;
    }

    let cols = w as usize;
    let band_count = bands.len();
    let total_levels = (rows as usize) * 8;
    const HALF: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    for col in 0..cols {
        let band_f = col as f32 / cols as f32 * band_count as f32;
        let band_idx = (band_f as usize).min(band_count - 1);
        let val = bands[band_idx].clamp(0.0, 1.0);
        let level = (val * total_levels as f32).round() as usize;
        let color = Style::default().fg(spectrum_color(val));

        let full_rows = level / 8; // complete rows from the bottom
        let partial = level % 8; // remainder in the transition row

        for row in 0..rows as usize {
            // Row 0 = top of display, row (rows-1) = bottom.
            let rows_from_bottom = rows as usize - 1 - row;

            let ch = if rows_from_bottom < full_rows {
                '█'
            } else if rows_from_bottom == full_rows && partial > 0 {
                HALF[partial - 1]
            } else {
                ' '
            };

            f.buffer_mut()
                .set_string(x + col as u16, y + row as u16, ch.to_string(), color);
        }
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
    let help = Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "[q]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" quit  "),
        Span::styled(
            "[r]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" reload  "),
        Span::styled(
            "[↑↓]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" scroll log  "),
        Span::styled(
            "[Esc]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" quit  "),
    ]));
    f.render_widget(help, area);
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Truncate a string to at most `max_chars` Unicode characters (not bytes).
/// This is UTF-8 safe — never panics on multi-byte characters like `▁▂▃▄▅▆▇█`.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
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
