// SPDX-License-Identifier: MIT
//! The Flightdeck installation, test for test as `tests/test_installer.py`.
#![allow(clippy::unwrap_used)]
mod common;

use common::{
    Manifest, Messages, Mock, MockWine, WineCalls, assert_patch_error, digest, digests, glob,
    gpu_overlay, put, read,
};
use fenix_patch::{
    Error,
    core::{self, Wine},
    fsx,
    json::{Value, json},
    process::Env,
    system::{System, silent},
    xml,
};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Arc, atomic::Ordering},
};

const FILES: [&str; 2] = [
    "files/lib/wine/x86_64-windows/ntdll.dll",
    "files/bin/wineserver",
];
const SCRIPTS: [&str; 2] = ["launch-msfs.sh", "xodus-wine-launch"];

struct Fixture {
    _serial: std::sync::MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    prefix: PathBuf,
    bundle: PathBuf,
    runner: PathBuf,
    mock: Mock,
}

impl Fixture {
    fn new() -> Self {
        let serial = common::serial();
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("runtime with spaces");
        let prefix = root.join("local/msfs-prefix");
        let bundle = base.join("bundle");
        let runner = base.join("original runner");
        for path in [
            root.join("private"),
            root.join("tools"),
            prefix.join("drive_c/windows/system32"),
            prefix.join("dosdevices"),
            runner.join("files/bin"),
            bundle.join("integration"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        put(
            &root.join("private/runtime.json"),
            &json!({"game_id": "msfs2024"}).to_string(),
        );
        put(&prefix.join("system.reg"), "original registry");
        put(&prefix.join("user-private.txt"), "synthetic private state");
        symlink(prefix.join("drive_c"), prefix.join("dosdevices/c:")).unwrap();
        symlink(&runner, root.join("runner")).unwrap();
        put(&runner.join("version"), "fixture version");
        put(&runner.join("files/bin/wine"), "fixture Wine");
        put(
            &runner.join("files/share/wine/fonts/tahoma.ttf"),
            "fixture Tahoma",
        );
        put(
            &runner.join("files/share/wine/fonts/tahomabd.ttf"),
            "fixture Tahoma Bold",
        );
        for name in FILES {
            put(&runner.join(name), &format!("old {name}"));
            put(&bundle.join("payload").join(name), &format!("new {name}"));
        }
        put(
            &prefix.join("drive_c/windows/system32/ntdll.dll"),
            "old prefix DLL",
        );
        for name in SCRIPTS {
            put(
                &root.join("tools").join(name),
                &format!("old script {name}"),
            );
            put(
                &bundle.join("integration").join(name),
                &format!("new script {name}"),
            );
        }
        put(
            &bundle.join("integration/FenixWindowGuard.exe"),
            "synthetic guard",
        );
        put(
            &bundle.join("integration/FenixMCDURefresh.exe"),
            "synthetic refresh helper",
        );
        put(
            &bundle.join("integration/fenix-display-refresh.py"),
            "synthetic refresh supervisor",
        );
        let mut integration: Vec<String> = fs::read_dir(bundle.join("integration"))
            .unwrap()
            .map(|entry| fsx::name(&entry.unwrap().path()))
            .collect();
        integration.sort();
        let integration: Vec<&str> = integration.iter().map(String::as_str).collect();
        let lock = json!({
            "format": 1, "version": "fixture", "runner_version": "fixture version",
            "files": digests(&bundle.join("payload"), &FILES),
            "runner_files": digests(&runner, &[FILES[0], FILES[1], "files/bin/wine"]),
            "integration": digests(&bundle.join("integration"), &integration),
            "accepted_scripts": {
                SCRIPTS[0]: [digest(&root.join("tools").join(SCRIPTS[0]))],
                SCRIPTS[1]: [digest(&root.join("tools").join(SCRIPTS[1]))],
            },
        });
        put(&bundle.join("bundle.json"), &lock.to_string());
        let mock = Mock::new(&bundle, &base.join("home"), Manifest::Fixed(lock));
        Self {
            _serial: serial,
            _temp: temp,
            base,
            root,
            prefix,
            bundle,
            runner,
            mock,
        }
    }

    fn install(&self) -> fenix_patch::Result<()> {
        core::install(
            &self.mock,
            self.root.as_os_str(),
            self.bundle.as_os_str(),
            &silent,
        )
    }

    fn restore(&self) -> fenix_patch::Result<()> {
        core::restore(&self.mock, self.root.as_os_str(), &silent)
    }

    fn marker(&self) -> PathBuf {
        self.root.join(core::MARKER)
    }

    fn state(&self) -> Value {
        fsx::read_json(&self.marker()).unwrap()
    }

    fn active_runner(&self) -> PathBuf {
        fs::canonicalize(self.root.join("runner")).unwrap()
    }

    fn snapshot(&self) -> Value {
        core::snapshot(&self.mock, self.root.as_os_str()).unwrap()
    }

    /// Store the lock in the bundle and use it as the installer's manifest.
    fn publish(&self, lock: &Value) {
        put(&self.bundle.join("bundle.json"), &lock.to_string());
        self.mock.set_lock(lock.clone());
    }

    /// An installed patch, and a newer release of it in the bundle.
    fn prepare_upgrade(&self) -> Value {
        self.install().unwrap();
        let mut state = self.state();
        state["configured"] = json!(true);
        fsx::write_json(&self.marker(), &state).unwrap();
        put(
            &self.prefix.join("aircraft-installed-later"),
            "keep aircraft and account state",
        );
        let mut lock = self.mock.lock();
        lock["previous_releases"] = json!({lock["version"].as_str().unwrap(): {"files": lock["files"], "integration": lock["integration"]}});
        lock["version"] = json!("fixture-next");
        for name in [FILES[0], FILES[1], "files/lib/wine/x86_64-unix/win32u.so"] {
            put(
                &self.bundle.join("payload").join(name),
                &format!("updated {name}"),
            );
            lock["files"][name] = json!(digest(&self.bundle.join("payload").join(name)));
        }
        self.publish(&lock);
        state
    }

    fn sleeper(&self) -> Child {
        Command::new("sleep").arg("30").spawn().unwrap()
    }
}

fn os_error(message: &str) -> Error {
    Error::os_text(message)
}

fn document(path: &Path) -> xml::Element {
    xml::parse(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn install_is_isolated_idempotent_and_restorable() {
    let fx = Fixture::new();
    fx.install().unwrap();
    let state = fx.state();
    assert_eq!(state["state"], "installed");
    assert_eq!(
        read(&fx.runner.join("files/bin/wineserver")),
        "old files/bin/wineserver"
    );
    assert_eq!(
        read(&fx.prefix.join("drive_c/windows/system32/ntdll.dll")),
        "new files/lib/wine/x86_64-windows/ntdll.dll"
    );
    assert_eq!(
        read(
            &fx.prefix
                .join("drive_c/windows/system32/FenixMCDURefresh.exe")
        ),
        "synthetic refresh helper"
    );
    assert_eq!(
        read(
            &fx.prefix
                .join("drive_c/windows/system32/fenix-display-refresh.py")
        ),
        "synthetic refresh supervisor"
    );
    assert_eq!(
        fs::read_link(fx.prefix.join("dosdevices/c:")).unwrap(),
        Path::new("../drive_c")
    );
    fx.install().unwrap();
    assert_eq!(fx.state(), state);
    put(&fx.prefix.join("new-user-state"), "retain this");
    fx.restore().unwrap();
    assert_eq!(fx.active_runner(), fx.runner);
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
    assert_eq!(
        read(&fx.prefix.join("drive_c/windows/system32/ntdll.dll")),
        "old prefix DLL"
    );
    let retained = glob(&fx.root.join("local"), "msfs-prefix.fenix-retained-");
    assert_eq!(read(&retained[0].join("new-user-state")), "retain this");
    assert!(!fx.marker().exists());
}

#[test]
fn wrong_runner_and_corrupt_bundle_change_nothing() {
    let fx = Fixture::new();
    put(&fx.runner.join("files/bin/wine"), "different ABI");
    assert!(fx.install().unwrap_err().is_patch());
    assert!(!fx.marker().exists());
    put(&fx.bundle.join("payload/files/bin/wineserver"), "tampered");
    assert!(
        core::verify_bundle(&fx.mock, fx.bundle.as_os_str())
            .unwrap_err()
            .is_patch()
    );
}

#[test]
fn variant_install_and_restore_preserve_proton_selection() {
    // Exercise manifest selection itself instead of mocking its result.
    let fx = Fixture::new();
    let mut lock = fx.mock.lock();
    *fx.mock.manifest.lock().unwrap() = Manifest::Real;
    let entry = json!({"runner_version": lock["runner_version"], "runner_files": lock["runner_files"], "files": lock["files"]});
    for name in FILES {
        put(
            &fx.bundle.join("payload/variants/experimental").join(name),
            &format!("new {name}"),
        );
    }
    lock["runner_version"] = json!("another base runner");
    lock["variants"] = json!({"experimental": entry});
    put(&fx.bundle.join("bundle.json"), &lock.to_string());
    let selected = json!({
        "schema": 1, "runner": format!("local/proton-tests/{}/runner", "a".repeat(32)),
        "version": "fixture version", "base_runner": "/original/runner",
    });
    let selection = fx.root.join("private/proton-selection.json");
    fsx::write_json(&selection, &selected).unwrap();
    fx.install().unwrap();
    let state = fx.state();
    assert_eq!(state["variant"], "experimental");
    assert_eq!(state["proton_before"], selected);
    let active = fsx::read_json(&selection).unwrap();
    assert_eq!(
        active["runner"],
        format!("{}/runner", state["work"].as_str().unwrap())
    );
    assert_eq!(active["base_runner"], selected["base_runner"]);
    core::verify_installed(&fx.mock, &fx.root, &state).unwrap();
    fx.restore().unwrap();
    assert_eq!(fsx::read_json(&selection).unwrap(), selected);
}

#[test]
fn classic_wine64_entry_point_is_selected() {
    let fx = Fixture::new();
    assert_eq!(fsx::name(&core::wine_binary(&fx.runner)), "wine");
    put(&fx.runner.join("files/bin/wine64"), "64-bit loader");
    assert_eq!(fsx::name(&core::wine_binary(&fx.runner)), "wine64");
    assert_eq!(
        core::wine_env(&fx.mock, &fx.prefix, &fx.runner)
            .get("WINE_DISABLE_FAST_SYNC")
            .unwrap(),
        "1"
    );
}

#[test]
fn configure_reports_the_readout_setting_separately() {
    let fx = Fixture::new();
    let directory = fx
        .prefix
        .join("drive_c/users/test/AppData/Roaming/Microsoft Flight Simulator 2024");
    fs::create_dir_all(&directory).unwrap();
    let configure = || {
        core::configure_prefix(&fx.prefix, core::DEFAULT_FOLDER, core::DisplayRenderer::Cpu)
            .unwrap()
    };
    let config = fx.prefix.join(core::CONFIG);
    assert_eq!(configure(), (false, false));
    put(
        &fx.prefix.join(core::PROGRAM).join("Fenix.exe"),
        "synthetic app",
    );
    // Without its sign-in settings nothing counts as configured.
    put(&config.join("persistancy.xml"), "<Settings/>");
    assert_eq!(configure(), (false, false));
    fs::remove_file(config.join("persistancy.xml")).unwrap();
    // Fenix writes persistancy.xml later; displays and autostart do not wait for it.
    put(&config.join("fenixConfig.xml"), "<Config/>");
    assert_eq!(configure(), (true, false));
    assert!(!config.join("persistancy.xml").exists());
    assert_eq!(
        document(&config.join("fenixConfig.xml")).findtext("displayMode"),
        "CPU"
    );
    assert!(
        document(&directory.join("exe.xml"))
            .find("Launch.Addon")
            .is_some()
    );
    let mut state = fenix_patch::json::Map::new();
    assert!(
        core::apply_settings(
            &mut state,
            &fx.prefix,
            core::DEFAULT_FOLDER,
            core::DisplayRenderer::Cpu
        )
        .unwrap()
        .contains("load the aircraft once")
    );
    assert_eq!(
        Value::Object(state.clone()),
        json!({"configured": true, "readouts": false, "display_renderer": "CPU"})
    );
    put(&config.join("persistancy.xml"), "<Settings/>");
    assert!(
        core::apply_settings(
            &mut state,
            &fx.prefix,
            core::DEFAULT_FOLDER,
            core::DisplayRenderer::Cpu
        )
        .unwrap()
        .contains("Legacy readouts")
    );
    assert_eq!(
        Value::Object(state),
        json!({"configured": true, "readouts": true, "display_renderer": "CPU"})
    );
    // States written before the separate key meant both settings.
    assert!(core::readouts_set(&json!({"configured": true})));
    assert!(!core::readouts_set(&json!({"configured": false})));
}

#[test]
fn gpu_renderer_requires_its_own_complete_verified_overlay() {
    use core::DisplayRenderer::{Cpu, Gpu};
    let fx = Fixture::new();
    let mut lock = fx.mock.lock();
    assert_eq!(core::DisplayRenderer::from_manifest(&lock).unwrap(), Cpu);
    assert_patch_error(
        core::DisplayRenderer::select(&lock, Some(Gpu)),
        "CPU displays only",
    );
    lock["display_renderer"] = json!("GPU");
    assert_patch_error(
        core::DisplayRenderer::from_manifest(&lock),
        "complete matched Wine and DXGI",
    );
    lock["dxvk"] = json!({"revision": "fixture"});
    assert_patch_error(
        core::DisplayRenderer::from_manifest(&lock),
        "complete matched Wine and DXGI",
    );
    gpu_overlay(&fx.bundle.join("payload"), &mut lock);
    assert_eq!(core::DisplayRenderer::from_manifest(&lock).unwrap(), Gpu);
    assert_eq!(
        core::DisplayRenderer::select(&lock, Some(Cpu)).unwrap(),
        Cpu
    );
    lock["variants"] = json!({
        "legacy": {"files": {}},
        "own-dxvk": {"dxvk": lock["dxvk"], "files": lock["files"]},
        "unsupported": {"display_renderer": "GPU", "files": lock["files"]},
        "gpu": {"display_renderer": "GPU", "dxvk": lock["dxvk"], "files": lock["files"]},
    });
    fx.publish(&lock);
    for variant in ["legacy", "own-dxvk"] {
        let entry = core::read_manifest(&fx.bundle, Some(variant)).unwrap();
        assert!(entry.get("display_renderer").is_none());
        assert_eq!(core::DisplayRenderer::from_manifest(&entry).unwrap(), Cpu);
        if variant == "legacy" {
            assert!(entry.get("dxvk").is_none());
        }
    }
    let entry = core::read_manifest(&fx.bundle, Some("unsupported")).unwrap();
    assert_patch_error(
        core::DisplayRenderer::from_manifest(&entry),
        "complete matched Wine and DXGI",
    );
    let entry = core::read_manifest(&fx.bundle, Some("gpu")).unwrap();
    assert_eq!(core::DisplayRenderer::from_manifest(&entry).unwrap(), Gpu);
    // A capability flag never bypasses payload integrity checking.
    put(
        &fx.bundle.join("payload").join(core::DXGI_FILE),
        "tampered DXGI",
    );
    assert_patch_error(fx.install(), "checksum mismatch");
    assert!(!fx.marker().exists());
    assert_eq!(fx.active_runner(), fx.runner);
}

#[test]
fn gpu_overlay_replaces_native_dxgi_symlinks_without_touching_their_targets() {
    let fx = Fixture::new();
    let mut lock = fx.mock.lock();
    gpu_overlay(&fx.bundle.join("payload"), &mut lock);
    fx.publish(&lock);
    let outside = fx.base.join("shared native dxgi.dll");
    put(&outside, "original shared native DXGI");
    let native = fx.prefix.join("drive_c/windows/system32/dxgi.dll");
    symlink(&outside, &native).unwrap();
    fx.install().unwrap();
    let expected = read(&fx.bundle.join("payload").join(core::DXGI_FILE));
    assert!(!native.is_symlink());
    assert_eq!(read(&native), expected);
    assert_eq!(read(&fx.active_runner().join(core::DXGI_FILE)), expected);
    assert_eq!(read(&outside), "original shared native DXGI");
    fx.restore().unwrap();
    assert!(native.is_symlink());
    assert_eq!(fs::read_link(&native).unwrap(), outside);
}

#[test]
fn upgrading_cpu_displays_applies_gpu_settings_and_preserves_the_previous_settings() {
    let fx = Fixture::new();
    let mut lock = fx.mock.lock();
    lock["version"] = json!("0.1.0-preview.7");
    fx.publish(&lock);
    put(
        &fx.prefix.join(core::PROGRAM).join("Fenix.exe"),
        "synthetic app",
    );
    put(
        &fx.prefix.join(core::CONFIG).join("fenixConfig.xml"),
        "<Config><UserChoice>keep</UserChoice></Config>",
    );
    put(
        &fx.prefix.join(core::CONFIG).join("persistancy.xml"),
        "<Settings/>",
    );
    fs::create_dir_all(
        fx.prefix
            .join("drive_c/users/test/AppData/Roaming")
            .join(core::DEFAULT_FOLDER),
    )
    .unwrap();
    let mut previous = fx.prepare_upgrade();
    assert_eq!(previous["display_renderer"], "CPU");
    // Shipping preview.7 states predate the renderer capability metadata.
    previous.as_object_mut().unwrap().remove("display_renderer");
    fsx::write_json(&fx.marker(), &previous).unwrap();
    let settings = fx.prefix.join(core::CONFIG).join("fenixConfig.xml");
    let before = read(&settings);
    let mut lock = fx.mock.lock();
    lock["version"] = json!("0.1.0-preview.8");
    gpu_overlay(&fx.bundle.join("payload"), &mut lock);
    fx.publish(&lock);
    fx.install().unwrap();
    let state = fx.state();
    assert_eq!(state["configured"], true);
    assert_eq!(state["version"], "0.1.0-preview.8");
    assert_eq!(state["display_renderer"], "GPU");
    assert_eq!(fx.snapshot()["display_renderer"], "GPU");
    let config = document(&settings);
    for (tag, value) in [
        ("displayMode", "GPU"),
        ("newRender", "true"),
        ("preferCPU", "false"),
        ("multithread", "true"),
        ("UserChoice", "keep"),
    ] {
        assert_eq!(config.findtext(tag), value);
    }
    assert_eq!(
        document(&fx.prefix.join(core::CONFIG).join("persistancy.xml")).findtext("fcuReadoutsType"),
        "0"
    );
    let backup = fx.root.join(state["upgrade_backup"].as_str().unwrap());
    assert_eq!(
        read(&backup.join("settings-before-update/fenixConfig.xml")),
        before
    );
    assert_eq!(state["backup"], previous["backup"]);
    core::configure_with_renderer(
        &fx.mock,
        fx.root.as_os_str(),
        &silent,
        Some(core::DisplayRenderer::Cpu),
    )
    .unwrap();
    assert_eq!(fx.state()["display_renderer"], "CPU");
    let config = document(&settings);
    assert_eq!(config.findtext("displayMode"), "CPU");
    assert_eq!(config.findtext("newRender"), "false");
    assert_eq!(config.findtext("preferCPU"), "true");
    core::configure(&fx.mock, fx.root.as_os_str(), &silent).unwrap();
    assert_eq!(fx.state()["display_renderer"], "CPU");
    assert_eq!(fx.state()["display_renderer_override"], "CPU");
    let mut lock = fx.mock.lock();
    lock["previous_releases"]["0.1.0-preview.8"] = json!({"files": lock["files"], "prefix_files": lock["prefix_files"], "integration": lock["integration"]});
    lock["version"] = json!("0.1.0-preview.9");
    fx.publish(&lock);
    fx.install().unwrap();
    assert_eq!(fx.state()["display_renderer"], "CPU");
    assert_eq!(fx.state()["display_renderer_override"], "CPU");
    assert_eq!(document(&settings).findtext("displayMode"), "CPU");
    core::configure_with_renderer(
        &fx.mock,
        fx.root.as_os_str(),
        &silent,
        Some(core::DisplayRenderer::Gpu),
    )
    .unwrap();
    assert_eq!(fx.state()["display_renderer_override"], "GPU");
    assert_eq!(fx.state()["display_renderer"], "GPU");
}

#[test]
fn upgrade_keeps_aircraft_settings_and_original_restore_point() {
    let fx = Fixture::new();
    let previous = fx.prepare_upgrade();
    let old_runner = fx.active_runner();
    assert_eq!(fx.snapshot()["update_available"], true);
    fx.mock.framework.reset();
    fx.mock.geometry.reset();
    fx.install().unwrap();
    assert_eq!(fx.mock.framework.count(), 0);
    assert_eq!(fx.mock.geometry.count(), 1);
    let state = fx.state();
    assert_eq!(state["version"], "fixture-next");
    assert_eq!(state["configured"], true);
    for key in [
        "backup",
        "previous_prefix",
        "previous_runner",
        "original_prefix_id",
    ] {
        assert_eq!(state[key], previous[key], "{key}");
    }
    assert_ne!(fx.active_runner(), old_runner);
    assert_eq!(
        read(&old_runner.join("files/bin/wineserver")),
        "new files/bin/wineserver"
    );
    assert_eq!(
        read(&fx.prefix.join("aircraft-installed-later")),
        "keep aircraft and account state"
    );
    assert_eq!(fx.snapshot()["update_available"], false);
    core::verify_installed(&fx.mock, &fx.root, &state).unwrap();
    fx.restore().unwrap();
    assert_eq!(fx.active_runner(), fx.runner);
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
    let retained = glob(&fx.root.join("local"), "msfs-prefix.fenix-retained-");
    assert!(retained[0].join("aircraft-installed-later").is_file());
}

#[test]
fn upgrade_rejects_changed_previous_files_without_changing_state() {
    let fx = Fixture::new();
    let previous = fx.prepare_upgrade();
    put(
        &fx.active_runner().join("files/bin/wineserver"),
        "custom binary",
    );
    assert_patch_error(fx.install(), "Installed patch file changed");
    assert_eq!(fx.state(), previous);
    assert!(fx.prefix.join("aircraft-installed-later").is_file());
}

#[test]
fn geometry_setup_failure_never_commits_the_staging_profile() {
    let fx = Fixture::new();
    let previous = fx.prepare_upgrade();
    let original_runner = fx.active_runner();
    fx.mock
        .geometry
        .fail(Some(Error::patch("bad dependency checksum")));
    assert_patch_error(fx.install(), "bad dependency checksum");
    fx.mock.geometry.fail(None);
    assert_eq!(fx.active_runner(), original_runner);
    assert_eq!(
        read(&fx.prefix.join("aircraft-installed-later")),
        "keep aircraft and account state"
    );
    assert_eq!(fx.state()["state"], "preparing");
    assert_eq!(fx.state()["previous_prefix"], previous["previous_prefix"]);
}

#[test]
fn provider_integrity_is_checked_for_new_releases_only() {
    let fx = Fixture::new();
    fx.install().unwrap();
    let state = fx.state();
    let geometry = fx.prefix.join(core::GEOMETRY_PATH);
    put(&geometry, "fixture geometry");
    let mut lock = fx.mock.lock();
    lock["prefix_files"] = json!({core::GEOMETRY_PATH: digest(&geometry)});
    fx.mock.set_lock(lock);
    core::verify_installed(&fx.mock, &fx.root, &state).unwrap();
    put(&geometry, "changed geometry");
    assert_patch_error(
        core::verify_installed(&fx.mock, &fx.root, &state),
        "dependency changed",
    );
}

#[test]
fn scoped_geometry_environment_replaces_inherited_global_override() {
    let fx = Fixture::new();
    {
        let mut environ = fx.mock.environ.lock().unwrap();
        for (name, value) in [
            ("WINE_D2D1_GEOMETRY_PROVIDER", "1"),
            ("WINESERVERSOCKET", "stale"),
            ("WINE_DLL_FILE_MAP", "stale"),
        ] {
            environ.set(name, value);
        }
    }
    let env: Env = core::wine_env(&fx.mock, &fx.prefix, &fx.runner);
    assert_eq!(
        env.get("WINE_D2D1_GEOMETRY_PROVIDER").unwrap(),
        "FenixDisplay.exe"
    );
    assert!(env.get("WINESERVERSOCKET").is_none());
    assert!(env.get("WINE_DLL_FILE_MAP").is_none());
}

/// `os.rename` fails when the staged profile is to take the place of the active one.
fn fail_commit(fx: &Fixture, message: &'static str) {
    let prefix = fx.prefix.clone();
    *fx.mock.rename.lock().unwrap() = Some(Box::new(move |source: &Path, target: &Path| {
        (fsx::name(source) == "prefix" && target == prefix).then(|| os_error(message))
    }));
}

#[test]
fn interrupted_upgrade_retains_current_profile_and_can_restore() {
    let fx = Fixture::new();
    fx.prepare_upgrade();
    fail_commit(&fx, "interrupted upgrade commit");
    let error = fx.install().unwrap_err();
    assert!(
        error.is_os() && error.to_string().contains("interrupted upgrade"),
        "{error:?}"
    );
    *fx.mock.rename.lock().unwrap() = None;
    let state = fx.state();
    assert_eq!(state["state"], "committing");
    let kept = fx
        .root
        .join(state["upgrade_previous_prefix"].as_str().unwrap())
        .join("aircraft-installed-later");
    assert!(kept.is_file());
    fx.restore().unwrap();
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
    assert!(kept.is_file());
}

#[test]
fn custom_scripts_are_not_overwritten() {
    let fx = Fixture::new();
    put(&fx.root.join("tools/launch-msfs.sh"), "user custom");
    assert!(fx.install().unwrap_err().is_patch());
    assert_eq!(read(&fx.root.join("tools/launch-msfs.sh")), "user custom");
}

#[test]
fn runtime_lock_excludes_install() {
    let fx = Fixture::new();
    let lock = fs::File::create(fx.root.join("private/play.lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    assert!(fx.install().unwrap_err().is_patch());
    drop(lock);
    assert!(!fx.marker().exists());
}

#[test]
fn interactive_waiter_retains_runtime_lock_through_shutdown() {
    let fx = Fixture::new();
    fx.install().unwrap();
    put(
        &fx.prefix.join(core::PROGRAM).join("Fenix.exe"),
        "synthetic app",
    );
    let child = fx.sleeper();
    let pid = child.id();
    *fx.mock.child.lock().unwrap() = Some(child);
    let lock_path = fx.root.join("private/play.lock");
    let mut wait = |process: &mut Child| -> fenix_patch::Result<i32> {
        assert_eq!(process.id(), pid);
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert_eq!(
            rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive),
            Err(rustix::io::Errno::WOULDBLOCK)
        );
        process.kill().unwrap();
        process.wait().unwrap();
        Ok(0)
    };
    core::windows_app(
        &fx.mock,
        fx.root.as_os_str(),
        None,
        &silent,
        false,
        Some(&mut wait),
    )
    .unwrap();
    core::locked(&fx.mock, fx.root.as_os_str(), true, false).unwrap();
}

#[test]
fn failed_interactive_waiter_reaps_child_and_releases_lock() {
    let fx = Fixture::new();
    fx.install().unwrap();
    put(
        &fx.prefix.join(core::PROGRAM).join("Fenix.exe"),
        "synthetic app",
    );
    let child = fx.sleeper();
    let pid = child.id();
    *fx.mock.child.lock().unwrap() = Some(child);
    let mut wait = |_: &mut Child| -> fenix_patch::Result<i32> { Err(Error::other("wait failed")) };
    let error = core::windows_app(
        &fx.mock,
        fx.root.as_os_str(),
        None,
        &silent,
        false,
        Some(&mut wait),
    )
    .unwrap_err();
    assert_eq!(error, Error::other("wait failed"));
    // The child was ended and collected: its process no longer exists.
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    core::locked(&fx.mock, fx.root.as_os_str(), true, false).unwrap();
}

#[test]
fn legacy_profile_can_open_installed_fenix_but_cannot_run_an_installer() {
    let fx = Fixture::new();
    put(&fx.root.join("private/fenix-compat.json"), "{}");
    let app = fx.prefix.join(core::PROGRAM).join("Fenix.exe");
    put(&app, "synthetic app");
    core::windows_app(&fx.mock, fx.root.as_os_str(), None, &silent, false, None).unwrap();
    assert_eq!(
        fx.mock.wine.reg.lock().unwrap().last().unwrap(),
        &[
            r"HKCU\Software\Wine\Explorer",
            "ShowSystray",
            "0",
            "REG_DWORD"
        ]
    );
    assert_eq!(
        fx.mock
            .spawned
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .last()
            .unwrap(),
        app.to_str().unwrap()
    );
    assert!(
        fx.mock
            .wine
            .run
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.last().unwrap() == "/reg:64")
    );
    fx.mock.spawned.lock().unwrap().clear();
    let error = core::windows_app(
        &fx.mock,
        fx.root.as_os_str(),
        Some(OsStr::new("/not-an-installed-app.exe")),
        &silent,
        false,
        None,
    )
    .unwrap_err();
    assert!(error.is_patch());
    assert!(fx.mock.spawned.lock().unwrap().is_empty());
}

#[test]
fn ui_fonts_repair_32_bit_only_registration_and_preserve_existing_files() {
    let fx = Fixture::new();
    put(
        &fx.prefix.join("system.reg"),
        "[Software\\\\Wow6432Node\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts]\n\"Tahoma (TrueType)\"=\"old-font.ttf\"\n",
    );
    let existing = fx.prefix.join("drive_c/windows/Fonts/tahoma.ttf");
    put(&existing, "user's existing Tahoma");
    let calls = Arc::new(WineCalls::default());
    let wine = MockWine::new(&fx.prefix, &fx.runner, calls.clone());
    core::ensure_ui_fonts(&fx.prefix, &fx.runner, &wine).unwrap();
    assert_eq!(read(&existing), "user's existing Tahoma");
    assert_eq!(
        read(&existing.with_file_name("tahomabd.ttf")),
        "fixture Tahoma Bold"
    );
    let run = calls.run.lock().unwrap();
    assert_eq!(run.len(), 2);
    let values: Vec<&str> = run
        .iter()
        .map(|call| call[call.iter().position(|arg| arg == "/v").unwrap() + 1].as_str())
        .collect();
    assert_eq!(values, ["Tahoma (TrueType)", "Tahoma Bold (TrueType)"]);
    assert!(run.iter().all(|call| call.last().unwrap() == "/reg:64"));
    assert_eq!(wine.prefix(), fx.prefix);
}

#[test]
fn ui_fonts_leave_registered_fonts_unchanged() {
    let fx = Fixture::new();
    put(
        &fx.prefix.join("system.reg"),
        "[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts] 1\n\"Tahoma (TrueType)\"=\"custom-regular.ttf\"\n\"Tahoma Bold (TrueType)\"=\"custom-bold.ttf\"\n",
    );
    let calls = Arc::new(WineCalls::default());
    let wine = MockWine::new(&fx.prefix, &fx.runner, calls.clone());
    core::ensure_ui_fonts(&fx.prefix, &fx.runner, &wine).unwrap();
    assert!(calls.run.lock().unwrap().is_empty());
    assert!(!fx.prefix.join("drive_c/windows/Fonts").exists());
}

#[test]
fn running_prefix_is_detected() {
    let fx = Fixture::new();
    let mut process = Command::new("sleep")
        .arg("20")
        .env("WINEPREFIX", &fx.prefix)
        .spawn()
        .unwrap();
    let result = fx.install();
    process.kill().unwrap();
    process.wait().unwrap();
    assert!(result.unwrap_err().is_patch());
}

#[test]
fn failed_preparation_restores_original_without_losing_stage() {
    let fx = Fixture::new();
    fx.mock.graphics.fail(Some(Error::patch("fixture failure")));
    assert!(fx.install().unwrap_err().is_patch());
    fx.mock.graphics.fail(None);
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
    fx.restore().unwrap();
    assert_eq!(fx.active_runner(), fx.runner);
    assert!(
        glob(&fx.root.join("local"), "fenix-patch-")
            .iter()
            .any(|work| work.join("prefix").exists())
    );
}

#[test]
fn recovery_between_prefix_renames() {
    let fx = Fixture::new();
    fail_commit(&fx, "power loss between renames");
    assert!(fx.install().unwrap_err().is_os());
    *fx.mock.rename.lock().unwrap() = None;
    assert!(!fx.prefix.exists());
    assert_eq!(fx.snapshot()["can_restore"], true);
    fx.restore().unwrap();
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
}

#[test]
fn recovery_during_restore_is_repeatable() {
    let fx = Fixture::new();
    fx.install().unwrap();
    *fx.mock.replace_link.lock().unwrap() = Some(os_error("restore interrupted"));
    assert!(fx.restore().unwrap_err().is_os());
    *fx.mock.replace_link.lock().unwrap() = None;
    fx.restore().unwrap();
    assert_eq!(fx.active_runner(), fx.runner);
}

#[test]
fn symlink_parent_escape_is_rejected() {
    let fx = Fixture::new();
    let outside = fx.base.join("outside");
    fs::create_dir(&outside).unwrap();
    let parent = fx.bundle.join("payload/files/lib/wine/x86_64-windows");
    fs::remove_file(parent.join("ntdll.dll")).unwrap();
    fs::remove_dir(&parent).unwrap();
    symlink(&outside, &parent).unwrap();
    assert!(
        core::verify_bundle(&fx.mock, fx.bundle.as_os_str())
            .unwrap_err()
            .is_patch()
    );
}

#[test]
fn configure_preserves_unrelated_xml_and_autostart_entries() {
    let fx = Fixture::new();
    let config = fx.prefix.join(core::CONFIG);
    put(
        &fx.prefix.join(core::PROGRAM).join("Fenix.exe"),
        "synthetic app",
    );
    put(
        &config.join("fenixConfig.xml"),
        "<Config><displayMode>GPU</displayMode><UserChoice>keep</UserChoice></Config>",
    );
    put(
        &config.join("persistancy.xml"),
        "<Settings><fcuReadoutsType>1</fcuReadoutsType><Volume>0.4</Volume></Settings>",
    );
    let directory = fx
        .prefix
        .join("drive_c/users/test/AppData/Roaming/Microsoft Flight Simulator 2024");
    put(
        &directory.join("exe.xml"),
        "<SimBase.Document><Launch.Addon><Name>Other addon</Name><Path>C:\\Other.exe</Path></Launch.Addon></SimBase.Document>",
    );
    assert_eq!(
        core::configure_prefix(&fx.prefix, core::DEFAULT_FOLDER, core::DisplayRenderer::Cpu)
            .unwrap(),
        (true, true)
    );
    assert_eq!(
        core::configure_prefix(&fx.prefix, core::DEFAULT_FOLDER, core::DisplayRenderer::Cpu)
            .unwrap(),
        (true, true)
    );
    let settings = document(&config.join("fenixConfig.xml"));
    assert_eq!(settings.findtext("displayMode"), "CPU");
    assert_eq!(settings.findtext("UserChoice"), "keep");
    assert_eq!(
        document(&config.join("persistancy.xml")).findtext("fcuReadoutsType"),
        "0"
    );
    let launch = document(&directory.join("exe.xml"));
    let entries: Vec<&xml::Element> = launch
        .elements()
        .filter(|element| element.tag == "Launch.Addon")
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].findtext("Name"), "Other addon");
}

#[test]
fn mocked_steps_are_the_ones_the_python_tests_replaced() {
    // Guards the fixture itself: the real steps would need Wine and the network.
    let fx = Fixture::new();
    let messages = Messages::default();
    core::install(
        &fx.mock,
        fx.root.as_os_str(),
        fx.bundle.as_os_str(),
        &|message| messages.push(message),
    )
    .unwrap();
    assert_eq!(
        (
            fx.mock.framework.count(),
            fx.mock.graphics.count(),
            fx.mock.geometry.count()
        ),
        (1, 1, 1)
    );
    assert_eq!(fx.mock.wine.stop.load(Ordering::SeqCst), 1);
    assert_eq!(
        messages.all(),
        [
            "Copying Wine profile and runner; the original profile remains available …",
            "Preparing fonts, graphics dependencies and Fenix settings …",
            "Patch installed. Install and sign in to Fenix, then apply the aircraft settings.",
        ]
    );
    assert!(fx.mock.host_check().is_ok());
}
