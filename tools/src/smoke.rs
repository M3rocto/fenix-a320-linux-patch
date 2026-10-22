// SPDX-License-Identifier: MIT
//! `fenix-tools smoke`: run own-code probes against the release binaries in a
//! fresh disposable prefix.
use crate::{
    Context,
    build::{prepare_dxvk, value},
    util,
    x11::check_host_windows,
};
use fenix_patch::{
    Error, Result, argv,
    core::{self, RealWine},
    error::repr_list,
    fsx,
    json::{Dict, Map, Value},
    process::{self, Cmd, Env, Output},
    py,
};
use regex::Regex;
use rustix::process::{Pid, Signal, kill_process};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    process::Child,
    sync::LazyLock,
    time::Duration,
};

// Present in the Steam Linux Runtime 3 (sniper) and 4 containers. The cachyos-10
// base links libgcc statically, so its overlay needs this one library more.
const RUNTIME_LIBRARIES: [&str; 1] = ["libgcc_s.so.1"];

static NEEDED: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]"));

/// Executable, window title, the title the patched Wine gives it, and whether
/// the window is a helper that X11 must not show. A pair tells the first
/// report of the probe from the second.
pub const WINDOW_GUARD: [(&str, &str, &str, (bool, bool)); 8] = [
    (
        "FenixDisplay.exe",
        "ProSimA322 Display",
        "Fenix Display",
        (true, true),
    ),
    (
        "FenixSystem.exe",
        "ProSimA322 System",
        "FenixSim A320 System 2.4.0.4720",
        (true, true),
    ),
    (
        "FenixCDU.exe",
        "ProSimA322 MCDU",
        "ProSimA322 MCDU",
        (true, true),
    ),
    (
        "UnrelatedApplication.exe",
        "ProSimA322 Display",
        "Fenix Display",
        (false, false),
    ),
    (
        "Fenix.exe",
        "ProSimA322 Display",
        "Fenix Display",
        (false, false),
    ),
    (
        "FenixDisplay.exe",
        "Fenix Account",
        "Fenix Account",
        (false, false),
    ),
    (
        "FenixSystem.exe",
        "FenixSim A320 System Login",
        "FenixSim A320 System 2.",
        (false, false),
    ),
    (
        "FenixDisplay.exe",
        "Fenix Settings",
        "ProSimA322 Display",
        (false, true),
    ),
];

#[derive(Debug, Clone)]
pub struct Options {
    pub runner: PathBuf,
    /// SteamLinuxRuntime directory the Proton requires; also starts the runner inside its container.
    pub steam_runtime: Option<PathBuf>,
    /// Exact Proton overlay from bundle.json; omitted for Xodus.
    pub variant: Option<String>,
    /// `ROOT/build/smoke` when not given.
    pub work: Option<PathBuf>,
    /// `ROOT/build/downloads` when not given.
    pub cache: Option<PathBuf>,
}

/// Python's `try: body finally: cleanup`: a failing clean-up is what the
/// command ends with, after the failure it followed has been shown.
fn finally(ctx: &Context<'_>, body: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (body, cleanup) {
        (Err(first), Err(second)) => {
            ctx.warn(&format!("fenix-tools: {first}"));
            Err(second)
        }
        (body, Ok(())) => body,
        (Ok(()), cleanup) => cleanup,
    }
}

/// The shared libraries a program asks the loader for.
fn needed(ctx: &Context<'_>, path: &Path) -> Result<BTreeSet<String>> {
    let env = ctx.system.environ();
    let mut cmd = Cmd::new(argv![&ctx.programs.readelf, "-d", path]);
    cmd.env = Some(&env);
    cmd.output = Output::Capture;
    let finished = ctx.run(&cmd)?;
    if !finished.stderr.is_empty() {
        ctx.warn(String::from_utf8_lossy(&finished.stderr).trim_end_matches('\n'));
    }
    if finished.code != 0 {
        return Err(process::failure(&cmd.argv, finished.code));
    }
    let output = util::decode(finished.stdout)?;
    Ok(NEEDED
        .captures_iter(&output)
        .map(|found| found[1].to_string())
        .collect())
}

/// A host library the build happened to link is absent from Steam's container.
pub fn check_libraries(
    ctx: &Context<'_>,
    original: &Path,
    payload: &Path,
    names: &Map<String, Value>,
) -> Result<()> {
    for name in names.keys() {
        if name.contains("/x86_64-unix/") || name.contains("/bin/") {
            let replaced = needed(ctx, &payload.join(name))?;
            let before = needed(ctx, &original.join(name))?;
            let extra: Vec<&str> = replaced
                .difference(&before)
                .map(String::as_str)
                .filter(|library| !RUNTIME_LIBRARIES.contains(library))
                .collect();
            if !extra.is_empty() {
                return Err(Error::Value(format!(
                    "{name} needs {}, which the runner it replaces does not",
                    extra.join(", ")
                )));
            }
        }
    }
    Ok(())
}

/// Start a Windows process as Steam does: inside the runtime container, which has only its own libraries.
pub fn check_container(
    ctx: &Context<'_>,
    runtime: &Path,
    runner: &Path,
    work: &Path,
) -> Result<()> {
    let mut env = ctx.system.environ();
    env.remove("DISPLAY");
    env.remove("WAYLAND_DISPLAY");
    env.set("WINEPREFIX", work.join("container-prefix"));
    env.set("WINEDEBUG", "-all");
    env.set("WINEDLLOVERRIDES", "winemenubuilder.exe=d;mscoree,mshtml=");
    let launch = fsx::resolve_strict(runtime)?.join("run");
    let started = (|| {
        let (code, output) = ctx.capture_merged(
            argv![
                &launch,
                "--",
                core::wine_binary(runner),
                "cmd.exe",
                "/c",
                "echo",
                "container start OK"
            ]
            .to_vec(),
            &env,
            work,
            300,
        )?;
        let output = util::decode(output)?;
        ctx.print(&output)?;
        if code != 0 || !output.contains("container start OK") {
            return Err(Error::Value(format!(
                "The patched runner does not start inside {}",
                fsx::name(runtime)
            )));
        }
        Ok(())
    })();
    let mut stop = Cmd::new(argv![
        &launch,
        "--",
        runner.join("files/bin/wineserver"),
        "-k"
    ]);
    stop.env = Some(&env);
    stop.cwd = Some(work);
    stop.timeout = Some(60);
    stop.output = ctx.shown();
    finally(ctx, started, ctx.run(&stop).map(|_| ()))
}

/// `Popen.terminate()` and `Popen.wait(timeout=10)`.
fn terminate(child: &mut Child, argv: &[OsString]) -> Result<()> {
    if child.try_wait()?.is_none() {
        let _ = kill_process(Pid::from_child(child), Signal::TERM);
    }
    match process::wait(child, Some(Duration::from_secs(10)), &|| Ok(()))? {
        Some(_) => Ok(()),
        None => Err(Error::Subprocess(format!(
            "Command '{}' timed out after 10 seconds",
            repr_list(argv)
        ))),
    }
}

struct Stage<'a, 'c> {
    ctx: &'a Context<'c>,
    options: &'a Options,
    work: &'a Path,
    runner: &'a Path,
    prefix: &'a Path,
}

impl Stage<'_, '_> {
    /// Wine in the disposable prefix: at most three minutes, and it must succeed.
    fn wine(&self, arguments: &[OsString], output: Output<'_>, env: &Env) -> Result<()> {
        let mut argv = vec![OsString::from(core::wine_binary(self.runner))];
        argv.extend_from_slice(arguments);
        let mut cmd = Cmd::new(argv);
        cmd.env = Some(env);
        cmd.cwd = Some(self.work);
        cmd.timeout = Some(180);
        cmd.output = output;
        self.ctx.run_checked(&cmd).map(|_| ())
    }

    /// A program of the build host, as Python started it without further arguments.
    fn host(&self, argv: Vec<OsString>) -> Result<()> {
        self.ctx.call(argv, &self.ctx.system.environ(), None)
    }

    /// Run a probe with everything it prints going to `name.log`; the log's text.
    fn logged(&self, name: &str, arguments: &[OsString], env: &Env) -> Result<String> {
        let path = self.work.join(format!("{name}.log"));
        let log = File::create(&path).map_err(|error| Error::io_path(&error, &path))?;
        self.wine(arguments, Output::File(&log), env)?;
        drop(log);
        util::read_text(&path)
    }

    fn virtual_fences(&self) -> bool {
        self.options
            .variant
            .as_deref()
            .is_some_and(|variant| matches!(variant, "experimental-11" | "cachyos-11"))
    }

    /// Check timeout, reimport and rewind-history behavior directly through
    /// Wine Vulkan, using the exact headers already included with DXVK.
    fn virtual_fence_probes(&self, env: &Env) -> Result<()> {
        let ctx = self.ctx;
        let lock = ctx.system.manifest(self.options.variant.as_deref())?;
        let source = prepare_dxvk(ctx, &lock, self.work, &ctx.system.environ())?
            .ok_or_else(|| value("Virtual fence probes need the overlay's pinned DXVK headers"))?;
        let name = "vulkan-shared-fence-context";
        let target = self.work.join(format!("{name}.exe"));
        self.host(
            argv![
                &ctx.programs.mingw_gxx,
                "-std=c++17",
                "-O2",
                "-static",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
                source.join("include/vulkan/include"),
                "-o",
                &target,
                ctx.root().join("tests/vulkan-shared-fence-context.cpp")
            ]
            .to_vec(),
        )?;
        for case in ["values", "batch-peak", "reset-history"] {
            let log = format!("{name}-{case}");
            let text = self.logged(&log, &argv![&target, case], env)?;
            ctx.print(&format!("{log}:\n{text}"))?;
        }
        Ok(())
    }

    /// Exercise the actual native DXGI/D3D11 and VKD3D-Proton path. Wine's
    /// built-in D3D libraries do not implement the display sharing contract.
    fn gpu_probes(&self, env: &Env) -> Result<()> {
        let (ctx, work, runner, prefix) = (self.ctx, self.work, self.runner, self.prefix);
        for (folder, names) in [
            ("dxvk", &["dxgi.dll", "d3d11.dll"][..]),
            ("vkd3d-proton", &["d3d12.dll", "d3d12core.dll"][..]),
        ] {
            for name in names {
                let source = runner.join(format!("files/lib/wine/{folder}/x86_64-windows/{name}"));
                let target = prefix.join("drive_c/windows/system32").join(name);
                fsx::unlink(&target)?;
                fsx::copy2(&source, &target)?;
            }
        }
        let gpu_env = env.with(&[
            (
                "WINEDLLOVERRIDES",
                "d3d11,dxgi,d3d12,d3d12core=n;winemenubuilder.exe=d;mscoree,mshtml=",
            ),
            ("DXVK_LOG_LEVEL", "warn"),
            ("VKD3D_DEBUG", "none"),
        ]);
        for name in [
            "dxgi-shared-resource-luid",
            "dxgi-d3d12-display-interop",
            "dxgi-shared-fence-values",
        ] {
            let target = work.join(format!("{name}.exe"));
            self.host(
                argv![
                    &ctx.programs.mingw_gxx,
                    "-std=c++17",
                    "-O2",
                    "-static",
                    "-o",
                    &target,
                    ctx.root().join("tests").join(format!("{name}.cpp")),
                    "-ld3d12",
                    "-ld3d11",
                    "-ldxgi",
                    "-ldxguid"
                ]
                .to_vec(),
            )?;
            let runs = match name {
                "dxgi-shared-fence-values" => {
                    let mut runs = vec![
                        ("dense", argv![&target, "dense"].to_vec()),
                        ("strided", argv![&target, "strided"].to_vec()),
                    ];
                    // The original Xodus backend uses the physical Vulkan
                    // timeline and already cannot rewind even its local fence.
                    // Proton 11 virtualizes values; its shared-context fix must
                    // also preserve decreasing values across processes.
                    if self.virtual_fences() {
                        for case in [
                            "rewind",
                            "initial",
                            "initial-high",
                            "lifetime",
                            "creator-exit",
                            "legacy-reject",
                        ] {
                            runs.push((case, argv![&target, case].to_vec()));
                        }
                    }
                    runs
                }
                "dxgi-d3d12-display-interop" => {
                    vec![("frames", argv![&target, "--frames", "1000"].to_vec())]
                }
                _ => vec![("owner", argv![&target].to_vec())],
            };
            for (case, arguments) in runs {
                let log = format!("{name}-{case}");
                let text = self.logged(&log, &arguments, &gpu_env)?;
                ctx.print(&format!("{log}:\n{text}"))?;
            }
        }
        if self.virtual_fences() {
            self.virtual_fence_probes(&gpu_env)?;
        }
        Ok(())
    }

    fn probes(&self, env: &mut Env) -> Result<()> {
        let (ctx, work, runner, prefix) = (self.ctx, self.work, self.runner, self.prefix);
        let root = ctx.root();
        self.wine(&argv!["wineboot", "-u"], Output::DiscardOutput, env)?;
        // The 32-bit modules are retained from Proton. They must still speak
        // exactly the protocol of the rebuilt 64-bit server (FSDT uses .NET x86).
        self.wine(
            &argv![
                prefix.join("drive_c/windows/syswow64/cmd.exe"),
                "/c",
                "echo 32-bit server ABI OK"
            ],
            ctx.shown(),
            env,
        )?;
        {
            let path = work.join("geometry-setup.log");
            let log = File::create(&path).map_err(|error| Error::io_path(&error, &path))?;
            let wine = RealWine::new(ctx.system, prefix, runner, &log).with_env(env.clone());
            ctx.system.graphics_and_fonts(prefix, runner, &wine)?;
            let cache = self
                .options
                .cache
                .clone()
                .unwrap_or_else(|| root.join("build/downloads"));
            ctx.system
                .prepare_geometry(&wine, &fsx::resolve(&cache)?, root, &|message| {
                    ctx.print(message)
                })?;
        }
        for name in [
            "loader-case-probe",
            "writecopy-probe",
            "iocp-smoke",
            "audio-notification-probe",
            "shared-cursor-probe",
        ] {
            let output = work.join(if name == "loader-case-probe" {
                "Fenix.exe".to_string()
            } else {
                format!("{name}.exe")
            });
            let libraries: &[&str] = match name {
                "audio-notification-probe" => &["-lole32", "-luuid"],
                "shared-cursor-probe" => &["-luser32", "-lgdi32"],
                _ => &[],
            };
            let mut compile = argv![
                &ctx.programs.mingw_gcc,
                "-O2",
                "-o",
                &output,
                root.join("tests").join(format!("{name}.c"))
            ]
            .to_vec();
            compile.extend(libraries.iter().map(OsString::from));
            self.host(compile)?;
            if name == "writecopy-probe" {
                env.set("WINE_TRACK_WRITECOPY", "1");
            }
            let text = self.logged(name, &argv![&output], env)?;
            ctx.print(&format!("{name}:\n{text}"))?;
            if name == "loader-case-probe"
                && !(text.contains("KERNEL32.DLL") && text.contains("KERNELBASE.dll"))
            {
                return Err(value("Loader spelling regression"));
            }
        }
        let probe = work.join("FenixDisplay.exe");
        self.host(
            argv![
                &ctx.programs.mingw_gcc,
                "-O2",
                "-o",
                &probe,
                root.join("tests/window-guard-probe.c"),
                "-luser32"
            ]
            .to_vec(),
        )?;
        let guard_argv = argv![
            core::wine_binary(runner),
            root.join("integration/FenixWindowGuard.exe")
        ];
        let mut guard = {
            let mut cmd = Cmd::new(guard_argv.clone());
            cmd.env = Some(env);
            cmd.output = ctx.shown();
            process::spawn(&cmd)?
        };
        let checked = (|| {
            for (executable, title, renamed, hidden) in WINDOW_GUARD {
                let target = work.join(executable);
                if target != probe {
                    fsx::copy2(&probe, &target)?;
                }
                ctx.print(&format!("X11 scope: {executable} {title}"))?;
                check_host_windows(
                    ctx,
                    &argv![core::wine_binary(runner), &target, "1", title, renamed],
                    env,
                    work,
                    hidden,
                )?;
            }
            // The guard remains the fallback outside the opt-in X11 driver.
            let fallback = env.with(&[("WINE_FENIX_HELPER_WINDOWS", "0")]);
            let mut cmd = Cmd::new(argv![core::wine_binary(runner), &probe, "0"]);
            cmd.env = Some(&fallback);
            cmd.cwd = Some(work);
            cmd.timeout = Some(30);
            cmd.output = ctx.shown();
            ctx.run_checked(&cmd).map(|_| ())
        })();
        finally(ctx, checked, terminate(&mut guard, &guard_argv))?;
        for name in [
            "path-metrics-probe",
            "stroke-joins-probe",
            "d2d-lifetime-probe",
        ] {
            let target = work.join(format!("{name}.exe"));
            self.host(
                argv![
                    &ctx.programs.mingw_gxx,
                    "-O2",
                    "-static",
                    "-o",
                    &target,
                    root.join("tests").join(format!("{name}.cpp")),
                    "-ld2d1",
                    "-ld3d11",
                    "-ldxgi",
                    "-ldxguid",
                    "-lole32"
                ]
                .to_vec(),
            )?;
            let scoped = work.join("FenixDisplay.exe");
            fsx::copy2(&target, &scoped)?;
            let text = self.logged(name, &argv![&scoped], env)?;
            ctx.print(&text)?;
            if name == "path-metrics-probe" {
                self.wine(&argv![&target, "--disabled"], ctx.shown(), env)?;
            }
        }
        let lock = ctx.system.manifest(self.options.variant.as_deref())?;
        if lock.item("dxvk").is_some() {
            self.gpu_probes(env)?;
        }
        Ok(())
    }
}

pub fn run(ctx: &Context<'_>, options: &Options) -> Result<()> {
    let (root, system) = (ctx.root(), ctx.system);
    let original = fsx::resolve_strict(&options.runner)?;
    let variant = options
        .variant
        .as_deref()
        .filter(|variant| !variant.is_empty());
    let lock = system.manifest(variant)?;
    core::verify_runner(system, &original, &lock)?;
    core::verify_bundle(system, root.as_os_str())?;
    let work = fsx::resolve(
        &options
            .work
            .clone()
            .unwrap_or_else(|| root.join("build/smoke")),
    )?;
    if fsx::exists(&work) {
        return Err(value("Use a fresh smoke directory"));
    }
    util::mkdir_new(&work)?;
    let runner = work.join("runner");
    core::copy_tree(system, &original, &runner)?;
    let payload = match variant {
        Some(variant) => root.join("payload/variants").join(variant),
        None => root.join("payload"),
    };
    let files = lock.need_table("files")?;
    check_libraries(ctx, &original, &payload, &files)?;
    for name in files.keys() {
        let target = runner.join(name);
        fsx::unlink(&target)?;
        fsx::copy2(&payload.join(name), &target)?;
    }
    if let Some(runtime) = &options.steam_runtime {
        check_container(ctx, runtime, &runner, &work)?;
    }
    let prefix = work.join("prefix");
    let mut env = core::wine_env(system, &prefix, &runner);
    env.set("WINEDLLOVERRIDES", "winemenubuilder.exe=d;mscoree,mshtml=");
    let stage = Stage {
        ctx,
        options,
        work: &work,
        runner: &runner,
        prefix: &prefix,
    };
    let probed = stage.probes(&mut env);
    let server = runner.join("files/bin/wineserver");
    let stop = |flag: &str, timeout: Option<u64>| {
        let mut cmd = Cmd::new(argv![&server, flag]);
        cmd.env = Some(&env);
        cmd.timeout = timeout;
        cmd.output = ctx.shown();
        ctx.run(&cmd).map(|_| ())
    };
    let stopped = stop("-k", None).and_then(|()| stop("-w", Some(30)));
    finally(ctx, probed, stopped)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use fenix_patch::system::Host;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Shared {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failed_cleanup_ends_the_command_like_a_python_finally() {
        let host = Host::new(std::env::temp_dir());
        let errors = Shared::default();
        let mut ctx = Context::host(&host);
        ctx.err = Mutex::new(Box::new(errors.clone()));
        let shown = || String::from_utf8(std::mem::take(&mut *errors.0.lock().unwrap())).unwrap();
        let (probe, server) = (value("the probe failed"), value("the server did not stop"));

        assert_eq!(finally(&ctx, Ok(()), Ok(())), Ok(()));
        assert_eq!(
            finally(&ctx, Err(probe.clone()), Ok(())),
            Err(probe.clone())
        );
        assert_eq!(
            finally(&ctx, Ok(()), Err(server.clone())),
            Err(server.clone())
        );
        assert_eq!(shown(), "");
        // Python reports both, the clean-up's last; the first is not lost here either.
        assert_eq!(finally(&ctx, Err(probe), Err(server.clone())), Err(server));
        assert_eq!(shown(), "fenix-tools: the probe failed\n");
    }

    /// The machine without the user's display, for the programs a test starts.
    struct Quiet(Host);

    impl fenix_patch::system::System for Quiet {
        fn root(&self) -> &Path {
            self.0.root()
        }

        fn environ(&self) -> Env {
            Env(std::env::vars_os()
                .filter(|(name, _)| name != "DISPLAY" && name != "WAYLAND_DISPLAY")
                .collect())
        }
    }

    #[test]
    #[cfg(target_env = "gnu")]
    fn the_real_readelf_names_the_libraries_of_a_program() {
        if fsx::which("readelf", std::env::var_os("PATH")).is_none() {
            eprintln!("readelf was not found; its real output is not read");
            return;
        }
        let quiet = Quiet(Host::new(std::env::temp_dir()));
        let errors = Shared::default();
        let mut ctx = Context::host(&quiet);
        ctx.err = Mutex::new(Box::new(errors.clone()));
        // This test program itself: linked against the C library like Wine's modules.
        let libraries = needed(&ctx, &std::env::current_exe().unwrap()).unwrap();
        assert!(libraries.contains("libc.so.6"), "{libraries:?}");
        assert!(libraries.iter().all(|name| !name.contains(['[', ']', ' '])));
        let missing = needed(&ctx, Path::new("/nonexistent/module.so")).unwrap_err();
        assert!(
            missing
                .to_string()
                .ends_with("returned non-zero exit status 1."),
            "{missing}"
        );
        // What readelf itself said about it is passed on.
        let shown = String::from_utf8(errors.0.lock().unwrap().clone()).unwrap();
        assert!(shown.contains("/nonexistent/module.so"), "{shown}");
    }

    #[test]
    fn a_started_program_is_asked_to_end_and_waited_for() {
        // No display of the user for anything a test starts.
        let env = Env(std::env::vars_os()
            .filter(|(name, _)| name != "DISPLAY" && name != "WAYLAND_DISPLAY")
            .collect());
        let start = |argv: &[OsString]| {
            let mut cmd = Cmd::new(argv.to_vec());
            cmd.env = Some(&env);
            process::spawn(&cmd).unwrap()
        };
        let argv = argv!["sh", "-c", "exit 0"];
        let mut done = start(&argv);
        terminate(&mut done, &argv).unwrap();
        // Already gone: nothing is signalled, nothing fails.
        terminate(&mut done, &argv).unwrap();
        let argv = argv!["sleep", "30"];
        let mut running = start(&argv);
        let started = std::time::Instant::now();
        terminate(&mut running, &argv).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(running.wait().unwrap().code(), None, "ended by the signal");
    }
}
