// SPDX-License-Identifier: MIT
//! Check actual X11 mapping while the Win32 probe keeps its window alive.
use crate::{Context, util};
use fenix_patch::{
    Error, Result,
    error::repr_list,
    process::{self, Cmd, Env, Output},
    py,
};
use regex::Regex;
use std::{
    ffi::OsString, fs::File, io::BufReader, os::fd::OwnedFd, path::Path, process::Child,
    sync::LazyLock, time::Duration,
};
use x11rb::{errors::ReplyError, protocol::xproto::ConnectionExt, rust_connection::RustConnection};

static HOST_WINDOW: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r"\Ahost xid=([0-9]+) helper=([0-9]+) logical_visible=([0-9]+)"));

/// The windows of an X11 display.
pub trait Windows {
    /// The map state of a window (0 unmapped, 1 unviewable, 2 viewable), or
    /// `None` when the display has no such window.
    fn map_state(&mut self, window: u32) -> Result<Option<u8>>;
}

struct Display(RustConnection);

/// Connect to the X11 display of the given name, e.g. `:99`.
pub fn open(display: &str) -> Result<Box<dyn Windows>> {
    match RustConnection::connect(Some(display)) {
        Ok((connection, _)) => Ok(Box::new(Display(connection))),
        Err(_) => Err(Error::other("Cannot open test X11 display")),
    }
}

impl Windows for Display {
    fn map_state(&mut self, window: u32) -> Result<Option<u8>> {
        let lost = |error: &dyn std::fmt::Display| {
            Error::other(format!("The test X11 display stopped answering: {error}"))
        };
        let request = self
            .0
            .get_window_attributes(window)
            .map_err(|error| lost(&error))?;
        match request.reply() {
            Ok(attributes) => Ok(Some(u8::from(attributes.map_state))),
            // The server knows no such window.
            Err(ReplyError::X11Error(_)) => Ok(None),
            Err(ReplyError::ConnectionError(error)) => Err(lost(&error)),
        }
    }
}

/// What the probe reports about one window, and what the display says to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub window: u128,
    pub helper: u128,
    pub visible: u128,
}

/// `host xid=… helper=… logical_visible=…` at the start of a line.
pub fn parse(line: &str) -> Option<Report> {
    let found = HOST_WINDOW.captures(line)?;
    // Longer numbers than any window identifier are no identifier at all.
    let number = |index: usize| found[index].parse::<u128>().unwrap_or(u128::MAX);
    Some(Report {
        window: number(1),
        helper: number(2),
        visible: number(3),
    })
}

/// One reported window against the display. `hidden` is what is expected of
/// this report: a helper window that X11 does not show, or an ordinary one
/// that it does.
pub fn judge(report: Report, hidden: Option<bool>, windows: &mut dyn Windows) -> Result<()> {
    let state = match u32::try_from(report.window) {
        Ok(window) if window != 0 => windows.map_state(window)?,
        _ => None,
    };
    let Some(state) = state else {
        return Err(Error::other("Probe did not expose a live X11 window"));
    };
    let as_expected = hidden.is_some_and(|hidden| {
        (report.helper != 0) == hidden && report.visible == 1 && (state != 0) != hidden
    });
    if !as_expected {
        return Err(Error::other(format!(
            "Unexpected X11 state: helper={}, Win32={}, mapped={state}",
            report.helper, report.visible
        )));
    }
    Ok(())
}

/// Start the probe and compare each window it reports with the display.
/// `hidden` says for its two reports whether the window is expected to be a
/// helper window that stays unmapped.
pub fn check_host_windows(
    ctx: &Context<'_>,
    command: &[OsString],
    env: &Env,
    cwd: &Path,
    hidden: (bool, bool),
) -> Result<()> {
    let display = env
        .get("DISPLAY")
        .filter(|display| !display.is_empty())
        .ok_or_else(|| {
            Error::other("X11 window checks require DISPLAY (Xwayland or Xvfb is sufficient)")
        })?;
    let mut windows = (ctx.display)(&display.to_string_lossy())?;
    // One pipe receives the probe's output and errors in the order they are written.
    let (reader, writer) = std::io::pipe()?;
    let writer = File::from(OwnedFd::from(writer));
    let mut cmd = Cmd::new(command.to_vec());
    cmd.env = Some(env);
    cmd.cwd = Some(cwd);
    cmd.output = Output::File(&writer);
    let mut child = process::spawn(&cmd)?;
    drop(cmd);
    drop(writer);
    let outcome = watch(
        ctx,
        &mut child,
        BufReader::new(reader),
        windows.as_mut(),
        [hidden.0, hidden.1],
        command,
    );
    if outcome.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    outcome
}

fn watch(
    ctx: &Context<'_>,
    child: &mut Child,
    mut output: BufReader<std::io::PipeReader>,
    windows: &mut dyn Windows,
    expected: [bool; 2],
    command: &[OsString],
) -> Result<()> {
    let mut count = 0;
    while let Some(line) = util::read_line(&mut output)? {
        ctx.print(line.trim_end_matches(py::is_space))?;
        if line.starts_with("window_visible=") {
            break;
        }
        let Some(report) = parse(&line) else {
            continue;
        };
        judge(report, expected.get(count).copied(), windows)?;
        count += 1;
    }
    ctx.system.interrupted()?;
    let interrupted = || ctx.system.interrupted();
    match process::wait(child, Some(Duration::from_secs(30)), &interrupted)? {
        None => Err(Error::Subprocess(format!(
            "Command '{}' timed out after 30 seconds",
            repr_list(command)
        ))),
        Some(code) if code != 0 || count != 2 => {
            Err(Error::other("Incomplete window mapping probe"))
        }
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::collections::HashMap;

    /// A display that knows the given windows.
    pub struct Known(pub HashMap<u32, u8>);

    impl Windows for Known {
        fn map_state(&mut self, window: u32) -> Result<Option<u8>> {
            Ok(self.0.get(&window).copied())
        }
    }

    #[test]
    fn only_the_probe_s_report_lines_are_read() {
        assert_eq!(
            parse("host xid=4194316 helper=1 logical_visible=1\n"),
            Some(Report {
                window: 4_194_316,
                helper: 1,
                visible: 1
            })
        );
        // Python's `re.match`: at the start of the line, anything may follow.
        assert_eq!(
            parse("host xid=07 helper=0 logical_visible=10 trailing").map(|report| report.window),
            Some(7)
        );
        for line in [
            " host xid=1 helper=1 logical_visible=1",
            "host xid= helper=1 logical_visible=1",
            "host xid=-1 helper=1 logical_visible=1",
            "host xid=1 helper=1",
            "window_visible=1",
            "",
        ] {
            assert_eq!(parse(line), None, "{line}");
        }
        assert_eq!(
            parse("host xid=999999999999999999999999999999999999999999 helper=1 logical_visible=1")
                .map(|report| report.window),
            Some(u128::MAX)
        );
    }

    #[test]
    fn a_window_passes_only_in_the_expected_state() {
        let mut display = Known(HashMap::from([(10, 0), (11, 1), (12, 2)]));
        let report = |window, helper, visible| Report {
            window,
            helper,
            visible,
        };
        // A helper window must stay unmapped, an ordinary one must be mapped.
        judge(report(10, 1, 1), Some(true), &mut display).unwrap();
        judge(report(10, 7, 1), Some(true), &mut display).unwrap();
        judge(report(12, 0, 1), Some(false), &mut display).unwrap();
        judge(report(11, 0, 1), Some(false), &mut display).unwrap();
        for (window, helper, visible, hidden, message) in [
            (
                12,
                1,
                1,
                Some(true),
                "Unexpected X11 state: helper=1, Win32=1, mapped=2",
            ),
            (
                11,
                1,
                1,
                Some(true),
                "Unexpected X11 state: helper=1, Win32=1, mapped=1",
            ),
            (
                10,
                0,
                1,
                Some(true),
                "Unexpected X11 state: helper=0, Win32=1, mapped=0",
            ),
            (
                10,
                1,
                0,
                Some(true),
                "Unexpected X11 state: helper=1, Win32=0, mapped=0",
            ),
            (
                10,
                1,
                2,
                Some(true),
                "Unexpected X11 state: helper=1, Win32=2, mapped=0",
            ),
            (
                10,
                0,
                1,
                Some(false),
                "Unexpected X11 state: helper=0, Win32=1, mapped=0",
            ),
            (
                12,
                1,
                1,
                Some(false),
                "Unexpected X11 state: helper=1, Win32=1, mapped=2",
            ),
            // A third report is one too many, whatever it says.
            (
                10,
                1,
                1,
                None,
                "Unexpected X11 state: helper=1, Win32=1, mapped=0",
            ),
            (
                0,
                1,
                1,
                Some(true),
                "Probe did not expose a live X11 window",
            ),
            (
                13,
                1,
                1,
                Some(true),
                "Probe did not expose a live X11 window",
            ),
            (
                1 << 32,
                1,
                1,
                Some(true),
                "Probe did not expose a live X11 window",
            ),
            (
                u128::MAX,
                1,
                1,
                Some(true),
                "Probe did not expose a live X11 window",
            ),
        ] {
            assert_eq!(
                judge(report(window, helper, visible), hidden, &mut display)
                    .unwrap_err()
                    .to_string(),
                message,
                "{window} {helper} {visible} {hidden:?}"
            );
        }
    }
}
