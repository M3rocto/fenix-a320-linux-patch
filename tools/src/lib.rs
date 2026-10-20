// SPDX-License-Identifier: MIT
//! Maintainer tooling of the Fenix A320 Linux Patch.
//!
//! [`build`] and [`proton`] rebuild the Wine modules from pinned sources,
//! [`smoke`] runs the project's own probes against them in a disposable
//! profile, and [`release`] writes the binary ZIP with its complete
//! corresponding source. Checksums, manifests, Wine's environment and JSON
//! output are the installer's own, from the `fenix_patch` library.
//!
//! Everything a command touches outside itself comes from a [`Context`], so
//! that tests can run whole commands in a temporary directory.
pub mod archive;
pub mod build;
pub mod cli;
pub mod elf;
pub mod licenses;
pub mod proton;
pub mod release;
pub mod smoke;
pub mod util;
pub mod x11;

use fenix_patch::{
    Error, Result, download,
    error::repr_list,
    fsx,
    process::{self, Cmd, Env, Finished, Output},
    system::System,
};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

/// The programs the commands start by name. Wine, its server, `configure` and
/// the Steam runtime's `run` are started from the directories they belong to.
#[derive(Debug, Clone)]
pub struct Programs {
    pub patch: OsString,
    pub autoreconf: OsString,
    /// Runs Wine's own `make_vulkan` inside the Wine source tree.
    pub python3: OsString,
    pub make: OsString,
    pub meson: OsString,
    pub ninja: OsString,
    pub strip: OsString,
    pub mingw_strip: OsString,
    pub mingw_gcc: OsString,
    pub mingw_gxx: OsString,
    pub readelf: OsString,
    pub cargo: OsString,
}

impl Default for Programs {
    fn default() -> Self {
        Self {
            patch: "patch".into(),
            autoreconf: "autoreconf".into(),
            python3: "python3".into(),
            make: "make".into(),
            meson: "meson".into(),
            ninja: "ninja".into(),
            strip: "strip".into(),
            mingw_strip: "x86_64-w64-mingw32-strip".into(),
            mingw_gcc: "x86_64-w64-mingw32-gcc".into(),
            mingw_gxx: "x86_64-w64-mingw32-g++".into(),
            readelf: "readelf".into(),
            cargo: "cargo".into(),
        }
    }
}

/// Opens the download of a source archive from its address.
pub type Fetch<'a> = dyn Fn(&str) -> Result<download::Stream> + Send + Sync + 'a;
/// Connects to the X11 display of the given name.
pub type OpenDisplay<'a> = dyn Fn(&str) -> Result<Box<dyn x11::Windows>> + Send + Sync + 'a;

/// What a command needs from its surroundings.
pub struct Context<'a> {
    /// The repository is `system.root()`; environment, Ctrl+C and the
    /// installer's own steps come from here as well.
    pub system: &'a dyn System,
    pub programs: Programs,
    pub out: Mutex<Box<dyn Write + Send + 'a>>,
    pub err: Mutex<Box<dyn Write + Send + 'a>>,
    /// Receives what started programs write to the terminal themselves;
    /// `None` leaves it on the terminal.
    pub terminal: Option<File>,
    pub fetch: Box<Fetch<'a>>,
    pub display: Box<OpenDisplay<'a>>,
}

impl<'a> Context<'a> {
    /// The real machine.
    pub fn host(system: &'a dyn System) -> Self {
        Self {
            system,
            programs: Programs::default(),
            out: Mutex::new(Box::new(std::io::stdout())),
            err: Mutex::new(Box::new(std::io::stderr())),
            terminal: None,
            fetch: Box::new(download::https),
            display: Box::new(x11::open),
        }
    }

    /// The repository, `ROOT` of the Python scripts.
    pub fn root(&self) -> &Path {
        self.system.root()
    }

    /// `print(text, flush=True)`.
    pub fn print(&self, text: &str) -> Result<()> {
        let mut out = self
            .out
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writeln!(out, "{text}")?;
        out.flush()?;
        Ok(())
    }

    /// One line on standard error.
    pub fn warn(&self, text: &str) {
        let mut err = self
            .err
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = writeln!(err, "{text}");
        let _ = err.flush();
    }

    /// Where a program's output goes that Python left on the terminal.
    pub fn shown(&self) -> Output<'_> {
        match &self.terminal {
            Some(file) => Output::File(file),
            None => Output::Inherit,
        }
    }

    /// `subprocess.run(cmd)` without `check`.
    pub fn run(&self, cmd: &Cmd<'_>) -> Result<Finished> {
        process::run(cmd, &|| self.system.interrupted())
    }

    /// `subprocess.run(cmd, check=True)`.
    pub fn run_checked(&self, cmd: &Cmd<'_>) -> Result<Finished> {
        process::run_checked(cmd, &|| self.system.interrupted())
    }

    /// `subprocess.run(argv, env=env, cwd=cwd, stdout=PIPE, stderr=STDOUT, timeout=timeout)`:
    /// the exit status, and output and errors in the order they were written.
    pub fn capture_merged(
        &self,
        argv: Vec<OsString>,
        env: &Env,
        cwd: &Path,
        timeout: u64,
    ) -> Result<(i32, Vec<u8>)> {
        let started = Instant::now();
        let timeout = Duration::from_secs(timeout);
        let (mut reader, writer) = std::io::pipe()?;
        let writer = File::from(OwnedFd::from(writer));
        let mut cmd = Cmd::new(argv);
        cmd.env = Some(env);
        cmd.cwd = Some(cwd);
        cmd.output = Output::File(&writer);
        let mut child = process::spawn(&cmd)?;
        let argv = std::mem::take(&mut cmd.argv);
        drop(cmd);
        drop(writer);
        let (sender, collected) = mpsc::channel();
        thread::spawn(move || {
            let mut data = Vec::new();
            let _ = reader.read_to_end(&mut data);
            let _ = sender.send(data);
        });
        let expired = || {
            Error::Subprocess(format!(
                "Command '{}' timed out after {} seconds",
                repr_list(&argv),
                timeout.as_secs()
            ))
        };
        let code = match process::wait(&mut child, Some(timeout), &|| self.system.interrupted()) {
            Ok(Some(code)) => code,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(expired());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        // The pipe stays open for as long as any program started by the
        // command still holds it; Python waited for that within the same time.
        match collected.recv_timeout(timeout.saturating_sub(started.elapsed())) {
            Ok(data) => Ok((code, data)),
            Err(_) => Err(expired()),
        }
    }

    /// `subprocess.run(argv, cwd=cwd, env=env, check=True)`, output on the terminal.
    pub fn call(&self, argv: Vec<OsString>, env: &Env, cwd: Option<&Path>) -> Result<()> {
        let mut cmd = Cmd::new(argv);
        cmd.env = Some(env);
        cmd.cwd = cwd;
        cmd.output = self.shown();
        self.run_checked(&cmd).map(|_| ())
    }
}

/// The repository this program was built in, or the current directory once
/// that no longer exists: `Path(__file__).resolve().parents[1]`.
pub fn repository() -> Result<PathBuf> {
    let built = fsx::parent(Path::new(env!("CARGO_MANIFEST_DIR")));
    if built.is_dir() {
        fsx::resolve(built)
    } else {
        Ok(std::env::current_dir()?)
    }
}
