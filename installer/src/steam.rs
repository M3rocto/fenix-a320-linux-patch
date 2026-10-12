// SPDX-License-Identifier: MIT
//! Transactional installation for the Steam editions of MSFS 2020 and 2024.
//!
//! Steam updates its own Proton builds, while the overlay belongs to one exact
//! Wine build. A separate compatibility tool is therefore created from a pinned
//! Proton and selected only for the simulator. The simulator's Windows profile is
//! prepared in a copy; the original profile is retained for Restore.
use crate::{
    argv,
    core::{self, Lock, Waiter, Wine},
    error::{Error, Result},
    fsx::{self, MIB, atomic, contained, digest, read_json, regular, write_json},
    json::{self, Dict, Map, Value, json},
    process::{self, Cmd, Env, Input, Output},
    py,
    system::{Progress, System},
    vdf,
};
use regex::Regex;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, BufReader},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Game {
    pub key: &'static str,
    pub appid: &'static str,
    pub title: &'static str,
    pub settings: &'static str,
    pub executable: &'static str,
}

pub const GAMES: [Game; 2] = [
    Game {
        key: "msfs2020",
        appid: "1250410",
        title: "Microsoft Flight Simulator (2020)",
        settings: "Microsoft Flight Simulator",
        executable: "FlightSimulator.exe",
    },
    Game {
        key: "msfs2024",
        appid: "2537590",
        title: "Microsoft Flight Simulator 2024",
        settings: "Microsoft Flight Simulator 2024",
        executable: "FlightSimulator2024.exe",
    },
];
pub const STATE: &str = "fenix-a320-linux-patch";
pub const TOOL_MARKER: &str = "fenix-a320-tool.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonDownload {
    pub variant: String,
    pub url: String,
    pub sha256: String,
    pub folder: String,
}

/// Exact public builds that an overlay in bundle.json was compiled for, in order
/// of preference: the Wine 11 generation that Proton Experimental currently uses,
/// then the earlier Wine 10 build.
pub fn proton_downloads() -> Vec<ProtonDownload> {
    [
        (
            "cachyos-11",
            "https://github.com/CachyOS/proton-cachyos/releases/download/cachyos-11.0-20261005-slr/proton-cachyos-11.0-20261005-slr-x86_64.tar.xz",
            "096bfe73b506d6565b04ecc45214197a4091818f16ed91f5324b4d2082d0a263",
            "proton-cachyos-11.0-20261005-slr-x86_64",
        ),
        (
            "cachyos-10",
            "https://github.com/CachyOS/proton-cachyos/releases/download/cachyos-10.0-sunset-slr/proton-cachyos-10.0-sunset-slr-x86_64.tar.xz",
            "d6e1baa66937ed58e854b2013c0ef3b3f1fbc51d7be09e34a5ada3442ea9452d",
            "proton-cachyos-10.0-sunset-slr-x86_64",
        ),
    ]
    .into_iter()
    .map(|(variant, url, sha256, folder)| ProtonDownload {
        variant: variant.to_string(),
        url: url.to_string(),
        sha256: sha256.to_string(),
        folder: folder.to_string(),
    })
    .collect()
}

/// The simulator runs with the configuration the overlay was verified with.
pub const GAME_ENV: [(&str, &str); 9] = [
    (
        "WINE_TRACK_WRITECOPY",
        "apps:Fenix.exe,FenixSystem.exe,FenixDisplay.exe,FenixCDU.exe,FlightSimulator2024.exe,FlightSimulator.exe",
    ),
    ("WINE_D2D1_DISPLAY_EFFECTS", "FenixDisplay.exe;FenixCDU.exe"),
    ("WINE_D2D1_GEOMETRY_PROVIDER", "FenixDisplay.exe"),
    (
        "WINE_DWRITE_UNHINTED_OUTLINES",
        "FenixDisplay.exe;FenixCDU.exe",
    ),
    ("WINE_FENIX_HELPER_WINDOWS", "1"),
    ("DOTNET_SYSTEM_GLOBALIZATION_USENLS", "1"),
    ("DOTNET_ReadyToRun", "0"),
    ("PROTON_NO_ESYNC", "1"),
    ("PROTON_NO_FSYNC", "1"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub game: &'static str,
    /// Steam client directory
    pub root: PathBuf,
    /// Steam library containing the simulator
    pub library: PathBuf,
}

impl Target {
    pub fn new(game: &str, root: &Path, library: &Path) -> Result<Self> {
        let game = GAMES
            .iter()
            .find(|known| known.key == game)
            .ok_or_else(|| Error::patch(format!("Unknown simulator: {game}")))?;
        Ok(Self {
            game: game.key,
            root: root.to_path_buf(),
            library: library.to_path_buf(),
        })
    }

    fn info(&self) -> &'static Game {
        GAMES
            .iter()
            .find(|known| known.key == self.game)
            .unwrap_or(&GAMES[1])
    }

    pub fn appid(&self) -> &'static str {
        self.info().appid
    }

    pub fn title(&self) -> &'static str {
        self.info().title
    }

    pub fn settings(&self) -> &'static str {
        self.info().settings
    }

    pub fn compat(&self) -> PathBuf {
        self.library.join("steamapps/compatdata").join(self.appid())
    }

    pub fn prefix(&self) -> PathBuf {
        self.compat().join("pfx")
    }

    pub fn state(&self) -> PathBuf {
        self.library
            .join("steamapps")
            .join(STATE)
            .join(self.appid())
    }

    pub fn marker(&self) -> PathBuf {
        self.state().join("state.json")
    }

    pub fn tools(&self) -> PathBuf {
        self.root.join("compatibilitytools.d")
    }
}

// Python's `\s` additionally covers the four separator control characters.
static LIBRARY_PATH: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r#""path"[\s\x1c-\x1f]+"((?:[^"\\]|\\.)*)""#));
static ESCAPED: LazyLock<Regex> = LazyLock::new(|| py::pattern(r"\\(.)"));

/// The manifest expecting `files` on top of the runner's own files.
fn overlaid(lock: &Value, files: &Map<String, Value>) -> Result<Value> {
    let mut items = json::object(lock.clone(), "bundle.json")?;
    items.insert(
        "runner_files".to_string(),
        Value::Object(json::merged(&lock.need_table("runner_files")?, files)),
    );
    Ok(Value::Object(items))
}

pub fn steam_roots(system: &dyn System, explicit: Option<&OsStr>) -> Result<Vec<PathBuf>> {
    if let Some(explicit) = explicit.filter(|explicit| !explicit.is_empty()) {
        let root = fsx::resolve_strict(&fsx::expanduser(explicit, system.var("HOME")))?;
        if !root.join("steamapps").is_dir() {
            return Err(Error::patch(
                "This is not a Steam directory: it has no steamapps folder.",
            ));
        }
        return Ok(vec![root]);
    }
    let home = system.home();
    let mut roots = Vec::new();
    for candidate in [
        ".local/share/Steam",
        ".steam/root",
        ".steam/steam",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        "snap/steam/common/.local/share/Steam",
    ] {
        let root = match fsx::resolve_strict(&home.join(candidate)) {
            Ok(root) => root,
            Err(Error::Os(_)) => continue,
            Err(error) => return Err(error),
        };
        if root.join("steamapps").is_dir() && !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

pub fn libraries(root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = vec![root.to_path_buf()];
    let list = root.join("steamapps/libraryfolders.vdf");
    let content = match regular(&list, Some(MIB)).and_then(fsx::read_text_lossy) {
        Ok(content) => content,
        Err(Error::Os(_) | Error::Patch(_)) => return Ok(found),
        Err(error) => return Err(error),
    };
    for entry in LIBRARY_PATH.captures_iter(&content) {
        let value = ESCAPED.replace_all(&entry[1], "$1").into_owned();
        let path = match fsx::resolve_strict(&fsx::path(&value)) {
            Ok(path) => path,
            // An unplugged library
            Err(Error::Os(_)) => continue,
            Err(error) => return Err(error),
        };
        if path.join("steamapps").is_dir() && !found.contains(&path) {
            found.push(path);
        }
    }
    Ok(found)
}

/// Installed Steam simulators whose Windows profile or patch state exists.
pub fn targets(system: &dyn System, steam_root: Option<&OsStr>) -> Result<Vec<Target>> {
    let mut found: Vec<Target> = Vec::new();
    for root in steam_roots(system, steam_root)? {
        for library in libraries(&root)? {
            for game in GAMES {
                let target = Target {
                    game: game.key,
                    root: root.clone(),
                    library: library.clone(),
                };
                // An interrupted exchange leaves no profile in place for a
                // moment; its patch state must still be found for Restore.
                if library
                    .join("steamapps")
                    .join(format!("appmanifest_{}.acf", game.appid))
                    .is_file()
                    && (target.prefix().join("system.reg").is_file() || target.marker().is_file())
                    && !found.iter().any(|item| item.compat() == target.compat())
                {
                    found.push(target);
                }
            }
        }
    }
    Ok(found)
}

pub fn target(system: &dyn System, game: &str, steam_root: Option<&OsStr>) -> Result<Target> {
    let Some(info) = GAMES.iter().find(|known| known.key == game) else {
        return Err(Error::patch(format!("Unknown simulator: {game}")));
    };
    let mut found: Vec<Target> = targets(system, steam_root)?
        .into_iter()
        .filter(|item| item.game == game)
        .collect();
    if found.is_empty() {
        return Err(Error::patch(format!(
            "{} was not found in Steam. Install it and start it once, so that Steam creates its Windows profile, then close it.",
            info.title
        )));
    }
    if found.len() > 1 {
        return Err(Error::patch(format!(
            "{} exists in several Steam installations. Choose one with --steam-root.",
            info.title
        )));
    }
    let item = found.remove(0);
    if !item.marker().is_file() {
        checked(&item, false)?;
    }
    Ok(item)
}

pub fn checked(item: &Target, recovery: bool) -> Result<()> {
    let prefix = item.prefix();
    let mut paths = vec![item.compat()];
    if !recovery {
        paths.extend([
            prefix.clone(),
            prefix.join("drive_c"),
            prefix.join("drive_c/windows"),
            prefix.join("drive_c/windows/system32"),
        ]);
    }
    for path in paths {
        if path.is_symlink()
            || !path.is_dir()
            || fsx::stat(&path)?.uid() != rustix::process::getuid().as_raw()
        {
            return Err(Error::patch(format!(
                "The Steam profile needs owned, ordinary directories: {}",
                fsx::name(&path)
            )));
        }
    }
    if !recovery {
        regular(&prefix.join("system.reg"), None)?;
    }
    Ok(())
}

pub fn locked(system: &dyn System, item: &Target, idle: bool, recovery: bool) -> Result<Lock> {
    if !recovery || fsx::exists(&item.compat()) {
        checked(item, recovery)?;
    }
    let state = item.state();
    fsx::mkdir_parents(&state, 0o700)?;
    if state.is_symlink() || fsx::stat(&state)?.uid() != rustix::process::getuid().as_raw() {
        return Err(Error::patch("Invalid patch state directory"));
    }
    let file = core::lock_file(
        &state.join("lock"),
        "Invalid patch lock",
        "Another Fenix patch operation is using this simulator.",
    )?;
    if idle && fsx::exists(&item.prefix()) {
        system.ensure_idle(&item.prefix())?;
    }
    Ok(Lock::new(state, file))
}

pub fn tool_name(variant: &str) -> String {
    format!("Proton-Fenix-A320-{variant}")
}

pub fn tool_title(system: &dyn System, variant: Option<&str>) -> Result<String> {
    let version = core::plain_version(system.manifest(variant)?.need_str("runner_version")?);
    Ok(format!("Proton Fenix A320 ({version})"))
}

pub fn tool_state(tool: &Path) -> Result<Option<Value>> {
    let path = tool.join(TOOL_MARKER);
    if path.is_file() && !path.is_symlink() {
        read_json(&path).map(Some)
    } else {
        Ok(None)
    }
}

pub fn is_proton(path: &Path) -> bool {
    path.join("proton").is_file()
        && path.join("version").is_file()
        && path.join("files/bin/wineserver").is_file()
}

pub fn installed_protons(item: &Target) -> Result<Vec<PathBuf>> {
    let mut folders = vec![
        item.tools(),
        PathBuf::from("/usr/share/steam/compatibilitytools.d"),
    ];
    folders.extend(
        libraries(&item.root)?
            .iter()
            .map(|library| library.join("steamapps/common")),
    );
    let mut found = Vec::new();
    for folder in folders {
        let Ok(entries) = fsx::sorted_dir(&folder) else {
            continue;
        };
        for entry in entries {
            if entry.is_dir() && is_proton(&entry) && !fsx::exists(&entry.join(TOOL_MARKER)) {
                found.push(entry);
            }
        }
    }
    Ok(found)
}

/// The overlay variant an unmodified Proton matches. The base build of the
/// manifest is Flightdeck's runner and never a Steam Proton.
fn pristine_variant(system: &dyn System, path: &Path) -> Result<Option<String>> {
    match core::runner_variant(system, path, false) {
        Ok(variant) => Ok(variant),
        Err(Error::Patch(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `os.path.normpath` for a link target.
fn normpath(target: &[u8]) -> Vec<u8> {
    let absolute = target.starts_with(b"/");
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in target.split(|byte| *byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." if parts.last().is_some_and(|last| *last != b"..") => {
                parts.pop();
            }
            b".." if absolute => {}
            part => parts.push(part),
        }
    }
    let mut out = if absolute { b"/".to_vec() } else { Vec::new() };
    out.extend(parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

fn timestamps(seconds: i64, nanoseconds: u32) -> rustix::fs::Timestamps {
    let time = rustix::fs::Timespec {
        tv_sec: seconds,
        tv_nsec: nanoseconds as _,
    };
    rustix::fs::Timestamps {
        last_access: time,
        last_modification: time,
    }
}

/// Extract one pinned Proton archive; reject members outside its folder.
///
/// The rules are those of Python's `tarfile` "data" filter: no absolute or
/// escaping names, no links leading outside, no devices, no ownership, and no
/// permission bits beyond the owner's own.
pub fn extract_proton(
    system: &dyn System,
    archive: &Path,
    destination: &Path,
    folder: &str,
) -> Result<PathBuf> {
    let unsafe_member = || Error::patch("Unsafe member in the Proton archive");
    let broken =
        |error: io::Error| Error::other(format!("The Proton archive could not be read: {error}"));
    let file = File::open(archive).map_err(|error| Error::io_path(&error, archive))?;
    let mut stream = tar::Archive::new(lzma_rust2::XzReader::new(BufReader::new(file), true));
    let base = fsx::resolve(destination)?;
    let inside = |path: &Path| -> Result<()> {
        if fsx::resolve(path)?.starts_with(&base) {
            Ok(())
        } else {
            Err(unsafe_member())
        }
    };
    let set_time = |path: &Path, time: (i64, u32)| {
        rustix::fs::utimensat(
            rustix::fs::CWD,
            path,
            &timestamps(time.0, time.1),
            rustix::fs::AtFlags::empty(),
        )
        .map_err(|error| Error::io_path(&io::Error::from(error), path))
    };
    let mut files: Vec<(PathBuf, u32)> = Vec::new();
    let mut directories: Vec<(PathBuf, (i64, u32))> = Vec::new();
    for entry in stream.entries().map_err(broken)? {
        system.interrupted()?;
        let mut entry = entry.map_err(broken)?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            continue;
        }
        let name = entry.path_bytes().into_owned();
        let name: &[u8] = &name[name
            .iter()
            .position(|byte| *byte != b'/')
            .unwrap_or(name.len())..];
        let relative = Path::new(OsStr::from_bytes(name));
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(unsafe_member());
        }
        let target = base.join(relative);
        inside(&target)?;
        let mode = entry.header().mode().map_err(broken)?;
        let mut time = (
            i64::try_from(entry.header().mtime().map_err(broken)?).unwrap_or(0),
            0u32,
        );
        if let Some(extensions) = entry.pax_extensions().map_err(broken)? {
            for extension in extensions {
                let extension = extension.map_err(broken)?;
                if extension.key_bytes() == b"mtime"
                    && let Ok(value) = extension.value()
                {
                    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
                    let digits: String = fraction
                        .chars()
                        .chain(std::iter::repeat('0'))
                        .take(9)
                        .collect();
                    if let (Ok(seconds), Ok(nanoseconds)) = (seconds.parse(), digits.parse()) {
                        time = (seconds, nanoseconds);
                    }
                }
            }
        }
        let link = entry.link_name_bytes().map(|link| link.into_owned());
        let regular_file = kind.is_file() || kind.is_contiguous() || kind.is_gnu_sparse();
        if !(regular_file || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
            return Err(unsafe_member());
        }
        let link = match (&link, kind.is_symlink() || kind.is_hard_link()) {
            (Some(link), true) => {
                if link.starts_with(b"/") {
                    return Err(unsafe_member());
                }
                let from = if kind.is_symlink() {
                    fsx::parent(&target).to_path_buf()
                } else {
                    base.clone()
                };
                inside(&from.join(OsStr::from_bytes(link)))?;
                Some(PathBuf::from(OsString::from_vec(normpath(link))))
            }
            (None, true) => return Err(unsafe_member()),
            _ => None,
        };
        fsx::mkdir_parents(fsx::parent(&target), 0o777)?;
        if kind.is_dir() {
            fsx::mkdir_parents(&target, 0o777)?;
            directories.push((target, time));
            continue;
        }
        if fs::symlink_metadata(&target).is_ok() {
            fsx::unlink(&target)?;
        }
        if kind.is_symlink() {
            fsx::symlink(link.as_deref().unwrap_or(Path::new(".")), &target)?;
            continue;
        }
        // Owner may always read and write; execute bits only if the owner had one.
        let mut limited = mode & 0o755;
        if limited & 0o100 == 0 {
            limited &= !0o111;
        }
        limited |= 0o600;
        if kind.is_hard_link() {
            let source = base.join(link.as_deref().unwrap_or(Path::new(".")));
            fs::hard_link(&source, &target)
                .map_err(|error| Error::io_paths(&error, &source, &target))?;
        } else {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o666)
                .open(&target)
                .map_err(|error| Error::io_path(&error, &target))?;
            io::copy(&mut entry, &mut output).map_err(broken)?;
            files.push((target.clone(), mode & 0o555));
        }
        fsx::chmod(&target, limited)?;
        set_time(&target, time)?;
    }
    directories.sort_by(|a, b| b.0.cmp(&a.0));
    for (directory, time) in directories {
        set_time(&directory, time)?;
    }
    // Proton ships its files read-only. Profiles link to them, so keep
    // that: a Windows installer must not write through into the tool.
    for (path, mode) in files {
        fsx::chmod(&path, mode)?;
    }
    let result = destination.join(folder);
    if !is_proton(&result) {
        return Err(Error::patch("The Proton archive has an unexpected layout."));
    }
    Ok(result)
}

/// An unmodified Proton that exactly matches one overlay, and its variant.
pub fn source_proton(
    system: &dyn System,
    item: &Target,
    explicit: Option<&OsStr>,
    progress: Progress<'_>,
    download_into: &Path,
) -> Result<(PathBuf, String)> {
    if let Some(explicit) = explicit.filter(|explicit| !explicit.is_empty()) {
        let path = fsx::resolve_strict(&fsx::expanduser(explicit, system.var("HOME")))?;
        let variant = if is_proton(&path) {
            pristine_variant(system, &path)?
        } else {
            None
        };
        return match variant {
            Some(variant) => Ok((path, variant)),
            None => Err(Error::patch(format!(
                "This Proton build has no matching Fenix overlay. Supported: {}",
                supported(system)?
            ))),
        };
    }
    let mut installed: Vec<(String, PathBuf)> = Vec::new();
    for path in installed_protons(item)? {
        if let Some(variant) = pristine_variant(system, &path)?
            && !installed.iter().any(|(known, _)| *known == variant)
        {
            installed.push((variant, path));
        }
    }
    let using = |path: &Path| {
        progress(&format!(
            "Using the installed {} as the base for the Fenix Proton.",
            fsx::name(path)
        ))
    };
    let available = system.manifest(None)?.table("variants")?;
    for pinned in system.proton_downloads() {
        if !available.contains_key(&pinned.variant) {
            continue;
        }
        if let Some((variant, path)) = installed
            .iter()
            .find(|(variant, _)| *variant == pinned.variant)
        {
            using(path)?;
            return Ok((path.clone(), variant.clone()));
        }
        progress(&format!(
            "Downloading the pinned Proton build {} (about 350 MB) …",
            pinned.folder
        ))?;
        let file = pinned.url.rsplit('/').next().unwrap_or(&pinned.url);
        let archive = system.download(
            &pinned.url,
            &item.state().join("downloads").join(file),
            &pinned.sha256,
            1024 * MIB,
        )?;
        progress("Extracting Proton …")?;
        let path = extract_proton(system, &archive, download_into, &pinned.folder)?;
        core::verify_runner(system, &path, &system.manifest(Some(&pinned.variant))?)?;
        return Ok((path, pinned.variant));
    }
    if let Some((variant, path)) = installed.into_iter().next() {
        using(&path)?;
        return Ok((path, variant));
    }
    Err(Error::patch(format!(
        "No supported Proton build is available. Supported: {}",
        supported(system)?
    )))
}

pub fn supported(system: &dyn System) -> Result<String> {
    let lock = system.manifest(None)?;
    let mut versions = Vec::new();
    for entry in lock.table("variants")?.values() {
        versions.push(core::plain_version(entry.need_str("runner_version")?));
    }
    Ok(versions.join(", "))
}

/// Copy an unmodified Proton to its final Steam location; the overlay follows later.
pub fn create_tool(
    system: &dyn System,
    source: &Path,
    tool: &Path,
    variant: Option<&str>,
) -> Result<()> {
    fsx::mkdir_parents(fsx::parent(tool), 0o777)?;
    core::copy_tree(system, source, tool)?;
    let state = json!({"format": 1, "state": "staging", "variant": variant, "version": system.manifest(None)?.need("version")?});
    write_json(&tool.join(TOOL_MARKER), &state)?;
    let name = fsx::name(tool);
    let title = tool_title(system, variant)?;
    let description = format!(
        "\"compatibilitytools\"\n{{\n  \"compat_tools\"\n  {{\n    \"{name}\"\n    {{\n      \"install_path\" \".\"\n      \
         \"display_name\" \"{title}\"\n      \"from_oslist\"  \"windows\"\n      \"to_oslist\"    \"linux\"\n    }}\n  }}\n}}\n"
    );
    atomic(
        &tool.join("compatibilitytool.vdf"),
        description.as_bytes(),
        0o644,
    )?;
    let environment: Map<String, Value> = GAME_ENV
        .iter()
        .map(|(name, value)| (name.to_string(), json!(value)))
        .collect();
    // Proton itself reads this file; its format is Proton's, not this installer's.
    let settings = format!(
        "# Written by the Fenix A320 Linux Patch. Proton applies these to games using this tool.\nuser_settings = {}\n",
        json::dumps(&Value::Object(environment), Some(4))
    );
    atomic(&tool.join("user_settings.py"), settings.as_bytes(), 0o644)
}

pub fn verify_tool(
    system: &dyn System,
    tool: &Path,
    variant: &Value,
    version: &Value,
) -> Result<()> {
    let ready = match tool_state(tool)? {
        Some(state) => {
            json::truthy(&state)
                && state.is("state", "ready")
                && json::equal(state.field("variant"), variant)
                && json::equal(state.field("version"), version)
        }
        None => false,
    };
    if !ready {
        return Err(Error::patch(format!(
            "The Fenix Proton tool is incomplete or belongs to another patch version: {}",
            fsx::name(tool)
        )));
    }
    let lock = system.manifest(core::variant_name(variant)?.as_deref())?;
    core::verify_runner(system, tool, &overlaid(&lock, &lock.need_table("files")?)?)
}

/// Simulator content inside the profile: moved at commit instead of copied.
pub fn carried_directories(item: &Target) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let users = item.prefix().join("drive_c/users");
    if !users.is_dir() {
        return Ok(found);
    }
    for home in fsx::sorted_dir(&users)? {
        let roaming = home.join("AppData/Roaming");
        let settings = roaming.join(item.settings());
        let folder = settings.join("Packages");
        let chain = [&home, &home.join("AppData"), &roaming, &settings, &folder];
        if chain.iter().all(|part| part.is_dir() && !part.is_symlink()) {
            found.push(fsx::text(folder.strip_prefix(item.compat()).map_err(
                |_| Error::Value("the profile lies outside its folder".to_string()),
            )?));
        }
    }
    Ok(found)
}

fn parts(path: &str) -> Vec<String> {
    fsx::path(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .filter(|part| part != ".")
        .collect()
}

/// cp -a, except that excluded relative directories are left out.
pub fn copy_without(
    system: &dyn System,
    source: &Path,
    destination: &Path,
    excluded: &[Vec<String>],
) -> Result<()> {
    if excluded.is_empty() {
        return core::copy_tree(system, source, destination);
    }
    fsx::mkdir(destination, 0o700)?;
    for child in fsx::sorted_dir(source)? {
        let name = fsx::name(&child);
        let below: Vec<Vec<String>> = excluded
            .iter()
            .filter(|path| path.first() == Some(&name))
            .map(|path| path[1..].to_vec())
            .collect();
        if below.iter().any(Vec::is_empty) {
            continue;
        }
        if !below.is_empty() && child.is_dir() && !child.is_symlink() {
            copy_without(system, &child, &destination.join(&name), &below)?;
        } else {
            core::copy_tree(
                system,
                &child,
                &destination.join(child.file_name().unwrap_or_default()),
            )?;
        }
    }
    fsx::copystat(source, destination)
}

/// Size of the ordinary files below `base`, without the excluded relative folders.
pub fn tree_size(base: &Path, excluded: &[String]) -> Result<u64> {
    let skip: Vec<PathBuf> = excluded.iter().map(|path| base.join(path)).collect();
    let mut total = 0;
    let mut pending = vec![base.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !skip.contains(&path) {
                    pending.push(path);
                }
            } else if !path.is_dir() {
                let info = fsx::lstat(&path)?;
                if info.file_type().is_file() {
                    total += info.size();
                }
            }
        }
    }
    Ok(total)
}

/// Let this Proton create or upgrade the profile exactly as a Steam launch would.
pub fn proton_setup(
    system: &dyn System,
    tool: &Path,
    compat: &Path,
    item: &Target,
    log: &File,
) -> Result<()> {
    let mut env = Env(system
        .environ()
        .0
        .into_iter()
        .filter(|(key, _)| {
            ![&b"WINE"[..], b"STEAM_COMPAT", b"PROTON_"]
                .iter()
                .any(|prefix| key.as_bytes().starts_with(prefix))
        })
        .collect());
    env.set("STEAM_COMPAT_DATA_PATH", compat);
    env.set("STEAM_COMPAT_CLIENT_INSTALL_PATH", &item.root);
    let mut cmd = Cmd::new(argv![tool.join("proton"), "getcompatpath", "/"]);
    cmd.env = Some(&env);
    cmd.cwd = Some(compat);
    cmd.stdin = Input::Null;
    cmd.output = Output::File(log);
    cmd.timeout = Some(900);
    if process::run(&cmd, &|| system.interrupted())?.code != 0 {
        return Err(Error::patch(
            "Proton could not prepare the Windows profile. See the Fenix setup log.",
        ));
    }
    Ok(())
}

/// Proton's shared Wine Mono registers itself as .NET Framework 4.
///
/// Microsoft's setup then reports success without installing anything. Remove
/// that registration and the builtin loader, as Winetricks does.
pub fn remove_shared_mono(wine: &dyn Wine) -> Result<()> {
    let accepted = core::RunOptions {
        accepted: &[0, 1],
        ..core::RunOptions::default()
    };
    for key in [
        r"HKLM\Software\Microsoft\NET Framework Setup\NDP\v4",
        r"HKLM\Software\Microsoft\NET Framework Setup\NDP\v3.5",
        r"HKLM\Software\Microsoft\.NETFramework\v2.0.50727\SBSDisabled",
    ] {
        for view in ["/reg:64", "/reg:32"] {
            wine.run(&argv!["reg", "delete", key, "/f", view], &accepted)?;
        }
    }
    for folder in ["system32", "syswow64"] {
        fsx::unlink_missing_ok(&contained(
            wine.prefix(),
            format!("drive_c/windows/{folder}/mscoree.dll"),
        )?)?;
    }
    // Proton links its placeholder framework files to read-only files in the
    // tool; Windows Installer cannot replace those and fails the whole setup.
    for folder in ["Framework", "Framework64"] {
        let directory = contained(
            wine.prefix(),
            format!("drive_c/windows/Microsoft.NET/{folder}/v4.0.30319"),
        )?;
        if directory.is_dir() && !directory.is_symlink() {
            for path in fsx::list_dir(&directory)? {
                if path.is_symlink() {
                    fsx::unlink(&path)?;
                }
            }
        }
    }
    Ok(())
}

pub fn has_display(system: &dyn System) -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| system.var(name).is_some_and(|value| !value.is_empty()))
}

pub fn require_display(system: &dyn System) -> Result<()> {
    // Microsoft's .NET setup creates windows even when silent; under Wine 11
    // it waits forever when there is no display to create them on.
    if !has_display(system) {
        return Err(Error::patch(
            "Run the installation inside your desktop session: Microsoft's .NET setup does not finish without a display.",
        ));
    }
    Ok(())
}

/// An installed earlier release whose files this installer knows and can replace.
pub fn updatable(system: &dyn System, state: &Value) -> Result<bool> {
    if !state.is("state", "installed") {
        return Ok(false);
    }
    let lock =
        match core::state_variant(state).and_then(|variant| system.manifest(variant.as_deref())) {
            Ok(lock) => lock,
            Err(Error::Patch(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
    Ok(state
        .str("version")
        .is_some_and(|version| lock.field("previous_releases").get(version).is_some()))
}

fn belongs(item: &Target, state: &Value) -> Result<()> {
    if !state.is("target", "steam") || !state.is("game", item.game) {
        return Err(Error::patch(
            "The patch state belongs to another simulator.",
        ));
    }
    Ok(())
}

fn prefix_files(item: &Target, files: &Map<String, Value>) -> Result<()> {
    for (name, sha) in files {
        if !json::is_text(
            sha,
            &digest(regular(&contained(&item.prefix(), name)?, None)?)?,
        ) {
            return Err(Error::patch(format!(
                "Installed Fenix dependency changed: {name}"
            )));
        }
    }
    Ok(())
}

/// Replace the overlay of an installed earlier release in place. The Windows profile stays as it is.
pub fn update(
    system: &dyn System,
    item: &Target,
    bundle: &Path,
    state: &Value,
    progress: Progress<'_>,
) -> Result<()> {
    belongs(item, state)?;
    let variant = core::variant_name(state.need("variant")?)?;
    let lock = system.manifest(variant.as_deref())?;
    let version = lock.need("version")?.clone();
    let installed = state.need("version")?.clone();
    let previous = lock
        .need("previous_releases")?
        .need(state.need_str("version")?)?
        .clone();
    let tool = contained(&item.tools(), state.need_str("tool")?)?;
    let current = tool_state(&tool)?.unwrap_or_else(|| json!({}));
    let current_state = current.str("state").unwrap_or("");
    let current_version = current.field("version");
    // The tool may already carry this release: another simulator shares it, or an update was interrupted.
    if !json::equal(current.field("variant"), state.field("variant"))
        || !matches!(current_state, "ready" | "updating")
        || !(json::equal(current_version, &installed) || json::equal(current_version, &version))
    {
        return Err(Error::patch(format!(
            "The Fenix Proton tool does not belong to this installation: {}",
            fsx::name(&tool)
        )));
    }
    if current_state == "ready" {
        let files = if json::equal(current_version, &version) {
            lock.need_table("files")?
        } else {
            previous.need_table("files")?
        };
        core::verify_runner(system, &tool, &overlaid(&lock, &files)?)?;
    }
    prefix_files(item, &previous.table("prefix_files")?)?;
    for other in tool_users(system, item, &fsx::name(&tool))? {
        if fsx::exists(&other.prefix()) {
            system.ensure_idle(&other.prefix())?;
        }
    }
    progress(&format!(
        "Updating the Fenix Proton from {} to {} …",
        json::text(&installed),
        json::text(&version)
    ))?;
    let mut marker = json::object(current, "The tool state")?;
    marker.insert("state".into(), json!("updating"));
    marker.insert("version".into(), version.clone());
    write_json(&tool.join(TOOL_MARKER), &Value::Object(marker.clone()))?;
    core::apply_overlay(system, &item.prefix(), &tool, bundle, variant.as_deref())?;
    marker.insert("state".into(), json!("ready"));
    write_json(&tool.join(TOOL_MARKER), &Value::Object(marker))?;
    let mut updated = json::object(state.clone(), "The patch state")?;
    updated.insert("version".into(), version);
    updated.insert("updated_from".into(), installed);
    let updated = Value::Object(updated);
    verify_installed(system, item, &updated)?;
    write_json(&item.marker(), &updated)?;
    progress("Patch updated. The Windows profile, Fenix and its settings are unchanged.")
}

pub fn install(
    system: &dyn System,
    item: &Target,
    bundle: &OsStr,
    progress: Progress<'_>,
    proton: Option<&OsStr>,
) -> Result<()> {
    system.host_check()?;
    let bundle = core::verify_bundle(system, bundle)?;
    let version = system.manifest(None)?.need("version")?.clone();
    let held = locked(system, item, true, false)?;
    let state_directory = &held.root;
    let marker = item.marker();
    if fsx::exists(&marker) {
        let state = read_json(&marker)?;
        if state.is("state", "installed") && json::equal(state.field("version"), &version) {
            verify_installed(system, item, &state)?;
            return progress("This patch version is already installed.");
        }
        if updatable(system, &state)? {
            return update(system, item, &bundle, &state, progress);
        }
        return Err(Error::patch(
            "A previous patch transaction exists. Restore it before reinstalling.",
        ));
    }
    system.require_display()?;
    let proton = proton.filter(|proton| !proton.is_empty());
    let stamp = format!("{}-{}", py::Utc::now().stamp(), &py::uuid4_hex()?[..8]);
    let work = state_directory.join(format!("work-{stamp}"));
    let mut ready = Vec::new();
    if proton.is_none() && item.tools().is_dir() {
        for path in fsx::sorted_dir(&item.tools())? {
            if fsx::name(&path).starts_with("Proton-Fenix-A320-")
                && tool_state(&path)?.is_some_and(|state| state.is("state", "ready"))
            {
                ready.push(path);
            }
        }
    }
    let carried = carried_directories(item)?;
    let needed = tree_size(&item.compat(), &carried)? + 1024 * MIB;
    if fsx::free_space(state_directory)? < needed {
        return Err(Error::patch(
            "Not enough free space for the independent Windows profile copy.",
        ));
    }
    let mut state = Map::new();
    state.insert("format".into(), json!(1));
    state.insert("target".into(), json!("steam"));
    state.insert("game".into(), json!(item.game));
    state.insert("version".into(), version.clone());
    state.insert("state".into(), json!("preparing"));
    state.insert("work".into(), json!(fsx::name(&work)));
    state.insert("previous".into(), json!(format!("before-{stamp}")));
    state.insert("carried".into(), json!(carried));
    state.insert("configured".into(), json!(false));
    state.insert("tool".into(), Value::Null);
    state.insert("tool_created".into(), json!(false));
    state.insert("variant".into(), Value::Null);
    fsx::mkdir(&work, 0o700)?;
    let save = |state: &Map<String, Value>| write_json(&marker, &Value::Object(state.clone()));
    save(&state)?;
    let result = (|| {
        let (tool, variant) = if let Some(tool) = ready.first() {
            let known = tool_state(tool)?.unwrap_or(Value::Null);
            let variant = known.need("variant")?.clone();
            verify_tool(system, tool, &variant, &version)?;
            state.insert("tool".into(), json!(fsx::name(tool)));
            state.insert("variant".into(), variant.clone());
            (tool.clone(), core::variant_name(&variant)?)
        } else {
            let (source, variant) =
                source_proton(system, item, proton, progress, &work.join("proton"))?;
            let tool = item.tools().join(tool_name(&variant));
            if fsx::exists(&tool) || tool.is_symlink() {
                return Err(Error::patch(format!(
                    "An unfinished Fenix Proton tool exists: {}. Use Restore, or remove that folder.",
                    fsx::text(&tool)
                )));
            }
            if fsx::free_space(&item.root)? < tree_size(&source, &[])? + 1024 * MIB {
                return Err(Error::patch(
                    "Not enough free space for the Fenix Proton tool.",
                ));
            }
            state.insert("tool".into(), json!(fsx::name(&tool)));
            state.insert("variant".into(), json!(variant));
            state.insert("tool_created".into(), json!(true));
            save(&state)?;
            progress(
                "Creating the separate Fenix Proton tool; your other Proton versions stay unchanged …",
            )?;
            create_tool(system, &source, &tool, Some(&variant))?;
            (tool, Some(variant))
        };
        save(&state)?;
        let _ = fs::remove_dir_all(work.join("proton"));
        progress("Copying the Windows profile; the original remains available …")?;
        let staged = work.join("compat");
        let excluded: Vec<Vec<String>> = carried.iter().map(|path| parts(path)).collect();
        copy_without(system, &item.compat(), &staged, &excluded)?;
        let prefix = staged.join("pfx");
        let drive = prefix.join("dosdevices/c:");
        match fs::create_dir(fsx::parent(&drive)) {
            Err(error) if !fsx::parent(&drive).is_dir() => {
                return Err(Error::io_path(&error, fsx::parent(&drive)));
            }
            _ => {}
        }
        if drive.is_symlink() {
            fsx::unlink(&drive)?;
        }
        fsx::symlink(Path::new("../drive_c"), &drive)?;
        let logpath = work.join("setup.log");
        {
            let log = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&logpath)
                .map_err(|error| Error::io_path(&error, &logpath))?;
            fsx::chmod(&logpath, 0o600)?;
            // Settle Proton's own profile migration first: a later launch
            // then finds its expected version and keeps native .NET files.
            system.proton_setup(&tool, &staged, item, &log)?;
            let wine = system.wine(&prefix, &tool, &log);
            let prepared = (|| {
                if !core::has_framework(&prefix)? {
                    remove_shared_mono(wine.as_ref())?;
                }
                // As Windows XP, .NET 4.0 also installs its own dxva2/evr over
                // Proton's read-only links. The simulator keeps Proton's media stack.
                system.prepare_framework(
                    wine.as_ref(),
                    &state_directory.join("downloads"),
                    progress,
                    "win7",
                )?;
                progress("Preparing fonts, graphics dependencies and Fenix settings …")?;
                system.graphics_and_fonts(&prefix, &tool, wine.as_ref())?;
                core::apply_settings(&mut state, &prefix, item.settings())?;
                system.prepare_geometry(
                    wine.as_ref(),
                    &state_directory.join("downloads"),
                    &bundle,
                    progress,
                )
            })();
            wine.stop()?;
            prepared?;
        }
        core::apply_overlay(system, &prefix, &tool, &bundle, variant.as_deref())?;
        let mut tool_marker =
            json::object(tool_state(&tool)?.unwrap_or(Value::Null), "The tool state")?;
        tool_marker.insert("state".into(), json!("ready"));
        write_json(&tool.join(TOOL_MARKER), &Value::Object(tool_marker))?;
        system.ensure_idle(&item.prefix())?;
        state.insert("state".into(), json!("committing"));
        save(&state)?;
        let previous = state_directory.join(format!("before-{stamp}"));
        system.rename(&item.compat(), &previous)?;
        system.rename(&staged, &item.compat())?;
        for path in &carried {
            system.rename(&previous.join(path), &item.compat().join(path))?;
        }
        state.insert("state".into(), json!("installed"));
        save(&state)?;
        progress(&format!(
            "Patch installed. Next, let Steam use “{}” for {}: quit Steam and choose “select”, or restart Steam and force it under Properties → Compatibility.",
            tool_title(system, variant.as_deref())?,
            item.title()
        ))
    })();
    if result.is_err() {
        progress(
            "Setup did not finish. The original profile or its backup is retained. Use Restore.",
        )?;
    }
    result
}

pub fn verify_installed(system: &dyn System, item: &Target, state: &Value) -> Result<()> {
    belongs(item, state)?;
    let lock = system.manifest(core::state_variant(state)?.as_deref())?;
    let version = lock.need("version")?;
    if !json::equal(state.field("version"), version) {
        if updatable(system, state)? {
            return Err(Error::patch(format!(
                "Patch {} is installed. Update it to {} first.",
                json::text(state.need("version")?),
                json::text(version)
            )));
        }
        return Err(Error::patch(
            "This installed patch version is not supported by the current installer.",
        ));
    }
    let tool = contained(&item.tools(), state.need_str("tool")?)?;
    verify_tool(
        system,
        &tool,
        state.need("variant")?,
        state.need("version")?,
    )?;
    prefix_files(item, &lock.table("prefix_files")?)
}

/// Other simulators whose patch state refers to this tool.
pub fn tool_users(system: &dyn System, item: &Target, name: &str) -> Result<Vec<Target>> {
    let mut users = Vec::new();
    for other in targets(system, Some(item.root.as_os_str()))? {
        if other.compat() != item.compat() && other.marker().is_file() {
            match read_json(&other.marker()) {
                Ok(state) if state.is("tool", name) => users.push(other),
                Ok(_) => {}
                // An unreadable state still counts as a user.
                Err(error) if error.is_data_error() => users.push(other),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(users)
}

pub fn restore(system: &dyn System, item: &Target, progress: Progress<'_>) -> Result<()> {
    let held = locked(system, item, false, true)?;
    let state_directory = &held.root;
    let marker = item.marker();
    let mut state = json::object(read_json(&marker)?, "The patch state")?;
    let view = Value::Object(state.clone());
    let previous = contained(state_directory, view.need_str("previous")?)?;
    if fsx::exists(&item.prefix()) {
        system.ensure_idle(&item.prefix())?;
    }
    let staged = contained(state_directory, view.need_str("work")?)?.join("compat/pfx");
    if fsx::exists(&staged) {
        system.ensure_idle(&staged)?;
    }
    let save = |state: &Map<String, Value>| write_json(&marker, &Value::Object(state.clone()));
    state.insert("state".into(), json!("restoring"));
    save(&state)?;
    let mut retained_name = view
        .item("retained")
        .filter(|name| json::truthy(name))
        .cloned();
    if fsx::exists(&previous) {
        if fsx::exists(&item.compat()) {
            if let Some(carried) = view.item("carried") {
                let carried = carried.as_array().ok_or_else(|| {
                    Error::other("The patch state lists no folders under carried")
                })?;
                for path in carried {
                    let path = path.as_str().ok_or_else(|| {
                        Error::other("The patch state names a carried folder that is not text")
                    })?;
                    let (source, destination) = (
                        contained(&item.compat(), path)?,
                        contained(&previous, path)?,
                    );
                    if source.is_dir() && !source.is_symlink() && !fsx::exists(&destination) {
                        system.rename(&source, &destination)?;
                    }
                }
            }
            let retained = state_directory.join(format!("retained-{}", py::uuid4_hex()?));
            system.rename(&item.compat(), &retained)?;
            state.insert("retained".into(), json!(fsx::name(&retained)));
            retained_name = Some(json!(fsx::name(&retained)));
            save(&state)?;
        }
        system.rename(&previous, &item.compat())?;
    } else if !fsx::exists(&item.compat()) {
        return Err(Error::patch(
            "The original profile backup is missing; nothing was removed.",
        ));
    }
    let tool_name = view.item("tool").filter(|tool| json::truthy(tool));
    let mut selection = "";
    if let Some(tool) = tool_name
        && selected_tool(item).is_some_and(|selected| json::is_text(tool, &selected))
    {
        let before = match view.item("tool_before") {
            None => Ok(None),
            Some(Value::String(before)) => Ok(Some(before.as_str())),
            Some(_) => Err(Error::patch("Invalid compatibility tool selection")),
        };
        match before.and_then(|before| write_selection(system, item, before)) {
            Ok(()) => {}
            Err(error) if error.is_data_error() => {
                selection = " Steam still has the Fenix Proton selected for the simulator: choose another one under Properties → Compatibility.";
            }
            Err(error) => return Err(error),
        }
    }
    let mut removed = false;
    if let Some(tool) = tool_name {
        let tool = contained(
            &item.tools(),
            tool.as_str()
                .ok_or_else(|| Error::other("The patch state names a tool that is not text"))?,
        )?;
        // Only ever remove a tool carrying this installer's marker, and only when unused.
        if tool.is_dir()
            && !tool.is_symlink()
            && tool.join(TOOL_MARKER).is_file()
            && tool_users(system, item, &fsx::name(&tool))?.is_empty()
        {
            fsx::rmtree(&tool)?;
            removed = true;
        }
    }
    state.insert("state".into(), json!("restored"));
    write_json(
        &state_directory.join(format!("restored-{}.json", &py::uuid4_hex()?[..8])),
        &Value::Object(state),
    )?;
    fsx::unlink(&marker)?;
    let place = match &retained_name {
        Some(retained) => format!(
            "Original Windows profile restored. The newer profile is retained in {}.",
            fsx::text(&state_directory.join(retained.as_str().unwrap_or_default()))
        ),
        None => format!(
            "The original Windows profile is in place. The unfinished copy and its setup log remain in {}.",
            fsx::text(&state_directory.join(view.need_str("work")?))
        ),
    };
    progress(&format!(
        "{place}{}{selection}",
        if removed {
            " The Fenix Proton tool was removed."
        } else {
            ""
        }
    ))
}

pub fn installed_state(system: &dyn System, item: &Target) -> Result<Value> {
    if !fsx::exists(&item.marker()) {
        return Err(Error::patch("Install the compatibility patch first."));
    }
    let state = read_json(&item.marker())?;
    if !state.is("state", "installed") {
        return Err(Error::patch(
            "Finish patch installation first, or use Restore.",
        ));
    }
    verify_installed(system, item, &state)?;
    Ok(state)
}

pub fn configure(system: &dyn System, item: &Target, progress: Progress<'_>) -> Result<()> {
    let held = locked(system, item, true, false)?;
    let state = installed_state(system, item)?;
    let backup = held.root.join(format!("settings-{}", py::uuid4_hex()?));
    fsx::mkdir(&backup, 0o700)?;
    core::backup_settings(&item.prefix(), &backup)?;
    let mut state = json::object(state, "The patch state")?;
    let message = core::apply_settings(&mut state, &item.prefix(), item.settings())?;
    write_json(&item.marker(), &Value::Object(state.clone()))?;
    if state.get("configured") != Some(&Value::Bool(true)) {
        return Err(Error::patch(core::NEEDS_SIGN_IN));
    }
    progress(message)
}

pub fn windows_app(
    system: &dyn System,
    item: &Target,
    executable: Option<&OsStr>,
    progress: Progress<'_>,
    manager: bool,
    wait: Option<Waiter<'_>>,
) -> Result<()> {
    let held = locked(system, item, true, false)?;
    let state = installed_state(system, item)?;
    core::run_app(
        system,
        &item.prefix(),
        &contained(&item.tools(), state.need_str("tool")?)?,
        &held.root.join("fenix-app.log"),
        executable,
        progress,
        manager,
        wait,
    )
}

/// The compatibility tool Steam currently forces for this simulator, if readable.
pub fn selected_tool(item: &Target) -> Option<String> {
    let path = item.root.join("config/config.vdf");
    let content = regular(&path, Some(16 * MIB))
        .and_then(fsx::read_text)
        .ok()?;
    vdf::compat_tool(&content, item.appid()).ok().flatten()
}

pub fn steam_running() -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    let user = rustix::process::getuid().as_raw();
    entries.flatten().any(|entry| {
        entry.file_name().as_bytes().iter().all(u8::is_ascii_digit)
            && entry.metadata().is_ok_and(|info| info.uid() == user)
            && fs::read(entry.path().join("comm"))
                .is_ok_and(|name| String::from_utf8_lossy(&name).trim() == "steam")
    })
}

/// Change only this simulator's entry in Steam's configuration; Steam must be closed.
pub fn write_selection(system: &dyn System, item: &Target, tool: Option<&str>) -> Result<()> {
    if system.steam_running() {
        return Err(Error::patch(
            "Steam is running and would overwrite the selection. Quit Steam completely (Steam → Exit), then try again.",
        ));
    }
    let path = item.root.join("config/config.vdf");
    let content = py::decode(fsx::read(regular(&path, Some(16 * MIB))?)?)?;
    let changed = vdf::set_compat_tool(&content, item.appid(), tool)?;
    if changed != content {
        let stamp = py::Utc::now().stamp();
        atomic(
            &item
                .state()
                .join(format!("steam-config-before-{stamp}.vdf")),
            content.as_bytes(),
            0o600,
        )?;
        atomic(&path, changed.as_bytes(), fsx::stat(&path)?.mode() & 0o7777)?;
    }
    Ok(())
}

/// Force the Fenix Proton for the simulator, as Properties → Compatibility does.
pub fn select_tool(system: &dyn System, item: &Target, progress: Progress<'_>) -> Result<()> {
    let _held = locked(system, item, false, false)?;
    let state = installed_state(system, item)?;
    let tool = state.need_str("tool")?;
    let current = selected_tool(item);
    if current.as_deref() == Some(tool) {
        return progress(&format!(
            "Steam already uses the Fenix Proton for {}.",
            item.title()
        ));
    }
    write_selection(system, item, Some(tool))?;
    let title = core::variant_name(state.need("variant")?)
        .and_then(|variant| tool_title(system, variant.as_deref()));
    let mut state = json::object(state.clone(), "The patch state")?;
    state.insert("tool_before".into(), json!(current));
    state.insert("tool_selected_by_installer".into(), json!(true));
    write_json(&item.marker(), &Value::Object(state))?;
    progress(&format!(
        "Steam will start {} with “{}”. You can start Steam again.",
        item.title(),
        title?
    ))
}

pub fn snapshot(system: &dyn System, item: &Target) -> Result<Value> {
    let mut result = Map::new();
    result.insert("target".into(), json!("steam"));
    result.insert("game".into(), json!(item.game));
    result.insert("profile".into(), json!(fsx::text(&item.compat())));
    result.insert("state".into(), json!("unavailable"));
    result.insert(
        "version".into(),
        system.manifest(None)?.need("version")?.clone(),
    );
    for key in ["configured", "installed", "fenix_installed", "can_restore"] {
        result.insert(key.into(), json!(false));
    }
    result.insert("message".into(), json!(""));
    let filled = (|| {
        result.insert(
            "fenix_installed".into(),
            json!(
                item.prefix()
                    .join(core::PROGRAM)
                    .join("Fenix.exe")
                    .is_file()
            ),
        );
        let manager = match core::manager_path(&item.prefix()) {
            Ok(_) => true,
            Err(Error::Os(_) | Error::Patch(_)) => false,
            Err(error) => return Err(error),
        };
        result.insert("manager_installed".into(), json!(manager));
        if fsx::exists(&item.marker()) {
            let state = read_json(&item.marker())?;
            json::object(state.clone(), "The patch state")?;
            let installed = state.is("state", "installed");
            core::state_report(&mut result, &state);
            result.insert("can_restore".into(), json!(true));
            result.insert("installed_version".into(), state.field("version").clone());
            result.insert("tool".into(), state.field("tool").clone());
            result.insert("update_available".into(), json!(updatable(system, &state)?));
            if installed {
                let tool = state.field("tool");
                let selected = match selected_tool(item) {
                    Some(selected) => json::is_text(tool, &selected),
                    None => tool.is_null(),
                };
                result.insert("tool_selected".into(), json!(selected));
                let title = || {
                    tool_title(
                        system,
                        core::variant_name(state.need("variant")?)?.as_deref(),
                    )
                };
                if !selected {
                    result.insert(
                        "message".into(),
                        json!(format!(
                            "Steam does not use “{}” for {} yet.",
                            title()?,
                            item.title()
                        )),
                    );
                }
                result.insert("tool_title".into(), json!(title()?));
                result.insert("steam_running".into(), json!(system.steam_running()));
            }
        } else {
            system.host_check()?;
            checked(item, false)?;
            result.insert("state".into(), json!("available"));
        }
        let idle = match system.ensure_idle(&item.prefix()) {
            Ok(()) => true,
            Err(Error::Patch(_)) => false,
            Err(error) => return Err(error),
        };
        result.insert("idle".into(), json!(idle));
        Ok(())
    })();
    match filled {
        Ok(()) => {}
        Err(error) if error.is_reportable() => {
            result.insert("message".into(), json!(error.to_string()));
        }
        Err(error) => return Err(error),
    }
    Ok(Value::Object(result))
}
