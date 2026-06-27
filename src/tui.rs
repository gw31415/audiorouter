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
}

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
                KeyCode::Down | KeyCode::PageDown => {
                    scroll = scroll.saturating_add(1);
                }
                KeyCode::Up | KeyCode::PageUp => {
                    scroll = scroll.saturating_sub(1);
                }
                _ => {}
            }
        }

        if st.last_tick.elapsed() >= TICK_RATE {
            st.last_tick = Instant::now();
        }

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

            // ── Top-level layout ──────────────────────────────────────
            let area = f.area();

            // Compact status bar for small terminals.
            if area.height < 16 {
                draw_compact(f, area, engine, start_time, plan);
                return;
            }

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // status bar
                    Constraint::Min(16),   // routing graph (node graph)
                    Constraint::Length(7), // log
                    Constraint::Length(1), // help
                ])
                .split(area);

            draw_status_bar(f, chunks[0], plan, start_time, reload_message);
            draw_routing_graph(f, chunks[1], plan, meter_bank);
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
    start_time: Instant,
    reload_message: &Option<String>,
) {
    let elapsed = start_time.elapsed();
    let mins = elapsed.as_secs() / 60;
    let secs = elapsed.as_secs() % 60;

    let title = if let Some(msg) = reload_message {
        format!(
            " audiorouter · {} Hz · buffer {} · ↑{}m{:02}s · ⚠ {} ",
            plan.config.engine.sample_rate, plan.config.engine.buffer_size, mins, secs, msg
        )
    } else {
        format!(
            " audiorouter · {} Hz · buffer {} · ↑{}m{:02}s · {} routes ",
            plan.config.engine.sample_rate,
            plan.config.engine.buffer_size,
            mins,
            secs,
            plan.routes.len(),
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.bold().cyan());

    f.render_widget(block, area);
}

// ── Compact fallback for tiny terminals ─────────────────────────────────────

fn draw_compact(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    engine: &AudioEngine,
    start_time: Instant,
    plan: &ValidatedConfig,
) {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "audiorouter · {} Hz · {} routes · ↑{}s",
            plan.config.engine.sample_rate,
            plan.routes.len(),
            start_time.elapsed().as_secs()
        ),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];

    for route in &plan.routes {
        let meter = engine.meter_bank().get(&route.from, 1);
        let level = meter.map(|m| m.snapshot().peak).unwrap_or(0.0);
        let bar_len = ((level * 10.0) as usize).min(10);
        let bar: String = "█".repeat(bar_len);
        lines.push(Line::from(format!(
            "{} → {} {:>6.1}dB {}",
            route.from, route.to, route.gain_db, bar
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

    // Node height depends on channel count; assign vertical positions.
    let row_spacing = 7u16; // lines per device node (including gap)
    let node_h = 5u16; // actual node box height

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
    for route in &plan.routes {
        if let (Some(src), Some(dst)) = (
            nodes.iter().find(|n| n.alias == route.from),
            nodes.iter().find(|n| n.alias == route.to),
        ) {
            let src_right = src.x + col_w.saturating_sub(4);
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
            f,
            node.x,
            node.y,
            col_w.saturating_sub(4),
            node_h,
            node,
            dev,
            meter_bank,
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
fn draw_edge(
    f: &mut ratatui::Frame<'_>,
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
    route: &crate::validate::ValidatedRoute,
    mid_x: u16,
) {
    let color = if route.mute {
        Color::DarkGray
    } else {
        Color::LightBlue
    };

    let gain_label = if route.mute {
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
                .set_string(x, y1, "─", Style::default().fg(color));
        }
    } else {
        // Step path: right from source → corner → vertical → corner → right to target.
        let half1 = mid_x;
        let half2 = mid_x + 1;

        // Horizontal from source to mid.
        for x in x1..half1 {
            f.buffer_mut()
                .set_string(x, y1, "─", Style::default().fg(color));
        }
        // Corner at source side.
        let corner1 = if y2 > y1 { "┌" } else { "└" };
        f.buffer_mut()
            .set_string(half1, y1, corner1, Style::default().fg(color));

        // Vertical segment.
        let (vy_start, vy_end) = if y2 > y1 { (y1 + 1, y2) } else { (y2 + 1, y1) };
        for y in vy_start..vy_end {
            f.buffer_mut()
                .set_string(half1, y, "│", Style::default().fg(color));
        }
        // Corner at target side.
        let corner2 = if y2 > y1 { "┐" } else { "┘" };
        f.buffer_mut()
            .set_string(half1, y2, corner2, Style::default().fg(color));

        // Horizontal from mid to target.
        for x in half2..x2 {
            f.buffer_mut()
                .set_string(x, y2, "─", Style::default().fg(color));
        }
    }

    // Gain label at midpoint.
    let label_x = mid_x.saturating_sub(gain_label.len() as u16 / 2);
    f.buffer_mut().set_string(
        label_x,
        y1.min(y2),
        &gain_label,
        Style::default().fg(Color::Yellow),
    );
}

/// Draw a device node box with icon, name, device string, level meter, waveform.
#[allow(clippy::too_many_arguments)]
fn draw_device_node(
    f: &mut ratatui::Frame<'_>,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    node: &NodeInfo,
    dev: &crate::validate::ResolvedDeviceRole,
    meter_bank: &crate::meter::MeterBank,
) {
    let w = w.max(16);
    let h = h.max(5);

    let (icon, border_color) = match node.role {
        DeviceRole::Input => ("🎤", Color::Green),
        DeviceRole::Output => ("🔊", Color::Blue),
        DeviceRole::Both => ("🔄", Color::Magenta),
    };

    // Draw border using set_string (manual box-drawing for colored borders).
    let border_style = Style::default().fg(border_color);
    let top_bottom = "─".repeat(w.saturating_sub(2) as usize);
    f.buffer_mut()
        .set_string(x, y, format!("╭{}╮", top_bottom), border_style);
    f.buffer_mut()
        .set_string(x, y + h - 1, format!("╰{}╯", top_bottom), border_style);
    for ry in 1..h.saturating_sub(1) {
        f.buffer_mut().set_string(x, y + ry, "│", border_style);
        f.buffer_mut()
            .set_string(x + w - 1, y + ry, "│", border_style);
    }

    // Line 1: icon + alias name.
    let max_name = (w as usize).saturating_sub(6);
    let name_display = truncate_chars(&node.alias, max_name);
    f.buffer_mut().set_string(
        x + 2,
        y + 1,
        format!("{} {}", icon, name_display),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );

    // Line 2: device string (truncated).
    let max_dev = (w as usize).saturating_sub(4);
    let dev_display = truncate_chars(&dev.device, max_dev);
    f.buffer_mut().set_string(
        x + 2,
        y + 2,
        &dev_display,
        Style::default().fg(Color::DarkGray),
    );

    // Line 3: channel info + limiter badge.
    let ch_info = if node.role == DeviceRole::Input {
        format!("{}ch in", dev.required_input_channels)
    } else if node.role == DeviceRole::Output {
        let lim = if dev.limiter { " LIMIT" } else { "" };
        format!("{}ch out{}", dev.required_output_channels, lim)
    } else {
        format!(
            "{}in/{}out",
            dev.required_input_channels, dev.required_output_channels
        )
    };
    let ch_color = if dev.limiter && node.role == DeviceRole::Output {
        Color::Yellow
    } else {
        Color::Gray
    };
    f.buffer_mut()
        .set_string(x + 2, y + 3, &ch_info, Style::default().fg(ch_color));

    // Line 4: waveform from channel 1.
    if let Some(meter) = meter_bank.get(&node.alias, 1) {
        let snap = meter.snapshot();
        let wave = sparkline_from(&snap.waveform);
        let max_wave = (w as usize).saturating_sub(4);
        let wave_display = truncate_chars(&wave, max_wave);
        let wave_color = if snap.clipped {
            Color::Red
        } else if snap.peak > 0.9 {
            Color::LightRed
        } else if snap.peak > 0.5 {
            Color::Yellow
        } else {
            Color::Green
        };
        f.buffer_mut()
            .set_string(x + 2, y + 4, &wave_display, Style::default().fg(wave_color));

        // Clip indicator.
        if snap.clipped {
            f.buffer_mut()
                .set_string(x + w - 4, y + 1, "⚡", Style::default().fg(Color::Red));
        }

        // Mini level bar on the same line, right-aligned.
        let bar_max = (w as usize)
            .saturating_sub(4)
            .saturating_sub(wave_display.chars().count() + 1);
        if bar_max > 2 {
            let filled = (snap.rms.clamp(0.0, 1.0) * bar_max as f32).round() as usize;
            let hold_pos = (snap.peak_hold.clamp(0.0, 1.0) * bar_max as f32).round() as usize;
            let mut bar = String::with_capacity(bar_max);
            for i in 0..bar_max {
                if i == hold_pos && hold_pos > 0 && hold_pos < bar_max {
                    bar.push('│'); // peak-hold marker
                } else if i < filled {
                    bar.push('■');
                } else {
                    bar.push(' ');
                }
            }
            let bar_x = x + 2 + wave_display.chars().count() as u16 + 1;
            f.buffer_mut()
                .set_string(bar_x, y + 4, &bar, Style::default().fg(wave_color));
        }
    }
}

// ── Log panel ──────────────────────────────────────────────────────────────

fn draw_log(f: &mut ratatui::Frame<'_>, area: Rect, log_lines: &[String], scroll: u16) {
    let block = Block::default().borders(Borders::ALL).title(" Log ");
    f.render_widget(&block, area);

    let inner = block.inner(area);
    let visible = inner.height as usize;
    let total = log_lines.len();

    let start = if total > visible {
        total.saturating_sub(visible)
    } else {
        0
    };
    let start = start.saturating_sub(scroll as usize);
    let end = (start + visible).min(total);

    let lines: Vec<Line<'_>> = log_lines[start..end]
        .iter()
        .map(|s| {
            if s.contains("failed") || s.contains("error") || s.contains("fatal") {
                Line::from(Span::styled(s.clone(), Style::default().fg(Color::Red)))
            } else if s.contains("reload") || s.contains("changed") {
                Line::from(Span::styled(s.clone(), Style::default().fg(Color::Yellow)))
            } else {
                Line::from(s.clone())
            }
        })
        .collect();

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
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

/// Convert a waveform history slice into a text bar string.
/// Maps each sample to a vertical bar character for a scrolling oscilloscope effect.
fn sparkline_from(waveform: &[f32]) -> String {
    const BARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    waveform
        .iter()
        .map(|&v| {
            let idx = ((v.clamp(0.0, 1.0) * (BARS.len() - 1) as f32).round() as usize)
                .min(BARS.len() - 1);
            BARS[idx]
        })
        .collect()
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
