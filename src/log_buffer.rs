//! In-memory tracing sink for the ratatui log panel.
//!
//! When `audiorouter run` owns an interactive terminal, tracing output must not
//! go to stderr because it would overlap the alternate-screen TUI. Instead, the
//! tracing subscriber writes formatted log lines into this process-local buffer;
//! the TUI drains it each frame and renders the lines in the log panel.

use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

static LOG_LINES: OnceLock<Arc<Mutex<Vec<String>>>> = OnceLock::new();

/// Initialise the global TUI log buffer if it has not already been created.
pub fn init() -> Arc<Mutex<Vec<String>>> {
    LOG_LINES
        .get_or_init(|| Arc::new(Mutex::new(Vec::new())))
        .clone()
}

/// Drain all buffered tracing lines for display in the TUI log panel.
pub fn drain() -> Vec<String> {
    let Some(lines) = LOG_LINES.get() else {
        return Vec::new();
    };
    let Ok(mut guard) = lines.lock() else {
        return Vec::new();
    };
    guard.drain(..).collect()
}

fn push_line(line: String) {
    if line.trim().is_empty() {
        return;
    }
    let lines = init();
    if let Ok(mut guard) = lines.lock() {
        guard.push(line);
    }
}

/// `tracing_subscriber` writer factory for TUI mode.
#[derive(Clone, Copy, Debug, Default)]
pub struct TuiLogMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TuiLogMakeWriter {
    type Writer = TuiLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        TuiLogWriter { buf: Vec::new() }
    }
}

/// Per-event writer. `tracing_subscriber` may write in chunks, so it buffers
/// until newline or drop, then appends complete lines to the global buffer.
pub struct TuiLogWriter {
    buf: Vec<u8>,
}

impl Write for TuiLogWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line).trim_end().to_string();
            push_line(line);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_partial();
        Ok(())
    }
}

impl TuiLogWriter {
    fn flush_partial(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.buf).trim_end().to_string();
        self.buf.clear();
        push_line(line);
    }
}

impl Drop for TuiLogWriter {
    fn drop(&mut self) {
        self.flush_partial();
    }
}
