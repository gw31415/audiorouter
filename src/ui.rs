//! UI helpers: colored, structured output for a modern CLI feel.
//!
//! All user-facing formatting lives here so that `main.rs` and `devices.rs`
//! stay focused on logic. Colors auto-disable when stderr/stdout is not a TTY
//! or when `NO_COLOR` is set (handled by the `colored` crate).

use colored::*;

/// Print an error message with a red `✗` prefix.
pub fn error(msg: impl AsRef<str>) {
    eprintln!("{} {}", "✗".red().bold(), msg.as_ref());
}

/// Print a warning message with a yellow `⚠` prefix.
pub fn warning(msg: impl AsRef<str>) {
    eprintln!("{} {}", "⚠".yellow().bold(), msg.as_ref());
}

/// Print a success/ok message with a green `✓` prefix.
pub fn success(msg: impl AsRef<str>) {
    println!("{} {}", "✓".green().bold(), msg.as_ref());
}

/// Print a section header in bold cyan.
pub fn header(text: impl AsRef<str>) {
    println!("{}", text.as_ref().cyan().bold());
}

/// Print a blank line separator.
pub fn separator() {
    println!();
}

/// Print a list item with a right-aligned detail field.
pub fn item_with_detail(name: impl AsRef<str>, detail: impl AsRef<str>) {
    println!("  {} {}", name.as_ref().bold(), detail.as_ref().dimmed());
}

/// Print a device list entry with input/output channel counts and optional sample rates.
/// Shows a marker (e.g. "(default)") in green after the entry.
pub fn device_entry(
    name: &str,
    in_channels: Option<u16>,
    out_channels: Option<u16>,
    rates: Option<&[String]>,
    marker: Option<&str>,
) {
    let ch_str = match (in_channels, out_channels) {
        (Some(i), Some(o)) => format!("{}in {}out", i, o),
        (Some(i), None) => format!("{}in", i),
        (None, Some(o)) => format!("{}out", o),
        (None, None) => String::new(),
    };
    let mut line = format!("  {} {}", name.bold(), ch_str.magenta());
    if let Some(rates) = rates
        && !rates.is_empty()
    {
        line.push_str(&format!(" {}", rates.join(", ").dimmed()));
    }
    if let Some(m) = marker {
        line.push_str(&format!(" {}", format!("({m})").green().bold()));
    }
    println!("{line}");
}

/// Print a device entry when supported configs are unavailable.
pub fn device_entry_unavailable(name: &str, marker: Option<&str>, err: &str) {
    let mut line = format!("  {} {}", name.bold(), "(configs unavailable)".dimmed());
    if let Some(m) = marker {
        line.push_str(&format!("  {}", m.green().bold()));
    }
    line.push_str(&format!("\n    {}", err.red()));
    println!("{line}");
}
