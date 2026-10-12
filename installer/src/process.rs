// SPDX-License-Identifier: MIT
//! Helper programs, run the way Python's `subprocess.run` ran them.
use crate::error::{Error, Result, repr_list};
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Environment variables in a fixed order, as a Python dictionary keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env(pub Vec<(OsString, OsString)>);

impl Env {
    pub fn get(&self, name: &str) -> Option<&OsString> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    pub fn set(&mut self, name: impl Into<OsString>, value: impl Into<OsString>) {
        let (name, value) = (name.into(), value.into());
        match self.0.iter_mut().find(|(key, _)| *key == name) {
            Some(entry) => entry.1 = value,
            None => self.0.push((name, value)),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(key, _)| key != name);
    }

    /// `{**self, **other}`.
    pub fn with(&self, other: &[(&str, &str)]) -> Env {
        let mut merged = self.clone();
        for (name, value) in other {
            merged.set(*name, *value);
        }
        merged
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Inherit,
    Null,
}

#[derive(Debug)]
pub enum Output<'a> {
    Inherit,
    Null,
    /// The program's output and errors both go to this file.
    File(&'a File),
    /// Output and errors are collected separately.
    Capture,
    /// Output is collected, errors are discarded.
    CaptureOutput,
    /// Output is discarded, errors are shown.
    DiscardOutput,
}

#[derive(Debug)]
pub struct Cmd<'a> {
    pub argv: Vec<OsString>,
    /// The complete environment; `None` inherits the installer's.
    pub env: Option<&'a Env>,
    pub cwd: Option<&'a Path>,
    pub stdin: Input,
    pub output: Output<'a>,
    pub timeout: Option<u64>,
}

impl<'a> Cmd<'a> {
    pub fn new(argv: impl Into<Vec<OsString>>) -> Self {
        Self {
            argv: argv.into(),
            env: None,
            cwd: None,
            stdin: Input::Inherit,
            output: Output::Inherit,
            timeout: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Finished {
    /// Python's `returncode`: the exit status, or the negative signal number.
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0))
}

/// `subprocess.Popen`.
pub fn spawn(cmd: &Cmd<'_>) -> Result<Child> {
    let program = cmd
        .argv
        .first()
        .ok_or_else(|| Error::other("empty command"))?;
    let mut command = Command::new(program);
    command.args(&cmd.argv[1..]);
    if let Some(env) = cmd.env {
        command
            .env_clear()
            .envs(env.0.iter().map(|(key, value)| (key, value)));
    }
    if let Some(cwd) = cmd.cwd {
        command.current_dir(cwd);
    }
    command.stdin(match cmd.stdin {
        Input::Inherit => Stdio::inherit(),
        Input::Null => Stdio::null(),
    });
    match &cmd.output {
        Output::Inherit => {}
        Output::Null => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        Output::File(file) => {
            let duplicate = |file: &File| file.try_clone().map_err(|error| Error::io(&error));
            command.stdout(duplicate(file)?).stderr(duplicate(file)?);
        }
        Output::Capture => {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        Output::CaptureOutput => {
            command.stdout(Stdio::piped()).stderr(Stdio::null());
        }
        Output::DiscardOutput => {
            command.stdout(Stdio::null());
        }
    }
    command.spawn().map_err(|error| {
        // Python names the working directory when that is what is missing.
        let missing = cmd
            .cwd
            .filter(|cwd| error.kind() == std::io::ErrorKind::NotFound && !cwd.exists());
        Error::io_path(
            &error,
            missing.map_or_else(|| PathBuf::from(program), Path::to_path_buf),
        )
    })
}

/// Wait for a child like `Popen.wait(timeout)`: `Ok(None)` when the time is up.
pub fn wait(
    child: &mut Child,
    timeout: Option<Duration>,
    interrupted: &dyn Fn() -> Result<()>,
) -> Result<Option<i32>> {
    let started = Instant::now();
    let mut delay = Duration::from_micros(500);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(code(status)));
        }
        interrupted()?;
        let remaining = match timeout {
            Some(timeout) => match timeout.checked_sub(started.elapsed()) {
                Some(remaining) if !remaining.is_zero() => remaining,
                _ => return Ok(None),
            },
            None => Duration::MAX,
        };
        delay = (delay * 2).min(remaining).min(Duration::from_millis(50));
        thread::sleep(delay);
    }
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// `subprocess.run(argv, ...)` without `check`.
pub fn run(cmd: &Cmd<'_>, interrupted: &dyn Fn() -> Result<()>) -> Result<Finished> {
    let mut child = spawn(cmd)?;
    let collect = |pipe: Option<Box<dyn Read + Send>>| {
        pipe.map(|mut pipe| {
            thread::spawn(move || {
                let mut data = Vec::new();
                let _ = pipe.read_to_end(&mut data);
                data
            })
        })
    };
    let stdout = collect(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = collect(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let code = match wait(
        &mut child,
        cmd.timeout.map(Duration::from_secs),
        interrupted,
    ) {
        Ok(Some(code)) => code,
        Ok(None) => {
            reap(&mut child);
            return Err(Error::Subprocess(format!(
                "Command '{}' timed out after {} seconds",
                repr_list(&cmd.argv),
                cmd.timeout.unwrap_or(0)
            )));
        }
        Err(error) => {
            // Ctrl+C reaches the program as well; give it a moment before it is killed.
            if error == Error::Interrupted {
                let _ = wait(&mut child, Some(Duration::from_millis(250)), &|| Ok(()));
            }
            reap(&mut child);
            return Err(error);
        }
    };
    let join = |reader: Option<thread::JoinHandle<Vec<u8>>>| {
        reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    };
    Ok(Finished {
        code,
        stdout: join(stdout),
        stderr: join(stderr),
    })
}

/// `subprocess.CalledProcessError` for a finished program.
pub fn failure(argv: &[OsString], code: i32) -> Error {
    const SIGNALS: [&str; 32] = [
        "",
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
    ];
    let command = repr_list(argv);
    Error::Subprocess(if code >= 0 {
        format!("Command '{command}' returned non-zero exit status {code}.")
    } else {
        match SIGNALS
            .get(code.unsigned_abs() as usize)
            .filter(|name| !name.is_empty())
        {
            Some(name) => format!("Command '{command}' died with <Signals.{name}: {}>.", -code),
            None => format!("Command '{command}' died with unknown signal {}.", -code),
        }
    })
}

/// `subprocess.run(argv, ..., check=True)`.
pub fn run_checked(cmd: &Cmd<'_>, interrupted: &dyn Fn() -> Result<()>) -> Result<Finished> {
    let finished = run(cmd, interrupted)?;
    if finished.code != 0 {
        return Err(failure(&cmd.argv, finished.code));
    }
    Ok(finished)
}

/// An array of `OsString` from mixed text and path arguments.
#[macro_export]
macro_rules! argv {
    ($($item:expr),* $(,)?) => {
        [$(::std::ffi::OsString::from($item)),*]
    };
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn never() -> Result<()> {
        Ok(())
    }

    #[test]
    fn exit_codes_output_and_messages() {
        let mut cmd = Cmd::new(argv!["sh", "-c", "echo out; echo err >&2; exit 3"]);
        cmd.output = Output::Capture;
        let finished = run(&cmd, &never).unwrap();
        assert_eq!(
            (
                finished.code,
                finished.stdout.as_slice(),
                finished.stderr.as_slice()
            ),
            (3, &b"out\n"[..], &b"err\n"[..])
        );
        assert_eq!(
            run_checked(&cmd, &never).unwrap_err().to_string(),
            "Command '['sh', '-c', 'echo out; echo err >&2; exit 3']' returned non-zero exit status 3."
        );
        let mut killed = Cmd::new(argv!["sh", "-c", "kill -TERM $$"]);
        killed.output = Output::Null;
        assert_eq!(
            run_checked(&killed, &never).unwrap_err().to_string(),
            "Command '['sh', '-c', 'kill -TERM $$']' died with <Signals.SIGTERM: 15>."
        );
        let missing = Cmd::new(argv!["/nonexistent/program"]);
        assert_eq!(
            run(&missing, &never).unwrap_err().to_string(),
            "[Errno 2] No such file or directory: '/nonexistent/program'"
        );
    }

    #[test]
    fn timeout_and_interrupt_end_the_program() {
        let mut cmd = Cmd::new(argv!["sleep", "30"]);
        cmd.timeout = Some(0);
        let started = Instant::now();
        assert_eq!(
            run(&cmd, &never).unwrap_err().to_string(),
            "Command '['sleep', '30']' timed out after 0 seconds"
        );
        cmd.timeout = None;
        assert_eq!(
            run(&cmd, &|| Err(Error::Interrupted)).unwrap_err(),
            Error::Interrupted
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn environment_keeps_dictionary_semantics() {
        let mut env = Env(vec![("A".into(), "1".into()), ("B".into(), "2".into())]);
        env.set("A", "3");
        env.set("C", "4");
        env.remove("B");
        assert_eq!(env.0, [("A".into(), "3".into()), ("C".into(), "4".into())]);
        let mut cmd = Cmd::new(argv!["sh", "-c", "echo $A$C$HOME"]);
        cmd.env = Some(&env);
        cmd.output = Output::CaptureOutput;
        assert_eq!(run(&cmd, &never).unwrap().stdout, b"34\n");
    }
}
