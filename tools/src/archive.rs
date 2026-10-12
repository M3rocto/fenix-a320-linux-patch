// SPDX-License-Identifier: MIT
//! Unpacks an upstream source archive, and nothing else that it may contain.
use crate::util;
use fenix_patch::{Error, Result, fsx};
use flate2::read::MultiGzDecoder;
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
    },
    path::{Component, Path, PathBuf},
};
use tar::EntryType;

fn value(message: impl Into<String>) -> Error {
    Error::Value(message.into())
}

/// A failure of the archive itself (Python: `tarfile.ReadError`).
fn unreadable(error: &io::Error) -> Error {
    Error::other(format!("The source archive cannot be read: {error}"))
}

/// `Path(name).parts`, with the root as a part of its own.
fn parts(name: &[u8]) -> Vec<&OsStr> {
    Path::new(OsStr::from_bytes(name))
        .components()
        .filter_map(|part| match part {
            Component::CurDir => None,
            other => Some(other.as_os_str()),
        })
        .collect()
}

/// Extract `archive` below the new directory `target` without its top-level
/// folder. Only files, directories and symbolic links that stay inside
/// `target` are accepted, and nothing that exists is ever replaced.
pub fn extract(archive: &Path, target: &Path) -> Result<()> {
    util::mkdir_new(target)?;
    let root = fsx::resolve(target)?;
    let mut file = File::open(archive).map_err(|error| Error::io_path(&error, archive))?;
    let mut magic = [0u8; 2];
    let compressed = file.read(&mut magic)? == 2 && magic == [0x1f, 0x8b];
    file.seek(SeekFrom::Start(0))?;
    let reader: Box<dyn Read> = if compressed {
        Box::new(MultiGzDecoder::new(BufReader::new(file)))
    } else {
        Box::new(BufReader::new(file))
    };
    let mut source = tar::Archive::new(reader);
    for entry in source.entries().map_err(|error| unreadable(&error))? {
        let mut entry = entry.map_err(|error| unreadable(&error))?;
        let mut kind = entry.header().entry_type();
        // Git's archives begin with a comment that applies to all members.
        if kind == EntryType::XGlobalHeader {
            continue;
        }
        let mut name = entry.path_bytes().into_owned();
        // The oldest tar format marks a directory by the slash after its name.
        if entry.header().as_bytes()[156] == 0 && entry.header().path_bytes().ends_with(b"/") {
            kind = EntryType::Directory;
        }
        if kind == EntryType::Directory {
            while name.ends_with(b"/") {
                name.pop();
            }
        }
        let all = parts(&name);
        let Some(parts) = all.get(1..).filter(|parts| !parts.is_empty()) else {
            continue;
        };
        let file = matches!(
            kind,
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse
        );
        if parts.contains(&OsStr::new(".."))
            || name.starts_with(b"/")
            || !(file || kind == EntryType::Directory || kind == EntryType::Symlink)
        {
            return Err(value(format!(
                "Unexpected member in upstream source archive: {}",
                String::from_utf8_lossy(&name)
            )));
        }
        let path = parts
            .iter()
            .fold(target.to_path_buf(), |path, part| path.join(part));
        let parent = fsx::parent(&path);
        if !fsx::resolve(parent)?.starts_with(&root) {
            return Err(value("Source archive path escapes its root"));
        }
        if kind == EntryType::Directory {
            fsx::mkdir_parents(&path, 0o777)?;
        } else if kind == EntryType::Symlink {
            let link = entry
                .link_name_bytes()
                .map(|link| PathBuf::from(OsString::from_vec(link.into_owned())))
                .unwrap_or_default();
            if link.is_absolute() || !fsx::resolve(&parent.join(&link))?.starts_with(&root) {
                return Err(value("Source archive symlink escapes its root"));
            }
            fsx::mkdir_parents(parent, 0o777)?;
            fsx::symlink(&link, &path)?;
        } else {
            let mode = entry.header().mode().map_err(|error| unreadable(&error))?;
            fsx::mkdir_parents(parent, 0o777)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o666)
                .open(&path)
                .map_err(|error| Error::io_path(&error, &path))?;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let count = entry
                    .read(&mut buffer)
                    .map_err(|error| unreadable(&error))?;
                if count == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|error| Error::io_path(&error, &path))?;
            }
            drop(output);
            fsx::chmod(&path, mode & 0o777)?;
        }
    }
    Ok(())
}
