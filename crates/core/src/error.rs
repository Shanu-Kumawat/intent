use std::fmt;

/// Errors shared by the CLI and GUI. `code()` is the stable machine-readable
/// identifier used in CLI JSON error output (SPEC §3).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("no task with id {0}")]
    NotFound(String),
    #[error("invalid date: {0}")]
    BadDate(String),
    #[error("invalid value: {0}")]
    BadValue(String),
    #[error("usage: {0}")]
    Usage(String),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl CoreError {
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::NotFound(_) => "not_found",
            CoreError::BadDate(_) => "bad_date",
            CoreError::BadValue(_) => "bad_value",
            CoreError::Usage(_) => "usage",
            CoreError::Db(_) => "db",
            CoreError::Io(_) => "io",
        }
    }

    /// Process exit code per SPEC §3.
    pub fn exit_code(&self) -> i32 {
        match self {
            CoreError::NotFound(_) => 3,
            CoreError::BadDate(_) | CoreError::BadValue(_) => 4,
            CoreError::Usage(_) => 2,
            CoreError::Db(_) | CoreError::Io(_) => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

/// Convenience for building `BadValue` errors.
pub fn bad_value(msg: impl fmt::Display) -> CoreError {
    CoreError::BadValue(msg.to_string())
}
