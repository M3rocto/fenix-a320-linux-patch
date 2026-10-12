// SPDX-License-Identifier: MIT
//! The Steam installation, test for test as `tests/test_steam.py`.
#![allow(clippy::unwrap_used)]
mod common;

use common::{Manifest, Messages, Mock, assert_patch_error, digest, digests, glob, put, read};
use fenix_patch::{
    Error, core, fsx, guide,
    json::{self, Value, json},
    steam,
    system::{System, silent},
    targets, vdf, xml,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
};

const FILES: [&str; 2] = [
    "files/lib/wine/x86_64-windows/ntdll.dll",
    "files/bin/wineserver",
];
const HELPERS: [&str; 3] = [
    "FenixWindowGuard.exe",
    "FenixMCDURefresh.exe",
    "fenix-display-refresh.py",
];

struct Fixture {
    _serial: std::sync::MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    library: PathBuf,
    bundle: PathBuf,
    proton: PathBuf,
    item: steam::Target,
    prefix: PathBuf,
    settings: PathBuf,
    mock: Arc<Mock>,
}

impl Fixture {
    fn new() -> Self {
        let serial = common::serial();
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("Steam client");
        let library = base.join("second library");
        let bundle = base.join("bundle");
        let proton = root.join("compatibilitytools.d/pinned-proton");
        fs::create_dir_all(root.join("steamapps")).unwrap();
        put(
            &root.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n \"0\" {{ \"path\" \"{}\" }}\n \"1\" {{ \"path\" \"{}\" }}\n \"2\" {{ \"path\" \"/missing/library\" }}\n}}\n",
                root.display(),
                library.display()
            ),
        );
        put(
            &library.join("steamapps/appmanifest_2537590.acf"),
            "manifest",
        );
        let item = steam::Target::new("msfs2024", &root, &library).unwrap();
        let prefix = item.prefix();
        fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        fs::create_dir(prefix.join("dosdevices")).unwrap();
        symlink(prefix.join("drive_c"), prefix.join("dosdevices/c:")).unwrap();
        put(&prefix.join("system.reg"), "original registry");
        put(&item.compat().join("version"), "11.0-100");
        let settings =
            prefix.join("drive_c/users/steamuser/AppData/Roaming/Microsoft Flight Simulator 2024");
        put(&settings.join("UserCfg.opt"), "settings");
        put(
            &settings.join("Packages/Community/addon/layout.json"),
            "large simulator content",
        );
        put(&proton.join("proton"), "launcher");
        put(&proton.join("version"), "fixture version");
        put(&proton.join("files/bin/wine"), "fixture Wine");
        for name in FILES {
            put(&proton.join(name), &format!("old {name}"));
            put(
                &bundle.join("payload/variants/fixture").join(name),
                &format!("new {name}"),
            );
        }
        for name in HELPERS {
            put(
                &bundle.join("integration").join(name),
                &format!("synthetic {name}"),
            );
        }
        let entry = json!({
            "runner_version": "1 fixture version",
            "files": digests(&bundle.join("payload/variants/fixture"), &FILES),
            "runner_files": digests(&proton, &[FILES[0], FILES[1], "files/bin/wine"]),
        });
        let lock = json!({
            "format": 1, "version": "fixture", "runner_version": "base", "files": {}, "runner_files": {"absent": "0"},
            "integration": digests(&bundle.join("integration"), &HELPERS),
            "variants": {"fixture": entry},
        });
        put(&bundle.join("bundle.json"), &lock.to_string());
        let mut mock = Mock::new(&bundle, &base.join("home"), Manifest::Variants(lock));
        mock.mock_proton_setup = true;
        Self {
            _serial: serial,
            _temp: temp,
            base,
            root,
            library,
            bundle,
            proton,
            item,
            prefix,
            settings,
            mock: Arc::new(mock),
        }
    }

    fn system(&self) -> Arc<dyn System> {
        self.mock.clone()
    }

    fn tool(&self) -> PathBuf {
        self.root
            .join("compatibilitytools.d/Proton-Fenix-A320-fixture")
    }

    fn install(&self, item: &steam::Target) -> fenix_patch::Result<()> {
        steam::install(
            self.mock.as_ref(),
            item,
            self.bundle.as_os_str(),
            &silent,
            None,
        )
    }

    fn restore(&self, item: &steam::Target) -> fenix_patch::Result<()> {
        steam::restore(self.mock.as_ref(), item, &silent)
    }

    fn configure(&self, item: &steam::Target) -> fenix_patch::Result<()> {
        steam::configure(self.mock.as_ref(), item, &silent)
    }

    fn select_tool(&self) -> fenix_patch::Result<()> {
        steam::select_tool(self.mock.as_ref(), &self.item, &silent)
    }

    fn windows_app(&self) -> fenix_patch::Result<()> {
        steam::windows_app(self.mock.as_ref(), &self.item, None, &silent, false, None)
    }

    fn state(&self, item: &steam::Target) -> Value {
        fsx::read_json(&item.marker()).unwrap()
    }

    fn snapshot(&self, item: &steam::Target) -> Value {
        steam::snapshot(self.mock.as_ref(), item).unwrap()
    }

    fn tool_state(&self) -> Value {
        steam::tool_state(&self.tool()).unwrap().unwrap()
    }

    fn config(&self) -> PathBuf {
        self.root.join("config/config.vdf")
    }

    fn publish(&self, lock: &Value) {
        put(&self.bundle.join("bundle.json"), &lock.to_string());
        self.mock.set_lock(lock.clone());
    }

    /// The second simulator of the same library.
    fn other(&self) -> steam::Target {
        put(
            &self.library.join("steamapps/appmanifest_1250410.acf"),
            "manifest",
        );
        let other = steam::Target::new("msfs2020", &self.root, &self.library).unwrap();
        fs::create_dir_all(other.prefix().join("drive_c/windows/system32")).unwrap();
        put(&other.prefix().join("system.reg"), "2020 registry");
        other
    }

    /// A later release of the same overlay with one changed file.
    fn release(&self) {
        let mut lock = self.mock.lock();
        let name = "files/bin/wineserver";
        let previous = json!({lock["version"].as_str().unwrap(): {"files": lock["variants"]["fixture"]["files"]}});
        lock["variants"]["fixture"]["previous_releases"] = previous;
        lock["version"] = json!("fixture-next");
        put(
            &self.bundle.join("payload/variants/fixture").join(name),
            &format!("newer {name}"),
        );
        lock["variants"]["fixture"]["files"][name] = json!(digest(
            &self.bundle.join("payload/variants/fixture").join(name)
        ));
        self.publish(&lock);
    }

    fn files(&self, base: &Path) -> BTreeMap<PathBuf, String> {
        tree(base)
            .into_iter()
            .filter(|path| path.is_file())
            .map(|path| (path.strip_prefix(base).unwrap().to_path_buf(), read(&path)))
            .collect()
    }
}

/// `sorted(base.rglob("*"))`.
fn tree(base: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![base.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() && !path.is_symlink() {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    found
}

fn document(path: &Path) -> xml::Element {
    xml::parse(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn detects_simulator_in_secondary_library_only_with_profile() {
    let fx = Fixture::new();
    let found = steam::targets(fx.mock.as_ref(), Some(fx.root.as_os_str())).unwrap();
    assert_eq!(
        found
            .iter()
            .map(|item| (item.game, item.library.clone()))
            .collect::<Vec<_>>(),
        [("msfs2024", fx.library.clone())]
    );
    assert_patch_error(
        steam::target(fx.mock.as_ref(), "msfs2020", Some(fx.root.as_os_str())),
        "start it once",
    );
    let selected = targets::select(
        &fx.system(),
        None,
        Some("msfs2024"),
        Some(fx.root.as_os_str()),
    )
    .unwrap();
    assert_eq!(selected.prefix(), found[0].prefix());
    assert_eq!(selected.kind(), "steam");
}

#[test]
fn install_creates_separate_tool_and_restore_returns_everything() {
    let fx = Fixture::new();
    let original_tool = fx.files(&fx.proton);
    let messages = Messages::default();
    steam::install(
        fx.mock.as_ref(),
        &fx.item,
        fx.bundle.as_os_str(),
        &|message| messages.push(message),
        None,
    )
    .unwrap();
    let state = fx.state(&fx.item);
    assert_eq!(
        (
            state["state"].as_str(),
            state["tool"].as_str(),
            state["variant"].as_str()
        ),
        (
            Some("installed"),
            Some("Proton-Fenix-A320-fixture"),
            Some("fixture")
        )
    );
    // The user's own Proton is never modified; the overlay lives in the copy.
    assert_eq!(fx.files(&fx.proton), original_tool);
    assert_eq!(
        read(&fx.tool().join("files/bin/wineserver")),
        "new files/bin/wineserver"
    );
    assert_eq!(read(&fx.tool().join("files/bin/wine")), "fixture Wine");
    // Proton prepared the profile before the overlay replaced any binary.
    assert_eq!(
        *fx.mock.setups.lock().unwrap(),
        [(fx.tool(), digest(&fx.proton.join("files/bin/wineserver")))]
    );
    assert!(
        read(&fx.tool().join("compatibilitytool.vdf")).contains("\"Proton-Fenix-A320-fixture\"")
    );
    let settings = read(&fx.tool().join("user_settings.py"));
    let settings = json::loads(settings.split_once("user_settings = ").unwrap().1).unwrap();
    assert!(
        settings["WINE_TRACK_WRITECOPY"]
            .as_str()
            .unwrap()
            .contains("FlightSimulator.exe")
    );
    assert_eq!(settings["WINE_D2D1_GEOMETRY_PROVIDER"], "FenixDisplay.exe");
    assert_eq!(
        read(&fx.prefix.join("drive_c/windows/system32/ntdll.dll")),
        "new files/lib/wine/x86_64-windows/ntdll.dll"
    );
    assert_eq!(read(&fx.item.compat().join("version")), "tool version");
    assert_eq!(
        fs::read_link(fx.prefix.join("dosdevices/c:")).unwrap(),
        Path::new("../drive_c")
    );
    // Simulator content was moved, not duplicated.
    let previous = fx.item.state().join(state["previous"].as_str().unwrap());
    assert_eq!(
        read(&fx.settings.join("Packages/Community/addon/layout.json")),
        "large simulator content"
    );
    assert!(
        !previous
            .join(state["carried"][0].as_str().unwrap())
            .exists()
    );
    assert_eq!(read(&previous.join("pfx/system.reg")), "original registry");
    assert!(messages.last().contains("Properties → Compatibility"));
    fx.install(&fx.item).unwrap();
    assert_eq!(fx.state(&fx.item), state);
    put(
        &fx.settings.join("Packages/Community/fenix/aircraft.cfg"),
        "installed after the patch",
    );
    put(&fx.prefix.join("new-user-state"), "retain this");
    fx.restore(&fx.item).unwrap();
    assert_eq!(read(&fx.item.compat().join("version")), "11.0-100");
    assert!(
        !fx.prefix
            .join("drive_c/windows/system32/ntdll.dll")
            .exists()
    );
    assert_eq!(
        read(&fx.settings.join("Packages/Community/addon/layout.json")),
        "large simulator content"
    );
    assert_eq!(
        read(&fx.settings.join("Packages/Community/fenix/aircraft.cfg")),
        "installed after the patch"
    );
    assert!(!fx.tool().exists());
    assert!(!fx.item.marker().exists());
    let retained = glob(&fx.item.state(), "retained-");
    assert_eq!(
        retained
            .iter()
            .filter(|path| path.join("pfx/new-user-state").exists())
            .count(),
        1
    );
    assert!(fx.proton.is_dir());
}

#[test]
fn unsupported_proton_changes_nothing() {
    let fx = Fixture::new();
    put(&fx.proton.join("files/bin/wine"), "an updated Proton");
    let before = tree(&fx.item.compat());
    assert_patch_error(fx.install(&fx.item), "No supported Proton");
    assert_eq!(tree(&fx.item.compat()), before);
    assert!(!fx.tool().exists());
    fx.restore(&fx.item).unwrap();
    assert!(!fx.item.marker().exists());
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
}

#[test]
fn failed_setup_keeps_original_profile_and_restore_cleans_up() {
    let fx = Fixture::new();
    fx.mock
        .geometry
        .fail(Some(Error::patch("synthetic failure")));
    assert_patch_error(fx.install(&fx.item), "synthetic failure");
    assert_eq!(read(&fx.item.compat().join("version")), "11.0-100");
    assert_eq!(
        read(&fx.settings.join("Packages/Community/addon/layout.json")),
        "large simulator content"
    );
    assert_eq!(
        read(&fx.tool().join("files/bin/wineserver")),
        "old files/bin/wineserver"
    );
    assert_patch_error(fx.install(&fx.item), "Restore");
    assert_patch_error(fx.configure(&fx.item), "Finish patch installation");
    fx.restore(&fx.item).unwrap();
    assert!(!fx.tool().exists());
    assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
    fx.mock.geometry.fail(None);
    fx.install(&fx.item).unwrap();
    assert_eq!(fx.state(&fx.item)["state"], "installed");
}

#[test]
fn recovery_at_each_commit_step() {
    let fx = Fixture::new();
    for failing in [1, 2, 3] {
        let mut calls = 0;
        *fx.mock.rename.lock().unwrap() = Some(Box::new(move |_: &Path, _: &Path| {
            calls += 1;
            (calls == failing).then(|| Error::os_text("synthetic power loss"))
        }));
        assert!(fx.install(&fx.item).unwrap_err().is_os(), "step {failing}");
        *fx.mock.rename.lock().unwrap() = None;
        assert_eq!(fx.state(&fx.item)["state"], "committing");
        // Found by its patch state even while no profile is in place.
        let found = steam::target(fx.mock.as_ref(), "msfs2024", Some(fx.root.as_os_str())).unwrap();
        assert_eq!(found.compat(), fx.item.compat());
        assert_eq!(fx.snapshot(&found)["state"], "committing");
        fx.restore(&found).unwrap();
        assert_eq!(read(&fx.item.compat().join("version")), "11.0-100");
        assert_eq!(read(&fx.prefix.join("system.reg")), "original registry");
        assert_eq!(
            read(&fx.settings.join("Packages/Community/addon/layout.json")),
            "large simulator content"
        );
        assert!(!fx.tool().exists());
    }
}

#[test]
fn second_simulator_shares_the_tool_until_both_are_restored() {
    let fx = Fixture::new();
    let other = fx.other();
    fx.install(&fx.item).unwrap();
    put(
        &fx.proton.join("files/bin/wine"),
        "Steam updated this Proton meanwhile",
    );
    fx.install(&other).unwrap();
    assert_eq!(fx.mock.setups.lock().unwrap().len(), 2);
    assert_eq!(fx.state(&other)["tool_created"], false);
    fx.restore(&fx.item).unwrap();
    assert!(fx.tool().is_dir());
    steam::verify_installed(fx.mock.as_ref(), &other, &fx.state(&other)).unwrap();
    fx.restore(&other).unwrap();
    assert!(!fx.tool().exists());
}

#[test]
fn changed_tool_and_running_profile_are_rejected() {
    let fx = Fixture::new();
    fx.install(&fx.item).unwrap();
    *fx.mock.idle.lock().unwrap() = Some(Box::new(|_: &Path| Err(Error::patch("Close MSFS"))));
    assert_patch_error(fx.configure(&fx.item), "Close MSFS");
    *fx.mock.idle.lock().unwrap() = None;
    put(&fx.tool().join("files/bin/wineserver"), "tampered");
    assert_patch_error(fx.configure(&fx.item), "differs from the supported build");
    assert_patch_error(fx.windows_app(), "differs from the supported build");
}

#[test]
fn configure_registers_autostart_in_the_matching_simulator_folder() {
    let fx = Fixture::new();
    fx.install(&fx.item).unwrap();
    put(&fx.prefix.join(core::PROGRAM).join("Fenix.exe"), "MZ");
    assert_patch_error(fx.configure(&fx.item), "Start Fenix once and sign in");
    assert_eq!(fx.state(&fx.item)["configured"], false);
    put(
        &fx.prefix.join(core::CONFIG).join("fenixConfig.xml"),
        "<config><displayMode>GPU</displayMode></config>",
    );
    put(
        &fx.prefix.join(core::CONFIG).join("persistancy.xml"),
        "<state/>",
    );
    fx.configure(&fx.item).unwrap();
    let launch = document(&fx.settings.join("exe.xml"));
    let entry = launch.find("Launch.Addon").unwrap();
    assert_eq!(
        entry.findtext("Path"),
        r"C:\Program Files\FenixSim A320\deps\FenixBootstrapper.exe"
    );
    assert_eq!(
        document(&fx.prefix.join(core::CONFIG).join("fenixConfig.xml")).findtext("displayMode"),
        "CPU"
    );
    assert_eq!(fx.state(&fx.item)["configured"], true);
    assert_eq!(fx.state(&fx.item)["readouts"], true);
    let status = fx.snapshot(&fx.item);
    assert!(status["installed"] == true && status["fenix_installed"] == true);
    assert_eq!(status["tool_selected"], false);
    put(&fx.config(), &config());
    fx.select_tool().unwrap();
    assert_eq!(fx.snapshot(&fx.item)["tool_selected"], true);
}

#[test]
fn selection_changes_only_this_simulator_and_restore_undoes_it() {
    let fx = Fixture::new();
    fx.install(&fx.item).unwrap();
    put(&fx.config(), &config());
    fx.mock.steam_running.store(true, Ordering::SeqCst);
    assert_patch_error(fx.select_tool(), "Quit Steam");
    fx.mock.steam_running.store(false, Ordering::SeqCst);
    assert_eq!(read(&fx.config()), config());
    fx.select_tool().unwrap();
    let text = read(&fx.config());
    assert_eq!(
        vdf::compat_tool(&text, "2537590").unwrap().as_deref(),
        Some("Proton-Fenix-A320-fixture")
    );
    assert_eq!(
        vdf::compat_tool(&text, "440").unwrap().as_deref(),
        Some("proton_experimental")
    );
    assert_eq!(text.replace(ENTRY, ""), config());
    let saved: Vec<String> = glob(&fx.item.state(), "steam-config-before-")
        .iter()
        .map(|path| read(path))
        .collect();
    assert_eq!(saved, [config()]);
    fx.restore(&fx.item).unwrap();
    assert_eq!(read(&fx.config()), config());
}

#[test]
fn restore_returns_a_previously_forced_tool() {
    let fx = Fixture::new();
    fx.install(&fx.item).unwrap();
    let before = vdf::set_compat_tool(&config(), "2537590", Some("proton_experimental")).unwrap();
    put(&fx.config(), &before);
    fx.select_tool().unwrap();
    assert_eq!(
        steam::selected_tool(&fx.item).as_deref(),
        Some("Proton-Fenix-A320-fixture")
    );
    fx.mock.steam_running.store(true, Ordering::SeqCst);
    let messages = Messages::default();
    steam::restore(fx.mock.as_ref(), &fx.item, &|message| {
        messages.push(message)
    })
    .unwrap();
    // Steam was running: its configuration is left alone and the user is told.
    assert!(messages.last().contains("Properties → Compatibility"));
    assert_eq!(
        steam::selected_tool(&fx.item).as_deref(),
        Some("Proton-Fenix-A320-fixture")
    );
}

#[test]
fn update_replaces_the_overlay_and_keeps_the_profile() {
    let fx = Fixture::new();
    fx.install(&fx.item).unwrap();
    put(&fx.prefix.join(core::PROGRAM).join("Fenix.exe"), "MZ");
    put(
        &fx.prefix.join(core::CONFIG).join("fenixConfig.xml"),
        "<config/>",
    );
    put(
        &fx.prefix.join(core::CONFIG).join("persistancy.xml"),
        "<state/>",
    );
    fx.configure(&fx.item).unwrap();
    put(&fx.config(), &config());
    fx.select_tool().unwrap();
    let target = targets::Steam::new(fx.system(), fx.item.clone());
    assert_eq!(guide::overview(&target).unwrap()["complete"], true);
    let before = fx.state(&fx.item);
    fx.release();
    assert_eq!(fx.snapshot(&fx.item)["update_available"], true);
    let view = guide::overview(&target).unwrap();
    assert_eq!(
        (
            view["current"].as_str(),
            view["steps"][0]["button"].as_str()
        ),
        (Some("install"), Some("Update patch"))
    );
    assert!(
        view["steps"][0]["text"]
            .as_str()
            .unwrap()
            .contains("fixture-next")
    );
    assert_patch_error(fx.configure(&fx.item), "Update it to fixture-next first");
    // A tool that is not the installed release is never overwritten.
    let original = read(&fx.tool().join("files/bin/wine"));
    put(&fx.tool().join("files/bin/wine"), "tampered");
    assert_patch_error(fx.install(&fx.item), "differs from the supported build");
    assert_eq!(fx.state(&fx.item), before);
    assert_eq!(
        read(&fx.tool().join("files/bin/wineserver")),
        "new files/bin/wineserver"
    );
    put(&fx.tool().join("files/bin/wine"), &original);
    let messages = Messages::default();
    guide::perform(
        fx.mock.as_ref(),
        &target,
        "install",
        fx.bundle.as_os_str(),
        &|message| messages.push(message),
        None,
        None,
    )
    .unwrap();
    assert!(messages.last().contains("Patch updated"));
    assert_eq!(
        read(&fx.tool().join("files/bin/wineserver")),
        "newer files/bin/wineserver"
    );
    assert_eq!(
        fx.tool_state(),
        json!({"format": 1, "state": "ready", "variant": "fixture", "version": "fixture-next"})
    );
    let state = fx.state(&fx.item);
    let mut reverted = state.clone();
    reverted["version"] = json!("fixture");
    let mut expected = before.clone();
    expected["updated_from"] = json!("fixture");
    assert_eq!(reverted, expected);
    assert_eq!(state["version"], "fixture-next");
    // No new profile, no second .NET setup, and the settings stay applied.
    assert_eq!(fx.mock.setups.lock().unwrap().len(), 1);
    assert!(fx.prefix.join(core::PROGRAM).join("Fenix.exe").is_file());
    assert!(
        fx.item
            .state()
            .join(state["previous"].as_str().unwrap())
            .join("pfx/system.reg")
            .is_file()
    );
    let view = guide::overview(&target).unwrap();
    assert!(view["complete"] == true && view["update"] == false);
    fx.configure(&fx.item).unwrap();
    let messages = Messages::default();
    steam::install(
        fx.mock.as_ref(),
        &fx.item,
        fx.bundle.as_os_str(),
        &|message| messages.push(message),
        None,
    )
    .unwrap();
    assert_eq!(messages.all(), ["This patch version is already installed."]);
}

#[test]
fn update_of_a_shared_tool_reaches_both_simulators() {
    let fx = Fixture::new();
    let other = fx.other();
    fx.install(&fx.item).unwrap();
    fx.install(&other).unwrap();
    fx.release();
    // The tool is replaced for both, so the other simulator must be closed as well.
    let busy = other.prefix();
    *fx.mock.idle.lock().unwrap() = Some(Box::new(move |prefix: &Path| {
        if prefix == busy {
            Err(Error::patch("Close MSFS 2020"))
        } else {
            Ok(())
        }
    }));
    assert_patch_error(fx.install(&fx.item), "Close MSFS 2020");
    *fx.mock.idle.lock().unwrap() = None;
    assert_eq!(fx.tool_state()["version"], "fixture");
    fx.install(&fx.item).unwrap();
    assert_eq!(
        read(&fx.tool().join("files/bin/wineserver")),
        "newer files/bin/wineserver"
    );
    // The other simulator now runs the new tool; its own record follows with its update.
    assert_eq!(fx.snapshot(&other)["update_available"], true);
    assert_patch_error(fx.configure(&other), "Update it to fixture-next first");
    fx.install(&other).unwrap();
    assert_eq!(fx.state(&other)["version"], "fixture-next");
    steam::verify_installed(fx.mock.as_ref(), &fx.item, &fx.state(&fx.item)).unwrap();
    steam::verify_installed(fx.mock.as_ref(), &other, &fx.state(&other)).unwrap();
    assert_eq!(fx.mock.setups.lock().unwrap().len(), 2);
}

#[test]
fn update_finishes_after_an_interruption_and_rejects_unknown_releases() {
    let fx = Fixture::new();
    fx.install(&fx.item).unwrap();
    fx.release();
    // Interrupted while the tool was being rewritten: one file new, the marker says so.
    let name = "files/bin/wineserver";
    fs::write(
        fx.tool().join(name),
        read(&fx.bundle.join("payload/variants/fixture").join(name)),
    )
    .unwrap();
    let mut marker = fx.tool_state();
    marker["state"] = json!("updating");
    marker["version"] = json!("fixture-next");
    fsx::write_json(&fx.tool().join(steam::TOOL_MARKER), &marker).unwrap();
    assert_patch_error(fx.windows_app(), "Update it to fixture-next first");
    fx.install(&fx.item).unwrap();
    assert_eq!(fx.state(&fx.item)["version"], "fixture-next");
    assert_eq!(fx.tool_state()["state"], "ready");
    let mut ancient = fx.state(&fx.item);
    ancient["version"] = json!("ancient");
    fsx::write_json(&fx.item.marker(), &ancient).unwrap();
    assert_eq!(fx.snapshot(&fx.item)["update_available"], false);
    assert_patch_error(fx.install(&fx.item), "Restore it before reinstalling");
    assert_patch_error(
        fx.configure(&fx.item),
        "not supported by the current installer",
    );
}

#[test]
fn guide_leads_through_the_steps_in_order() {
    let fx = Fixture::new();
    let target = targets::Steam::new(fx.system(), fx.item.clone());
    put(&fx.config(), &config());
    let current = || {
        let view = guide::overview(&target).unwrap();
        let done: Vec<String> = view["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|step| step["done"] == true)
            .map(|step| step["id"].as_str().unwrap().to_string())
            .collect();
        (view["current"].as_str().map(str::to_string), done)
    };
    let steps = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    assert_eq!(current(), (Some("install".into()), steps(&[])));
    let messages = Messages::default();
    let perform = |action: &str| {
        guide::perform(
            fx.mock.as_ref(),
            &target,
            action,
            fx.bundle.as_os_str(),
            &|message| messages.push(message),
            None,
            None,
        )
    };
    perform("install").unwrap();
    assert_eq!(current(), (Some("select".into()), steps(&["install"])));
    perform("select").unwrap();
    assert_eq!(
        current(),
        (Some("installer".into()), steps(&["install", "select"]))
    );
    assert_patch_error(perform("installer"), "Choose the Fenix Installer");
    put(&fx.prefix.join(core::PROGRAM).join("Fenix.exe"), "MZ");
    assert_eq!(current().0.as_deref(), Some("open"));
    // Signing in creates fenixConfig.xml. persistancy.xml only follows
    // once the aircraft has been loaded, which needs the next step first.
    put(
        &fx.prefix.join(core::CONFIG).join("fenixConfig.xml"),
        "<config/>",
    );
    assert_eq!(current().0.as_deref(), Some("configure"));
    perform("configure").unwrap();
    assert!(messages.last().contains("load the aircraft once"));
    assert_eq!(
        current(),
        (
            Some("readouts".into()),
            steps(&["install", "select", "installer", "open", "configure"])
        )
    );
    assert!(fx.settings.join("exe.xml").is_file());
    let view = guide::overview(&target).unwrap();
    assert_eq!(view["complete"], false);
    assert!(
        view["steps"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("from Steam")
    );
    perform("readouts").unwrap();
    assert_eq!(current().0.as_deref(), Some("readouts"));
    put(
        &fx.prefix.join(core::CONFIG).join("persistancy.xml"),
        "<state/>",
    );
    perform("readouts").unwrap();
    assert_eq!(
        document(&fx.prefix.join(core::CONFIG).join("persistancy.xml")).findtext("fcuReadoutsType"),
        "0"
    );
    let view = guide::overview(&target).unwrap();
    assert!(
        !view["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["id"] == "readouts")
    );
    assert!(view["complete"] == true && view["can_restore"] == true);
    assert!(view["finish"].as_str().unwrap().contains("from Steam"));
    let mut state = fx.state(&fx.item);
    state["state"] = json!("committing");
    fsx::write_json(&fx.item.marker(), &state).unwrap();
    let view = guide::overview(&target).unwrap();
    assert!(view["broken"] == true && view["current"].is_null() && view["can_restore"] == true);
    assert!(view["message"].as_str().unwrap().contains("Restore"));
}

#[test]
fn installation_needs_a_display() {
    let fx = Fixture::new();
    fx.mock.check_display.store(true, Ordering::SeqCst);
    assert!(fx.mock.var("DISPLAY").is_none() && fx.mock.var("WAYLAND_DISPLAY").is_none());
    assert_patch_error(fx.install(&fx.item), "desktop session");
    assert!(!fx.item.marker().exists());
}

#[test]
fn preferred_proton_wins_over_another_installed_match() {
    let fx = Fixture::new();
    put(&fx.proton.join("version"), "other version");
    let preferred = fx.root.join("compatibilitytools.d/a-newer-proton");
    for (relative, content) in fx.files(&fx.proton) {
        let suffix = if fsx::name(&relative) == "wine" {
            " 11"
        } else {
            ""
        };
        put(&preferred.join(&relative), &format!("{content}{suffix}"));
    }
    put(&preferred.join("version"), "preferred version");
    let mut lock = fx.mock.lock();
    let entry = lock["variants"]["fixture"].clone();
    let mut older = entry.clone();
    older["runner_version"] = json!("1 other version");
    let mut newer = entry.clone();
    newer["runner_version"] = json!("1 preferred version");
    newer["runner_files"]["files/bin/wine"] = json!(digest(&preferred.join("files/bin/wine")));
    lock["variants"] = json!({"older": older, "newer": newer});
    fx.publish(&lock);
    for name in FILES {
        for variant in ["older", "newer"] {
            put(
                &fx.bundle.join("payload/variants").join(variant).join(name),
                &format!("new {name}"),
            );
        }
    }
    *fx.mock.downloads.lock().unwrap() = vec![steam::ProtonDownload {
        variant: "newer".into(),
        url: "https://invalid.example/x.tar.xz".into(),
        sha256: "0".into(),
        folder: "x".into(),
    }];
    fx.install(&fx.item).unwrap();
    assert_eq!(fx.state(&fx.item)["variant"], "newer");
}

#[test]
fn symlinked_profile_is_rejected() {
    let fx = Fixture::new();
    let moved = fx.base.join("elsewhere");
    fs::rename(fx.item.compat(), &moved).unwrap();
    symlink(&moved, fx.item.compat()).unwrap();
    assert_patch_error(fx.install(&fx.item), "ordinary directories");
}

const ENTRY: &str = "\t\t\t\t\t\"2537590\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"Proton-Fenix-A320-fixture\"\n\
\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n";

/// The configuration of the Python tests, written with spaces for tabs.
fn config() -> String {
    r#""InstallConfigStore"
{
 "Software"
 {
  "Valve"
  {
   "Steam"
   {
    "Accounts"
    {
     "someone"
     {
      "SteamID"		"1"
     }
    }
    "CompatToolMapping"
    {
     "440"
     {
      "name"		"proton_experimental"
      "config"		""
      "priority"		"250"
     }
    }
    "quoted"		"a \"brace\" { inside"
   }
  }
 }
 "WebStorage"
 {
 }
}
"#
    .lines()
    .map(|line| {
        let indent = line.len() - line.trim_start_matches(' ').len();
        format!("{}{}\n", "\t".repeat(indent), &line[indent..])
    })
    .collect()
}

#[test]
fn mapping_is_created_replaced_and_removed_without_touching_anything_else() {
    let config = config();
    let (start, end) = (
        config.find("\t\t\t\t\"CompatToolMapping\"").unwrap(),
        config.find("\t\t\t\t\"quoted\"").unwrap(),
    );
    let bare = config.replace(&config[start..end], "");
    assert_eq!(vdf::compat_tool(&bare, "440").unwrap(), None);
    let created = vdf::set_compat_tool(&bare, "1250410", Some("Tool A")).unwrap();
    assert_eq!(
        vdf::compat_tool(&created, "1250410").unwrap().as_deref(),
        Some("Tool A")
    );
    let vdf::Plain::Block(top) = &vdf::plain(&vdf::parse(&created).unwrap())[0].1 else {
        panic!("not a block")
    };
    assert_eq!(top[1].0, "WebStorage");
    let replaced = vdf::set_compat_tool(&created, "1250410", Some("Tool B")).unwrap();
    assert_eq!(
        vdf::compat_tool(&replaced, "1250410").unwrap().as_deref(),
        Some("Tool B")
    );
    assert_eq!(replaced.len(), created.len());
    assert_eq!(
        vdf::set_compat_tool(
            &vdf::set_compat_tool(&config, "1250410", Some("Tool A")).unwrap(),
            "1250410",
            None
        )
        .unwrap(),
        config
    );
    assert_eq!(
        vdf::set_compat_tool(&config, "1250410", None).unwrap(),
        config
    );
    assert_eq!(
        vdf::set_compat_tool(&created, "1250410", None).unwrap(),
        bare
    );
}

#[test]
fn unknown_layouts_and_names_are_rejected() {
    let config = config();
    for text in [
        "\"Other\"\n{\n}\n",
        &config[..config.len() - 3],
        &config.replace("\"Valve\"", "Valve"),
        "",
    ] {
        assert!(
            vdf::set_compat_tool(text, "1250410", Some("Tool"))
                .unwrap_err()
                .is_patch(),
            "{text:.20}"
        );
    }
    for tool in ["bad\"name", "bad\nname", ""] {
        assert!(
            vdf::set_compat_tool(&config, "1250410", Some(tool))
                .unwrap_err()
                .is_patch(),
            "{tool}"
        );
    }
    assert!(
        vdf::set_compat_tool(&config, "12x", Some("Tool"))
            .unwrap_err()
            .is_patch()
    );
}

#[test]
fn the_test_configuration_is_the_one_of_the_python_tests() {
    // Guards the tab conversion above against the original text.
    let config = config();
    assert!(config.starts_with("\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n"));
    assert!(config.contains("\t\t\t\t\t\t\"SteamID\"\t\t\"1\"\n"));
    assert!(config.contains("\t\t\t\t\"quoted\"\t\t\"a \\\"brace\\\" { inside\"\n"));
    assert!(config.ends_with("\t\"WebStorage\"\n\t{\n\t}\n}\n"));
    assert_eq!(OsStr::new(&config).len(), 356);
}
