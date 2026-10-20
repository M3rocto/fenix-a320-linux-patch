// SPDX-License-Identifier: MIT
//! What the Python tests did with `unittest.mock.patch`: single steps of the
//! installer replaced, everything else real.
#![allow(dead_code, clippy::unwrap_used)]
use fenix_patch::{
    Error, Result,
    core::{self, RunOptions, Wine},
    fsx,
    json::{self, Value},
    process::{self, Cmd, Env},
    steam,
    system::{Host, Progress, System},
};
use std::{
    ffi::OsString,
    fs::{self, File},
    path::{Path, PathBuf},
    process::Child,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

/// One fixture at a time. A helper program started by another test holds every
/// open file for an instant, including lock files, which then seem to be in use.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn put(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

pub fn digest(path: &Path) -> String {
    fsx::digest(path).unwrap()
}

/// `{name: digest(base / name)}` in the given order.
pub fn digests(base: &Path, names: &[&str]) -> Value {
    let mut map = json::Map::new();
    for name in names {
        map.insert(name.to_string(), Value::String(digest(&base.join(name))));
    }
    Value::Object(map)
}

/// A complete synthetic owner-adapter overlay; its bytes are still checked by
/// the real installer paths, just like any other pinned payload.
pub fn gpu_overlay(payload: &Path, entry: &mut Value) {
    for name in core::GPU_FILES {
        let path = payload.join(name);
        if !path.exists() {
            put(&path, &format!("synthetic GPU {name}"));
        }
        entry["files"][name] = Value::String(digest(&path));
    }
    entry["dxvk"] = serde_json::json!({
        "source": "fixture-dxvk.tar.gz",
        "revision": "a".repeat(40),
    });
    entry["display_renderer"] = Value::String("GPU".to_string());
    entry["prefix_files"]["drive_c/windows/system32/dxgi.dll"] =
        entry["files"][core::DXGI_FILE].clone();
}

/// Paths below `base` whose name matches `prefix*`, as `Path.glob` in the Python tests.
pub fn glob(base: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(base)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| fsx::name(path).starts_with(prefix))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// How `core.manifest` is replaced.
pub enum Manifest {
    /// Not replaced: read from the installer's directory.
    Real,
    /// `patch.object(core, "manifest", return_value=lock)`.
    Fixed(Value),
    /// The `side_effect` of the Steam tests: a variant's entries on top of the lock.
    Variants(Value),
}

/// One replaced step: how often it ran, and an error it shall raise.
#[derive(Default)]
pub struct Step {
    pub calls: AtomicUsize,
    pub error: Mutex<Option<Error>>,
}

impl Step {
    fn call(&self) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.calls.store(0, Ordering::SeqCst);
    }

    pub fn fail(&self, error: Option<Error>) {
        *self.error.lock().unwrap() = error;
    }
}

/// Calls made on the replaced `core.Wine`.
#[derive(Default)]
pub struct WineCalls {
    pub run: Mutex<Vec<Vec<String>>>,
    pub reg: Mutex<Vec<Vec<String>>>,
    pub stop: AtomicUsize,
}

pub struct MockWine {
    prefix: PathBuf,
    runner: PathBuf,
    env: Env,
    calls: Arc<WineCalls>,
}

impl MockWine {
    pub fn new(prefix: &Path, runner: &Path, calls: Arc<WineCalls>) -> Self {
        Self {
            prefix: prefix.to_path_buf(),
            runner: runner.to_path_buf(),
            env: Env::default(),
            calls,
        }
    }
}

impl Wine for MockWine {
    fn prefix(&self) -> &Path {
        &self.prefix
    }

    fn runner(&self) -> &Path {
        &self.runner
    }

    fn env(&self) -> &Env {
        &self.env
    }

    fn run(&self, args: &[OsString], _options: &RunOptions<'_>) -> Result<()> {
        self.calls.run.lock().unwrap().push(
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        );
        Ok(())
    }

    fn reg(&self, key: &str, name: &str, value: &str, kind: &str) -> Result<()> {
        self.calls
            .reg
            .lock()
            .unwrap()
            .push([key, name, value, kind].map(str::to_string).to_vec());
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        self.calls.stop.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

type RenameHook = Box<dyn FnMut(&Path, &Path) -> Option<Error> + Send>;
type IdleHook = Box<dyn Fn(&Path) -> Result<()> + Send>;

pub struct Mock {
    pub host: Host,
    pub manifest: Mutex<Manifest>,
    pub wine: Arc<WineCalls>,
    pub framework: Step,
    pub graphics: Step,
    pub geometry: Step,
    /// `patch.object(os, "rename", side_effect=...)`: an error to raise instead of renaming.
    pub rename: Mutex<Option<RenameHook>>,
    /// `patch.object(core, "replace_link", side_effect=...)`.
    pub replace_link: Mutex<Option<Error>>,
    /// `patch.object(core, "ensure_idle", side_effect=...)`.
    pub idle: Mutex<Option<IdleHook>>,
    /// `patch.dict(os.environ, ...)`; HOME always points into the test's directory.
    pub environ: Mutex<Env>,
    /// `patch.object(subprocess, "Popen", return_value=child)`.
    pub child: Mutex<Option<Child>>,
    pub spawned: Mutex<Vec<Vec<String>>>,
    /// `patch.object(steam, "proton_setup", side_effect=...)` and what it saw.
    pub mock_proton_setup: bool,
    pub setups: Mutex<Vec<(PathBuf, String)>>,
    /// `patch.object(steam, "PROTON_DOWNLOADS", ...)`.
    pub downloads: Mutex<Vec<steam::ProtonDownload>>,
    pub steam_running: AtomicBool,
    /// False while `steam.require_display` is replaced by a no-op.
    pub check_display: AtomicBool,
}

impl Mock {
    pub fn new(root: &Path, home: &Path, manifest: Manifest) -> Self {
        let mut environ = Env(std::env::vars_os()
            .filter(|(name, _)| {
                !matches!(
                    name.to_str(),
                    Some("HOME" | "XDG_DATA_HOME" | "DISPLAY" | "WAYLAND_DISPLAY")
                )
            })
            .collect());
        environ.set("HOME", home);
        environ.set("XDG_DATA_HOME", home.join("data"));
        Self {
            host: Host::new(root.to_path_buf()),
            manifest: Mutex::new(manifest),
            wine: Arc::new(WineCalls::default()),
            framework: Step::default(),
            graphics: Step::default(),
            geometry: Step::default(),
            rename: Mutex::new(None),
            replace_link: Mutex::new(None),
            idle: Mutex::new(None),
            environ: Mutex::new(environ),
            child: Mutex::new(None),
            spawned: Mutex::new(Vec::new()),
            mock_proton_setup: false,
            setups: Mutex::new(Vec::new()),
            downloads: Mutex::new(Vec::new()),
            steam_running: AtomicBool::new(false),
            check_display: AtomicBool::new(false),
        }
    }

    /// The lock the tests keep changing, as `self.lock` in Python.
    pub fn lock(&self) -> Value {
        match &*self.manifest.lock().unwrap() {
            Manifest::Fixed(lock) | Manifest::Variants(lock) => lock.clone(),
            Manifest::Real => self.host.manifest(None).unwrap(),
        }
    }

    pub fn set_lock(&self, lock: Value) {
        let mut manifest = self.manifest.lock().unwrap();
        *manifest = match &*manifest {
            Manifest::Fixed(_) => Manifest::Fixed(lock),
            Manifest::Variants(_) => Manifest::Variants(lock),
            Manifest::Real => Manifest::Real,
        };
    }
}

impl System for Mock {
    fn root(&self) -> &Path {
        self.host.root()
    }

    fn var(&self, name: &str) -> Option<OsString> {
        self.environ.lock().unwrap().get(name).cloned()
    }

    fn environ(&self) -> Env {
        self.environ.lock().unwrap().clone()
    }

    fn manifest(&self, variant: Option<&str>) -> Result<Value> {
        match &*self.manifest.lock().unwrap() {
            Manifest::Real => core::read_manifest(self.root(), variant),
            Manifest::Fixed(lock) => Ok(lock.clone()),
            Manifest::Variants(lock) => match variant {
                None => Ok(lock.clone()),
                Some(variant) => {
                    let entry = lock["variants"][variant]
                        .as_object()
                        .ok_or_else(|| Error::Key(variant.to_string()))?;
                    let mut merged = json::merged(lock.as_object().unwrap(), entry);
                    if !entry.contains_key("dxvk") {
                        merged.remove("dxvk");
                    }
                    if !entry.contains_key("display_renderer") {
                        merged.remove("display_renderer");
                    }
                    merged.insert("variant".to_string(), Value::String(variant.to_string()));
                    Ok(Value::Object(merged))
                }
            },
        }
    }

    fn host_check(&self) -> Result<()> {
        Ok(())
    }

    fn wine<'a>(&'a self, prefix: &Path, runner: &Path, _log: &'a File) -> Box<dyn Wine + 'a> {
        Box::new(MockWine::new(prefix, runner, self.wine.clone()))
    }

    fn prepare_framework(
        &self,
        _wine: &dyn Wine,
        _cache: &Path,
        _progress: Progress<'_>,
        _first_version: &str,
    ) -> Result<()> {
        self.framework.call()
    }

    fn graphics_and_fonts(&self, _prefix: &Path, _runner: &Path, _wine: &dyn Wine) -> Result<()> {
        self.graphics.call()
    }

    fn prepare_geometry(
        &self,
        _wine: &dyn Wine,
        _cache: &Path,
        _bundle: &Path,
        _progress: Progress<'_>,
    ) -> Result<()> {
        self.geometry.call()
    }

    fn ensure_idle(&self, prefix: &Path) -> Result<()> {
        match &*self.idle.lock().unwrap() {
            Some(hook) => hook(prefix),
            None => core::ensure_idle(prefix),
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        if let Some(hook) = self.rename.lock().unwrap().as_mut()
            && let Some(error) = hook(from, to)
        {
            return Err(error);
        }
        fsx::rename(from, to)
    }

    fn replace_link(&self, path: &Path, target: &Path) -> Result<()> {
        match self.replace_link.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => core::replace_link(path, target),
        }
    }

    fn spawn_app(&self, cmd: &Cmd<'_>) -> Result<Child> {
        self.spawned.lock().unwrap().push(
            cmd.argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        );
        match self.child.lock().unwrap().take() {
            Some(child) => Ok(child),
            None => process::spawn(&Cmd::new(vec!["true".into()])),
        }
    }

    fn proton_setup(
        &self,
        tool: &Path,
        compat: &Path,
        item: &steam::Target,
        log: &File,
    ) -> Result<()> {
        if !self.mock_proton_setup {
            return steam::proton_setup(self, tool, compat, item, log);
        }
        // Proton would settle its own profile version here.
        self.setups.lock().unwrap().push((
            tool.to_path_buf(),
            digest(&tool.join("files/bin/wineserver")),
        ));
        fs::write(compat.join("version"), "tool version").unwrap();
        Ok(())
    }

    fn proton_downloads(&self) -> Vec<steam::ProtonDownload> {
        self.downloads.lock().unwrap().clone()
    }

    fn steam_running(&self) -> bool {
        self.steam_running.load(Ordering::SeqCst)
    }

    fn require_display(&self) -> Result<()> {
        if self.check_display.load(Ordering::SeqCst) {
            steam::require_display(self)
        } else {
            Ok(())
        }
    }
}

/// `self.assertRaisesRegex(core.PatchError, text)`.
#[track_caller]
pub fn assert_patch_error<T: std::fmt::Debug>(result: Result<T>, text: &str) {
    match result {
        Err(Error::Patch(message)) if message.contains(text) => {}
        other => panic!("expected a patch error containing {text:?}, got {other:?}"),
    }
}

/// Collects progress messages, as `messages.append`.
#[derive(Default)]
pub struct Messages(pub Mutex<Vec<String>>);

impl Messages {
    pub fn push(&self, message: &str) -> Result<()> {
        self.0.lock().unwrap().push(message.to_string());
        Ok(())
    }

    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    pub fn last(&self) -> String {
        self.all().last().cloned().unwrap_or_default()
    }
}
