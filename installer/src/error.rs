// SPDX-License-Identifier: MIT
//! One error type that keeps the message texts of the original Python installer.
//!
//! Launchers and users read these messages, so each variant renders exactly as
//! the corresponding Python exception did with `str(error)`.
use std::{
    ffi::OsStr,
    fmt, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A refusal or failure explained to the user (Python: `PatchError`).
    Patch(String),
    /// A failed system call or I/O operation (Python: `OSError`).
    Os(OsError),
    /// Malformed data such as invalid JSON or text encoding (Python: `ValueError`).
    Value(String),
    /// A required key is missing from a state file (Python: `KeyError`).
    Key(String),
    /// A helper program failed or timed out (Python: `subprocess.SubprocessError`).
    Subprocess(String),
    /// Any other failure. Python ended these with a traceback.
    Other(String),
    /// Ctrl+C (Python: `KeyboardInterrupt`).
    Interrupted,
    /// Input ended while a question was open (Python: `EOFError`).
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsError {
    pub errno: Option<i32>,
    pub text: String,
    pub path: Option<PathBuf>,
    pub path2: Option<PathBuf>,
}

impl Error {
    pub fn patch(message: impl Into<String>) -> Self {
        Self::Patch(message.into())
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// An `OSError` without an error number, e.g. a failed download.
    pub fn os_text(text: impl Into<String>) -> Self {
        Self::Os(OsError {
            errno: None,
            text: text.into(),
            path: None,
            path2: None,
        })
    }

    pub fn errno(errno: i32, path: impl AsRef<Path>) -> Self {
        Self::Os(OsError {
            errno: Some(errno),
            text: strerror(errno),
            path: Some(path.as_ref().to_path_buf()),
            path2: None,
        })
    }

    pub fn io(error: &io::Error) -> Self {
        Self::Os(os_error(error, None, None))
    }

    pub fn io_path(error: &io::Error, path: impl AsRef<Path>) -> Self {
        Self::Os(os_error(error, Some(path.as_ref()), None))
    }

    pub fn io_paths(error: &io::Error, path: impl AsRef<Path>, path2: impl AsRef<Path>) -> Self {
        Self::Os(os_error(error, Some(path.as_ref()), Some(path2.as_ref())))
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        match self {
            Self::Os(error) => error.errno,
            _ => None,
        }
    }

    pub fn is_patch(&self) -> bool {
        matches!(self, Self::Patch(_))
    }

    pub fn is_os(&self) -> bool {
        matches!(self, Self::Os(_))
    }

    /// Python: `except (OSError, ValueError, KeyError, PatchError)`.
    pub fn is_reportable(&self) -> bool {
        matches!(
            self,
            Self::Patch(_) | Self::Os(_) | Self::Value(_) | Self::Key(_)
        )
    }

    /// Python: `except (OSError, ValueError, PatchError)`.
    pub fn is_data_error(&self) -> bool {
        matches!(self, Self::Patch(_) | Self::Os(_) | Self::Value(_))
    }
}

fn os_error(error: &io::Error, path: Option<&Path>, path2: Option<&Path>) -> OsError {
    let errno = error.raw_os_error();
    OsError {
        errno,
        text: errno.map_or_else(|| error.to_string(), strerror),
        path: path.map(Path::to_path_buf),
        path2: path2.map(Path::to_path_buf),
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::io(&error)
    }
}

impl From<rustix::io::Errno> for Error {
    fn from(error: rustix::io::Errno) -> Self {
        Self::io(&io::Error::from(error))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Patch(message)
            | Self::Value(message)
            | Self::Subprocess(message)
            | Self::Other(message) => f.write_str(message),
            Self::Os(error) => error.fmt(f),
            Self::Key(key) => f.write_str(&repr(key)),
            Self::Interrupted => f.write_str("Interrupted"),
            Self::Eof => f.write_str("EOF when reading a line"),
        }
    }
}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.errno {
            Some(errno) => write!(f, "[Errno {errno}] {}", self.text)?,
            None => f.write_str(&self.text)?,
        }
        if let Some(path) = &self.path {
            write!(f, ": {}", repr_os(path.as_os_str()))?;
            if let Some(path2) = &self.path2 {
                write!(f, " -> {}", repr_os(path2.as_os_str()))?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// glibc's messages, so that the static musl build words errors like Python did.
pub fn strerror(errno: i32) -> String {
    match usize::try_from(errno)
        .ok()
        .and_then(|index| STRERROR.get(index))
    {
        Some(text) if !text.is_empty() => (*text).to_string(),
        _ => format!("Unknown error {errno}"),
    }
}

/// Python's `repr()` of a string.
pub fn repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        push_repr_char(&mut out, c, quote);
    }
    out.push(quote);
    out
}

/// `repr()` of a file name; undecodable bytes appear as Python's surrogate escapes.
pub fn repr_os(text: &OsStr) -> String {
    let bytes = text.as_bytes();
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push(quote);
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            push_repr_char(&mut out, c, quote);
        }
        for byte in chunk.invalid() {
            out.push_str(&format!("\\udc{byte:02x}"));
        }
    }
    out.push(quote);
    out
}

fn push_repr_char(out: &mut String, c: char, quote: char) {
    match c {
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        c if c == quote => {
            out.push('\\');
            out.push(c);
        }
        ' '..='~' => out.push(c),
        c if printable(c) => out.push(c),
        c => match u32::from(c) {
            code @ ..=0xff => out.push_str(&format!("\\x{code:02x}")),
            code @ ..=0xffff => out.push_str(&format!("\\u{code:04x}")),
            code => out.push_str(&format!("\\U{code:08x}")),
        },
    }
}

/// Close to `str.isprintable()` without Unicode tables: control, format,
/// separator and private-use ranges are escaped, everything else is shown.
fn printable(c: char) -> bool {
    !matches!(u32::from(c),
        0..=0x1f | 0x7f..=0xa0 | 0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x1680 | 0x180e
        | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x206f | 0x3000 | 0xd800..=0xf8ff
        | 0xfeff | 0xfff0..=0xfffb | 0xf0000..=0x10_ffff)
}

/// Python's `repr()` of a list of strings, as used in subprocess messages.
pub fn repr_list<S: AsRef<OsStr>>(items: &[S]) -> String {
    let parts: Vec<String> = items.iter().map(|item| repr_os(item.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

const STRERROR: [&str; 134] = [
    "Success",
    "Operation not permitted",
    "No such file or directory",
    "No such process",
    "Interrupted system call",
    "Input/output error",
    "No such device or address",
    "Argument list too long",
    "Exec format error",
    "Bad file descriptor",
    "No child processes",
    "Resource temporarily unavailable",
    "Cannot allocate memory",
    "Permission denied",
    "Bad address",
    "Block device required",
    "Device or resource busy",
    "File exists",
    "Invalid cross-device link",
    "No such device",
    "Not a directory",
    "Is a directory",
    "Invalid argument",
    "Too many open files in system",
    "Too many open files",
    "Inappropriate ioctl for device",
    "Text file busy",
    "File too large",
    "No space left on device",
    "Illegal seek",
    "Read-only file system",
    "Too many links",
    "Broken pipe",
    "Numerical argument out of domain",
    "Numerical result out of range",
    "Resource deadlock avoided",
    "File name too long",
    "No locks available",
    "Function not implemented",
    "Directory not empty",
    "Too many levels of symbolic links",
    "",
    "No message of desired type",
    "Identifier removed",
    "Channel number out of range",
    "Level 2 not synchronized",
    "Level 3 halted",
    "Level 3 reset",
    "Link number out of range",
    "Protocol driver not attached",
    "No CSI structure available",
    "Level 2 halted",
    "Invalid exchange",
    "Invalid request descriptor",
    "Exchange full",
    "No anode",
    "Invalid request code",
    "Invalid slot",
    "",
    "Bad font file format",
    "Device not a stream",
    "No data available",
    "Timer expired",
    "Out of streams resources",
    "Machine is not on the network",
    "Package not installed",
    "Object is remote",
    "Link has been severed",
    "Advertise error",
    "Srmount error",
    "Communication error on send",
    "Protocol error",
    "Multihop attempted",
    "RFS specific error",
    "Bad message",
    "Value too large for defined data type",
    "Name not unique on network",
    "File descriptor in bad state",
    "Remote address changed",
    "Can not access a needed shared library",
    "Accessing a corrupted shared library",
    ".lib section in a.out corrupted",
    "Attempting to link in too many shared libraries",
    "Cannot exec a shared library directly",
    "Invalid or incomplete multibyte or wide character",
    "Interrupted system call should be restarted",
    "Streams pipe error",
    "Too many users",
    "Socket operation on non-socket",
    "Destination address required",
    "Message too long",
    "Protocol wrong type for socket",
    "Protocol not available",
    "Protocol not supported",
    "Socket type not supported",
    "Operation not supported",
    "Protocol family not supported",
    "Address family not supported by protocol",
    "Address already in use",
    "Cannot assign requested address",
    "Network is down",
    "Network is unreachable",
    "Network dropped connection on reset",
    "Software caused connection abort",
    "Connection reset by peer",
    "No buffer space available",
    "Transport endpoint is already connected",
    "Transport endpoint is not connected",
    "Cannot send after transport endpoint shutdown",
    "Too many references: cannot splice",
    "Connection timed out",
    "Connection refused",
    "Host is down",
    "No route to host",
    "Operation already in progress",
    "Operation now in progress",
    "Stale file handle",
    "Structure needs cleaning",
    "Not a XENIX named type file",
    "No XENIX semaphores available",
    "Is a named type file",
    "Remote I/O error",
    "Disk quota exceeded",
    "No medium found",
    "Wrong medium type",
    "Operation canceled",
    "Required key not available",
    "Key has expired",
    "Key has been revoked",
    "Key was rejected by service",
    "Owner died",
    "State not recoverable",
    "Operation not possible due to RF-kill",
    "Memory page has hardware error",
];

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn messages_match_python() {
        assert_eq!(
            Error::errno(2, "/a/b c").to_string(),
            "[Errno 2] No such file or directory: '/a/b c'"
        );
        assert_eq!(Error::Key("backup".into()).to_string(), "'backup'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("a'b\"c\n"), "'a\\'b\"c\\n'");
        assert_eq!(repr("ä\u{7f}\u{200b}"), "'ä\\x7f\\u200b'");
        assert_eq!(repr_list(&["cp", "-a", "a b"]), "['cp', '-a', 'a b']");
        assert_eq!(strerror(40), "Too many levels of symbolic links");
        assert_eq!(strerror(4000), "Unknown error 4000");
        let error = io::Error::from_raw_os_error(17);
        assert_eq!(
            Error::io_paths(&error, "a", "b").to_string(),
            "[Errno 17] File exists: 'a' -> 'b'"
        );
    }
}
