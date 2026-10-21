use std::fmt;
use std::io;

/// Why an operation stopped. Cancellation and deadlines are reported apart from
/// ordinary failures so callers can map them to exit code 130 or partial results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other,
    Cancelled,
    Timeout,
}

/// One error type for the whole program. `code` carries the portable errno name
/// (ENOENT, EACCES, …) of filesystem failures, which several callers act on.
#[derive(Debug, Clone)]
pub struct Error {
    pub kind: Kind,
    pub message: String,
    pub code: Option<&'static str>,
    /// Files that were saved before the failure (playlist entries, verification).
    pub files: Vec<String>,
    pub verification_failed: bool,
    /// Byte offset for configuration syntax errors.
    pub position: Option<usize>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn msg(message: impl Into<String>) -> Self {
        Error { kind: Kind::Other, message: message.into(), code: None, files: Vec::new(), verification_failed: false, position: None }
    }

    pub fn cancelled() -> Self {
        Error { kind: Kind::Cancelled, ..Error::msg("Cancelled") }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Error { kind: Kind::Timeout, ..Error::msg(message) }
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == Kind::Cancelled
    }

    pub fn is_code(&self, code: &str) -> bool {
        self.code == Some(code)
    }

    pub fn not_found(&self) -> bool {
        self.is_code("ENOENT")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Portable errno name for an I/O error, matching what Node reported before.
pub fn io_code(error: &io::Error) -> Option<&'static str> {
    #[cfg(windows)]
    if let Some(raw) = error.raw_os_error() {
        match raw {
            2 | 3 => return Some("ENOENT"),
            5 => return Some("EPERM"),
            17 => return Some("EXDEV"),
            32 | 33 => return Some("EBUSY"),
            39 | 112 => return Some("ENOSPC"),
            1 | 50 => return Some("ENOTSUP"),
            80 | 183 => return Some("EEXIST"),
            145 => return Some("ENOTEMPTY"),
            267 => return Some("ENOTDIR"),
            1142 => return Some("EMLINK"),
            _ => {}
        }
    }
    #[cfg(unix)]
    if let Some(raw) = error.raw_os_error() {
        let code = match raw {
            libc::ENOENT => "ENOENT",
            libc::EACCES => "EACCES",
            libc::EPERM => "EPERM",
            libc::EEXIST => "EEXIST",
            libc::EXDEV => "EXDEV",
            libc::EISDIR => "EISDIR",
            libc::ENOTDIR => "ENOTDIR",
            libc::ENOSPC => "ENOSPC",
            libc::ENOTEMPTY => "ENOTEMPTY",
            libc::EMLINK => "EMLINK",
            libc::ENOSYS => "ENOSYS",
            libc::EINVAL => "EINVAL",
            libc::EBUSY => "EBUSY",
            libc::ELOOP => "ELOOP",
            libc::ESRCH => "ESRCH",
            x if x == libc::ENOTSUP || x == libc::EOPNOTSUPP => "ENOTSUP",
            _ => "",
        };
        if !code.is_empty() {
            return Some(code);
        }
    }
    Some(match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::CrossesDevices => "EXDEV",
        io::ErrorKind::IsADirectory => "EISDIR",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        io::ErrorKind::StorageFull => "ENOSPC",
        io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY",
        io::ErrorKind::Unsupported => "ENOTSUP",
        io::ErrorKind::InvalidInput => "EINVAL",
        io::ErrorKind::TimedOut => "ETIMEDOUT",
        io::ErrorKind::ConnectionReset => "ECONNRESET",
        io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
        _ => return None,
    })
}

/// Locale-independent description of common errno codes (as Node printed them).
fn describe(code: &str) -> Option<&'static str> {
    Some(match code {
        "ENOENT" => "no such file or directory",
        "EACCES" => "permission denied",
        "EPERM" => "operation not permitted",
        "EEXIST" => "file already exists",
        "ENOTDIR" => "not a directory",
        "EISDIR" => "illegal operation on a directory",
        "ENOTEMPTY" => "directory not empty",
        "ENOSPC" => "no space left on device",
        "EXDEV" => "cross-device link not permitted",
        "EBUSY" => "resource busy or locked",
        _ => return None,
    })
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        let code = io_code(&error);
        let message = match code.and_then(|code| describe(code).map(|text| format!("{code}: {text}"))) {
            Some(message) => message,
            None => error.to_string(),
        };
        Error { code, ..Error::msg(message) }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error { code: Some("EJSON"), ..Error::msg(error.to_string()) }
    }
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Io(io) => io.into(),
            ureq::Error::Timeout(_) => Error::timeout("request timed out"),
            other => Error::msg(format!("fetch failed: {other}")),
        }
    }
}

#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::error::Error::msg(format!($($arg)*))) };
}

#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => { $crate::error::Error::msg(format!($($arg)*)) };
}

/// `Ok(None)` for a missing path, the error otherwise.
pub fn optional<T>(result: io::Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
