// SPDX-License-Identifier: MIT
//! `smoke` from the copy of the runner to the last `wineserver`, with
//! stand-ins for Wine, the compilers and the Steam runtime, and a display
//! that shows nothing.
#![allow(clippy::unwrap_used)]
mod common;

use common::{
    Display, Lab, Outcome, Run, Tree, VARIANT, World, calls, digest, put, read, runner,
    steam_runtime, windows,
};
use fenix_tools::x11;
use std::{
    net::TcpListener,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture {
    lab: Lab,
    tree: Tree,
    world: World,
    display: Display,
    runner: PathBuf,
    variant_runner: PathBuf,
    runtime: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let lab = Lab::new();
        let tree = Tree::create(&lab.base.join("repo"));
        let (base, variant) = (lab.base.join("runner"), lab.base.join("runner-x"));
        runner(&base, "1700000000 synthetic-runner-1.0");
        runner(&variant, "1800000000 synthetic-proton-x");
        tree.seal(Some((&base, &variant)));
        let runtime = lab.base.join("SteamLinuxRuntime_sniper");
        steam_runtime(&runtime);
        let display = Display::start(windows(&lab.base));
        let world = World::new(&tree.root, lab.env(&lab.base));
        world.set("DISPLAY", &display.name);
        Self {
            lab,
            tree,
            world,
            display,
            runner: base,
            variant_runner: variant,
            runtime,
        }
    }

    fn tools(&self, argv: &[&str]) -> Outcome {
        Run {
            world: &self.world,
            programs: self.lab.programs(),
            fetch: None,
            display: Some(&x11::open),
            output: self.lab.base.join("output.txt"),
        }
        .tools(argv)
    }

    /// `smoke` on the base runner in the new directory `<base>/{work}`.
    fn smoke(&self, work: &str, more: &[&str]) -> Outcome {
        let work = self.lab.base.join(work);
        let mut argv = vec![
            "smoke",
            "--runner",
            self.runner.to_str().unwrap(),
            "--work",
            work.to_str().unwrap(),
        ];
        argv.extend_from_slice(more);
        self.tools(&argv)
    }

    /// The calls of the stand-ins, with this test's directory as `<LAB>`,
    /// the directory the tests run in as `<CWD>` and the display as `<X>`.
    fn calls(&self) -> Vec<String> {
        let cwd = std::env::current_dir().unwrap();
        let mut calls: Vec<String> = calls(&self.lab.base, &self.lab.base)
            .into_iter()
            .map(|line| {
                line.replace("<ROOT>", "<LAB>")
                    .replace(&format!("cwd={}", cwd.display()), "cwd=<CWD>")
                    .replace(&format!("DISPLAY={}", self.display.name), "DISPLAY=<X>")
            })
            .collect();
        // The guard runs beside the first probe, so either may record its
        // call first. It is listed where the command starts it.
        if let Some(at) = calls
            .iter()
            .position(|call| call.contains("/FenixWindowGuard.exe] "))
            && let Some(compiled) = calls
                .iter()
                .position(|call| call.contains("/window-guard-probe.c] "))
        {
            let guard = calls.remove(at);
            calls.insert(compiled + 1, guard);
        }
        calls
    }
}

const OVERRIDES: &str = "WINEDLLOVERRIDES=winemenubuilder.exe=d;mscoree,mshtml=";
const TRACKED: &str = "WINE_TRACK_WRITECOPY=apps:Fenix.exe,FenixSystem.exe,FenixDisplay.exe,FenixCDU.exe,FlightSimulator2024.exe";
const FONTS: &str = r"[HKLM\Software\Microsoft\Windows NT\CurrentVersion\Fonts]";

/// What the stand-ins record for a whole run in `<LAB>/{work}`, as the
/// Python script started them. `container` adds the start inside the runtime.
fn expected_calls(work: &str, payload: &str, original: &str, container: bool) -> Vec<String> {
    let mut expected = Vec::new();
    for name in [
        "files/lib/wine/x86_64-unix/ntdll.so",
        "files/bin/wineserver",
        "files/lib/wine/x86_64-unix/bcrypt.so",
        "files/lib/wine/x86_64-unix/dwrite.so",
        "files/lib/wine/x86_64-unix/win32u.so",
        "files/lib/wine/x86_64-unix/winex11.so",
    ] {
        for base in [payload, original] {
            expected.push(format!(
                "readelf [-d] [<LAB>/{base}/{name}] cwd=<CWD> DISPLAY=<X>"
            ));
        }
    }
    let wine = format!("<LAB>/{work}/runner/files/bin/wine");
    if container {
        let tail = format!("cwd=<LAB>/{work} WINEPREFIX=<LAB>/{work}/container-prefix {OVERRIDES}");
        expected.extend([
            format!("run [--] [{wine}] [cmd.exe] [/c] [echo] [container start OK] {tail}"),
            format!("wine [cmd.exe] [/c] [echo] [container start OK] {tail}"),
            format!("run [--] [<LAB>/{work}/runner/files/bin/wineserver] [-k] {tail}"),
            format!("wineserver [-k] {tail}"),
        ]);
    }
    let prefix = format!("WINEPREFIX=<LAB>/{work}/prefix {OVERRIDES}");
    let early = format!("{prefix} {TRACKED} WINE_FENIX_HELPER_WINDOWS=1 DISPLAY=<X>");
    let late = format!("{prefix} WINE_TRACK_WRITECOPY=1 WINE_FENIX_HELPER_WINDOWS=1 DISPLAY=<X>");
    let at = format!("<LAB>/{work}");
    expected.extend([
        format!("wine [wineboot] [-u] cwd={at} {early}"),
        format!(
            "wine [{at}/prefix/drive_c/windows/syswow64/cmd.exe] [/c] [echo 32-bit server ABI OK] cwd={at} {early}"
        ),
    ]);
    for (family, file) in [
        ("Arial", "arial.ttf"),
        ("Arial Bold", "arialbd.ttf"),
        ("Courier New", "cour.ttf"),
        ("Courier New Bold", "courbd.ttf"),
        ("Georgia", "georgia.ttf"),
        ("Times New Roman", "times.ttf"),
        ("Microsoft Sans Serif", "micross.ttf"),
    ] {
        expected.push(format!(
            "wine [reg] [add] {FONTS} [/v] [{family} (TrueType)] [/t] [REG_SZ] [/d] [{file}] [/f] cwd={at}/prefix {early}"
        ));
    }
    expected.extend([
        // The stand-in for wineboot registers Tahoma, so only the bold face is missing.
        format!(
            "wine [reg] [add] {FONTS} [/v] [Tahoma Bold (TrueType)] [/t] [REG_SZ] [/d] [tahomabd.ttf] [/f] [/reg:64] cwd={at}/prefix {early}"
        ),
        format!(
            r"wine [reg] [add] [HKCU\Software\Microsoft\Avalon.Graphics] [/v] [DisableHWAcceleration] [/t] [REG_DWORD] [/d] [1] [/f] cwd={at}/prefix {early}"
        ),
        format!(
            r"wine [reg] [add] [HKCU\Software\Wine\Explorer] [/v] [ShowSystray] [/t] [REG_DWORD] [/d] [0] [/f] cwd={at}/prefix {early}"
        ),
    ]);
    let gcc = "x86_64-w64-mingw32-gcc [-O2] [-o]";
    for (source, program, libraries, env) in [
        ("loader-case-probe", "Fenix", "", &early),
        ("writecopy-probe", "writecopy-probe", "", &late),
        ("iocp-smoke", "iocp-smoke", "", &late),
        (
            "audio-notification-probe",
            "audio-notification-probe",
            " [-lole32] [-luuid]",
            &late,
        ),
        (
            "shared-cursor-probe",
            "shared-cursor-probe",
            " [-luser32] [-lgdi32]",
            &late,
        ),
        (
            "applicationdata-unpackaged-probe",
            "applicationdata-unpackaged-probe",
            " [-lruntimeobject] [-lole32]",
            &late,
        ),
        (
            "volume-roundtrip-probe",
            "volume-roundtrip-probe",
            "",
            &late,
        ),
        (
            "volume-separators-probe",
            "volume-separators-probe",
            "",
            &late,
        ),
        ("pbkdf2-probe", "pbkdf2-probe", " [-lbcrypt]", &late),
        (
            "ncrypt-ecdh-probe",
            "ncrypt-ecdh-probe",
            " [-lncrypt]",
            &late,
        ),
        (
            "sip-pe-probe",
            "sip-pe-probe",
            " [-lcrypt32] [-ladvapi32]",
            &late,
        ),
        ("cwd-contract-probe", "cwd-contract-probe", "", &late),
    ] {
        let libraries = if source == "cwd-contract-probe" {
            format!(" [-municode] [-DCWD_PROBE_PREFIX=L\"{at}/prefix\"]")
        } else {
            libraries.to_string()
        };
        expected.extend([
            format!(
                "{gcc} [{at}/{program}.exe] [<LAB>/repo/tests/{source}.c]{libraries} cwd=<CWD> DISPLAY=<X>"
            ),
            format!("wine [{at}/{program}.exe] cwd={at} {env}"),
        ]);
        if source == "cwd-contract-probe" {
            expected.push(format!(
                "wine [{at}/{program}.exe] [--negative] cwd={at} {env}"
            ));
        }
    }
    expected.extend([
        format!(
            "{gcc} [{at}/FenixDisplay.exe] [<LAB>/repo/tests/window-guard-probe.c] [-luser32] cwd=<CWD> DISPLAY=<X>"
        ),
        format!("wine [<LAB>/repo/integration/FenixWindowGuard.exe] cwd=<CWD> {late}"),
    ]);
    for (executable, title, renamed) in [
        ("FenixDisplay.exe", "ProSimA322 Display", "Fenix Display"),
        (
            "FenixSystem.exe",
            "ProSimA322 System",
            "FenixSim A320 System 2.4.0.4720",
        ),
        ("FenixCDU.exe", "ProSimA322 MCDU", "ProSimA322 MCDU"),
        (
            "UnrelatedApplication.exe",
            "ProSimA322 Display",
            "Fenix Display",
        ),
        ("Fenix.exe", "ProSimA322 Display", "Fenix Display"),
        ("FenixDisplay.exe", "Fenix Account", "Fenix Account"),
        (
            "FenixSystem.exe",
            "FenixSim A320 System Login",
            "FenixSim A320 System 2.",
        ),
        ("FenixDisplay.exe", "Fenix Settings", "ProSimA322 Display"),
    ] {
        expected.push(format!(
            "wine [{at}/{executable}] [1] [{title}] [{renamed}] cwd={at} {late}"
        ));
    }
    expected.push(format!(
        "wine [{at}/FenixDisplay.exe] [0] cwd={at} {prefix} WINE_TRACK_WRITECOPY=1 WINE_FENIX_HELPER_WINDOWS=0 DISPLAY=<X>"
    ));
    for name in [
        "path-metrics-probe",
        "stroke-joins-probe",
        "d2d-lifetime-probe",
    ] {
        expected.extend([
            format!(
                "x86_64-w64-mingw32-g++ [-O2] [-static] [-o] [{at}/{name}.exe] [<LAB>/repo/tests/{name}.cpp] \
                 [-ld2d1] [-ld3d11] [-ldxgi] [-ldxguid] [-lole32] cwd=<CWD> DISPLAY=<X>"
            ),
            format!("wine [{at}/FenixDisplay.exe] cwd={at} {late}"),
        ]);
        if name == "path-metrics-probe" {
            expected.push(format!(
                "wine [{at}/path-metrics-probe.exe] [--disabled] cwd={at} {late}"
            ));
        }
    }
    expected.extend(stop_calls(work, "WINE_TRACK_WRITECOPY=1"));
    expected
}

/// The two calls that end every run once Wine may have been started.
fn stop_calls(work: &str, tracking: &str) -> Vec<String> {
    ["-k", "-w"]
        .map(|flag| {
            format!(
                "wineserver [{flag}] cwd=<CWD> WINEPREFIX=<LAB>/{work}/prefix {OVERRIDES} {tracking} \
                 WINE_FENIX_HELPER_WINDOWS=1 DISPLAY=<X>"
            )
        })
        .to_vec()
}

/// What the probes of the window table print, as the display confirmed them.
fn window_lines() -> String {
    let mut lines = String::new();
    for (index, (executable, title, renamed, hidden)) in
        fenix_tools::smoke::WINDOW_GUARD.iter().enumerate()
    {
        let window = 0x40_0000 + 2 * index;
        lines += &format!(
            "X11 scope: {executable} {title}\n\
             probe {executable} as \"{title}\", renamed \"{renamed}\"\n\
             host xid={window} helper={} logical_visible=1\n\
             fixme:something on standard error\n\
             host xid={} helper={} logical_visible=1\n\
             window_visible=1\n",
            u8::from(hidden.0),
            window + 1,
            u8::from(hidden.1)
        );
    }
    lines
}

fn probe_lines() -> String {
    let mut lines =
        "loader-case-probe:\nloaded KERNEL32.DLL\nloaded KERNELBASE.dll\n\n".to_string();
    for name in [
        "writecopy-probe",
        "iocp-smoke",
        "audio-notification-probe",
        "shared-cursor-probe",
        "applicationdata-unpackaged-probe",
        "volume-roundtrip-probe",
        "volume-separators-probe",
        "pbkdf2-probe",
        "ncrypt-ecdh-probe",
        "sip-pe-probe",
        "cwd-contract-probe",
    ] {
        lines += &format!("{name}:\n{name}.exe says hello\n{name}.exe notes something\n\n");
        if name == "cwd-contract-probe" {
            lines += "cwd-contract-negative:\ncwd-contract-probe.exe says hello\ncwd-contract-probe.exe notes something\n\n";
        }
    }
    lines
}

fn failure(text: &str) -> String {
    format!("fenix-tools: {text}\n")
}

#[test]
fn a_whole_smoke_test_runs_every_probe_and_stops_wine() {
    let fixture = Fixture::new();
    let base = &fixture.lab.base;
    let started = Instant::now();
    let outcome = fixture.smoke(
        "work",
        &[
            "--steam-runtime",
            fixture.runtime.to_str().unwrap(),
            "--cache",
            base.join("cache").to_str().unwrap(),
        ],
    );
    let scoped = "FenixDisplay.exe says hello\nFenixDisplay.exe notes something\n\n";
    assert_eq!(
        outcome.output,
        format!(
            "container start OK\n\n32-bit server ABI OK\r\n\
             geometry stand-in: {} {}\n{}{}{scoped}{scoped}{scoped}",
            base.join("cache").display(),
            fixture.tree.root.display(),
            probe_lines(),
            window_lines(),
        )
    );
    assert_eq!(outcome.code, 0);
    assert_eq!(
        fixture.calls(),
        expected_calls("work", "repo/payload", "runner", true)
    );
    // The display was asked about every reported window, and only those.
    assert_eq!(
        *fixture.display.asked.lock().unwrap(),
        (0x40_0000..0x40_0010).collect::<Vec<u32>>()
    );
    assert_eq!(
        fixture
            .world
            .geometry
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    // The guard was stopped at once, not waited for.
    assert!(started.elapsed() < Duration::from_secs(60));

    let work = base.join("work");
    assert!(work.join("prefix/drive_d").is_dir());
    assert!(work.join("prefix/drive_c").is_dir());
    for relative in ["drive_c", "drive_d", "dosdevices/unc/cwd-own-test/share"] {
        assert_eq!(
            read(
                &work
                    .join("prefix")
                    .join(relative)
                    .join("probe/own-control.txt")
            ),
            "synthetic cwd control\n"
        );
    }
    assert_eq!(
        digest(&work.join("prefix/drive_c/probe/cwd-contract-probe.exe")),
        digest(&work.join("cwd-contract-probe.exe"))
    );
    for (drive, target) in [("c:", "../drive_c"), ("d:", "../drive_d"), ("z:", "/")] {
        assert_eq!(
            std::fs::read_link(work.join("prefix/dosdevices").join(drive)).unwrap(),
            PathBuf::from(target)
        );
    }
    let lock = fixture.tree.lock();
    for (name, expected) in lock["files"].as_object().unwrap() {
        // The copy has the overlay, the runner it was copied from has not.
        assert_eq!(digest(&work.join("runner").join(name)), *expected, "{name}");
        assert_ne!(digest(&fixture.runner.join(name)), *expected, "{name}");
    }
    assert_eq!(
        digest(&work.join("runner/files/bin/wine")),
        lock["runner_files"]["files/bin/wine"]
    );
    for (name, content) in [
        ("geometry-setup.log", ""),
        (
            "loader-case-probe.log",
            "loaded KERNEL32.DLL\r\nloaded KERNELBASE.dll\r\n",
        ),
        (
            "iocp-smoke.log",
            "iocp-smoke.exe says hello\r\niocp-smoke.exe notes something\n",
        ),
        (
            "stroke-joins-probe.log",
            "FenixDisplay.exe says hello\r\nFenixDisplay.exe notes something\n",
        ),
        ("FenixSystem.exe", "compiled window-guard-probe.c\n"),
        (
            "UnrelatedApplication.exe",
            "compiled window-guard-probe.c\n",
        ),
        // The last probe that ran under this name.
        ("FenixDisplay.exe", "compiled d2d-lifetime-probe.cpp\n"),
        ("Fenix.exe", "compiled window-guard-probe.c\n"),
        ("prefix/drive_c/windows/Fonts/georgia.ttf", "georgia.ttf"),
        ("prefix/drive_c/windows/Fonts/tahomabd.ttf", "tahomabd.ttf"),
        (
            "prefix/drive_c/windows/syswow64/libvkd3d-shader-1.dll",
            "i386-windows libvkd3d-shader-1.dll\n",
        ),
    ] {
        assert_eq!(read(&work.join(name)), content, "{name}");
    }
    assert!(
        !work
            .join("prefix/drive_c/windows/Fonts/tahoma.ttf")
            .exists()
    );

    // What is there is never reused.
    let outcome = fixture.smoke("work", &[]);
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("Use a fresh smoke directory"))
    );
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_proton_overlay_is_tested_with_its_own_runner_and_payload() {
    let fixture = Fixture::new();
    let outcome = fixture.tools(&[
        "smoke",
        "--runner",
        fixture.variant_runner.to_str().unwrap(),
        "--variant",
        VARIANT,
        "--work",
        fixture.lab.base.join("proton").to_str().unwrap(),
    ]);
    assert_eq!(outcome.code, 0, "{}", outcome.output);
    assert!(
        outcome.output.starts_with(&format!(
            "32-bit server ABI OK\r\ngeometry stand-in: {} {}\n",
            fixture.tree.root.join("build/downloads").display(),
            fixture.tree.root.display()
        )),
        "{}",
        outcome.output
    );
    assert_eq!(
        fixture.calls(),
        expected_calls(
            "proton",
            &format!("repo/payload/variants/{VARIANT}"),
            "runner-x",
            false
        )
    );
    let lock = fixture.tree.lock();
    for (name, expected) in lock["variants"][VARIANT]["files"].as_object().unwrap() {
        let copied = fixture.lab.base.join("proton/runner").join(name);
        assert_eq!(digest(&copied), *expected, "{name}");
    }

    // Each runner only with the build that was made for it.
    for (runner, variant) in [
        (&fixture.runner, Some(VARIANT)),
        (&fixture.variant_runner, None),
    ] {
        let mut argv = vec!["smoke", "--runner", runner.to_str().unwrap()];
        if let Some(variant) = variant {
            argv.extend(["--variant", variant]);
        }
        let outcome = fixture.tools(&argv);
        assert_eq!(
            (outcome.code, outcome.output),
            (
                1,
                failure("The Wine runner version does not match this Fenix build.")
            )
        );
    }
    let outcome = fixture.smoke("unknown", &["--variant", "no-such-proton"]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure("No Fenix compatibility build is available for this Proton version.")
        )
    );
    assert!(fixture.calls().is_empty());
    assert!(!fixture.tree.root.join("build").exists());
}

#[test]
fn runner_and_bundle_are_verified_before_anything_is_copied() {
    let fixture = Fixture::new();
    let base = &fixture.lab.base;
    let outcome = fixture.tools(&["smoke"]);
    assert_eq!(outcome.code, 2, "{}", outcome.output);
    assert!(outcome.output.contains("--runner"), "{}", outcome.output);
    let outcome = fixture.tools(&["smoke", "--runner", base.join("absent").to_str().unwrap()]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure(&format!(
                "[Errno 2] No such file or directory: '{}'",
                base.join("absent").display()
            ))
        )
    );

    let kernel32 = fixture
        .runner
        .join("files/lib/wine/x86_64-windows/kernel32.dll");
    put(&kernel32, "MZ another build\n");
    let outcome = fixture.smoke("changed-runner", &[]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure(
                "The Wine runner differs from the supported build: files/lib/wine/x86_64-windows/kernel32.dll"
            )
        )
    );
    put(&kernel32, "MZ kernel32\n");

    let module = fixture
        .tree
        .root
        .join("payload/files/lib/wine/x86_64-windows/d2d1.dll");
    let original = read(&module);
    put(&module, "not what bundle.json pins\n");
    let outcome = fixture.smoke("changed-payload", &[]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure("Patch checksum mismatch: files/lib/wine/x86_64-windows/d2d1.dll")
        )
    );
    put(&module, original);
    for work in ["changed-runner", "changed-payload"] {
        assert!(!base.join(work).exists(), "{work}");
    }
    assert!(fixture.calls().is_empty());
}

#[test]
fn a_library_the_runner_did_not_need_is_refused() {
    let fixture = Fixture::new();
    let payload = fixture.tree.root.join("payload");
    let names = [
        "files/lib/wine/x86_64-unix/win32u.so",
        "files/lib/wine/x86_64-unix/winex11.so",
        "files/lib/wine/x86_64-windows/ntdll.dll",
    ];
    // libgcc is in Steam's containers; a Windows module links no host library at all.
    put(
        &payload.join(names[0]),
        "NEEDED libc.so.6\nNEEDED libgcc_s.so.1\n",
    );
    put(
        &payload.join(names[1]),
        "NEEDED libxkbregistry.so.0\nNEEDED libc.so.6\nNEEDED libgcc_s.so.1\nNEEDED libX11.so.6\n",
    );
    put(&payload.join(names[2]), "NEEDED libunwind.so.8\n");
    fixture
        .tree
        .seal(Some((&fixture.runner, &fixture.variant_runner)));
    let outcome = fixture.smoke("libraries", &[]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure(
                "files/lib/wine/x86_64-unix/winex11.so needs libX11.so.6, libxkbregistry.so.0, \
                 which the runner it replaces does not"
            )
        )
    );
    let calls = fixture.calls();
    assert_eq!(calls.len(), 12, "{calls:#?}");
    assert!(calls.iter().all(|call| call.starts_with("readelf [-d] ")));
    // The copy exists, but nothing in it was replaced or started.
    let copied = fixture.lab.base.join("libraries/runner").join(names[1]);
    assert_eq!(digest(&copied), digest(&fixture.runner.join(names[1])));
}

#[test]
fn a_runner_that_does_not_start_in_the_container_ends_the_test() {
    let fixture = Fixture::new();
    fixture.world.set("FAKE_CONTAINER_FAILS", "1");
    let runtime = fixture.runtime.to_str().unwrap();
    let outcome = fixture.smoke("container", &["--steam-runtime", runtime]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            format!(
                "wine: could not load kernel32.dll\n\n{}",
                failure("The patched runner does not start inside SteamLinuxRuntime_sniper")
            )
        )
    );
    let calls = fixture.calls();
    let expected = expected_calls("container", "repo/payload", "runner", true);
    // The server in the container is stopped; no profile was made outside it.
    assert_eq!(calls, expected[..16].to_vec());
    assert!(calls[15].starts_with(
        "wineserver [-k] cwd=<LAB>/container WINEPREFIX=<LAB>/container/container-prefix "
    ));
    assert!(!fixture.lab.base.join("container/prefix").exists());

    let outcome = fixture.smoke(
        "no-runtime",
        &[
            "--steam-runtime",
            fixture.lab.base.join("absent").to_str().unwrap(),
        ],
    );
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure(&format!(
                "[Errno 2] No such file or directory: '{}'",
                fixture.lab.base.join("absent").display()
            ))
        )
    );
}

#[test]
fn wine_is_stopped_whatever_a_probe_does() {
    let fixture = Fixture::new();
    let early = format!("{TRACKED} WINE_FENIX_HELPER_WINDOWS=1 DISPLAY=<X>");
    let stopped = |work: &str, tracking: &str| {
        let calls = fixture.calls();
        assert_eq!(
            calls[calls.len() - 2..],
            stop_calls(work, tracking),
            "{calls:#?}"
        );
        calls
    };

    fixture.world.set("FAKE_LOADER_REGRESSION", "1");
    let outcome = fixture.smoke("loader", &[]);
    assert_eq!(outcome.code, 1);
    assert!(
        outcome.output.ends_with(&format!(
            "loader-case-probe:\nloaded KERNEL32.DLL\n\n{}",
            failure("Loader spelling regression")
        )),
        "{}",
        outcome.output
    );
    let calls = stopped("loader", TRACKED);
    assert_eq!(
        calls[calls.len() - 3],
        format!(
            "wine [<LAB>/loader/Fenix.exe] cwd=<LAB>/loader WINEPREFIX=<LAB>/loader/prefix {OVERRIDES} {early}"
        )
    );
    fixture.world.set("FAKE_LOADER_REGRESSION", "");

    // A probe that fails ends the test; its log is kept but not shown.
    fixture.world.set("FAKE_FAILING_PROBE", "iocp-smoke.exe");
    let outcome = fixture.smoke("failing", &[]);
    let work = fixture.lab.base.join("failing");
    assert_eq!(outcome.code, 1);
    assert!(
        outcome.output.ends_with(&format!(
            "writecopy-probe.exe notes something\n\n{}",
            failure(&format!(
                "Command '['{}', '{}']' returned non-zero exit status 3.",
                work.join("runner/files/bin/wine").display(),
                work.join("iocp-smoke.exe").display()
            ))
        )),
        "{}",
        outcome.output
    );
    assert_eq!(
        read(&work.join("iocp-smoke.log")),
        "iocp-smoke.exe says hello\r\niocp-smoke.exe notes something\n"
    );
    stopped("failing", "WINE_TRACK_WRITECOPY=1");
    fixture.world.set("FAKE_FAILING_PROBE", "");

    // Ctrl+C ends the probe and the test, but not before Wine is stopped.
    fixture
        .world
        .set("FAKE_INTERRUPTED_PROBE", "shared-cursor-probe.exe");
    let started = Instant::now();
    let outcome = fixture.smoke("interrupted", &[]);
    assert_eq!(outcome.code, 130, "{}", outcome.output);
    assert!(
        outcome
            .output
            .ends_with("audio-notification-probe.exe notes something\n\n"),
        "{}",
        outcome.output
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    let calls = stopped("interrupted", "WINE_TRACK_WRITECOPY=1");
    assert!(
        calls[calls.len() - 3].starts_with("wine [<LAB>/interrupted/shared-cursor-probe.exe] ")
    );
    fixture.world.set("FAKE_INTERRUPTED_PROBE", "");

    // The exit status of wineserver itself does not matter.
    fixture.world.set("FAKE_WINESERVER_STATUS", "1");
    let outcome = fixture.smoke("status", &[]);
    assert_eq!(outcome.code, 0, "{}", outcome.output);
    stopped("status", "WINE_TRACK_WRITECOPY=1");
}

#[test]
fn windows_are_checked_against_the_display() {
    let fixture = Fixture::new();
    let scope = |outcome: &Outcome| {
        let from = outcome.output.find("X11 scope:").unwrap();
        outcome.output[from..].to_string()
    };
    let stopped = |work: &str| {
        let calls = fixture.calls();
        assert_eq!(
            calls[calls.len() - 2..],
            stop_calls(work, "WINE_TRACK_WRITECOPY=1"),
            "{calls:#?}"
        );
        calls
    };
    let first = "X11 scope: FenixDisplay.exe ProSimA322 Display\n";
    let report = "probe FenixDisplay.exe as \"ProSimA322 Display\", renamed \"Fenix Display\"\n\
                  host xid=4194304 helper=1 logical_visible=1\n";

    // A helper window that X11 shows after all.
    fixture.display.windows.lock().unwrap().insert(0x40_0000, 2);
    let outcome = fixture.smoke("mapped", &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        scope(&outcome),
        format!(
            "{first}{report}{}",
            failure("Unexpected X11 state: helper=1, Win32=1, mapped=2")
        )
    );
    let calls = stopped("mapped");
    // One probe was started, and nothing after it.
    assert!(calls[calls.len() - 3].starts_with("wine [<LAB>/mapped/FenixDisplay.exe] [1] "));

    // An ordinary window that X11 does not show: the second report of the first case that expects one.
    fixture.display.windows.lock().unwrap().insert(0x40_0000, 0);
    fixture.display.windows.lock().unwrap().insert(0x40_0007, 0);
    let outcome = fixture.smoke("unmapped", &[]);
    assert_eq!(outcome.code, 1);
    assert!(
        outcome.output.ends_with(&format!(
            "host xid=4194311 helper=0 logical_visible=1\n{}",
            failure("Unexpected X11 state: helper=0, Win32=1, mapped=0")
        )),
        "{}",
        outcome.output
    );
    stopped("unmapped");
    fixture.display.windows.lock().unwrap().insert(0x40_0007, 2);

    // A window the display does not know.
    fixture.display.windows.lock().unwrap().remove(&0x40_0001);
    let outcome = fixture.smoke("gone", &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        scope(&outcome),
        format!(
            "{first}{report}fixme:something on standard error\n\
             host xid=4194305 helper=1 logical_visible=1\n{}",
            failure("Probe did not expose a live X11 window")
        )
    );
    stopped("gone");
    fixture.display.windows.lock().unwrap().insert(0x40_0001, 0);

    // A probe that reports everything and then fails.
    fixture.world.set("FAKE_PROBE_STATUS", "4");
    let outcome = fixture.smoke("status", &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        scope(&outcome),
        format!(
            "{first}{report}fixme:something on standard error\n\
             host xid=4194305 helper=1 logical_visible=1\nwindow_visible=1\n{}",
            failure("Incomplete window mapping probe")
        )
    );
    stopped("status");
    fixture.world.set("FAKE_PROBE_STATUS", "0");

    // A probe that reports one window only.
    let table = fixture.lab.base.join("windows.txt");
    let lines = read(&table);
    put(
        &table,
        lines.replacen("|4194305 1\n", "|helper-less report\n", 1),
    );
    let outcome = fixture.smoke("once", &[]);
    assert_eq!(outcome.code, 1);
    assert!(
        outcome.output.ends_with(&format!(
            "window_visible=1\n{}",
            failure("Incomplete window mapping probe")
        )),
        "{}",
        outcome.output
    );
    stopped("once");
    put(&table, lines);

    // No display at all, and one that nobody listens on.
    fixture.world.set("DISPLAY", "");
    let outcome = fixture.smoke("unset", &[]);
    assert_eq!(outcome.code, 1);
    assert_eq!(
        scope(&outcome),
        format!(
            "{first}{}",
            failure("X11 window checks require DISPLAY (Xwayland or Xvfb is sufficient)")
        )
    );
    // The guard may be stopped before it has recorded anything; Wine is stopped all the same.
    let calls = fixture.calls();
    for (index, flag) in [(2, "-k"), (1, "-w")] {
        let call = &calls[calls.len() - index];
        assert!(
            call.starts_with(&format!(
                "wineserver [{flag}] cwd=<CWD> WINEPREFIX=<LAB>/unset/prefix "
            )),
            "{call}"
        );
    }
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    for (work, name) in [
        ("closed", format!("127.0.0.1:{}", closed - 6000)),
        ("malformed", "no display name".to_string()),
    ] {
        fixture.world.set("DISPLAY", &name);
        let outcome = fixture.smoke(work, &[]);
        assert_eq!(outcome.code, 1);
        assert_eq!(
            scope(&outcome),
            format!("{first}{}", failure("Cannot open test X11 display")),
            "{name}"
        );
    }
}

#[test]
fn the_display_answers_for_live_windows_only() {
    let display = Display::start([(7, 0), (8, 1), (9, 2)].into());
    let mut windows = x11::open(&display.name).unwrap();
    assert_eq!(windows.map_state(9).unwrap(), Some(2));
    assert_eq!(windows.map_state(8).unwrap(), Some(1));
    assert_eq!(windows.map_state(7).unwrap(), Some(0));
    assert_eq!(windows.map_state(10).unwrap(), None);
    // The connection survives the error and keeps its place in the sequence.
    assert_eq!(windows.map_state(9).unwrap(), Some(2));
    assert_eq!(*display.asked.lock().unwrap(), [9, 8, 7, 10, 9]);
}
