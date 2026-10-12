// SPDX-License-Identifier: MIT
//! The command line: the same commands, options, messages and exit codes as
//! `python3 -m fenix_patch` had.
use crate::{
    error::{Error, Result},
    guide::{self, Console},
    json::{self, Value, json},
    steam,
    system::{Host, System},
    targets, web,
};
use clap::{CommandFactory, Parser, error::ErrorKind};
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    os::fd::AsFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Parser, Debug)]
#[command(
    name = "fenix-patch",
    about = "Fenix A320 Linux Patch — MSFS 2020/2024 on Steam, MSFS 2024 in Flightdeck",
    args_override_self = true,
    infer_long_args = true
)]
pub struct Args {
    #[arg(
        value_parser = ["status", "install", "select", "configure", "restore", "installer", "manager", "open", "targets", "gui"],
        default_value = "gui",
        hide_default_value = true
    )]
    pub command: String,
    /// Steam edition of the simulator
    #[arg(long, value_parser = ["msfs2020", "msfs2024"])]
    pub steam: Option<String>,
    /// Steam directory, when it is not found automatically
    #[arg(long, value_name = "STEAM_ROOT")]
    pub steam_root: Option<OsString>,
    /// Unmodified, exactly supported Proton build to base the Fenix Proton on
    #[arg(long)]
    pub proton: Option<OsString>,
    /// Flightdeck runtime directory
    #[arg(long)]
    pub runtime: Option<OsString>,
    /// Extracted, matching binary release [default: the directory of this program]
    #[arg(long)]
    pub bundle: Option<OsString>,
    /// Official Fenix Installer downloaded from your account
    #[arg(long)]
    pub exe: Option<OsString>,
    /// Emit structured progress for launchers
    #[arg(long)]
    pub json: bool,
    /// Guided setup in this terminal instead of the browser
    #[arg(long)]
    pub text: bool,
    /// Print the setup address instead of opening a browser
    #[arg(long)]
    pub no_browser: bool,
}

/// What the command line needs from its surroundings.
pub struct Cli {
    /// Stops at the next opportunity after Ctrl+C.
    pub system: Arc<dyn System>,
    /// The same machine for steps of the browser setup, which Ctrl+C never cuts short.
    pub background: Arc<dyn System>,
    /// Set by Ctrl+C.
    pub interrupt: Arc<AtomicBool>,
    pub stdout: Box<dyn Write + Send>,
    pub stderr: Box<dyn Write + Send>,
}

impl Cli {
    /// The real machine, with the installer's own files next to the program.
    pub fn host(interrupt: Arc<AtomicBool>) -> Self {
        let root = Host::program_directory();
        Self {
            system: Arc::new(Host::new(root.clone()).with_interrupt(interrupt.clone())),
            background: Arc::new(Host::new(root)),
            interrupt,
            stdout: Box::new(io::stdout()),
            stderr: Box::new(io::stderr()),
        }
    }
}

/// The terminal of the guided text setup.
struct Terminal<'a> {
    out: &'a mut (dyn Write + Send),
    interrupt: &'a AtomicBool,
    pending: Vec<u8>,
}

impl Console for Terminal<'_> {
    fn print(&mut self, line: &str) -> Result<()> {
        writeln!(self.out, "{line}")?;
        Ok(())
    }

    /// Python's `input()`: the prompt without a line end, then one line of input.
    fn input(&mut self, prompt: &str) -> Result<String> {
        self.out.flush()?;
        let stdin = io::stdin();
        if stdin.is_terminal() && io::stdout().is_terminal() {
            // Without the readline module Python shows the prompt on standard error.
            let mut errors = io::stderr();
            errors.write_all(prompt.as_bytes())?;
            errors.flush()?;
        } else {
            self.out.write_all(prompt.as_bytes())?;
            self.out.flush()?;
        }
        loop {
            if let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                return Ok(String::from_utf8_lossy(&line[..end]).into_owned());
            }
            if self.interrupt.swap(false, Ordering::SeqCst) {
                return Err(Error::Interrupted);
            }
            let mut waiting = [rustix::event::PollFd::new(
                &stdin,
                rustix::event::PollFlags::IN,
            )];
            let timeout = rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            match rustix::event::poll(&mut waiting, Some(&timeout)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(Error::from(error)),
            }
            let mut buffer = [0u8; 4096];
            match rustix::io::read(stdin.as_fd(), &mut buffer) {
                Ok(0) if self.pending.is_empty() => return Err(Error::Eof),
                Ok(0) => {
                    return Ok(
                        String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned()
                    );
                }
                Ok(count) => self.pending.extend_from_slice(&buffer[..count]),
                Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
                Err(error) => return Err(Error::from(error)),
            }
        }
    }
}

fn given(value: &Option<OsString>) -> Option<&std::ffi::OsStr> {
    value.as_deref().filter(|value| !value.is_empty())
}

fn usage_error(cli: &mut Cli, message: &str) -> i32 {
    let error = Args::command().error(ErrorKind::ValueValidation, message);
    let _ = write!(cli.stderr, "{}", error.render());
    2
}

fn execute(cli: &mut Cli, args: &Args) -> Result<i32> {
    let system = cli.system.clone();
    let bundle = args
        .bundle
        .clone()
        .unwrap_or_else(|| system.root().as_os_str().to_os_string());
    let steam_game = args.steam.as_deref().filter(|game| !game.is_empty());
    if args.command == "gui" {
        // The browser setup runs its steps in the background; Ctrl+C is handled by the setup itself.
        let system = if args.text || !steam::has_display(system.as_ref()) {
            system
        } else {
            cli.background.clone()
        };
        let found = if given(&args.runtime).is_some() || steam_game.is_some() {
            vec![targets::select(
                &system,
                args.runtime.as_deref(),
                steam_game,
                args.steam_root.as_deref(),
            )?]
        } else {
            targets::detect(&system, args.steam_root.as_deref())?
        };
        // Without a desktop session there is no browser to show the setup in.
        if args.text || !steam::has_display(system.as_ref()) {
            let mut terminal = Terminal {
                out: cli.stdout.as_mut(),
                interrupt: &cli.interrupt,
                pending: Vec::new(),
            };
            guide::text_wizard(
                system.as_ref(),
                &found,
                &bundle,
                args.proton.as_deref(),
                &mut terminal,
            )?;
        } else {
            let interrupt = cli.interrupt.clone();
            web::serve(
                system,
                found,
                &bundle,
                args.proton.as_deref(),
                !args.no_browser,
                &|| interrupt.swap(false, Ordering::SeqCst),
                cli.stdout.as_mut(),
            )?;
        }
        return Ok(0);
    }
    if args.command == "targets" {
        let found: Vec<Value> = targets::detect(&system, args.steam_root.as_deref())?
            .iter()
            .map(|item| json!({"kind": item.kind(), "label": item.label()}))
            .collect();
        writeln!(cli.stdout, "{}", json::dumps(&Value::Array(found), Some(2)))?;
        return Ok(0);
    }
    let target = targets::select(
        &system,
        args.runtime.as_deref(),
        steam_game,
        args.steam_root.as_deref(),
    )?;
    match args.command.as_str() {
        "status" => {
            writeln!(cli.stdout, "{}", json::dumps(&target.status()?, Some(2)))?;
            return Ok(0);
        }
        "select" if target.kind() != "steam" => {
            return Ok(usage_error(cli, "select applies to Steam simulators only"));
        }
        "installer" if given(&args.exe).is_none() => {
            return Ok(usage_error(
                cli,
                "installer requires --exe /path/to/FenixInstaller.exe",
            ));
        }
        _ => {}
    }
    let as_json = args.json;
    let out = std::sync::Mutex::new(cli.stdout.as_mut());
    let progress = |message: &str| -> Result<()> {
        system.interrupted()?;
        let mut out = out
            .lock()
            .map_err(|_| Error::other("The output is no longer available."))?;
        if as_json {
            writeln!(out, "{}", json::dumps(&json!({"message": message}), None))?;
        } else {
            writeln!(out, "{message}")?;
        }
        out.flush()?;
        Ok(())
    };
    match args.command.as_str() {
        "install" => target.install(&bundle, &progress, args.proton.as_deref())?,
        "select" => target.select(&progress)?,
        "configure" => target.configure(&progress)?,
        "restore" => target.restore(&progress)?,
        "installer" => target.app(&progress, given(&args.exe), false)?,
        "open" => target.app(&progress, None, false)?,
        "manager" => target.app(&progress, None, true)?,
        _ => {}
    }
    Ok(0)
}

/// Run one command line; the result is the process exit status.
pub fn run(cli: &mut Cli, argv: impl IntoIterator<Item = OsString>) -> i32 {
    let args = match Args::try_parse_from(argv) {
        Ok(args) => args,
        Err(error) => {
            let rendered = error.render();
            let _ = if error.use_stderr() {
                write!(cli.stderr, "{rendered}")
            } else {
                write!(cli.stdout, "{rendered}")
            };
            let _ = cli.stdout.flush();
            return error.exit_code();
        }
    };
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));
    let outcome = execute(cli, &args);
    let flushed = cli.stdout.flush();
    match outcome {
        Err(Error::Interrupted | Error::Eof) => 130,
        Err(error) => {
            let _ = if args.json {
                writeln!(
                    cli.stderr,
                    "{}",
                    json::dumps(&json!({"error": error.to_string()}), None)
                )
            } else {
                writeln!(cli.stderr, "Fenix patch: {error}")
            };
            1
        }
        // Ctrl+C in the last moment still counts, as it did in Python.
        Ok(_) if cli.interrupt.swap(false, Ordering::SeqCst) => 130,
        Ok(code) => {
            if let Err(error) = flushed {
                let _ = writeln!(cli.stderr, "Fenix patch: {}", Error::io(&error));
                return 1;
            }
            code
        }
    }
}
