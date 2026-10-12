// SPDX-License-Identifier: MIT
//! The installer's contact points with the machine it runs on.
//!
//! Everything goes through [`System`], whose defaults do the real work. Tests
//! replace single steps, as the Python tests did with `unittest.mock.patch`.
use crate::{
    core::{self, Wine},
    download,
    error::Result,
    fsx,
    json::Value,
    process::{self, Cmd, Env},
    steam,
};
use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Receives each progress message; a failure to deliver it ends the operation.
pub type Progress<'a> = &'a (dyn Fn(&str) -> Result<()> + Sync);

/// A progress receiver that discards every message.
pub fn silent(_: &str) -> Result<()> {
    Ok(())
}

pub trait AsSystem {
    fn as_system(&self) -> &dyn System;
}

impl<T: System> AsSystem for T {
    fn as_system(&self) -> &dyn System {
        self
    }
}

pub trait System: AsSystem + Send + Sync {
    /// The directory of the installer and its own `bundle.json`.
    fn root(&self) -> &Path;

    /// Fails with [`crate::Error::Interrupted`] once after Ctrl+C.
    fn interrupted(&self) -> Result<()> {
        Ok(())
    }

    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn environ(&self) -> Env {
        Env(std::env::vars_os().collect())
    }

    fn home(&self) -> PathBuf {
        fsx::home(self.var("HOME"))
    }

    fn manifest(&self, variant: Option<&str>) -> Result<Value> {
        core::read_manifest(self.root(), variant)
    }

    fn host_check(&self) -> Result<()> {
        core::host_check(self.as_system())
    }

    /// Fetch a pinned file over HTTPS unless it is already there.
    fn download(
        &self,
        url: &str,
        destination: &Path,
        expected: &str,
        max_size: u64,
    ) -> Result<PathBuf> {
        download::download(self.as_system(), url, destination, expected, max_size)
    }

    fn wine<'a>(&'a self, prefix: &Path, runner: &Path, log: &'a File) -> Box<dyn Wine + 'a> {
        Box::new(core::RealWine::new(self.as_system(), prefix, runner, log))
    }

    fn prepare_framework(
        &self,
        wine: &dyn Wine,
        cache: &Path,
        progress: Progress<'_>,
        first_version: &str,
    ) -> Result<()> {
        core::prepare_framework(self.as_system(), wine, cache, progress, first_version)
    }

    fn graphics_and_fonts(&self, prefix: &Path, runner: &Path, wine: &dyn Wine) -> Result<()> {
        core::graphics_and_fonts(prefix, runner, wine)
    }

    fn prepare_geometry(
        &self,
        wine: &dyn Wine,
        cache: &Path,
        bundle: &Path,
        progress: Progress<'_>,
    ) -> Result<()> {
        core::prepare_geometry(self.as_system(), wine, cache, bundle, progress)
    }

    fn ensure_idle(&self, prefix: &Path) -> Result<()> {
        core::ensure_idle(prefix)
    }

    /// `os.rename` at the points where an installation is committed or undone.
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        fsx::rename(from, to)
    }

    fn replace_link(&self, path: &Path, target: &Path) -> Result<()> {
        core::replace_link(path, target)
    }

    /// Start an official Fenix program.
    fn spawn_app(&self, cmd: &Cmd<'_>) -> Result<Child> {
        process::spawn(cmd)
    }

    fn proton_setup(
        &self,
        tool: &Path,
        compat: &Path,
        item: &steam::Target,
        log: &File,
    ) -> Result<()> {
        steam::proton_setup(self.as_system(), tool, compat, item, log)
    }

    fn proton_downloads(&self) -> Vec<steam::ProtonDownload> {
        steam::proton_downloads()
    }

    fn steam_running(&self) -> bool {
        steam::steam_running()
    }

    fn require_display(&self) -> Result<()> {
        steam::require_display(self.as_system())
    }
}

/// The machine as it is.
#[derive(Debug, Clone)]
pub struct Host {
    root: PathBuf,
    interrupt: Option<Arc<AtomicBool>>,
}

impl Host {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            interrupt: None,
        }
    }

    /// Stop at the next opportunity once `flag` is set, as Python did on `KeyboardInterrupt`.
    pub fn with_interrupt(mut self, flag: Arc<AtomicBool>) -> Self {
        self.interrupt = Some(flag);
        self
    }

    /// The directory that contains the running program.
    pub fn program_directory() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|program| program.parent().map(Path::to_path_buf))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

impl System for Host {
    fn root(&self) -> &Path {
        &self.root
    }

    fn interrupted(&self) -> Result<()> {
        match &self.interrupt {
            // The flag is taken, so that the clean-up that follows runs undisturbed.
            Some(flag) if flag.swap(false, Ordering::SeqCst) => Err(crate::Error::Interrupted),
            _ => Ok(()),
        }
    }
}
