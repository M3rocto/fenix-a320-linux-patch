// SPDX-License-Identifier: MIT
#![allow(clippy::unwrap_used)]
use fenix_patch::{
    Error, Result,
    core::{RunOptions, Wine},
    desktop, fsx,
    process::Env,
    system::{System, silent},
};
use std::{
    cell::Cell,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

fn put(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "synthetic runtime").unwrap();
}

fn runtime(prefix: &Path, version: &str) {
    let dotnet = prefix.join("drive_c/Program Files/dotnet");
    put(&dotnet.join("dotnet.exe"));
    put(&dotnet.join("host/fxr").join(version).join("hostfxr.dll"));
    for name in ["coreclr.dll", "System.Private.CoreLib.dll"] {
        put(&dotnet
            .join("shared/Microsoft.NETCore.App")
            .join(version)
            .join(name));
    }
    for name in ["PresentationFramework.dll", "WindowsBase.dll"] {
        put(&dotnet
            .join("shared/Microsoft.WindowsDesktop.App")
            .join(version)
            .join(name));
    }
}

struct Host {
    root: PathBuf,
    downloads: AtomicUsize,
}

impl System for Host {
    fn root(&self) -> &Path {
        &self.root
    }

    fn download(&self, url: &str, path: &Path, expected: &str, limit: u64) -> Result<PathBuf> {
        assert_eq!(url, desktop::URL);
        assert_eq!(expected, desktop::SHA256);
        assert_eq!(path.file_name().unwrap(), desktop::PACKAGE);
        assert!(limit >= 60_056_984);
        self.downloads.fetch_add(1, Ordering::SeqCst);
        put(path);
        Ok(path.to_path_buf())
    }
}

struct Installer {
    prefix: PathBuf,
    env: Env,
    calls: Cell<usize>,
    writes_runtime: bool,
    fails: bool,
}

impl Wine for Installer {
    fn prefix(&self) -> &Path {
        &self.prefix
    }
    fn runner(&self) -> &Path {
        &self.prefix
    }
    fn env(&self) -> &Env {
        &self.env
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }

    fn run(&self, args: &[OsString], options: &RunOptions<'_>) -> Result<()> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(&args[1..], ["/install", "/quiet", "/norestart"]);
        assert_eq!(options.accepted, &[0, 3010]);
        if self.fails {
            return Err(Error::patch("desktop installer failed"));
        }
        if self.writes_runtime {
            runtime(&self.prefix, "10.0.13");
        }
        Ok(())
    }
}

#[test]
fn only_a_complete_stable_x64_desktop_10_runtime_satisfies_the_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let prefix = temp.path();
    assert!(!desktop::installed(prefix).unwrap());
    runtime(prefix, "8.0.31");
    runtime(prefix, "10.0.0-preview.1");
    assert!(!desktop::installed(prefix).unwrap());
    runtime(prefix, "10.0.0");
    assert!(desktop::installed(prefix).unwrap());
    let coreclr =
        prefix.join("drive_c/Program Files/dotnet/shared/Microsoft.NETCore.App/10.0.0/coreclr.dll");
    fs::remove_file(&coreclr).unwrap();
    assert!(!desktop::installed(prefix).unwrap());
    put(&coreclr);
    fs::remove_file(prefix.join("drive_c/Program Files/dotnet/host/fxr/10.0.0/hostfxr.dll"))
        .unwrap();
    assert!(!desktop::installed(prefix).unwrap());
    runtime(prefix, "10.0.99");
    assert!(desktop::installed(prefix).unwrap());
    // An x86-only runtime is not usable by the x64 installer.
    fs::rename(
        prefix.join("drive_c/Program Files"),
        prefix.join("drive_c/Program Files (x86)"),
    )
    .unwrap();
    assert!(!desktop::installed(prefix).unwrap());
}

#[test]
fn installation_is_verified_and_repeated_launches_do_not_download_or_install_again() {
    let temp = tempfile::tempdir().unwrap();
    let host = Host {
        root: temp.path().to_path_buf(),
        downloads: AtomicUsize::new(0),
    };
    let wine = Installer {
        prefix: temp.path().join("prefix"),
        env: Env::default(),
        calls: Cell::new(0),
        writes_runtime: true,
        fails: false,
    };
    let cache = temp.path().join("cache");
    desktop::prepare(&host, &wine, &cache, &silent).unwrap();
    assert!(desktop::installed(wine.prefix()).unwrap());
    desktop::prepare(&host, &wine, &cache, &silent).unwrap();
    assert_eq!(host.downloads.load(Ordering::SeqCst), 1);
    assert_eq!(wine.calls.get(), 1);
}

#[test]
fn success_exit_without_runtime_files_is_not_treated_as_a_successful_install() {
    let temp = tempfile::tempdir().unwrap();
    let host = Host {
        root: temp.path().to_path_buf(),
        downloads: AtomicUsize::new(0),
    };
    let mut wine = Installer {
        prefix: temp.path().join("prefix"),
        env: Env::default(),
        calls: Cell::new(0),
        writes_runtime: false,
        fails: false,
    };
    let cache = temp.path().join("cache");
    let error = desktop::prepare(&host, &wine, &cache, &silent).unwrap_err();
    assert!(error.to_string().contains("did not finish installing"));
    wine.fails = true;
    assert_eq!(
        desktop::prepare(&host, &wine, &cache, &silent).unwrap_err(),
        Error::patch("desktop installer failed")
    );
    assert!(!fsx::exists(
        &wine.prefix.join("drive_c/Program Files/dotnet")
    ));
}
