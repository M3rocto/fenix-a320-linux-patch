// SPDX-License-Identifier: MIT
//! `fenix-tools build`: rebuild the Wine modules from the exact archived
//! sources and patches.
use crate::{Context, archive::extract, util};
use fenix_patch::{
    Error, Result, argv, fsx,
    json::{self, Dict, Map, Value},
    process::Env,
};
use std::{
    ffi::OsString,
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// What `make` builds, and where each module belongs in a runner.
pub const TARGETS: [(&str, &str); 11] = [
    (
        "dlls/ntdll/x86_64-windows/ntdll.dll",
        "files/lib/wine/x86_64-windows/ntdll.dll",
    ),
    ("dlls/ntdll/ntdll.so", "files/lib/wine/x86_64-unix/ntdll.so"),
    ("server/wineserver", "files/bin/wineserver"),
    (
        "dlls/crypt32/x86_64-windows/crypt32.dll",
        "files/lib/wine/x86_64-windows/crypt32.dll",
    ),
    (
        "dlls/mmdevapi/x86_64-windows/mmdevapi.dll",
        "files/lib/wine/x86_64-windows/mmdevapi.dll",
    ),
    (
        "dlls/kernelbase/x86_64-windows/kernelbase.dll",
        "files/lib/wine/x86_64-windows/kernelbase.dll",
    ),
    (
        "dlls/d2d1/x86_64-windows/d2d1.dll",
        "files/lib/wine/x86_64-windows/d2d1.dll",
    ),
    (
        "dlls/dwrite/x86_64-windows/dwrite.dll",
        "files/lib/wine/x86_64-windows/dwrite.dll",
    ),
    (
        "dlls/dwrite/dwrite.so",
        "files/lib/wine/x86_64-unix/dwrite.so",
    ),
    (
        "dlls/win32u/win32u.so",
        "files/lib/wine/x86_64-unix/win32u.so",
    ),
    (
        "dlls/winex11.drv/winex11.so",
        "files/lib/wine/x86_64-unix/winex11.so",
    ),
];

// configure links ntdll.so against libunwind when the build host has it. The
// pinned runners do not, and neither Steam Linux Runtime ships the library.
pub const CONFIGURE: [&str; 5] = [
    "--enable-win64",
    "--disable-tests",
    "--enable-silent-rules",
    "--without-ffmpeg",
    "--without-unwind",
];

#[derive(Debug, Clone)]
pub struct Options {
    pub prepare_only: bool,
    /// Fresh source/object directory; `ROOT/build` when not given.
    pub build_dir: Option<PathBuf>,
    pub record_build: bool,
    pub jobs: i64,
}

pub(crate) fn value(message: &str) -> Error {
    Error::Value(message.to_string())
}

/// `json.loads((ROOT / "bundle.json").read_text())`.
pub(crate) fn read_lock(root: &Path) -> Result<Map<String, Value>> {
    json::object(
        json::loads(&fsx::read_text(&root.join("bundle.json"))?)?,
        "bundle.json",
    )
}

/// `(ROOT / "bundle.json").write_text(json.dumps(lock, indent=2) + "\n")`.
pub(crate) fn write_lock(root: &Path, lock: Map<String, Value>) -> Result<()> {
    let text = json::dumps(&Value::Object(lock), Some(2)) + "\n";
    util::write_text(&root.join("bundle.json"), &text)
}

/// Download `url` to the new file `path`. A file that is already there is
/// never written to; what an interrupted download leaves is removed again.
pub(crate) fn fetch(ctx: &Context<'_>, url: &str, path: &Path) -> Result<()> {
    let stream = (ctx.fetch)(url)?;
    // As in Python, nothing is created before the server has answered.
    let first = stream.next(ctx.system)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o666)
        .open(path)
        .map_err(|error| Error::io_path(&error, path))?;
    let written = (|| {
        let mut chunk = first;
        while let Some(data) = chunk {
            output
                .write_all(&data)
                .map_err(|error| Error::io_path(&error, path))?;
            chunk = stream.next(ctx.system)?;
        }
        Ok(())
    })();
    drop(output);
    if written.is_err() {
        let _ = fsx::unlink_missing_ok(path);
    }
    written
}

/// `patch --batch --fuzz=0 -p1 -i patch`.
pub(crate) fn apply_patch(ctx: &Context<'_>, patch: &Path, source: &Path, env: &Env) -> Result<()> {
    ctx.call(
        argv![
            &ctx.programs.patch,
            "--batch",
            "--fuzz=0",
            "-p1",
            "-i",
            patch
        ]
        .to_vec(),
        env,
        Some(source),
    )
}

/// Everything between the prepared source tree and the built modules in `objects`.
pub(crate) fn compile(
    ctx: &Context<'_>,
    source: &Path,
    output: &Path,
    flags: &OsString,
    jobs: i64,
    env: &Env,
) -> Result<()> {
    let run = |command: Vec<OsString>, cwd: &Path| ctx.call(command, env, Some(cwd));
    run(argv!["./tools/make_specfiles"].to_vec(), source)?;
    run(
        argv![
            &ctx.programs.python3,
            "dlls/winevulkan/make_vulkan",
            "--xml",
            source.join("dlls/winevulkan/vk.xml"),
            "--video-xml",
            source.join("dlls/winevulkan/video.xml")
        ]
        .to_vec(),
        source,
    )?;
    run(argv![&ctx.programs.autoreconf, "-f"].to_vec(), source)?;
    fsx::mkdir(output, 0o777)?;
    let assignment = |name: &str| {
        let mut text = OsString::from(name);
        text.push(flags);
        text
    };
    let mut configure = vec![OsString::from(source.join("configure"))];
    configure.extend(CONFIGURE.map(OsString::from));
    configure.extend([assignment("CFLAGS="), assignment("CROSSCFLAGS=")]);
    run(configure, output)?;
    run(argv![&ctx.programs.make, "depend"].to_vec(), output)?;
    let mut make = argv![&ctx.programs.make, format!("-j{jobs}")].to_vec();
    make.extend(TARGETS.map(|(target, _)| OsString::from(target)));
    run(make, output)
}

/// Copy one built module to `file` and remove its debugging information.
pub(crate) fn install_module(
    ctx: &Context<'_>,
    built: &Path,
    file: &Path,
    source: &Path,
    env: &Env,
) -> Result<()> {
    fsx::mkdir_parents(fsx::parent(file), 0o777)?;
    fsx::copy2(built, file)?;
    let strip = if fsx::suffix(&fsx::name(file)) == ".dll" {
        &ctx.programs.mingw_strip
    } else {
        &ctx.programs.strip
    };
    ctx.call(
        argv![strip, "--strip-debug", file].to_vec(),
        env,
        Some(source),
    )
}

/// `{name: sha(base / name) for name in names}`.
fn digests(base: &Path, names: &Map<String, Value>) -> Result<Map<String, Value>> {
    let mut found = Map::new();
    for name in names.keys() {
        found.insert(name.clone(), Value::String(fsx::digest(&base.join(name))?));
    }
    Ok(found)
}

pub fn run(ctx: &Context<'_>, options: &Options) -> Result<()> {
    let root = ctx.root();
    let mut lock = read_lock(root)?;
    let view = Value::Object(lock.clone());
    let sources = root.join("sources");
    util::mkdir_exist_ok(&sources)?;
    let codeload = "https://codeload.github.com/xodus-gaming/";
    let urls = [
        (
            "wine-source.tar.gz",
            format!("{codeload}wine/tar.gz/{}", view.need_str("wine_revision")?),
        ),
        (
            "xgameruntime-source.tar.gz",
            format!(
                "{codeload}xgameruntime/tar.gz/{}",
                view.need_str("xgameruntime_revision")?
            ),
        ),
    ];
    for (name, expected) in &view.need_table("sources")? {
        let path = sources.join(name);
        if !fsx::exists(&path) {
            let (_, url) = urls
                .iter()
                .find(|(known, _)| known == name)
                .ok_or_else(|| Error::Key(name.clone()))?;
            fetch(ctx, url, &path)?;
        }
        if !json::is_text(expected, &fsx::digest(&path)?) {
            return Err(Error::Value(format!("Source checksum mismatch: {name}")));
        }
    }
    let build = fsx::resolve(
        &options
            .build_dir
            .clone()
            .unwrap_or_else(|| root.join("build")),
    )?;
    fsx::mkdir_parents(&build, 0o777)?;
    let source = build.join("wine");
    if fsx::exists(&source) {
        return Err(value(
            "Use an empty build directory; existing work is never overwritten",
        ));
    }
    extract(&sources.join("wine-source.tar.gz"), &source)?;
    let submodule = source.join("dlls/xgameruntime");
    if fsx::exists(&submodule) {
        std::fs::remove_dir(&submodule).map_err(|error| Error::io_path(&error, &submodule))?;
    }
    extract(&sources.join("xgameruntime-source.tar.gz"), &submodule)?;
    let inherited = ctx.system.environ();
    for (name, expected) in &view.need_table("patches")? {
        let path = root.join("patches").join(name);
        if !json::is_text(expected, &fsx::digest(&path)?) {
            return Err(Error::Value(format!("Patch checksum mismatch: {name}")));
        }
        apply_patch(ctx, &path, &source, &inherited)?;
    }
    if options.prepare_only {
        return Ok(());
    }
    let mut env = inherited;
    env.set("XDG_CACHE_HOME", build.join("cache"));
    let output = build.join("objects");
    let mut flags = OsString::from("-g -O2 -ffile-prefix-map=");
    flags.push(root);
    flags.push("=/usr/src/fenix-a320-linux-patch -ffile-prefix-map=");
    flags.push(&build);
    flags.push("=/usr/src/fenix-a320-linux-patch/build");
    // The protocol 931 generated header/trace are supplied by the patch. Do not
    // let make_requests increment the ABI used by the pinned 32-bit clients:
    // the build starts at make_specfiles.
    compile(ctx, &source, &output, &flags, options.jobs, &env)?;
    for (target, relative) in TARGETS {
        let file = root.join("payload").join(relative);
        install_module(ctx, &output.join(target), &file, &source, &env)?;
    }
    let gcc = |arguments: &[OsString]| {
        let mut command = argv![&ctx.programs.mingw_gcc, "-Os", "-s", "-municode"].to_vec();
        command.extend_from_slice(arguments);
        ctx.call(command, &env, Some(&source))
    };
    gcc(&argv![
        "-mwindows",
        "-Wl,--no-insert-timestamp",
        "-o",
        root.join("integration/FenixWindowGuard.exe"),
        root.join("native/window-guard.c"),
        "-luser32"
    ])?;
    gcc(&argv![
        "-Wl,--no-insert-timestamp",
        "-o",
        root.join("integration/FenixGeometrySetup.exe"),
        root.join("native/geometry-setup.c"),
        "-lsetupapi"
    ])?;
    gcc(&argv![
        "-Wl,--no-insert-timestamp",
        "-o",
        root.join("integration/FenixMCDURefresh.exe"),
        root.join("native/mcdu-refresh.c")
    ])?;
    let current = digests(&root.join("payload"), &view.need_table("files")?)?;
    let integration = digests(&root.join("integration"), &view.need_table("integration")?)?;
    if options.record_build {
        let helper = |name: &str| {
            integration
                .get(name)
                .cloned()
                .ok_or_else(|| Error::Key(name.to_string()))
        };
        let guard = helper("FenixWindowGuard.exe");
        lock.insert("files".to_string(), Value::Object(current));
        lock.insert(
            "integration".to_string(),
            Value::Object(integration.clone()),
        );
        let prefix_files = lock
            .entry("prefix_files")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| Error::other("bundle.json: prefix_files is not a JSON object"))?;
        let system32 = "drive_c/windows/system32/";
        prefix_files.insert(format!("{system32}FenixWindowGuard.exe"), guard?);
        for name in ["FenixMCDURefresh.exe", "fenix-display-refresh.py"] {
            prefix_files.insert(format!("{system32}{name}"), helper(name)?);
        }
        write_lock(root, lock)
    } else if !json::equal(&Value::Object(current), view.need("files")?)
        || !json::equal(&Value::Object(integration), view.need("integration")?)
    {
        Err(value(
            "Build finished with different hashes. Compiler/platform/timestamps can differ; review before --record-build and repin Flightdeck.",
        ))
    } else {
        Ok(())
    }
}
