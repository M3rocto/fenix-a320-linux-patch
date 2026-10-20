// SPDX-License-Identifier: MIT
//! Transactional, per-user Flightdeck installation.
//!
//! Never executes code from a downloaded bundle. Bundle metadata is compared
//! with the manifest shipped with this installer before any file is used.
use crate::{
    argv, download,
    error::{Error, Result},
    fsx::{self, MIB, atomic, contained, digest, read_json, regular, write_json},
    json::{self, Dict, Map, Value, json},
    process::{self, Cmd, Env, Input, Output},
    py,
    system::{Progress, System},
    xml,
};
use regex::Regex;
use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::Child,
    sync::LazyLock,
    time::Duration,
};

pub const MARKER: &str = "private/fenix-linux-patch.json";
pub const PROGRAM: &str = "drive_c/Program Files/FenixSim A320";
pub const CONFIG: &str = "drive_c/ProgramData/Fenix/FenixSim A320";
pub const ENV: [(&str, &str); 10] = [
    ("WINEARCH", "win64"),
    ("WINEESYNC", "0"),
    ("WINEFSYNC", "0"),
    (
        "WINE_TRACK_WRITECOPY",
        "apps:Fenix.exe,FenixSystem.exe,FenixDisplay.exe,FenixCDU.exe,FlightSimulator2024.exe",
    ),
    ("WINE_D2D1_DISPLAY_EFFECTS", "FenixDisplay.exe;FenixCDU.exe"),
    ("WINE_D2D1_GEOMETRY_PROVIDER", "FenixDisplay.exe"),
    ("WINE_FENIX_HELPER_WINDOWS", "1"),
    (
        "WINE_DWRITE_UNHINTED_OUTLINES",
        "FenixDisplay.exe;FenixCDU.exe",
    ),
    ("DOTNET_SYSTEM_GLOBALIZATION_USENLS", "1"),
    ("DOTNET_ReadyToRun", "0"),
];
pub const DOWNLOADS: [(&str, &str, &str); 2] = [
    (
        "dotNetFx40_Full_x86_x64.exe",
        "https://download.microsoft.com/download/9/5/A/95A9616B-7A37-4AF6-BC36-D6EA96C8DAAE/dotNetFx40_Full_x86_x64.exe",
        "65e064258f2e418816b304f646ff9e87af101e4c9552ab064bb74d281c38659f",
    ),
    (
        "NDP48-x86-x64-AllOS-ENU.exe",
        "https://download.microsoft.com/download/f/3/a/f3a6af84-da23-40a5-8d1c-49cc10c8e76f/NDP48-x86-x64-AllOS-ENU.exe",
        "0a3a390c47e639d0f7fc65b21195fee6b7f65b066f80f70c60fab191d14b7e40",
    ),
];
pub const GEOMETRY_DOWNLOADS: [(&str, &str, &str); 2] = [
    (
        "Windows6.1-KB2670838-x64.msu",
        "https://download.microsoft.com/download/1/4/9/14936FE9-4D16-4019-A093-5E00182609EB/Windows6.1-KB2670838-x64.msu",
        "9fe71e7dcd2280ce323880b075ade6e56c49b68fc702a9b4c0a635f0f1fb9db8",
    ),
    (
        "msdelta.dll",
        "https://msdl.microsoft.com/download/symbols/msdelta.dll/559F38C482000/msdelta.dll",
        "29c10fb3ffa0e3cfd04c5247c3ed3a975575fad44d8a1e447b23b43213781653",
    ),
];
pub const GEOMETRY_SHA256: &str =
    "663f1d59ec1c014b9ea47a6cef71b3d43579e9759a82f3ee2cbd80b8c6d9e85f";
pub const GEOMETRY_PATH: &str = "drive_c/windows/system32/d2d1_geometry.dll";
const SCRIPTS: [&str; 2] = ["launch-msfs.sh", "xodus-wine-launch"];
const NO_BUILD: &str = "No Fenix compatibility build is available for this Proton version.";

pub const DXGI_FILE: &str = "files/lib/wine/dxvk/x86_64-windows/dxgi.dll";
/// Every ABI partner of the owner-adapter query must be supplied together.
pub const GPU_FILES: [&str; 8] = [
    "files/lib/wine/x86_64-windows/ntdll.dll",
    "files/lib/wine/x86_64-unix/ntdll.so",
    "files/bin/wineserver",
    "files/lib/wine/x86_64-windows/gdi32.dll",
    "files/lib/wine/x86_64-windows/win32u.dll",
    "files/lib/wine/x86_64-unix/win32u.so",
    "files/lib/wine/x86_64-windows/wow64win.dll",
    DXGI_FILE,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayRenderer {
    Cpu,
    Gpu,
}

impl DisplayRenderer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Gpu => "GPU",
        }
    }

    /// Missing metadata is an older, CPU-only overlay. GPU is an explicit
    /// capability of a complete, pinned Wine/DXGI overlay, never an override.
    pub fn from_manifest(lock: &Value) -> Result<Self> {
        match lock.item("display_renderer") {
            None => Ok(Self::Cpu),
            Some(value) if value.as_str() == Some("CPU") => Ok(Self::Cpu),
            Some(value) if value.as_str() == Some("GPU") => {
                let dxvk = lock.item("dxvk").and_then(Value::as_object);
                let files = lock.need_table("files")?;
                if dxvk.is_none_or(|dxvk| dxvk.is_empty())
                    || GPU_FILES.iter().any(|name| !files.contains_key(*name))
                {
                    return Err(Error::patch(
                        "GPU displays require the complete matched Wine and DXGI overlay for this Proton build.",
                    ));
                }
                Ok(Self::Gpu)
            }
            _ => Err(Error::patch("Unknown display renderer in bundle.json.")),
        }
    }

    pub fn select(lock: &Value, requested: Option<Self>) -> Result<Self> {
        let supported = Self::from_manifest(lock)?;
        if requested == Some(Self::Gpu) && supported != Self::Gpu {
            return Err(Error::patch(
                "This Proton overlay supports CPU displays only. Install a GPU-capable Fenix patch first.",
            ));
        }
        Ok(requested.unwrap_or(supported))
    }

    /// A manual fallback is a user preference, distinct from the renderer an
    /// older release happened to configure automatically.
    pub fn for_state(lock: &Value, state: &Value, requested: Option<Self>) -> Result<Self> {
        let saved = match state.item("display_renderer_override") {
            None | Some(Value::Null) => None,
            Some(value) if value.as_str() == Some("CPU") => Some(Self::Cpu),
            Some(value) if value.as_str() == Some("GPU") => Some(Self::Gpu),
            _ => return Err(Error::patch("Unknown saved display renderer preference.")),
        };
        Self::select(lock, requested.or(saved))
    }

    /// States predating renderer metadata always configured the CPU path.
    pub fn from_state(state: &Value) -> Self {
        if state.is("display_renderer", "GPU") {
            Self::Gpu
        } else {
            Self::Cpu
        }
    }
}

// Python's `\s` additionally covers the four separator control characters.
static BUILD_NUMBER: LazyLock<Regex> = LazyLock::new(|| py::pattern(r"\A\d+[\s\x1c-\x1f]+"));
static FRAMEWORK_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    py::pattern(r"\[Software\\\\Microsoft\\\\NET Framework Setup\\\\NDP\\\\v4\\\\Full\][^\[]*")
});
static FRAMEWORK_RELEASE: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r#""Release"=dword:([0-9a-fA-F]+)"#));
static FONTS_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    py::pattern(r"(?m)^\[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts\][^\[]*")
});
static PRODUCT_CODE: LazyLock<Regex> = LazyLock::new(|| py::pattern(r"\A\{[a-fA-F0-9-]{36}\}\z"));

/// The installer's own manifest, optionally with one variant's entries applied.
pub fn read_manifest(root: &Path, variant: Option<&str>) -> Result<Value> {
    let lock = json::object(read_json(&root.join("bundle.json"))?, "bundle.json")?;
    let Some(variant) = variant.filter(|variant| !variant.is_empty()) else {
        return Ok(Value::Object(lock));
    };
    let entry = match lock.get("variants") {
        Some(Value::Object(variants)) => variants.get(variant),
        _ => None,
    };
    let Some(entry) = entry else {
        return Err(Error::patch(NO_BUILD));
    };
    let entry = entry
        .as_object()
        .ok_or_else(|| Error::other("bundle.json: a variant is not a JSON object"))?;
    let mut merged = json::merged(&lock, entry);
    if !entry.contains_key("dxvk") {
        merged.remove("dxvk");
    }
    // A renderer is a capability of this exact overlay, not of its base.
    if !entry.contains_key("display_renderer") {
        merged.remove("display_renderer");
    }
    merged.insert("variant".to_string(), json!(variant));
    Ok(Value::Object(merged))
}

/// A variant as state files record it: `null` is the manifest's base build.
pub fn variant_name(value: &Value) -> Result<Option<String>> {
    match value {
        value if !json::truthy(value) => Ok(None),
        Value::String(variant) => Ok(Some(variant.clone())),
        _ => Err(Error::patch(NO_BUILD)),
    }
}

/// The variant a state file names.
pub fn state_variant(state: &Value) -> Result<Option<String>> {
    variant_name(state.field("variant"))
}

/// The manifest with the overlay's files expected in the runner.
fn patched(lock: &Value) -> Result<Value> {
    let mut items = json::object(lock.clone(), "bundle.json")?;
    let files = json::merged(
        &lock.need_table("runner_files")?,
        &lock.need_table("files")?,
    );
    items.insert("runner_files".to_string(), Value::Object(files));
    Ok(Value::Object(items))
}

/// Match complete Wine identities; a family/version label is not an ABI.
pub fn runner_variant(
    system: &dyn System,
    runner: &Path,
    overlaid: bool,
) -> Result<Option<String>> {
    let lock = system.manifest(None)?;
    let variants: Vec<String> = lock.table("variants")?.keys().cloned().collect();
    for variant in std::iter::once(None).chain(variants.into_iter().map(Some)) {
        let mut candidate = system.manifest(variant.as_deref())?;
        let verified = (|| {
            if overlaid {
                candidate = patched(&candidate)?;
            }
            verify_runner(system, runner, &candidate)
        })();
        match verified {
            Ok(()) => return Ok(variant),
            Err(Error::Os(_) | Error::Patch(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Err(Error::patch(
        "Für diese Proton-Version fehlt ein passender Fenix-Patch. Wähle eine Version mit Fenix-Unterstützung.",
    ))
}

pub fn wine_binary(runner: &Path) -> PathBuf {
    let wine64 = runner.join("files/bin/wine64");
    if wine64.is_file() {
        wine64
    } else {
        runner.join("files/bin/wine")
    }
}

pub fn runtime_path(system: &dyn System, value: &OsStr, recovery: bool) -> Result<PathBuf> {
    let path = fsx::resolve_strict(&fsx::expanduser(value, system.var("HOME")))?;
    let info = read_json(&path.join("private/runtime.json"))?;
    let info = json::object(info, "runtime.json")?;
    if info
        .get("game_id")
        .is_some_and(|game| game.as_str() != Some("msfs2024"))
    {
        return Err(Error::patch("This preview supports MSFS 2024 only."));
    }
    let mut required = vec!["private", "tools", "local"];
    if !recovery {
        required.extend([
            "local/msfs-prefix",
            "local/msfs-prefix/drive_c",
            "local/msfs-prefix/drive_c/windows",
            "local/msfs-prefix/drive_c/windows/system32",
        ]);
    }
    for relative in required {
        let item = contained(&path, relative)?;
        if item.is_symlink()
            || !item.is_dir()
            || fsx::stat(&item)?.uid() != rustix::process::getuid().as_raw()
        {
            return Err(Error::patch(format!(
                "Runtime needs owned, ordinary directories: {relative}"
            )));
        }
    }
    if !recovery {
        regular(&path.join("local/msfs-prefix/system.reg"), None)?;
    }
    if !path.join("runner").is_symlink() {
        return Err(Error::patch(
            "Expected Flightdeck's runner symlink; the original runner will be retained.",
        ));
    }
    Ok(path)
}

pub fn default_runtime(system: &dyn System) -> PathBuf {
    let data = match system.var("XDG_DATA_HOME") {
        Some(data) => fsx::path(data),
        None => system.home().join(".local/share"),
    };
    fsx::path(data.join("flightdeck/runtimes/msfs2024"))
}

/// Refuse while any process of this user runs in the given Wine profile.
pub fn ensure_idle(prefix: &Path) -> Result<()> {
    let expected = fsx::resolve(prefix)?;
    let own = std::process::id().to_string();
    for process in fsx::list_dir(Path::new("/proc"))? {
        let name = fsx::name(&process);
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) || name == own {
            continue;
        }
        let environment = match fs::read(process.join("environ")) {
            Ok(environment) => environment,
            // The process ended meanwhile, or belongs to someone else.
            Err(error) if matches!(error.raw_os_error(), Some(1 | 2 | 3 | 13)) => continue,
            Err(error) => return Err(Error::io_path(&error, process.join("environ"))),
        };
        for entry in environment.split(|byte| *byte == 0) {
            let Some(value) = entry.strip_prefix(b"WINEPREFIX=") else {
                continue;
            };
            if value == expected.as_os_str().as_bytes()
                || fsx::resolve(Path::new(OsStr::from_bytes(value)))? == expected
            {
                return Err(Error::patch(
                    "Close MSFS, Fenix and all installers for this profile first.",
                ));
            }
        }
    }
    Ok(())
}

/// An exclusive lease on a runtime or a Steam profile, held until dropped.
#[derive(Debug)]
pub struct Lock {
    /// The runtime, or the patch state directory of a Steam simulator.
    pub root: PathBuf,
    _file: File,
}

impl Lock {
    pub fn new(root: PathBuf, file: File) -> Self {
        Self { root, _file: file }
    }
}

/// Take a lock file the way Flightdeck and the Python installer do: `flock`,
/// exclusive, without waiting, on a private ordinary file.
pub fn lock_file(path: &Path, invalid: &str, busy: &str) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .mode(0o600)
        .open(path)
        .map_err(|error| Error::io_path(&error, path))?;
    let info = file.metadata()?;
    if !info.file_type().is_file()
        || info.uid() != rustix::process::getuid().as_raw()
        || info.nlink() != 1
    {
        return Err(Error::patch(invalid));
    }
    match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(file),
        Err(rustix::io::Errno::WOULDBLOCK) => Err(Error::patch(busy)),
        Err(error) => Err(Error::from(error)),
    }
}

pub fn locked(system: &dyn System, runtime: &OsStr, idle: bool, recovery: bool) -> Result<Lock> {
    let root = runtime_path(system, runtime, recovery)?;
    if fsx::exists(&root.join("private/proton-switch.json")) {
        return Err(Error::patch(
            "Recover the interrupted Proton switch before changing Fenix.",
        ));
    }
    let file = lock_file(
        &root.join("private/play.lock"),
        "Invalid runtime lock",
        "The simulator or another installation is using this runtime.",
    )?;
    if idle {
        system.ensure_idle(&root.join("local/msfs-prefix"))?;
    }
    Ok(Lock { root, _file: file })
}

fn glibc_version(output: &[u8]) -> Option<(u64, u64)> {
    let text = String::from_utf8_lossy(output);
    let line = text.lines().next()?;
    if !(line.contains("glibc") || line.contains("GNU libc") || line.contains("GLIBC")) {
        return None;
    }
    let version = line.split_whitespace().next_back()?;
    let mut parts = version.split('.').map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
    });
    Some((parts.next()?.ok()?, parts.next()?.ok()?))
}

/// The version of the C library of this machine, when it is glibc.
///
/// The installer itself is linked statically, so it asks the system's own
/// tools: `getconf GNU_LIBC_VERSION` first, then `ldd --version`.
fn host_glibc(system: &dyn System) -> Result<Option<(u64, u64)>> {
    for argv in [
        argv!["getconf", "GNU_LIBC_VERSION"],
        argv!["ldd", "--version"],
    ] {
        let mut cmd = Cmd::new(argv);
        cmd.stdin = Input::Null;
        cmd.output = Output::CaptureOutput;
        cmd.timeout = Some(10);
        match process::run(&cmd, &|| system.interrupted()) {
            Ok(finished) => {
                if let Some(version) = glibc_version(&finished.stdout) {
                    return Ok(Some(version));
                }
            }
            Err(Error::Interrupted) => return Err(Error::Interrupted),
            // The tool is missing or hangs: try the next one.
            Err(_) => {}
        }
    }
    Ok(None)
}

pub fn host_check(system: &dyn System) -> Result<()> {
    let name = rustix::system::uname();
    if name.sysname().to_bytes() != b"Linux"
        || !matches!(name.machine().to_bytes(), b"x86_64" | b"amd64")
    {
        return Err(Error::patch("The binary preview requires x86_64 Linux."));
    }
    match host_glibc(system)? {
        Some(version) if version < (2, 38) => Err(Error::patch(
            "This binary build requires glibc 2.38 or newer. Build from source on older distributions.",
        )),
        Some(_) => Ok(()),
        None => Err(Error::patch("The binary build requires glibc.")),
    }
}

/// `re.sub(r"^\d+\s+", "", version)`: the build number in front of a Proton version.
pub fn plain_version(version: &str) -> String {
    BUILD_NUMBER.replace(version, "").into_owned()
}

pub fn verify_runner(system: &dyn System, runner: &Path, lock: &Value) -> Result<()> {
    let version = fsx::read_text(regular(&runner.join("version"), Some(4096))?)?;
    if plain_version(py::strip(&version)) != plain_version(lock.need_str("runner_version")?) {
        return Err(Error::patch(
            "The Wine runner version does not match this Fenix build.",
        ));
    }
    for (name, expected) in &lock.need_table("runner_files")? {
        system.interrupted()?;
        if !json::is_text(expected, &digest(regular(&runner.join(name), None)?)?) {
            return Err(Error::patch(format!(
                "The Wine runner differs from the supported build: {name}"
            )));
        }
    }
    Ok(())
}

pub fn verify_bundle(system: &dyn System, bundle: &OsStr) -> Result<PathBuf> {
    let bundle = fsx::resolve_strict(&fsx::expanduser(bundle, system.var("HOME")))?;
    let lock = system.manifest(None)?;
    if !json::equal(&read_json(&bundle.join("bundle.json"))?, &lock) {
        return Err(Error::patch(
            "The patch bundle does not match this installer version.",
        ));
    }
    let mut files = lock.need_table("files")?;
    for (variant, entry) in &lock.table("variants")? {
        for (name, sha) in &entry.need_table("files")? {
            files.insert(format!("variants/{variant}/{name}"), sha.clone());
        }
    }
    for (relative, expected) in &files {
        system.interrupted()?;
        let path = contained(&bundle.join("payload"), relative)?;
        if !json::is_text(expected, &digest(regular(&path, None)?)?) {
            return Err(Error::patch(format!("Patch checksum mismatch: {relative}")));
        }
    }
    for (name, expected) in &lock.need_table("integration")? {
        if !json::is_text(
            expected,
            &digest(regular(
                &contained(&bundle.join("integration"), name)?,
                None,
            )?)?,
        ) {
            return Err(Error::patch(format!(
                "Integration checksum mismatch: {name}"
            )));
        }
    }
    Ok(bundle)
}

pub fn wine_env(system: &dyn System, prefix: &Path, runner: &Path) -> Env {
    let mut env = system.environ();
    for (name, value) in ENV {
        env.set(name, value);
    }
    env.set("WINEPREFIX", prefix);
    env.set("WINEDEBUG", "-all");
    env.set("WINE", wine_binary(runner));
    env.set("WINESERVER", runner.join("files/bin/wineserver"));
    env.set("WINE_DISABLE_FAST_SYNC", "1");
    for name in [
        "WINE_DLL_FILE_MAP",
        "WINELOADER",
        "WINEDLLPATH",
        "WINESERVERSOCKET",
        "WINEPRELOADRESERVE",
        "WINELOADERNOEXEC",
    ] {
        env.remove(name);
    }
    env.set("WINEDLLOVERRIDES", "winemenubuilder.exe=d");
    let mut arguments = env
        .get("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
        .cloned()
        .unwrap_or_default();
    arguments.push(" --disable-features=HideCursorWhileTyping");
    let stripped = match arguments.to_str() {
        Some(arguments) => OsString::from(py::strip(arguments)),
        None => arguments,
    };
    env.set("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", stripped);
    env
}

/// Only call after every process in the staged prefix has stopped.
pub fn apply_overlay(
    system: &dyn System,
    prefix: &Path,
    runner: &Path,
    bundle: &Path,
    variant: Option<&str>,
) -> Result<()> {
    let lock = system.manifest(variant)?;
    let mut payload = bundle.join("payload");
    if let Some(variant) = variant.filter(|variant| !variant.is_empty()) {
        payload = payload.join(format!("variants/{variant}"));
    }
    for (name, sha) in &lock.need_table("files")? {
        system.interrupted()?;
        let data = fsx::read(regular(&contained(&payload, name)?, None)?)?;
        if !json::is_text(sha, &fsx::digest_bytes(&data)) {
            return Err(Error::patch(format!(
                "Fenix patch checksum mismatch: {name}"
            )));
        }
        atomic(
            &contained(runner, name)?,
            &data,
            if name.contains("/bin/") { 0o755 } else { 0o644 },
        )?;
        if name.starts_with("files/lib/wine/x86_64-windows/") || name == DXGI_FILE {
            atomic(
                &contained(
                    prefix,
                    format!("drive_c/windows/system32/{}", fsx::name(Path::new(name))),
                )?,
                &data,
                0o644,
            )?;
        }
    }
    let integration = lock.need_table("integration")?;
    for name in [
        "FenixWindowGuard.exe",
        "FenixMCDURefresh.exe",
        "fenix-display-refresh.py",
    ] {
        if let Some(sha) = integration.get(name) {
            let data = fsx::read(regular(&bundle.join("integration").join(name), None)?)?;
            if !json::is_text(sha, &fsx::digest_bytes(&data)) {
                return Err(Error::patch(format!(
                    "Fenix helper checksum mismatch: {name}"
                )));
            }
            atomic(
                &contained(prefix, format!("drive_c/windows/system32/{name}"))?,
                &data,
                0o644,
            )?;
        }
    }
    // Include untouched ABI partners and the loader in the final verification.
    // A concurrently updated source runner must not produce a mixed build.
    verify_runner(system, runner, &patched(&lock)?)
}

pub struct RunOptions<'a> {
    pub env: &'a [(&'a str, &'a str)],
    pub accepted: &'a [i32],
    pub timeout: u64,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self {
            env: &[],
            accepted: &[0],
            timeout: 1800,
        }
    }
}

/// Wine in one profile, with everything it prints going to a private log.
pub trait Wine {
    fn prefix(&self) -> &Path;
    fn runner(&self) -> &Path;
    fn env(&self) -> &Env;
    fn run(&self, args: &[OsString], options: &RunOptions<'_>) -> Result<()>;
    fn stop(&self) -> Result<()>;

    fn reg(&self, key: &str, name: &str, value: &str, kind: &str) -> Result<()> {
        self.run(
            &argv!["reg", "add", key, "/v", name, "/t", kind, "/d", value, "/f"],
            &RunOptions::default(),
        )
    }
}

pub struct RealWine<'a> {
    system: &'a dyn System,
    prefix: PathBuf,
    runner: PathBuf,
    log: &'a File,
    env: Env,
}

impl<'a> RealWine<'a> {
    pub fn new(system: &'a dyn System, prefix: &Path, runner: &Path, log: &'a File) -> Self {
        Self {
            system,
            prefix: prefix.to_path_buf(),
            runner: runner.to_path_buf(),
            log,
            env: wine_env(system, prefix, runner),
        }
    }

    /// Python: `wine.env = env`, for a caller that runs Wine with its own environment.
    pub fn with_env(mut self, env: Env) -> Self {
        self.env = env;
        self
    }
}

impl Wine for RealWine<'_> {
    fn prefix(&self) -> &Path {
        &self.prefix
    }

    fn runner(&self) -> &Path {
        &self.runner
    }

    fn env(&self) -> &Env {
        &self.env
    }

    fn run(&self, args: &[OsString], options: &RunOptions<'_>) -> Result<()> {
        let mut argv = vec![OsString::from(wine_binary(&self.runner))];
        argv.extend_from_slice(args);
        let env = self.env.with(options.env);
        let mut cmd = Cmd::new(argv);
        cmd.env = Some(&env);
        cmd.cwd = Some(&self.prefix);
        cmd.stdin = Input::Null;
        cmd.output = Output::File(self.log);
        cmd.timeout = Some(options.timeout);
        let finished = process::run(&cmd, &|| self.system.interrupted())?;
        if !options.accepted.contains(&finished.code) {
            return Err(Error::patch(format!(
                "Windows setup failed (exit {}). See the private Fenix setup log.",
                finished.code
            )));
        }
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        // Used only for the private staging profile we created, never a user's running game.
        let server = self.runner.join("files/bin/wineserver");
        let command = |flag: &str| {
            let mut cmd = Cmd::new(argv![&server, flag]);
            cmd.env = Some(&self.env);
            cmd.output = Output::File(self.log);
            cmd.timeout = Some(30);
            process::run(&cmd, &|| self.system.interrupted())
                .map(|finished| (cmd.argv, finished.code))
        };
        // Proton returns 1 when no server exists, e.g. when an already installed
        // geometry dependency needs no Wine commands. Still wait for shutdown.
        let (argv, code) = command("-k")?;
        if !matches!(code, 0 | 1) {
            return Err(process::failure(&argv, code));
        }
        let (argv, code) = command("-w")?;
        if code != 0 {
            return Err(process::failure(&argv, code));
        }
        Ok(())
    }
}

pub fn has_framework(prefix: &Path) -> Result<bool> {
    let path = prefix.join("system.reg");
    let content = match regular(&path, Some(64 * MIB)).and_then(fsx::read_text_lossy) {
        Ok(content) => content,
        Err(Error::Os(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    let section = FRAMEWORK_SECTION.find(&content);
    let release = section.and_then(|section| {
        FRAMEWORK_RELEASE
            .captures(section.as_str())
            .and_then(|found| found.get(1))
    });
    // More digits than fit can only mean a larger number.
    let recent = release.is_some_and(|release| {
        u128::from_str_radix(release.as_str(), 16).map_or(true, |number| number >= 528_040)
    });
    Ok(recent
        && prefix
            .join("drive_c/windows/Microsoft.NET/Framework64/v4.0.30319/clr.dll")
            .is_file())
}

pub fn prepare_framework(
    system: &dyn System,
    wine: &dyn Wine,
    cache: &Path,
    progress: Progress<'_>,
    first_version: &str,
) -> Result<()> {
    if has_framework(wine.prefix())? {
        return progress("Microsoft .NET Framework 4.8 is already installed.");
    }
    let mut packages = Vec::new();
    for (name, url, expected) in DOWNLOADS {
        progress(&format!("Downloading Microsoft {name}"))?;
        packages.push(system.download(
            url,
            &cache.join(name),
            expected,
            download::DEFAULT_LIMIT,
        )?);
    }
    progress("Installing Microsoft .NET Framework. This can take several minutes …")?;
    let mut cmd = Cmd::new(argv![wine_binary(wine.runner()), "uninstaller", "--list"]);
    cmd.env = Some(wine.env());
    cmd.output = Output::Capture;
    cmd.timeout = Some(120);
    let listing =
        String::from_utf8_lossy(&process::run_checked(&cmd, &|| system.interrupted())?.stdout)
            .into_owned();
    for line in py::splitlines(&listing) {
        let (guid, title) = line.split_once('|').unwrap_or((line, ""));
        if title.starts_with("Wine Mono") && PRODUCT_CODE.is_match(guid) {
            wine.run(
                &argv!["uninstaller", "--silent", "--remove", guid],
                &RunOptions::default(),
            )?;
        }
    }
    wine.reg(r"HKCU\Software\Wine", "Version", first_version, "REG_SZ")?;
    let setup = RunOptions {
        env: &[("WINEDLLOVERRIDES", "fusion=b;winemenubuilder.exe=d")],
        accepted: &[0, 194],
        ..RunOptions::default()
    };
    let installed = (|| {
        wine.run(
            &argv![&packages[0], "/q", "/c:install.exe /q /norestart"],
            &setup,
        )?;
        wine.reg(
            r"HKCU\Software\Wine\DllOverrides",
            "mscoree",
            "native",
            "REG_SZ",
        )?;
        for key in [
            r"HKLM\Software\Microsoft\.NETFramework",
            r"HKLM\Software\Wow6432Node\Microsoft\.NETFramework",
        ] {
            wine.reg(key, "OnlyUseLatestCLR", "1", "REG_DWORD")?;
        }
        wine.reg(r"HKCU\Software\Wine", "Version", "win7", "REG_SZ")?;
        wine.run(&argv![&packages[1], "/q", "/norestart"], &setup)
    })();
    wine.reg(r"HKCU\Software\Wine", "Version", "win10", "REG_SZ")?;
    installed?;
    if !has_framework(wine.prefix())? {
        // Wine may not have flushed the registry yet.
        wine.run(&argv!["wineboot", "-u"], &RunOptions::default())?;
        wine.stop()?;
    }
    if !has_framework(wine.prefix())? {
        return Err(Error::patch(
            "Microsoft .NET Framework 4.8 was not detected after setup.",
        ));
    }
    Ok(())
}

/// `tempfile.gettempdir()` candidates, in Python's order.
fn temporary_directory(system: &dyn System, prefix: &str) -> Result<PathBuf> {
    let mut candidates: Vec<PathBuf> = ["TMPDIR", "TEMP", "TMP"]
        .iter()
        .filter_map(|name| system.var(name))
        .map(PathBuf::from)
        .collect();
    candidates.extend(["/tmp", "/var/tmp", "/usr/tmp"].map(PathBuf::from));
    candidates.push(std::env::current_dir()?);
    let mut last = Error::errno(2, "/tmp");
    for candidate in candidates {
        match fsx::mkdtemp(&candidate, prefix) {
            Ok(path) => return fsx::resolve(&path),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// Install the geometry-only provider in the private staging prefix.
///
/// Rendering stays in Wine. Microsoft packages are downloaded from Microsoft,
/// never redistributed. The x86 file is just the update's delta basis; only
/// the verified x64 geometry DLL is installed, under a separate filename.
pub fn prepare_geometry(
    system: &dyn System,
    wine: &dyn Wine,
    cache: &Path,
    bundle: &Path,
    progress: Progress<'_>,
) -> Result<()> {
    let target = contained(wine.prefix(), GEOMETRY_PATH)?;
    if target.is_file() && !target.is_symlink() && digest(&target)? == GEOMETRY_SHA256 {
        return Ok(());
    }
    let mut packages = Vec::new();
    for (name, url, expected) in GEOMETRY_DOWNLOADS {
        progress(&format!(
            "Downloading Microsoft geometry dependency: {name}"
        ))?;
        packages.push(system.download(url, &cache.join(name), expected, 16 * MIB)?);
    }
    let helper = bundle.join("integration/FenixGeometrySetup.exe");
    regular(&helper, None)?;
    let windows = |path: &Path| -> Result<String> {
        Ok(format!(
            "Z:{}",
            fsx::text(&fsx::resolve(path)?).replace('/', "\\")
        ))
    };
    let checked = |path: &Path, expected: &str| -> Result<()> {
        if digest(regular(path, Some(32 * MIB))?)? != expected {
            return Err(Error::patch(format!(
                "Geometry dependency checksum mismatch: {}",
                fsx::name(path)
            )));
        }
        Ok(())
    };
    let work = temporary_directory(system, "fenix-geometry-")?;
    let result = (|| {
        let run = |args: &[OsString], timeout| {
            let options = RunOptions {
                timeout,
                ..RunOptions::default()
            };
            wine.run(args, &options)
        };
        let cabinet = work.join("update.cab");
        progress("Preparing Direct2D geometry for Fenix route rendering …")?;
        run(
            &argv![
                &helper,
                "extract",
                windows(&packages[0])?,
                "Windows6.1-KB2670838-x64.cab",
                windows(&cabinet)?
            ],
            180,
        )?;
        checked(
            &cabinet,
            "470b9ab769a46bbd1acc5e352156e2173a57cbbff03ff6f950d0989d9a150fbc",
        )?;
        for (member, expected) in [
            (
                "0",
                "e74c3bf4727f145ad81d9f1c2674cf6f8bab20acccee9d9c4cf2c375c75aed20",
            ),
            (
                "1",
                "1cc95120454739d69a08fddd7e0e0f125e76115c4eba0748db94c7753c84788c",
            ),
        ] {
            run(
                &argv![
                    &helper,
                    "extract",
                    windows(&cabinet)?,
                    member,
                    windows(&work.join(member))?
                ],
                180,
            )?;
            checked(&work.join(member), expected)?;
        }
        let (basis, output) = (work.join("basis.dll"), work.join("geometry.dll"));
        let library = windows(&packages[1])?;
        run(
            &argv![
                &helper,
                "delta",
                &library,
                "-",
                windows(&work.join("0"))?,
                windows(&basis)?
            ],
            60,
        )?;
        checked(
            &basis,
            "82dc3ddb8c3441ef2de0eb43ed187bc56b747e422a2f240cd33c978356746a7d",
        )?;
        run(
            &argv![
                &helper,
                "delta",
                &library,
                windows(&basis)?,
                windows(&work.join("1"))?,
                windows(&output)?
            ],
            60,
        )?;
        checked(&output, GEOMETRY_SHA256)?;
        atomic(&target, &fsx::read(&output)?, 0o644)
    })();
    let removed = fsx::rmtree(&work);
    result.and(removed)
}

pub fn ensure_ui_fonts(prefix: &Path, runner: &Path, wine: &dyn Wine) -> Result<()> {
    // Chromium's Windows UI font fallback can recurse until its stack overflows
    // if Tahoma is absent from the 64-bit DirectWrite font collection. Old
    // profiles may contain the files and only the 32-bit registry entries.
    let registry = fsx::read_text_lossy(regular(&prefix.join("system.reg"), Some(64 * MIB))?)?;
    let entries = FONTS_SECTION
        .find(&registry)
        .map_or("", |section| section.as_str());
    for (name, family) in [("tahoma.ttf", "Tahoma"), ("tahomabd.ttf", "Tahoma Bold")] {
        let value = format!("{family} (TrueType)");
        // Python: re.search(r'(?m)^"' + re.escape(value) + r'"="[^"\r\n]+"', entries)
        let assignment = format!("\"{value}\"=\"");
        let registered = entries.split('\n').any(|line| {
            line.strip_prefix(&assignment).is_some_and(|rest| {
                rest.find(['"', '\r'])
                    .is_some_and(|end| end > 0 && rest[end..].starts_with('"'))
            })
        });
        if registered {
            continue;
        }
        let target = contained(prefix, format!("drive_c/windows/Fonts/{name}"))?;
        if !fsx::exists(&target) {
            let source = runner.join("files/share/wine/fonts").join(name);
            atomic(&target, &fsx::read(regular(&source, None)?)?, 0o644)?;
        }
        regular(&target, None)?;
        wine.run(
            &argv![
                "reg",
                "add",
                r"HKLM\Software\Microsoft\Windows NT\CurrentVersion\Fonts",
                "/v",
                &value,
                "/t",
                "REG_SZ",
                "/d",
                name,
                "/f",
                "/reg:64"
            ],
            &RunOptions::default(),
        )?;
    }
    Ok(())
}

pub fn graphics_and_fonts(prefix: &Path, runner: &Path, wine: &dyn Wine) -> Result<()> {
    for (arch, folder) in [("x86_64", "system32"), ("i386", "syswow64")] {
        for name in [
            "libvkd3d-1.dll",
            "libvkd3d-shader-1.dll",
            "libvkd3d-utils-1.dll",
        ] {
            let source = runner.join(format!("files/lib/vkd3d/{arch}-windows/{name}"));
            atomic(
                &contained(prefix, format!("drive_c/windows/{folder}/{name}"))?,
                &fsx::read(regular(&source, None)?)?,
                0o644,
            )?;
        }
    }
    let fonts = [
        ("arial.ttf", "Arial"),
        ("arialbd.ttf", "Arial Bold"),
        ("cour.ttf", "Courier New"),
        ("courbd.ttf", "Courier New Bold"),
        ("georgia.ttf", "Georgia"),
        ("times.ttf", "Times New Roman"),
        ("micross.ttf", "Microsoft Sans Serif"),
    ];
    for (name, family) in fonts {
        let source = runner.join("files/share/fonts").join(name);
        atomic(
            &contained(prefix, format!("drive_c/windows/Fonts/{name}"))?,
            &fsx::read(regular(&source, None)?)?,
            0o644,
        )?;
        wine.reg(
            r"HKLM\Software\Microsoft\Windows NT\CurrentVersion\Fonts",
            &format!("{family} (TrueType)"),
            name,
            "REG_SZ",
        )?;
    }
    ensure_ui_fonts(prefix, runner, wine)?;
    wine.reg(
        r"HKCU\Software\Microsoft\Avalon.Graphics",
        "DisableHWAcceleration",
        "1",
        "REG_DWORD",
    )?;
    wine.reg(
        r"HKCU\Software\Wine\Explorer",
        "ShowSystray",
        "0",
        "REG_DWORD",
    )
}

fn parse_xml(raw: &[u8], path: &Path) -> Result<xml::Element> {
    xml::parse(raw).map_err(|error| Error::other(format!("{} is {error}", fsx::name(path))))
}

fn xml_settings(path: &Path, changes: &[(&str, &str)]) -> Result<bool> {
    if !fsx::exists(path) {
        return Ok(false);
    }
    let raw = fsx::read(regular(path, Some(2 * MIB))?)?;
    if xml::has_declarations(&raw) {
        return Err(Error::patch(format!(
            "Unsupported XML declarations in {}",
            fsx::name(path)
        )));
    }
    let mut root = parse_xml(&raw, path)?;
    for (tag, value) in changes {
        root.set(tag, value);
    }
    atomic(path, &root.to_bytes(), 0o600)?;
    Ok(true)
}

pub const NEEDS_SIGN_IN: &str = "Start Fenix once and sign in, then close it and apply settings again. Display rendering needs its initial settings file.";
pub const DEFAULT_FOLDER: &str = "Microsoft Flight Simulator 2024";

/// Record what could be written and return the message for the user.
pub fn apply_settings(
    state: &mut Map<String, Value>,
    prefix: &Path,
    folder: &str,
    renderer: DisplayRenderer,
) -> Result<&'static str> {
    let (configured, readouts) = configure_prefix(prefix, folder, renderer)?;
    state.insert("configured".to_string(), json!(configured));
    state.insert("readouts".to_string(), json!(readouts));
    state.insert(
        "display_renderer".to_string(),
        if configured {
            json!(renderer.as_str())
        } else {
            Value::Null
        },
    );
    if !configured {
        return Ok(NEEDS_SIGN_IN);
    }
    if readouts {
        return Ok(match renderer {
            DisplayRenderer::Cpu => "CPU displays, Legacy readouts and Fenix autostart configured.",
            DisplayRenderer::Gpu => "GPU displays, Legacy readouts and Fenix autostart configured.",
        });
    }
    // Reported for a fresh installation: persistancy.xml first appears when the aircraft is loaded.
    Ok(match renderer {
        DisplayRenderer::Cpu => {
            "CPU displays and Fenix autostart configured. Fenix has not created the file for the FCU readout \
        setting yet: load the aircraft once in the simulator, close the simulator and apply settings again."
        }
        DisplayRenderer::Gpu => {
            "GPU displays and Fenix autostart configured. Fenix has not created the file for the FCU readout \
        setting yet: load the aircraft once in the simulator, close the simulator and apply settings again."
        }
    })
}

pub fn readouts_set(state: &Value) -> bool {
    // Before this key existed, "configured" already required the readout setting.
    let items = state.as_object();
    let value = match items.and_then(|items| items.get("readouts")) {
        Some(value) => Some(value),
        None => items.and_then(|items| items.get("configured")),
    };
    value == Some(&Value::Bool(true))
}

/// Whether the display settings with autostart, and the Legacy readouts, could be written.
pub fn configure_prefix(
    prefix: &Path,
    folder: &str,
    renderer: DisplayRenderer,
) -> Result<(bool, bool)> {
    if !prefix.join(PROGRAM).join("Fenix.exe").is_file() {
        return Ok((false, false));
    }
    let settings = contained(prefix, format!("{CONFIG}/fenixConfig.xml"))?;
    let persisted = contained(prefix, format!("{CONFIG}/persistancy.xml"))?;
    let ready = xml_settings(
        &settings,
        &[
            ("displayMode", renderer.as_str()),
            (
                "newRender",
                if renderer == DisplayRenderer::Gpu {
                    "true"
                } else {
                    "false"
                },
            ),
            (
                "preferCPU",
                if renderer == DisplayRenderer::Gpu {
                    "false"
                } else {
                    "true"
                },
            ),
            ("multithread", "true"),
        ],
    )?;
    let legacy = xml_settings(&persisted, &[("fcuReadoutsType", "0")])?;
    // The official installer owns the aircraft and its Community location.
    // Register its unchanged bootstrapper in the simulator's actual user profile.
    let mut candidates = Vec::new();
    for home in fsx::list_dir(&prefix.join("drive_c/users"))? {
        if ["Public", "Default", "Default User"].contains(&fsx::name(&home).as_str())
            || !home.is_dir()
        {
            continue;
        }
        let relative = home
            .strip_prefix(prefix)
            .map_err(|_| Error::Value("user folder outside the profile".to_string()))?;
        candidates.push(contained(
            prefix,
            relative.join("AppData/Roaming").join(folder),
        )?);
    }
    candidates.retain(|candidate| candidate.is_dir());
    if candidates.len() != 1 {
        return Err(Error::patch(format!(
            "Could not identify one {} settings folder. Run the simulator once first.",
            folder.replace("Microsoft Flight Simulator", "MSFS")
        )));
    }
    let path = candidates[0].join("exe.xml");
    let mut root = if fsx::exists(&path) {
        let raw = fsx::read(regular(&path, Some(2 * MIB))?)?;
        if xml::has_declarations(&raw) {
            return Err(Error::patch("Unsupported exe.xml declarations"));
        }
        parse_xml(&raw, &path)?
    } else {
        let mut root = xml::Element::new("SimBase.Document");
        root.attributes = vec![
            ("Type".to_string(), "Launch".to_string()),
            ("version".to_string(), "1,0".to_string()),
        ];
        root.append("Descr", "Launch");
        root.append("Filename", "exe.xml");
        root.append("Disabled", "False");
        root
    };
    let is_fenix = |node: &xml::Node| match node {
        xml::Node::Element(element) if element.tag == "Launch.Addon" => {
            format!("{}{}", element.findtext("Name"), element.findtext("Path"))
                .to_lowercase()
                .contains("fenix")
        }
        _ => false,
    };
    let mut seen = false;
    root.children
        .retain(|node| !is_fenix(node) || !std::mem::replace(&mut seen, true));
    if !seen {
        root.append("Launch.Addon", "");
    }
    let index = root
        .children
        .iter()
        .position(|node| seen && is_fenix(node))
        .unwrap_or(root.children.len() - 1);
    let Some(xml::Node::Element(entry)) = root.children.get_mut(index) else {
        unreachable!("the entry exists or was just appended");
    };
    for (tag, value) in [
        ("Name", "FenixA320"),
        ("Disabled", "False"),
        ("ManualLoad", "False"),
        (
            "Path",
            r"C:\Program Files\FenixSim A320\deps\FenixBootstrapper.exe",
        ),
    ] {
        entry.set(tag, value);
    }
    atomic(&path, &root.to_bytes(), 0o600)?;
    Ok((ready, ready && legacy))
}

/// `cp -a --reflink=auto`: an independent copy that shares storage where the file system can.
pub fn copy_tree(system: &dyn System, source: &Path, destination: &Path) -> Result<()> {
    let cmd = Cmd::new(argv![
        "cp",
        "-a",
        "--reflink=auto",
        "--",
        source,
        destination
    ]);
    process::run_checked(&cmd, &|| system.interrupted()).map(|_| ())
}

pub fn replace_link(path: &Path, target: &Path) -> Result<()> {
    let temp = path.with_file_name(format!(".fenix-link-{}", py::uuid4_hex()?));
    let result = fsx::symlink(target, &temp).and_then(|()| {
        fs::rename(&temp, path).map_err(|error| Error::io_paths(&error, &temp, path))
    });
    let cleaned = fsx::unlink_missing_ok(&temp);
    result.and(cleaned)
}

/// Total size of the ordinary files below `base`; directory links are not followed.
fn tree_files_size(base: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![base.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                total += entry.metadata().map_or(0, |info| info.size());
            }
        }
    }
    total
}

fn relative_text(path: &Path, root: &Path) -> String {
    fsx::text(path.strip_prefix(root).unwrap_or(path))
}

/// `[st_dev, st_ino]` of a directory, to recognise the original profile later.
fn identity(path: &Path) -> Result<Value> {
    let info = fsx::stat(path)?;
    Ok(json!([info.dev(), info.ino()]))
}

fn stamp() -> Result<String> {
    Ok(format!(
        "{}-{}",
        py::Utc::now().stamp(),
        &py::uuid4_hex()?[..8]
    ))
}

pub fn install(
    system: &dyn System,
    runtime: &OsStr,
    bundle: &OsStr,
    progress: Progress<'_>,
) -> Result<()> {
    system.host_check()?;
    let bundle = verify_bundle(system, bundle)?;
    let lock = system.manifest(None)?;
    let held = locked(system, runtime, true, false)?;
    let root = &held.root;
    let marker = root.join(MARKER);
    let mut upgrade: Option<Value> = None;
    if fsx::exists(&marker) {
        let state = read_json(&marker)?;
        let installed = state.is("state", "installed");
        if installed && json::equal(state.field("version"), lock.need("version")?) {
            verify_installed(system, root, &state)?;
            return progress("This patch version is already installed.");
        }
        if installed
            && state
                .str("version")
                .is_some_and(|version| lock.field("previous_releases").get(version).is_some())
        {
            verify_installed(system, root, &state)?;
            upgrade = Some(state);
        } else {
            return Err(Error::patch(
                "A previous patch transaction exists. Restore it before reinstalling.",
            ));
        }
    }
    let original_runner = fsx::resolve_strict(&root.join("runner"))?;
    let variant = match &upgrade {
        Some(previous) => state_variant(previous)?,
        None => runner_variant(system, &original_runner, false)?,
    };
    let lock = system.manifest(variant.as_deref())?;
    let renderer = match &upgrade {
        Some(previous) => DisplayRenderer::for_state(&lock, previous, None)?,
        None => DisplayRenderer::from_manifest(&lock)?,
    };
    for (name, accepted) in &lock.need_table("accepted_scripts")? {
        let mut accepted: Vec<Value> = accepted
            .as_array()
            .cloned()
            .ok_or_else(|| Error::other("bundle.json: accepted_scripts needs lists"))?;
        if let Some(previous) = &upgrade {
            let release = lock
                .need("previous_releases")?
                .need(previous.need_str("version")?)?;
            accepted.push(release.need("integration")?.need(name)?.clone());
        }
        let found = digest(regular(&root.join("tools").join(name), None)?)?;
        if !accepted.iter().any(|sha| json::is_text(sha, &found)) {
            return Err(Error::patch(format!(
                "Custom Flightdeck launch script detected; it will not be overwritten: {name}"
            )));
        }
    }
    let prefix = root.join("local/msfs-prefix");
    // Conservative bound; reflinks often consume much less physical storage.
    let size = tree_files_size(&prefix) + tree_files_size(&original_runner);
    if fsx::free_space(root)? < size + 1024 * 1024 * 1024 {
        return Err(Error::patch(
            "Not enough free space for the independent Wine profile and runner copies.",
        ));
    }
    let stamp = stamp()?;
    let backup = root
        .join("private")
        .join(format!("fenix-patch-backup-{stamp}"));
    fsx::mkdir(&backup, 0o700)?;
    let work = root.join("local").join(format!("fenix-patch-{stamp}"));
    fsx::mkdir(&work, 0o700)?;
    let selection = root.join("private/proton-selection.json");
    let mut state = Map::new();
    state.insert("format".into(), json!(1));
    state.insert("version".into(), lock.need("version")?.clone());
    state.insert("state".into(), json!("preparing"));
    state.insert("work".into(), json!(relative_text(&work, root)));
    state.insert("backup".into(), json!(relative_text(&backup, root)));
    state.insert(
        "previous_runner".into(),
        json!(fsx::text(&fsx::readlink(&root.join("runner"))?)),
    );
    state.insert(
        "previous_prefix".into(),
        json!(format!("local/msfs-prefix.before-fenix-{stamp}")),
    );
    state.insert("configured".into(), json!(false));
    state.insert("original_prefix_id".into(), identity(&prefix)?);
    state.insert("variant".into(), json!(variant));
    state.insert(
        "proton_before".into(),
        if fsx::exists(&selection) {
            read_json(&selection)?
        } else {
            Value::Null
        },
    );
    if let Some(previous) = &upgrade {
        // Keep the original pre-patch restore point. The profile being
        // updated is retained separately, including later aircraft/account
        // data. Never bootstrap .NET again in an already patched profile.
        write_json(&backup.join("previous-patch.json"), previous)?;
        for key in [
            "backup",
            "previous_runner",
            "previous_prefix",
            "original_prefix_id",
            "configured",
        ] {
            state.insert(key.into(), previous.need(key)?.clone());
        }
        state.insert("readouts".into(), json!(readouts_set(previous)));
        for key in ["display_renderer", "display_renderer_override"] {
            if let Some(value) = previous.item(key) {
                state.insert(key.into(), value.clone());
            }
        }
        state.insert(
            "proton_before".into(),
            previous
                .item("proton_before")
                .cloned()
                .unwrap_or(Value::Null),
        );
        state.insert("upgrade_backup".into(), json!(relative_text(&backup, root)));
        state.insert(
            "upgrade_previous_prefix".into(),
            json!(format!("local/msfs-prefix.before-fenix-update-{stamp}")),
        );
    }
    for name in SCRIPTS {
        fsx::copy2(&root.join("tools").join(name), &backup.join(name))?;
    }
    if root.join("private/import-manifest.json").is_file() {
        fsx::copy2(
            &root.join("private/import-manifest.json"),
            &backup.join("import-manifest.json"),
        )?;
    }
    let save = |state: &Map<String, Value>| write_json(&marker, &Value::Object(state.clone()));
    save(&state)?;
    let staged = work.join("prefix");
    let runner = work.join("runner");
    let result = (|| {
        progress(if upgrade.is_some() {
            "Updating the Fenix patch; installed aircraft and settings are retained …"
        } else {
            "Copying Wine profile and runner; the original profile remains available …"
        })?;
        copy_tree(system, &prefix, &staged)?;
        // A copied prefix may contain an absolute C: symlink. Never let
        // the staging installer write through it into the original.
        let drive = staged.join("dosdevices/c:");
        if drive.is_symlink() {
            fsx::unlink(&drive)?;
        }
        fsx::symlink(Path::new("../drive_c"), &drive)?;
        copy_tree(system, &original_runner, &runner)?;
        let logpath = backup.join("setup.log");
        {
            let log = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&logpath)
                .map_err(|error| Error::io_path(&error, &logpath))?;
            fsx::chmod(&logpath, 0o600)?;
            let wine = system.wine(&staged, &runner, &log);
            let prepared = (|| {
                if upgrade.is_none() {
                    system.prepare_framework(
                        wine.as_ref(),
                        &root.join("private/fenix-downloads"),
                        progress,
                        "winxp",
                    )?;
                    progress("Preparing fonts, graphics dependencies and Fenix settings …")?;
                    system.graphics_and_fonts(&staged, &runner, wine.as_ref())?;
                }
                system.prepare_geometry(
                    wine.as_ref(),
                    &root.join("private/fenix-downloads"),
                    &bundle,
                    progress,
                )
            })();
            wine.stop()?;
            prepared?;
        }
        // Bootstrap native Framework with the unchanged runner first.
        // Publish the overlay only after every staging Wine process exited.
        apply_overlay(system, &staged, &runner, &bundle, variant.as_deref())?;
        let renderer_changed = upgrade
            .as_ref()
            .is_some_and(|previous| DisplayRenderer::from_state(previous) != renderer);
        if renderer_changed {
            let settings_backup = backup.join("settings-before-update");
            fsx::mkdir(&settings_backup, 0o700)?;
            backup_settings(&staged, &settings_backup)?;
            progress("Updating the display renderer for this Proton build …")?;
        }
        if upgrade.is_none() || renderer_changed {
            apply_settings(&mut state, &staged, DEFAULT_FOLDER, renderer)?;
        }
        system.ensure_idle(&prefix)?;
        state.insert("state".into(), json!("committing"));
        save(&state)?;
        let previous_prefix = match state.get("upgrade_previous_prefix") {
            Some(name) => name,
            None => &state["previous_prefix"],
        };
        let previous_prefix = previous_prefix
            .as_str()
            .ok_or_else(|| Error::other("previous_prefix is not text"))?;
        system.rename(&prefix, &root.join(previous_prefix))?;
        system.rename(&staged, &prefix)?;
        system.replace_link(&root.join("runner"), &runner)?;
        for name in SCRIPTS {
            atomic(
                &root.join("tools").join(name),
                &fsx::read(&bundle.join("integration").join(name))?,
                0o700,
            )?;
        }
        let imported = root.join("private/import-manifest.json");
        if fsx::exists(&imported) {
            let mut info = read_json(&imported)?;
            for name in SCRIPTS {
                if let Some(files) = info
                    .as_object_mut()
                    .and_then(|info| info.get_mut("runtime_files"))
                {
                    let files = files.as_object_mut().ok_or_else(|| {
                        Error::other("import-manifest.json: runtime_files is not a JSON object")
                    })?;
                    files.insert(
                        name.to_string(),
                        json!(digest(&root.join("tools").join(name))?),
                    );
                }
            }
            write_json(&imported, &info)?;
        }
        if fsx::exists(&selection) {
            let selected = read_json(&selection)?;
            if json::truthy(&selected) {
                let mut selected = json::object(selected, "proton-selection.json")?;
                selected.insert("schema".into(), json!(2));
                selected.insert("runner".into(), json!(relative_text(&runner, root)));
                write_json(&selection, &Value::Object(selected))?;
            }
        }
        state.insert("state".into(), json!("installed"));
        save(&state)?;
        progress(if upgrade.is_some() {
            "Fenix compatibility patch updated."
        } else {
            "Patch installed. Install and sign in to Fenix, then apply the aircraft settings."
        })
    })();
    if result.is_err() {
        // Preserve a journal and all profiles; restore is explicitly available
        // after process failure or power loss, without guessing what finished.
        progress(
            "Setup did not finish. The original profile or its backup is retained. Use Restore.",
        )?;
    }
    result
}

pub fn verify_installed(system: &dyn System, root: &Path, state: &Value) -> Result<()> {
    let current = system.manifest(state_variant(state)?.as_deref())?;
    let mut lock = current.clone();
    if !json::equal(state.field("version"), current.need("version")?) {
        let previous = state.str("version").and_then(|version| {
            current
                .item("previous_releases")
                .and_then(|known| known.item(version))
        });
        lock = previous.cloned().ok_or_else(|| {
            Error::patch("This installed patch version is not supported by the current installer.")
        })?;
    }
    let runner = fsx::resolve_strict(&root.join("runner"))?;
    let expected = contained(root, state.need_str("work")?)?.join("runner");
    if runner != expected {
        return Err(Error::patch(
            "The active runner changed since patch installation.",
        ));
    }
    for (name, sha) in &json::merged(
        &current.need_table("runner_files")?,
        &lock.need_table("files")?,
    ) {
        system.interrupted()?;
        if !json::is_text(sha, &digest(regular(&runner.join(name), None)?)?) {
            return Err(Error::patch(format!(
                "Installed patch file changed: {name}"
            )));
        }
    }
    for (name, sha) in &lock.table("prefix_files")? {
        if !json::is_text(
            sha,
            &digest(regular(
                &contained(&root.join("local/msfs-prefix"), name)?,
                None,
            )?)?,
        ) {
            return Err(Error::patch(format!(
                "Installed Fenix dependency changed: {name}"
            )));
        }
    }
    Ok(())
}

pub fn restore(system: &dyn System, runtime: &OsStr, progress: Progress<'_>) -> Result<()> {
    let held = locked(system, runtime, true, true)?;
    let root = &held.root;
    let marker = root.join(MARKER);
    let mut state = json::object(read_json(&marker)?, "The patch state")?;
    let view = Value::Object(state.clone());
    if view.item("migrated").is_some_and(json::truthy) {
        return Err(Error::patch(
            "This migrated profile has no managed pre-patch restore point.",
        ));
    }
    let backup = contained(root, view.need_str("backup")?)?;
    let previous = contained(root, view.need_str("previous_prefix")?)?;
    let work = contained(root, view.need_str("work")?)?;
    if view.is("state", "restored") {
        return Err(Error::patch("This patch is already restored."));
    }
    if fsx::exists(&work.join("prefix")) {
        system.ensure_idle(&work.join("prefix"))?;
    }
    let prefix = root.join("local/msfs-prefix");
    let save = |state: &Map<String, Value>| write_json(&marker, &Value::Object(state.clone()));
    state.insert("state".into(), json!("restoring"));
    save(&state)?;
    if fsx::exists(&previous) {
        if fsx::exists(&prefix) {
            let retained = root
                .join("local")
                .join(format!("msfs-prefix.fenix-retained-{}", py::uuid4_hex()?));
            system.rename(&prefix, &retained)?;
            state.insert(
                "retained_prefix".into(),
                json!(relative_text(&retained, root)),
            );
            save(&state)?;
        }
        system.rename(&previous, &prefix)?;
    } else if !fsx::exists(&prefix)
        || !json::equal(&identity(&prefix)?, view.field("original_prefix_id"))
    {
        return Err(Error::patch(
            "The original profile backup is missing; nothing was removed.",
        ));
    }
    system.replace_link(
        &root.join("runner"),
        Path::new(view.need_str("previous_runner")?),
    )?;
    for name in SCRIPTS {
        atomic(
            &root.join("tools").join(name),
            &fsx::read(regular(&backup.join(name), None)?)?,
            0o700,
        )?;
    }
    if fsx::exists(&backup.join("import-manifest.json")) {
        atomic(
            &root.join("private/import-manifest.json"),
            &fsx::read(&backup.join("import-manifest.json"))?,
            0o600,
        )?;
    }
    match view
        .item("proton_before")
        .filter(|selected| json::truthy(selected))
    {
        Some(selected) => write_json(&root.join("private/proton-selection.json"), selected)?,
        None => fsx::unlink_missing_ok(&root.join("private/proton-selection.json"))?,
    }
    state.insert("state".into(), json!("restored"));
    write_json(&backup.join("restored.json"), &Value::Object(state))?;
    fsx::unlink(&marker)?;
    progress("Original runner and Windows profile restored. The newer profile is retained locally.")
}

/// Keep a copy of the Fenix settings before they are rewritten.
pub fn backup_settings(prefix: &Path, backup: &Path) -> Result<()> {
    let Ok(entries) = fsx::list_dir(&prefix.join(CONFIG)) else {
        return Ok(());
    };
    for path in entries {
        if ["fenixConfig.xml", "persistancy.xml"].contains(&fsx::name(&path).as_str()) {
            fsx::copy2(regular(&path, None)?, &backup.join(fsx::name(&path)))?;
        }
    }
    Ok(())
}

pub fn configure(system: &dyn System, runtime: &OsStr, progress: Progress<'_>) -> Result<()> {
    configure_with_renderer(system, runtime, progress, None)
}

pub fn configure_with_renderer(
    system: &dyn System,
    runtime: &OsStr,
    progress: Progress<'_>,
    requested: Option<DisplayRenderer>,
) -> Result<()> {
    let held = locked(system, runtime, true, false)?;
    let root = &held.root;
    let state = read_json(&root.join(MARKER))?;
    if !state.is("state", "installed") {
        return Err(Error::patch("Finish patch installation first."));
    }
    verify_installed(system, root, &state)?;
    let lock = system.manifest(state_variant(&state)?.as_deref())?;
    let renderer = DisplayRenderer::for_state(&lock, &state, requested)?;
    let prefix = root.join("local/msfs-prefix");
    // Preserve settings on every user-triggered reconfiguration.
    let backup =
        contained(root, state.need_str("backup")?)?.join(format!("settings-{}", py::uuid4_hex()?));
    fsx::mkdir(&backup, 0o700)?;
    backup_settings(&prefix, &backup)?;
    let mut state = json::object(state, "The patch state")?;
    let message = apply_settings(&mut state, &prefix, DEFAULT_FOLDER, renderer)?;
    if let Some(renderer) = requested {
        state.insert("display_renderer_override".into(), json!(renderer.as_str()));
    }
    write_json(&root.join(MARKER), &Value::Object(state.clone()))?;
    if state.get("configured") != Some(&Value::Bool(true)) {
        return Err(Error::patch(NEEDS_SIGN_IN));
    }
    progress(message)
}

pub fn manager_path(prefix: &Path) -> Result<PathBuf> {
    let users = fs::read_dir(prefix.join("drive_c/users"))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_else(|_| Vec::new());
    let mut candidates = Vec::new();
    for user in users {
        let candidate = user.join("AppData/Local/FenixApp/current/FenixApp.exe");
        if user.is_dir()
            && candidate.is_file()
            && fsx::resolve(&candidate)?.starts_with(fsx::resolve(prefix)?)
        {
            candidates.push(candidate);
        }
    }
    if candidates.len() != 1 {
        return Err(Error::patch("Install the official Fenix Installer first."));
    }
    regular(&candidates[0], None)?;
    Ok(candidates.remove(0))
}

/// Waits for a started Fenix program and returns its exit status.
pub type Waiter<'a> = &'a mut dyn FnMut(&mut Child) -> Result<i32>;

pub fn windows_app(
    system: &dyn System,
    runtime: &OsStr,
    executable: Option<&OsStr>,
    progress: Progress<'_>,
    manager: bool,
    wait: Option<Waiter<'_>>,
) -> Result<()> {
    let held = locked(system, runtime, true, false)?;
    let root = &held.root;
    let executable = executable.filter(|executable| !executable.is_empty());
    if fsx::exists(&root.join(MARKER)) {
        let state = read_json(&root.join(MARKER))?;
        if !state.is("state", "installed") {
            return Err(Error::patch("Install the compatibility patch first."));
        }
        verify_installed(system, root, &state)?;
    } else if executable.is_some() || !root.join("private/fenix-compat.json").is_file() {
        return Err(Error::patch("Install the compatibility patch first."));
    }
    run_app(
        system,
        &root.join("local/msfs-prefix"),
        &fsx::resolve_strict(&root.join("runner"))?,
        &root.join("private/fenix-app.log"),
        executable,
        progress,
        manager,
        wait,
    )
}

/// Run an official Fenix program; the caller holds the profile's lock.
#[allow(clippy::too_many_arguments)]
pub fn run_app(
    system: &dyn System,
    prefix: &Path,
    runner: &Path,
    log: &Path,
    executable: Option<&OsStr>,
    progress: Progress<'_>,
    manager: bool,
    wait: Option<Waiter<'_>>,
) -> Result<()> {
    let app = if let Some(executable) = executable.filter(|executable| !executable.is_empty()) {
        let missing = || {
            Error::patch(format!(
                "This file does not exist: {}",
                executable.to_string_lossy()
            ))
        };
        let app = match fsx::resolve_strict(&fsx::expanduser(executable, system.var("HOME"))) {
            Ok(app) => app,
            Err(Error::Os(_)) => return Err(missing()),
            Err(error) => return Err(error),
        };
        regular(&app, Some(1024 * MIB))?;
        let mut signature = Vec::new();
        File::open(&app)
            .and_then(|file| file.take(2).read_to_end(&mut signature))
            .map_err(|error| Error::io_path(&error, &app))?;
        if !fsx::suffix(&fsx::name(&app)).eq_ignore_ascii_case(".exe") || signature != b"MZ" {
            return Err(Error::patch(
                "Select the official Fenix Installer .exe from your Fenix account.",
            ));
        }
        app
    } else if manager {
        manager_path(prefix)?
    } else {
        let app = prefix.join(PROGRAM).join("Fenix.exe");
        regular(&app, None)?;
        app
    };
    let output = OpenOptions::new()
        .append(true)
        .create(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .mode(0o600)
        .open(log)
        .map_err(|error| Error::io_path(&error, log))?;
    let wine = system.wine(prefix, runner, &output);
    ensure_ui_fonts(prefix, runner, wine.as_ref())?;
    // Wine's fallback notification area otherwise becomes a separate
    // blank window on desktops without an XEmbed tray. Scope to this
    // runtime; the main Fenix UI remains available through Flightdeck.
    wine.reg(
        r"HKCU\Software\Wine\Explorer",
        "ShowSystray",
        "0",
        "REG_DWORD",
    )?;
    progress(
        "Fenix is open. Complete its setup or sign-in, then close the application to continue.",
    )?;
    let env = wine_env(system, prefix, runner);
    let mut cmd = Cmd::new(argv![wine_binary(runner), &app]);
    cmd.cwd = Some(fsx::parent(&app));
    cmd.env = Some(&env);
    cmd.stdin = Input::Null;
    cmd.output = Output::File(&output);
    let mut child = system.spawn_app(&cmd)?;
    let code = match wait {
        None => match process::wait(&mut child, None, &|| system.interrupted()) {
            Ok(code) => code.unwrap_or(0),
            Err(error) => {
                if error == Error::Interrupted {
                    let _ = process::wait(&mut child, Some(Duration::from_millis(250)), &|| Ok(()));
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        },
        // A launcher may offer cancellation while retaining this exact
        // runtime lease through companion cleanup. The default CLI/GUI
        // still simply waits for the official application to exit.
        Some(wait) => match wait(&mut child) {
            Ok(code) => code,
            Err(error) => {
                if matches!(child.try_wait(), Ok(None)) {
                    if let Some(pid) = i32::try_from(child.id())
                        .ok()
                        .and_then(rustix::process::Pid::from_raw)
                    {
                        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                    }
                    if !matches!(
                        process::wait(&mut child, Some(Duration::from_secs(3)), &|| Ok(())),
                        Ok(Some(_))
                    ) {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                return Err(error);
            }
        },
    };
    if code != 0 {
        return Err(process::failure(&cmd.argv, code));
    }
    Ok(())
}

/// The part of a status report that comes from the patch state.
pub fn state_report(result: &mut Map<String, Value>, state: &Value) {
    let installed = state.is("state", "installed");
    result.insert(
        "state".into(),
        match state.as_object().and_then(|items| items.get("state")) {
            Some(value) => value.clone(),
            None => json!("interrupted"),
        },
    );
    result.insert("installed".into(), json!(installed));
    result.insert(
        "configured".into(),
        json!(state.item("configured") == Some(&Value::Bool(true))),
    );
    result.insert("readouts".into(), json!(readouts_set(state)));
    result.insert(
        "display_renderer_override".into(),
        state.field("display_renderer_override").clone(),
    );
    result.insert(
        "display_renderer".into(),
        if state.item("configured") == Some(&Value::Bool(true)) {
            json!(DisplayRenderer::from_state(state).as_str())
        } else {
            Value::Null
        },
    );
}

pub fn snapshot(system: &dyn System, runtime: &OsStr) -> Result<Value> {
    let mut result = Map::new();
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
        let root = runtime_path(system, runtime, true)?;
        result.insert(
            "fenix_installed".into(),
            json!(
                root.join("local/msfs-prefix")
                    .join(PROGRAM)
                    .join("Fenix.exe")
                    .is_file()
            ),
        );
        let manager = match manager_path(&root.join("local/msfs-prefix")) {
            Ok(_) => true,
            Err(Error::Patch(_)) => false,
            Err(error) => return Err(error),
        };
        result.insert("manager_installed".into(), json!(manager));
        let marker = root.join(MARKER);
        if fsx::exists(&marker) {
            let state = read_json(&marker)?;
            json::object(state.clone(), "The patch state")?;
            let installed = state.is("state", "installed");
            state_report(&mut result, &state);
            result.insert(
                "can_restore".into(),
                json!(!state.item("migrated").is_some_and(json::truthy)),
            );
            result.insert(
                "installed_version".into(),
                state.item("version").cloned().unwrap_or(Value::Null),
            );
            let update = installed && {
                let known = system.manifest(None)?.table("previous_releases")?;
                state
                    .str("version")
                    .is_some_and(|version| known.contains_key(version))
            };
            result.insert("update_available".into(), json!(update));
        } else if fsx::exists(&root.join("private/fenix-compat.json")) {
            result.insert("state".into(), json!("legacy"));
            result.insert(
                "message".into(),
                json!("An earlier local Fenix patch is active. Keep using it; automatic replacement is disabled."),
            );
        } else {
            system.host_check()?;
            runner_variant(system, &fsx::resolve_strict(&root.join("runner"))?, false)?;
            result.insert("state".into(), json!("available"));
        }
        let idle = match system.ensure_idle(&root.join("local/msfs-prefix")) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_library_version_is_read_from_the_systems_own_tools() {
        for (output, expected) in [
            ("glibc 2.44\n", Some((2, 44))),
            ("glibc 2.38.9000\n", Some((2, 38))),
            (
                "ldd (GNU libc) 2.44\nCopyright (C) 2026 Free Software Foundation, Inc.\n",
                Some((2, 44)),
            ),
            ("ldd (Debian GLIBC 2.36-9+deb12u10) 2.36\n", Some((2, 36))),
            ("ldd (Ubuntu GLIBC 2.39-0ubuntu8.5) 2.39\n", Some((2, 39))),
            ("musl libc (x86_64)\nVersion 1.2.5\n", None),
            ("getconf: GNU_LIBC_VERSION: unknown variable\n", None),
            ("glibc two\n", None),
            ("", None),
        ] {
            assert_eq!(glibc_version(output.as_bytes()), expected, "{output:?}");
        }
        assert!((2, 37) < (2, 38) && (2, 100) > (2, 38) && (3, 0) > (2, 38));
    }

    #[test]
    fn build_numbers_are_ignored_in_versions() {
        assert_eq!(
            plain_version("1786134211 xodus-bleeding-edge-11.0"),
            "xodus-bleeding-edge-11.0"
        );
        assert_eq!(plain_version("fixture version"), "fixture version");
        assert_eq!(plain_version("12\u{1c}\t x 34 y"), "x 34 y");
        assert_eq!(plain_version("12"), "12");
    }

    #[test]
    fn variants_of_state_files() {
        assert_eq!(variant_name(&json!(null)).ok(), Some(None));
        assert_eq!(variant_name(&json!("")).ok(), Some(None));
        assert_eq!(
            variant_name(&json!("cachyos-11")).ok(),
            Some(Some("cachyos-11".to_string()))
        );
        assert_eq!(variant_name(&json!(5)), Err(Error::patch(NO_BUILD)));
        assert!(readouts_set(
            &json!({"readouts": true, "configured": false})
        ));
        assert!(!readouts_set(
            &json!({"readouts": null, "configured": true})
        ));
        assert!(!readouts_set(&json!({"configured": 1})));
    }
}
