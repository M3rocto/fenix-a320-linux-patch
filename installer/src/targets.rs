// SPDX-License-Identifier: MIT
//! The two kinds of simulator installation behind one small interface.
use crate::{
    core,
    error::{Error, Result},
    json::Value,
    steam,
    system::{Progress, System},
};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
};

pub trait Target: Send + Sync {
    fn kind(&self) -> &str;
    fn label(&self) -> String;
    fn title(&self) -> String;
    /// The Windows profile of the simulator.
    fn prefix(&self) -> PathBuf;
    fn status(&self) -> Result<Value>;
    fn install(&self, bundle: &OsStr, progress: Progress<'_>, proton: Option<&OsStr>)
    -> Result<()>;
    fn configure(&self, progress: Progress<'_>) -> Result<()>;
    fn configure_with_renderer(
        &self,
        progress: Progress<'_>,
        renderer: Option<core::DisplayRenderer>,
    ) -> Result<()> {
        if renderer.is_some() {
            return Err(Error::patch(
                "This target cannot select a display renderer.",
            ));
        }
        self.configure(progress)
    }
    fn restore(&self, progress: Progress<'_>) -> Result<()>;
    fn app(&self, progress: Progress<'_>, executable: Option<&OsStr>, manager: bool) -> Result<()>;

    /// Steam only: force the Fenix Proton for the simulator.
    fn select(&self, _progress: Progress<'_>) -> Result<()> {
        Err(Error::other(
            "This simulator has no compatibility tool to select.",
        ))
    }
}

pub struct Flightdeck {
    system: Arc<dyn System>,
    pub runtime: OsString,
}

impl Flightdeck {
    pub fn new(system: Arc<dyn System>, runtime: &OsStr) -> Self {
        Self {
            system,
            runtime: runtime.to_os_string(),
        }
    }
}

impl Target for Flightdeck {
    fn kind(&self) -> &str {
        "flightdeck"
    }

    fn label(&self) -> String {
        format!("Flightdeck · {}", self.runtime.to_string_lossy())
    }

    fn title(&self) -> String {
        "MSFS 2024 in Flightdeck".to_string()
    }

    fn prefix(&self) -> PathBuf {
        crate::fsx::path(&self.runtime).join("local/msfs-prefix")
    }

    fn status(&self) -> Result<Value> {
        core::snapshot(self.system.as_ref(), &self.runtime)
    }

    fn install(
        &self,
        bundle: &OsStr,
        progress: Progress<'_>,
        _proton: Option<&OsStr>,
    ) -> Result<()> {
        core::install(self.system.as_ref(), &self.runtime, bundle, progress)
    }

    fn configure(&self, progress: Progress<'_>) -> Result<()> {
        core::configure(self.system.as_ref(), &self.runtime, progress)
    }

    fn configure_with_renderer(
        &self,
        progress: Progress<'_>,
        renderer: Option<core::DisplayRenderer>,
    ) -> Result<()> {
        core::configure_with_renderer(self.system.as_ref(), &self.runtime, progress, renderer)
    }

    fn restore(&self, progress: Progress<'_>) -> Result<()> {
        core::restore(self.system.as_ref(), &self.runtime, progress)
    }

    fn app(&self, progress: Progress<'_>, executable: Option<&OsStr>, manager: bool) -> Result<()> {
        core::windows_app(
            self.system.as_ref(),
            &self.runtime,
            executable,
            progress,
            manager,
            None,
        )
    }
}

pub struct Steam {
    system: Arc<dyn System>,
    pub item: steam::Target,
}

impl Steam {
    pub fn new(system: Arc<dyn System>, item: steam::Target) -> Self {
        Self { system, item }
    }
}

impl Target for Steam {
    fn kind(&self) -> &str {
        "steam"
    }

    fn label(&self) -> String {
        format!(
            "Steam · {} · {}",
            self.item.title(),
            self.item.library.to_string_lossy()
        )
    }

    fn title(&self) -> String {
        format!("{} on Steam", self.item.title())
    }

    fn prefix(&self) -> PathBuf {
        self.item.prefix()
    }

    fn status(&self) -> Result<Value> {
        steam::snapshot(self.system.as_ref(), &self.item)
    }

    fn install(
        &self,
        bundle: &OsStr,
        progress: Progress<'_>,
        proton: Option<&OsStr>,
    ) -> Result<()> {
        steam::install(self.system.as_ref(), &self.item, bundle, progress, proton)
    }

    fn configure(&self, progress: Progress<'_>) -> Result<()> {
        steam::configure(self.system.as_ref(), &self.item, progress)
    }

    fn configure_with_renderer(
        &self,
        progress: Progress<'_>,
        renderer: Option<core::DisplayRenderer>,
    ) -> Result<()> {
        steam::configure_with_renderer(self.system.as_ref(), &self.item, progress, renderer)
    }

    fn restore(&self, progress: Progress<'_>) -> Result<()> {
        steam::restore(self.system.as_ref(), &self.item, progress)
    }

    fn app(&self, progress: Progress<'_>, executable: Option<&OsStr>, manager: bool) -> Result<()> {
        steam::windows_app(
            self.system.as_ref(),
            &self.item,
            executable,
            progress,
            manager,
            None,
        )
    }

    fn select(&self, progress: Progress<'_>) -> Result<()> {
        steam::select_tool(self.system.as_ref(), &self.item, progress)
    }
}

fn given(value: Option<&OsStr>) -> Option<&OsStr> {
    value.filter(|value| !value.is_empty())
}

/// Steam simulators first, then an existing Flightdeck MSFS 2024 runtime.
pub fn detect(
    system: &Arc<dyn System>,
    steam_root: Option<&OsStr>,
) -> Result<Vec<Arc<dyn Target>>> {
    let mut found: Vec<Arc<dyn Target>> = Vec::new();
    for item in steam::targets(system.as_ref(), steam_root)? {
        found.push(Arc::new(Steam::new(system.clone(), item)));
    }
    let runtime = core::default_runtime(system.as_ref());
    if Path::new(&runtime).join("private/runtime.json").is_file() {
        found.push(Arc::new(Flightdeck::new(
            system.clone(),
            runtime.as_os_str(),
        )));
    }
    Ok(found)
}

pub fn select(
    system: &Arc<dyn System>,
    runtime: Option<&OsStr>,
    game: Option<&str>,
    steam_root: Option<&OsStr>,
) -> Result<Arc<dyn Target>> {
    let game = game.filter(|game| !game.is_empty());
    match (given(runtime), game) {
        (Some(_), Some(_)) => Err(Error::patch(
            "Use either --runtime (Flightdeck) or --steam, not both.",
        )),
        (Some(runtime), None) => Ok(Arc::new(Flightdeck::new(system.clone(), runtime))),
        (None, Some(game)) => Ok(Arc::new(Steam::new(
            system.clone(),
            steam::target(system.as_ref(), game, steam_root)?,
        ))),
        (None, None) => {
            let mut found = detect(system, steam_root)?;
            match found.len() {
                1 => Ok(found.remove(0)),
                0 => Err(Error::patch(
                    "No simulator was found. Start MSFS once in Steam, or pass --steam-root or --runtime.",
                )),
                _ => {
                    let labels: Vec<String> = found
                        .iter()
                        .map(|item| format!("  {}", item.label()))
                        .collect();
                    Err(Error::patch(format!(
                        "Several simulators were found. Choose one with --steam msfs2020, --steam msfs2024 or --runtime:\n{}",
                        labels.join("\n")
                    )))
                }
            }
        }
    }
}
