// SPDX-License-Identifier: MIT
//! File operations with the semantics and error texts of `os`, `pathlib` and `shutil`.
//!
//! The installer writes into Steam libraries and Wine profiles. The checks in
//! here decide what may be replaced, so each one follows the original closely.
use crate::{
    error::{Error, Result},
    json::{self, Value},
    py,
};
use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, XattrFlags};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
};

pub const MIB: u64 = 1024 * 1024;

/// `Path(text)`: repeated and trailing slashes and `.` components are dropped.
pub fn path(text: impl AsRef<OsStr>) -> PathBuf {
    let text = text.as_ref();
    let mut out = PathBuf::new();
    for component in Path::new(text).components() {
        if component != Component::CurDir {
            out.push(component);
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `Path.name` for messages.
pub fn name(path: &Path) -> String {
    match path.components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        Some(Component::ParentDir) => "..".to_string(),
        _ => String::new(),
    }
}

/// `str(path)` for messages and state files.
pub fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `Path.parent`: the root is its own parent.
pub fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => path,
    }
}

/// The user's home directory as `Path.home()` determines it.
pub fn home(var: Option<OsString>) -> PathBuf {
    match var {
        Some(home) => {
            let bytes = home.as_bytes();
            let end = bytes
                .iter()
                .rposition(|byte| *byte != b'/')
                .map_or(0, |index| index + 1);
            if end == 0 {
                PathBuf::from("/")
            } else {
                path(OsStr::from_bytes(&bytes[..end]))
            }
        }
        None => std::env::home_dir().map_or_else(|| PathBuf::from("/"), path),
    }
}

/// `Path(text).expanduser()`.
pub fn expanduser(text: impl AsRef<OsStr>, home_var: Option<OsString>) -> PathBuf {
    let normal = path(text);
    let mut components = normal.components();
    match components.next() {
        Some(Component::Normal(first)) if first == "~" => {
            path(home(home_var).join(components.as_path()))
        }
        Some(Component::Normal(first)) if first.as_bytes().starts_with(b"~") => {
            match passwd_home(&first.as_bytes()[1..]) {
                Some(directory) => path(directory.join(components.as_path())),
                None => normal,
            }
        }
        _ => normal,
    }
}

fn passwd_home(user: &[u8]) -> Option<PathBuf> {
    let table = fs::read("/etc/passwd").ok()?;
    table.split(|byte| *byte == b'\n').find_map(|line| {
        let fields: Vec<&[u8]> = line.split(|byte| *byte == b':').collect();
        (fields.len() >= 6 && fields[0] == user).then(|| path(OsStr::from_bytes(fields[5])))
    })
}

/// `os.path.realpath`. With `strict`, a missing component or a symlink loop is an error.
pub fn realpath(input: &Path, strict: bool) -> Result<PathBuf> {
    let text = input.as_os_str().as_bytes();
    let mut resolved: Vec<u8> = if text.starts_with(b"/") {
        b"/".to_vec()
    } else {
        std::env::current_dir()?.into_os_string().into_vec()
    };
    // Unresolved parts, last first. `None` marks the end of a link target; the
    // entry below it is the link whose resolution then becomes known.
    let mut rest: Vec<Option<Vec<u8>>> = text
        .split(|byte| *byte == b'/')
        .rev()
        .map(|part| Some(part.to_vec()))
        .collect();
    let mut remaining = rest.len();
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    while remaining > 0 {
        let Some(entry) = rest.pop() else { break };
        let Some(part) = entry else {
            if let Some(Some(link)) = rest.pop() {
                seen.insert(link, Some(resolved.clone()));
            }
            continue;
        };
        remaining -= 1;
        if part.is_empty() || part == b"." {
            continue;
        }
        if part == b".." {
            let cut = resolved.iter().rposition(|byte| *byte == b'/').unwrap_or(0);
            resolved.truncate(cut.max(1));
            continue;
        }
        let mut candidate = resolved.clone();
        if candidate != b"/" {
            candidate.push(b'/');
        }
        candidate.extend_from_slice(&part);
        let candidate_path = PathBuf::from(OsString::from_vec(candidate.clone()));
        let target = match fs::symlink_metadata(&candidate_path) {
            Err(error) if strict => return Err(Error::io_path(&error, &candidate_path)),
            Err(_) => None,
            Ok(info) if !info.file_type().is_symlink() => {
                if strict && remaining > 0 && !info.is_dir() {
                    return Err(Error::errno(20, &candidate_path));
                }
                resolved = candidate;
                continue;
            }
            Ok(_) => match seen.get(&candidate) {
                Some(Some(known)) => {
                    resolved = known.clone();
                    continue;
                }
                Some(None) => {
                    // Seen but not resolved: a loop.
                    if strict {
                        return Err(Error::errno(40, &candidate_path));
                    }
                    resolved = candidate;
                    continue;
                }
                None => match fs::read_link(&candidate_path) {
                    Ok(target) => Some(target.into_os_string().into_vec()),
                    Err(error) if strict => return Err(Error::io_path(&error, &candidate_path)),
                    Err(_) => None,
                },
            },
        };
        let Some(target) = target else {
            resolved = candidate;
            continue;
        };
        if target.starts_with(b"/") {
            resolved = b"/".to_vec();
        }
        seen.insert(candidate.clone(), None);
        rest.push(Some(candidate));
        rest.push(None);
        let parts: Vec<&[u8]> = target.split(|byte| *byte == b'/').collect();
        remaining += parts.len();
        rest.extend(parts.into_iter().rev().map(|part| Some(part.to_vec())));
    }
    Ok(PathBuf::from(OsString::from_vec(resolved)))
}

/// `Path.resolve()`: symlinks followed as far as the path exists.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    realpath(path, false)
}

/// `Path.resolve(strict=True)`.
pub fn resolve_strict(path: &Path) -> Result<PathBuf> {
    realpath(path, true)
}

pub fn lstat(path: &Path) -> Result<Metadata> {
    fs::symlink_metadata(path).map_err(|error| Error::io_path(&error, path))
}

pub fn stat(path: &Path) -> Result<Metadata> {
    fs::metadata(path).map_err(|error| Error::io_path(&error, path))
}

/// `Path.exists()`: follows symlinks, any failure counts as absent.
pub fn exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

/// Only an ordinary file, never a link to one, and optionally bounded in size.
pub fn regular(path: &Path, limit: Option<u64>) -> Result<&Path> {
    let info = lstat(path)?;
    if !info.file_type().is_file() || limit.is_some_and(|limit| limit > 0 && info.size() > limit) {
        return Err(Error::patch(format!(
            "Expected a regular file: {}",
            name(path)
        )));
    }
    Ok(path)
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| Error::io_path(&error, path))
}

/// `Path.read_text()` in a UTF-8 locale.
pub fn read_text(path: &Path) -> Result<String> {
    py::decode(read(path)?)
}

/// `Path.read_text(errors="replace")`.
pub fn read_text_lossy(path: &Path) -> Result<String> {
    Ok(String::from_utf8_lossy(&read(path)?).into_owned())
}

pub fn read_json(path: &Path) -> Result<Value> {
    json::loads(&read_text(regular(path, Some(MIB))?)?)
}

/// SHA-256 of a file's content as lowercase hex.
pub fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|error| Error::io_path(&error, path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; MIB as usize];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| Error::io_path(&error, path))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(py::hex(&hasher.finalize()))
}

pub fn digest_bytes(data: &[u8]) -> String {
    py::hex(&Sha256::digest(data))
}

/// `os.mkdir(path, mode)`.
pub fn mkdir(path: &Path, mode: u32) -> Result<()> {
    fs::DirBuilder::new()
        .mode(mode)
        .create(path)
        .map_err(|error| Error::io_path(&error, path))
}

/// `Path.mkdir(mode, parents=True, exist_ok=True)`: parents get the default mode.
pub fn mkdir_parents(path: &Path, mode: u32) -> Result<()> {
    match fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound && parent(path) != path => {
            mkdir_parents(parent(path), 0o777)?;
            match fs::DirBuilder::new().mode(mode).create(path) {
                Err(error) if !path.is_dir() => Err(Error::io_path(&error, path)),
                _ => Ok(()),
            }
        }
        Err(error) if !path.is_dir() => Err(Error::io_path(&error, path)),
        Err(_) => Ok(()),
    }
}

/// `tempfile.mkstemp(prefix=prefix, dir=directory)`.
pub fn mkstemp(directory: &Path, prefix: &str) -> Result<(File, PathBuf)> {
    let mut last = None;
    for _ in 0..10_000 {
        let path = directory.join(format!("{prefix}{}", py::temp_suffix()?));
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .mode(0o600)
            .open(&path);
        match opened {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last = Some(Error::io_path(&error, &path))
            }
            Err(error) => return Err(Error::io_path(&error, &path)),
        }
    }
    Err(last.unwrap_or_else(|| Error::errno(17, directory)))
}

/// `tempfile.mkdtemp(prefix=prefix, dir=directory)`.
pub fn mkdtemp(directory: &Path, prefix: &str) -> Result<PathBuf> {
    let mut last = None;
    for _ in 0..10_000 {
        let path = directory.join(format!("{prefix}{}", py::temp_suffix()?));
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                last = Some(Error::io_path(&error, &path))
            }
            Err(error) => return Err(Error::io_path(&error, &path)),
        }
    }
    Err(last.unwrap_or_else(|| Error::errno(17, directory)))
}

/// `Path.unlink(missing_ok=True)`.
pub fn unlink_missing_ok(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(Error::io_path(&error, path)),
        _ => Ok(()),
    }
}

pub fn unlink(path: &Path) -> Result<()> {
    fs::remove_file(path).map_err(|error| Error::io_path(&error, path))
}

pub fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).map_err(|error| Error::io_paths(&error, from, to))
}

/// `Path.symlink_to`: `link` becomes a symbolic link to `target`.
pub fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|error| Error::io_paths(&error, target, link))
}

pub fn readlink(path: &Path) -> Result<PathBuf> {
    fs::read_link(path).map_err(|error| Error::io_path(&error, path))
}

pub fn chmod(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| Error::io_path(&error, path))
}

/// `shutil.rmtree`.
pub fn rmtree(path: &Path) -> Result<()> {
    fs::remove_dir_all(path).map_err(|error| Error::io_path(&error, path))
}

/// `sorted(path.iterdir())`.
pub fn sorted_dir(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = list_dir(path)?;
    entries.sort();
    Ok(entries)
}

/// `list(path.iterdir())`.
pub fn list_dir(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| Error::io_path(&error, path))? {
        entries.push(entry.map_err(|error| Error::io_path(&error, path))?.path());
    }
    Ok(entries)
}

/// `shutil.disk_usage(path).free`.
pub fn free_space(path: &Path) -> Result<u64> {
    let info =
        rustix::fs::statvfs(path).map_err(|error| Error::io_path(&io::Error::from(error), path))?;
    Ok(info.f_bavail.saturating_mul(info.f_frsize))
}

/// Write a file so that either the old or the complete new content exists.
pub fn atomic(path: &Path, content: &[u8], mode: u32) -> Result<()> {
    let directory = parent(path);
    mkdir_parents(directory, 0o777)?;
    let (mut file, temp) = mkstemp(directory, ".fenix-")?;
    let result = (|| {
        file.write_all(content)
            .map_err(|error| Error::io_path(&error, &temp))?;
        file.sync_all()
            .map_err(|error| Error::io_path(&error, &temp))?;
        drop(file);
        chmod(&temp, mode)?;
        rename(&temp, path)?;
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::DIRECTORY.bits() as i32)
            .open(directory)
            .map_err(|error| Error::io_path(&error, directory))?;
        handle
            .sync_all()
            .map_err(|error| Error::io_path(&error, directory))
    })();
    let cleaned = unlink_missing_ok(&temp);
    result.and(cleaned)
}

/// `json.dumps(value, indent=2)` and a final newline, written atomically with mode 0600.
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    atomic(path, (json::dumps(value, Some(2)) + "\n").as_bytes(), 0o600)
}

/// Permit Wine's file symlinks to be replaced, never parent escapes.
pub fn contained(root: &Path, relative: impl AsRef<Path>) -> Result<PathBuf> {
    let root = resolve(root)?;
    let relative = path(relative.as_ref());
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return Err(Error::patch("Invalid installation path"));
    }
    let path = if relative == Path::new(".") {
        root.clone()
    } else {
        root.join(relative)
    };
    if !resolve(parent(&path))?.starts_with(&root) {
        return Err(Error::patch(
            "Installation directory points outside the selected profile",
        ));
    }
    Ok(path)
}

fn timestamps(info: &Metadata) -> Timestamps {
    Timestamps {
        last_access: Timespec {
            tv_sec: info.atime(),
            tv_nsec: info.atime_nsec() as _,
        },
        last_modification: Timespec {
            tv_sec: info.mtime(),
            tv_nsec: info.mtime_nsec() as _,
        },
    }
}

/// `shutil.copystat`: times, extended attributes and permission bits.
pub fn copystat(source: &Path, destination: &Path) -> Result<()> {
    let info = stat(source)?;
    rustix::fs::utimensat(CWD, destination, &timestamps(&info), AtFlags::empty())
        .map_err(|error| Error::io_path(&io::Error::from(error), destination))?;
    copy_xattrs(source, destination)?;
    chmod(destination, info.mode() & 0o7777)
}

fn copy_xattrs(source: &Path, destination: &Path) -> Result<()> {
    use rustix::io::Errno;
    let fail = |error: Errno, path: &Path| Error::io_path(&io::Error::from(error), path);
    let mut names = vec![0u8; 4096];
    let length = loop {
        match rustix::fs::listxattr(source, names.as_mut_slice()) {
            Ok(length) => break length,
            Err(Errno::RANGE) => names.resize(names.len() * 4, 0),
            Err(Errno::NOTSUP | Errno::NODATA | Errno::INVAL) => return Ok(()),
            Err(error) => return Err(fail(error, source)),
        }
    };
    for name in names[..length]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = OsStr::from_bytes(name);
        let mut value = vec![0u8; 4096];
        let copied = loop {
            match rustix::fs::getxattr(source, name, value.as_mut_slice()) {
                Ok(length) => {
                    break rustix::fs::setxattr(
                        destination,
                        name,
                        &value[..length],
                        XattrFlags::empty(),
                    )
                    .map_err(|error| (error, destination));
                }
                Err(Errno::RANGE) => value.resize(value.len() * 4, 0),
                Err(error) => break Err((error, source)),
            }
        };
        match copied {
            Ok(())
            | Err((
                Errno::PERM | Errno::NOTSUP | Errno::NODATA | Errno::INVAL | Errno::ACCESS,
                _,
            )) => {}
            Err((error, path)) => return Err(fail(error, path)),
        }
    }
    Ok(())
}

/// `shutil.copy2(source, destination)` for a destination that names the new file.
pub fn copy2(source: &Path, destination: &Path) -> Result<()> {
    let same = match (fs::metadata(source), fs::metadata(destination)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    };
    if same {
        return Err(Error::os_text(format!(
            "{} and {} are the same file",
            crate::error::repr_os(source.as_os_str()),
            crate::error::repr_os(destination.as_os_str())
        )));
    }
    let mut input = File::open(source).map_err(|error| Error::io_path(&error, source))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o666)
        .open(destination)
        .map_err(|error| Error::io_path(&error, destination))?;
    io::copy(&mut input, &mut output)
        .map_err(|error| Error::io_paths(&error, source, destination))?;
    drop(output);
    copystat(source, destination)
}

/// `Path.suffix`, as `os.path.splitext` finds it: the last dot and what follows,
/// unless only dots come before it.
pub fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) if !name[..index].trim_start_matches('.').is_empty() => &name[index..],
        _ => "",
    }
}

/// `shutil.which`.
pub fn which(program: &str, search: Option<OsString>) -> Option<PathBuf> {
    let search = search.unwrap_or_else(|| OsString::from("/bin:/usr/bin"));
    std::env::split_paths(&search)
        .map(|directory| directory.join(program))
        .find(|candidate| {
            candidate.is_file()
                && rustix::fs::access(candidate, rustix::fs::Access::EXEC_OK).is_ok()
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn paths_follow_pathlib() {
        assert_eq!(path("a/./b//c/"), Path::new("a/b/c"));
        assert_eq!(path("./a"), Path::new("a"));
        assert_eq!(path(""), Path::new("."));
        assert_eq!(path("/"), Path::new("/"));
        assert_eq!(name(Path::new("/a/b.txt")), "b.txt");
        assert_eq!(parent(Path::new("/")), Path::new("/"));
        assert_eq!(parent(Path::new("a")), Path::new("."));
        assert_eq!(home(Some("/home/x//".into())), Path::new("/home/x"));
        assert_eq!(home(Some("".into())), Path::new("/"));
        assert_eq!(expanduser("~/a", Some("/h".into())), Path::new("/h/a"));
        assert_eq!(expanduser("~", Some("/h".into())), Path::new("/h"));
        // Compared as text: a trailing separator would show in messages.
        assert_eq!(expanduser("~/", Some("/h/".into())).to_str(), Some("/h"));
        assert_eq!(expanduser("~/a/", Some("/h".into())).to_str(), Some("/h/a"));
        assert_eq!(expanduser("a/~", Some("/h".into())), Path::new("a/~"));
        assert_eq!(suffix("Fenix.EXE"), ".EXE");
        assert_eq!(suffix(".exe"), "");
        assert_eq!(suffix("a."), ".");
        assert_eq!(suffix("..exe"), "");
        assert_eq!(suffix("a..b"), ".b");
    }

    #[test]
    fn realpath_follows_python() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        fs::create_dir_all(base.join("real/inner")).unwrap();
        fs::write(base.join("real/file"), "x").unwrap();
        std::os::unix::fs::symlink("real", base.join("link")).unwrap();
        std::os::unix::fs::symlink("loop-b", base.join("loop-a")).unwrap();
        std::os::unix::fs::symlink("loop-a", base.join("loop-b")).unwrap();
        std::os::unix::fs::symlink(base.join("real/inner"), base.join("absolute")).unwrap();
        assert_eq!(
            resolve(&base.join("link/inner/../file")).unwrap(),
            base.join("real/file")
        );
        assert_eq!(
            resolve(&base.join("absolute/..")).unwrap(),
            base.join("real")
        );
        assert_eq!(
            resolve(&base.join("link/missing/x/../y")).unwrap(),
            base.join("real/missing/y")
        );
        assert_eq!(
            resolve(&base.join("loop-a/x")).unwrap(),
            base.join("loop-a/x")
        );
        assert_eq!(
            resolve_strict(&base.join("link/missing/x"))
                .unwrap_err()
                .to_string(),
            format!(
                "[Errno 2] No such file or directory: '{}'",
                base.join("real/missing").display()
            )
        );
        assert_eq!(
            resolve_strict(&base.join("real/file/x"))
                .unwrap_err()
                .to_string(),
            format!(
                "[Errno 20] Not a directory: '{}'",
                base.join("real/file").display()
            )
        );
        assert_eq!(
            resolve_strict(&base.join("loop-a"))
                .unwrap_err()
                .raw_os_error(),
            Some(40)
        );
    }

    #[test]
    fn containment_rejects_escapes() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        fs::create_dir_all(base.join("root/inside")).unwrap();
        fs::create_dir_all(base.join("outside")).unwrap();
        std::os::unix::fs::symlink(base.join("outside"), base.join("root/escape")).unwrap();
        std::os::unix::fs::symlink(base.join("outside/file"), base.join("root/inside/link"))
            .unwrap();
        let root = base.join("root");
        assert_eq!(
            contained(&root, "inside/new/file").unwrap(),
            root.join("inside/new/file")
        );
        // A file symlink itself may be replaced.
        assert_eq!(
            contained(&root, "inside/link").unwrap(),
            root.join("inside/link")
        );
        for (relative, message) in [
            ("/etc/passwd", "Invalid installation path"),
            ("inside/../../x", "Invalid installation path"),
            (
                "escape/file",
                "Installation directory points outside the selected profile",
            ),
            (
                "",
                "Installation directory points outside the selected profile",
            ),
        ] {
            assert_eq!(
                contained(&root, relative).unwrap_err().to_string(),
                message,
                "{relative}"
            );
        }
    }

    #[test]
    fn atomic_write_and_copy_keep_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("new/dir/file.json");
        write_json(&target, &json::json!({"a": "ü"})).unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "{\n  \"a\": \"\\u00fc\"\n}\n"
        );
        assert_eq!(lstat(&target).unwrap().mode() & 0o7777, 0o600);
        assert_eq!(
            list_dir(parent(&target)).unwrap(),
            std::slice::from_ref(&target)
        );
        atomic(&target, b"x", 0o755).unwrap();
        assert_eq!(lstat(&target).unwrap().mode() & 0o7777, 0o755);
        let copy = temp.path().join("copy");
        let old = Timestamps {
            last_access: Timespec {
                tv_sec: 1_000_000_000,
                tv_nsec: 5,
            },
            last_modification: Timespec {
                tv_sec: 1_000_000_001,
                tv_nsec: 7,
            },
        };
        rustix::fs::utimensat(CWD, &target, &old, AtFlags::empty()).unwrap();
        copy2(&target, &copy).unwrap();
        let info = lstat(&copy).unwrap();
        assert_eq!(
            (info.mode() & 0o7777, info.mtime(), info.mtime_nsec()),
            (0o755, 1_000_000_001, 7)
        );
        assert_eq!(read(&copy).unwrap(), b"x");
        assert!(
            copy2(&copy, &copy)
                .unwrap_err()
                .to_string()
                .ends_with("are the same file")
        );
        assert_eq!(regular(&copy, Some(1)).unwrap(), copy);
        std::os::unix::fs::symlink(&copy, temp.path().join("link")).unwrap();
        assert_eq!(
            regular(&temp.path().join("link"), None)
                .unwrap_err()
                .to_string(),
            "Expected a regular file: link"
        );
        assert_eq!(digest(&copy).unwrap(), digest_bytes(b"x"));
    }
}
