// SPDX-License-Identifier: MIT
//! The program itself, on the repository it was built in: commands that end
//! before they write or start anything.
#![allow(clippy::unwrap_used)]
use std::process::{Command, Output};

fn tools(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fenix-tools"))
        .args(arguments)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn help_names_the_four_commands() {
    let output = tools(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let help = text(&output.stdout);
    for command in ["  build ", "  build-proton ", "  smoke ", "  release "] {
        assert!(help.contains(command), "{help}");
    }
    let output = tools(&["release", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("--installer <INSTALLER>"));
    let output = tools(&[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("Usage: fenix-tools <COMMAND>"));
}

#[test]
fn failures_are_one_line_and_status_one() {
    // bundle.json of this repository is read; nothing is written.
    let output = tools(&[
        "build-proton",
        "no-such-proton",
        "--build-dir",
        "/nonexistent/build",
    ]);
    assert_eq!(
        (
            output.status.code(),
            text(&output.stderr).as_str(),
            output.stdout.len()
        ),
        (Some(1), "fenix-tools: 'no-such-proton'\n", 0)
    );
    let output = tools(&["smoke", "--runner", "/nonexistent/runner"]);
    assert_eq!(
        (output.status.code(), text(&output.stderr).as_str()),
        (
            Some(1),
            "fenix-tools: [Errno 2] No such file or directory: '/nonexistent'\n"
        )
    );
    let output = tools(&["release", "--installer", "/nonexistent/fenix-patch"]);
    assert_eq!(
        (output.status.code(), text(&output.stderr).as_str()),
        (
            Some(1),
            "fenix-tools: [Errno 2] No such file or directory: '/nonexistent/fenix-patch'\n"
        )
    );
    let output = tools(&["build", "--jobs", "many"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("invalid literal for int() with base 10: 'many'"),
        "{}",
        text(&output.stderr)
    );
}
