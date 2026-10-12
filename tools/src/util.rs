// SPDX-License-Identifier: MIT
//! Pieces of `pathlib` and of text-mode files that the scripts relied on.
use fenix_patch::{Error, Result, fsx, py};
use std::{
    fs,
    io::{self, BufRead},
    path::Path,
};

/// `Path.mkdir(exist_ok=True)`: the parent must exist.
pub fn mkdir_exist_ok(path: &Path) -> Result<()> {
    match fsx::mkdir(path, 0o777) {
        Err(_) if path.is_dir() => Ok(()),
        other => other,
    }
}

/// `Path.mkdir(parents=True)`: the directory itself must be new.
pub fn mkdir_new(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().create(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && fsx::parent(path) != path => {
            fsx::mkdir_parents(fsx::parent(path), 0o777)?;
            fsx::mkdir(path, 0o777)
        }
        Err(error) => Err(Error::io_path(&error, path)),
        Ok(()) => Ok(()),
    }
}

/// `Path.write_text(text)`: the file is rewritten in place and keeps its mode.
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).map_err(|error| Error::io_path(&error, path))
}

/// What a text-mode file does to line ends while reading.
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `Path.read_text()` in a UTF-8 locale, line ends translated.
pub fn read_text(path: &Path) -> Result<String> {
    Ok(universal_newlines(&fsx::read_text(path)?))
}

/// The output of `subprocess.run(..., text=True)`.
pub fn decode(bytes: Vec<u8>) -> Result<String> {
    Ok(universal_newlines(&py::decode(bytes)?))
}

/// The next line of a text-mode pipe, ending in `"\n"` unless the stream
/// ended first; `None` at the end. A line ends at `"\n"`, `"\r\n"` or `"\r"`.
pub fn read_line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        let Some(end) = available
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
        else {
            if available.is_empty() {
                break;
            }
            let count = available.len();
            line.extend_from_slice(available);
            reader.consume(count);
            continue;
        };
        let carriage = available[end] == b'\r';
        line.extend_from_slice(&available[..end]);
        reader.consume(end + 1);
        // Whether "\r" is a line end of its own shows only in what follows.
        if carriage && reader.fill_buf()?.first() == Some(&b'\n') {
            reader.consume(1);
        }
        line.push(b'\n');
        return py::decode(line).map(Some);
    }
    if line.is_empty() {
        return Ok(None);
    }
    py::decode(line).map(Some)
}

/// `os.cpu_count()`: the processors that are online, whatever this process may use.
pub fn cpu_count() -> Option<usize> {
    let online = fs::read_to_string("/sys/devices/system/cpu/online").ok();
    let counted = online.as_deref().and_then(|text| {
        text.trim().split(',').try_fold(0usize, |total, range| {
            let (first, last) = range.split_once('-').unwrap_or((range, range));
            let (first, last) = (first.parse::<usize>().ok()?, last.parse::<usize>().ok()?);
            Some(total + last.checked_sub(first)? + 1)
        })
    });
    counted
        .filter(|count| *count > 0)
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::BufReader;

    /// Delivers its input in pieces, as a pipe does.
    struct Pieces(Vec<&'static [u8]>);

    impl io::Read for Pieces {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let piece = self.0.remove(0);
            buffer[..piece.len()].copy_from_slice(piece);
            Ok(piece.len())
        }
    }

    #[test]
    fn lines_end_like_in_a_python_text_stream() {
        let mut reader = BufReader::new(Pieces(vec![
            b"one\r",
            b"\ntwo\rthree\n",
            b"\n",
            b"fo",
            b"ur\r",
            b"last",
        ]));
        let mut lines = Vec::new();
        while let Some(line) = read_line(&mut reader).unwrap() {
            lines.push(line);
        }
        assert_eq!(lines, ["one\n", "two\n", "three\n", "\n", "four\n", "last"]);
        assert_eq!(universal_newlines("a\r\nb\rc\n\r\n"), "a\nb\nc\n\n");
        let mut broken = BufReader::new(&b"ok\n\xff\n"[..]);
        assert_eq!(read_line(&mut broken).unwrap().unwrap(), "ok\n");
        assert_eq!(
            read_line(&mut broken).unwrap_err().to_string(),
            "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte"
        );
    }

    #[test]
    fn directories_are_made_like_pathlib_does() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        mkdir_new(&base.join("a/b/c")).unwrap();
        assert_eq!(
            mkdir_new(&base.join("a/b/c")).unwrap_err().to_string(),
            format!("[Errno 17] File exists: '{}'", base.join("a/b/c").display())
        );
        mkdir_exist_ok(&base.join("a/b")).unwrap();
        mkdir_exist_ok(&base.join("a/new")).unwrap();
        assert_eq!(
            mkdir_exist_ok(&base.join("missing/new"))
                .unwrap_err()
                .raw_os_error(),
            Some(2)
        );
        fs::write(base.join("file"), "x").unwrap();
        assert_eq!(
            mkdir_exist_ok(&base.join("file"))
                .unwrap_err()
                .raw_os_error(),
            Some(17)
        );
        assert!(cpu_count().unwrap() >= 1);
    }
}
