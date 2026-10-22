// SPDX-License-Identifier: MIT
//! The modern Windows Desktop runtime required by the Fenix v3 installer.
//! This is separate from the aircraft's .NET Framework 4.8 prerequisite.
use crate::{
    Error, Result, argv,
    core::{RunOptions, Wine},
    download,
    system::{Progress, System},
};
use std::{fs, path::Path};

// Microsoft release metadata, 2026-10-13:
// https://builds.dotnet.microsoft.com/dotnet/release-metadata/10.0/releases.json
// The download was checked against that document's SHA-512 before pinning SHA-256.
pub const PACKAGE: &str = "windowsdesktop-runtime-10.0.13-win-x64.exe";
pub const URL: &str = "https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/10.0.13/windowsdesktop-runtime-10.0.13-win-x64.exe";
pub const SHA256: &str = "f5f0268b3d89ec3b9eb952b620330360b35c63593ead45816aeb26e934a4b7b6";

fn version(name: &str) -> bool {
    let mut parts = name.split('.');
    parts.next() == Some("10")
        && parts.next() == Some("0")
        && parts
            .next()
            .is_some_and(|patch| !patch.is_empty() && patch.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none()
}

/// Require the x64 host, core runtime and Desktop framework, not just an
/// installer registry entry or an unrelated .NET 8 / .NET Framework directory.
pub fn installed(prefix: &Path) -> Result<bool> {
    let dotnet = prefix.join("drive_c/Program Files/dotnet");
    if !dotnet.join("dotnet.exe").is_file() {
        return Ok(false);
    }
    let entries = match fs::read_dir(dotnet.join("shared/Microsoft.WindowsDesktop.App")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::io_path(&error, &dotnet)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| Error::io_path(&error, &dotnet))?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| version(name)) else {
            continue;
        };
        if ["PresentationFramework.dll", "WindowsBase.dll"]
            .iter()
            .all(|file| entry.path().join(file).is_file())
            && ["coreclr.dll", "System.Private.CoreLib.dll"]
                .iter()
                .all(|file| {
                    dotnet
                        .join("shared/Microsoft.NETCore.App")
                        .join(name)
                        .join(file)
                        .is_file()
                })
            && dotnet
                .join("host/fxr")
                .join(name)
                .join("hostfxr.dll")
                .is_file()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn prepare(
    system: &dyn System,
    wine: &dyn Wine,
    cache: &Path,
    progress: Progress<'_>,
) -> Result<()> {
    if installed(wine.prefix())? {
        return Ok(());
    }
    progress("Downloading Microsoft .NET 10 Desktop Runtime for the Fenix Installer …")?;
    let package = system.download(URL, &cache.join(PACKAGE), SHA256, download::DEFAULT_LIMIT)?;
    progress("Installing Microsoft .NET 10 Desktop Runtime. This can take several minutes …")?;
    wine.run(
        &argv![&package, "/install", "/quiet", "/norestart"],
        &RunOptions {
            accepted: &[0, 3010],
            ..RunOptions::default()
        },
    )?;
    if !installed(wine.prefix())? {
        return Err(Error::patch(
            "Microsoft .NET 10 Desktop Runtime did not finish installing. See the private Fenix setup log and retry before opening Fenix.",
        ));
    }
    progress("Microsoft .NET 10 Desktop Runtime is installed.")
}
