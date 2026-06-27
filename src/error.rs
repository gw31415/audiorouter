//! Error categories for distinguishing exit codes.
//!
//! - [`ErrorKind::Config`] -> exit code 1 (config/validation failure)
//! - [`ErrorKind::Runtime`] -> exit code 2 (runtime / device / I-O error)

/// Error category carried alongside messages to select exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Config validation or parse failure — exit code 1.
    Config,
    /// Runtime, device, or I-O error — exit code 2.
    Runtime,
}

/// A message tagged with an [`ErrorKind`] so the caller can pick the right exit code.
#[derive(Debug)]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

impl AppError {
    pub fn config(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Config,
            message: msg.into(),
        }
    }

    pub fn runtime(msg: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Runtime,
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

/// The exit code associated with an [`ErrorKind`].
pub fn exit_code_for(kind: ErrorKind) -> i32 {
    match kind {
        ErrorKind::Config => 1,
        ErrorKind::Runtime => 2,
    }
}
