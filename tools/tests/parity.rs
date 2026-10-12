// SPDX-License-Identifier: MIT
//! Temporary scaffolding for the move from Python to Rust: the maintainer
//! scripts `scripts/build.py`, `build-proton.py`, `smoke.py` and `release.py`
//! and the commands of `fenix-tools` are given the same synthetic repository,
//! the same stand-ins for the programs they start and the same display, and
//! everything they print, start and leave behind is compared.
//!
//! Delete this file together with the Python scripts. Without `python3` or
//! without the scripts every test here is skipped and says so.
#![allow(clippy::unwrap_used)]
mod common;

use common::{
    Display, Lab, Outcome, Raw, Run, Tree, VARIANT, World, calls, cargo_metadata, copy_tree, gzip,
    notice, put, read, runner, script, snapshot, steam_runtime, tar, tiny_program, windows,
    wine_archive,
};
use fenix_patch::{
    VERSION, argv, fsx,
    process::{self, Cmd, Env, Output},
};
use fenix_tools::{Programs, archive, repository, x11};
use flate2::read::GzDecoder;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    net::TcpListener,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

/// The interpreter, or `None` when there is nothing to compare with.
fn python() -> Option<PathBuf> {
    let Some(python) = fsx::which("python3", std::env::var_os("PATH")) else {
        notice("python3 was not found; the comparison with the Python scripts is skipped");
        return None;
    };
    if !repository().unwrap().join("scripts/release.py").is_file() {
        notice("the Python scripts are gone; delete tools/tests/parity.rs as well");
        return None;
    }
    Some(python)
}

/// The same repository twice, each with its own surroundings.
struct Pair {
    lab: Lab,
    interpreter: PathBuf,
    python: PathBuf,
    rust: PathBuf,
    /// Variables that both sides get on top of the test's environment.
    extra: Vec<(&'static str, String)>,
    display: Option<Display>,
}

impl Pair {
    fn new() -> Option<Self> {
        let interpreter = python()?;
        let lab = Lab::new();
        let (python, rust) = (lab.base.join("python"), lab.base.join("rust"));
        let worktree = repository().unwrap();
        for side in [&python, &rust] {
            Tree::create(&side.join("repo"));
            // The same file on both sides, so that it is in both releases.
            fs::copy(
                worktree.join("tests/x11_window_check.py"),
                side.join("repo/tests/x11_window_check.py"),
            )
            .unwrap();
        }
        for folder in ["scripts", "fenix_patch"] {
            fs::create_dir(python.join("repo").join(folder)).unwrap();
            for entry in fs::read_dir(worktree.join(folder)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|extension| extension == "py") {
                    let target = python
                        .join("repo")
                        .join(folder)
                        .join(path.file_name().unwrap());
                    fs::copy(&path, target).unwrap();
                }
            }
        }
        Some(Self {
            lab,
            interpreter,
            python,
            rust,
            extra: Vec::new(),
            display: None,
        })
    }

    fn env(&self, side: &Path) -> Env {
        let mut env = self.lab.env(side);
        for (name, value) in &self.extra {
            env.set(*name, value);
        }
        env
    }

    /// Run the interpreter with these arguments; `{repo}` is the repository.
    fn interpret(&self, arguments: &[&str]) -> Outcome {
        let output = self.python.join("output.txt");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output)
            .unwrap();
        let env = self.env(&self.python);
        let repo = self.python.join("repo");
        // No bytecode in the repository, no settings of the user, and
        // nothing held back in a buffer.
        let mut argv = argv![&self.interpreter, "-E", "-s", "-B", "-u"].to_vec();
        argv.extend(
            arguments
                .iter()
                .map(|argument| argument.replace("{repo}", repo.to_str().unwrap()).into()),
        );
        let mut cmd = Cmd::new(argv);
        cmd.env = Some(&env);
        cmd.stdin = process::Input::Null;
        cmd.output = Output::File(&file);
        cmd.timeout = Some(600);
        let finished = process::run(&cmd, &|| Ok(())).unwrap();
        let text = read(&output);
        fs::remove_file(&output).unwrap();
        Outcome {
            code: finished.code,
            output: text,
        }
    }

    fn script(&self, name: &str, arguments: &[&str]) -> Outcome {
        let mut argv = vec![format!("{{repo}}/scripts/{name}")];
        argv.extend(arguments.iter().map(|argument| argument.to_string()));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        self.interpret(&argv)
    }

    /// The command of `fenix-tools`, with programs found by name as the scripts find them.
    fn tools(&self, arguments: &[&str]) -> Outcome {
        let world = World::new(&self.rust.join("repo"), self.env(&self.rust));
        Run {
            world: &world,
            programs: Programs::default(),
            fetch: None,
            display: Some(&x11::open),
            output: self.rust.join("output.txt"),
        }
        .tools(arguments)
    }

    /// What a script printed, in the words of `fenix-tools`: a traceback
    /// becomes the line with its message.
    fn translated(&self, outcome: &Outcome) -> (i32, String) {
        let text = outcome
            .output
            .replace(self.python.to_str().unwrap(), "<SIDE>");
        let text = match text.find("Traceback (most recent call last):") {
            None => text,
            Some(at) => {
                let last = text.trim_end().lines().last().unwrap_or_default();
                let message = last.split_once(": ").map_or("", |(_, message)| message);
                format!("{}fenix-tools: {message}\n", &text[..at])
            }
        };
        (outcome.code, text)
    }

    /// Both sides must have printed and started the same and ended alike.
    /// `{side}` in an argument is the directory of the respective side.
    #[track_caller]
    fn same(&self, what: &str, script: &str, command: &str, arguments: &[&str]) -> (i32, String) {
        let on = |side: &Path| -> Vec<String> {
            arguments
                .iter()
                .map(|argument| argument.replace("{side}", side.to_str().unwrap()))
                .collect()
        };
        let python_arguments = on(&self.python);
        let python_arguments: Vec<&str> = python_arguments.iter().map(String::as_str).collect();
        let python = if script == "smoke.py" {
            // Microsoft's packages and the real helper are replaced on both sides.
            let mut argv = vec!["-c", SMOKE, "{repo}"];
            argv.extend(&python_arguments);
            self.interpret(&argv)
        } else {
            self.script(script, &python_arguments)
        };
        let mut rust_arguments = vec![command.to_string()];
        rust_arguments.extend(on(&self.rust));
        let rust_arguments: Vec<&str> = rust_arguments.iter().map(String::as_str).collect();
        let rust = self.tools(&rust_arguments);
        let expected = self.translated(&python);
        let found = (
            rust.code,
            rust.output.replace(self.rust.to_str().unwrap(), "<SIDE>"),
        );
        assert_eq!(found, expected, "{what}: output and exit status");
        assert_eq!(
            self.calls(&self.rust),
            self.calls(&self.python),
            "{what}: the programs that were started"
        );
        found
    }

    fn calls(&self, side: &Path) -> Vec<String> {
        let cwd = std::env::current_dir().unwrap();
        let mut calls: Vec<String> = calls(side, side)
            .into_iter()
            .map(|line| line.replace(&format!("cwd={}", cwd.display()), "cwd=<CWD>"))
            .collect();
        // The guard runs beside the first probe; see tests/smoke.rs.
        if let Some(at) = calls
            .iter()
            .position(|call| call.contains("/FenixWindowGuard.exe] "))
            && let Some(compiled) = calls
                .iter()
                .position(|call| call.contains("/window-guard-probe.c] "))
        {
            let guard = calls.remove(at);
            calls.insert(compiled + 1, guard);
        }
        calls
    }

    /// Everything below `relative` must be the same on both sides: names,
    /// kinds, permissions, link targets and contents.
    #[track_caller]
    fn same_files(&self, what: &str, relative: &str) -> usize {
        let of = |side: &Path| -> Vec<(String, String)> {
            snapshot(&side.join(relative))
                .into_iter()
                .filter(|(name, _)| {
                    !["scripts", "fenix_patch", "dist", "out"]
                        .iter()
                        .any(|only| name == only || name.starts_with(&format!("{only}/")))
                })
                .collect()
        };
        let (python, rust) = (of(&self.python), of(&self.rust));
        if python != rust {
            let differing: Vec<_> = python
                .iter()
                .filter(|entry| !rust.contains(entry))
                .chain(rust.iter().filter(|entry| !python.contains(entry)))
                .take(20)
                .collect();
            panic!("{what}: {relative} differs, e.g. {differing:#?}");
        }
        rust.len()
    }

    fn on_both(&self, change: impl Fn(&Path)) {
        for side in [&self.python, &self.rust] {
            change(side);
        }
    }

    fn edit_lock(&self, change: impl Fn(&mut fenix_patch::json::Value)) {
        self.on_both(|side| {
            Tree {
                root: side.join("repo"),
            }
            .edit(&change)
        });
    }
}

/// `scripts/smoke.py` with `core.prepare_geometry` replaced by a line that
/// says what it was asked for, as the test's own machine does for the tool.
const SMOKE: &str = r#"
import runpy, sys
root = sys.argv[1]
sys.path.insert(0, root)
from fenix_patch import core
core.prepare_geometry = lambda wine, cache, bundle, progress: progress("geometry stand-in: %s %s" % (cache, bundle))
sys.argv = [root + "/scripts/smoke.py"] + sys.argv[2:]
runpy.run_path(sys.argv[0], run_name="__main__")
"#;

/// `extract()` of `scripts/build.py` on one archive; the result in one line.
const EXTRACT: &str = r#"
import sys
sys.path.insert(0, sys.argv[1])
from pathlib import Path
from build import extract
try:
    extract(Path(sys.argv[2]), Path(sys.argv[3]))
    print("extracted")
except Exception as error:
    print("refused: " + str(error))
"#;

#[test]
fn hostile_and_harmless_archives_are_treated_alike() {
    let Some(pair) = Pair::new() else { return };
    let long = format!("top/{}/{}.c", "d".repeat(70), "f".repeat(70));
    // A record of an extended header begins with its own length.
    let record = format!("{} path={long}\n", long.len() + 10);
    assert_eq!(record.len(), long.len() + 10);
    let cases: Vec<(&str, Vec<Raw<'_>>, Option<&str>)> = vec![
        (
            "plain",
            vec![
                Raw::directory("top/"),
                Raw::file("top/file.txt", 0o640, b"content\n"),
                Raw::file("top/bin/tool", 0o6755, b"#!/bin/sh\n"),
                Raw::file("top/world-writable", 0o666, b""),
                Raw::link("top/bin/alias", "tool"),
                Raw::link("top/up", "bin/../file.txt"),
                Raw::directory("top/empty/nested/"),
                Raw::file("./top/dotted/./name//here", 0o644, b"dots\n"),
                Raw::file("only-one-part", 0o644, b"skipped\n"),
                Raw::file("top", 0o644, b"skipped as well\n"),
                // The oldest format: a directory is a file whose name ends in a slash.
                Raw::kind("top/v7-directory/", 0, ""),
                Raw::file("top/regular-with-slash/", 0o644, b"a file all the same\n"),
                Raw::file(
                    "other-top/also-extracted",
                    0o600,
                    b"any first part is dropped\n",
                ),
            ],
            Some("extracted"),
        ),
        (
            "long-name",
            vec![
                Raw {
                    name: b"top/PaxHeaders/long",
                    kind: b'x',
                    link: b"",
                    mode: 0o644,
                    data: record.as_bytes(),
                },
                Raw::file("top/truncated-name", 0o644, b"long\n"),
            ],
            Some("extracted"),
        ),
        (
            "through-a-link-inside",
            vec![
                Raw::directory("top/real/"),
                Raw::link("top/alias", "real"),
                Raw::file("top/alias/file", 0o644, b"lands in real\n"),
            ],
            Some("extracted"),
        ),
        (
            "parent",
            vec![
                Raw::file("top/ok", 0o644, b"before\n"),
                Raw::file("top/../escaped", 0o644, b"x"),
            ],
            Some("refused: Unexpected member in upstream source archive: top/../escaped"),
        ),
        (
            "parent-later",
            vec![Raw::file("top/a/b/../../../escaped", 0o644, b"x")],
            Some("refused: Unexpected member in upstream source archive: top/a/b/../../../escaped"),
        ),
        (
            "parent-only",
            vec![Raw::directory("top/..")],
            Some("refused: Unexpected member in upstream source archive: top/.."),
        ),
        (
            "absolute",
            vec![Raw::file("/tmp/fenix-tools-escaped", 0o644, b"x")],
            Some("refused: Unexpected member in upstream source archive: /tmp/fenix-tools-escaped"),
        ),
        (
            "character-device",
            vec![Raw::kind("top/null", b'3', "")],
            Some("refused: Unexpected member in upstream source archive: top/null"),
        ),
        (
            "block-device",
            vec![Raw::kind("top/disk", b'4', "")],
            Some("refused: Unexpected member in upstream source archive: top/disk"),
        ),
        (
            "fifo",
            vec![Raw::kind("top/pipe", b'6', "")],
            Some("refused: Unexpected member in upstream source archive: top/pipe"),
        ),
        (
            "hard-link",
            vec![
                Raw::file("top/file", 0o644, b"x"),
                Raw::kind("top/second-name", b'1', "top/file"),
            ],
            Some("refused: Unexpected member in upstream source archive: top/second-name"),
        ),
        (
            "link-absolute",
            vec![Raw::link("top/passwords", "/etc/passwd")],
            Some("refused: Source archive symlink escapes its root"),
        ),
        (
            "link-parent",
            vec![Raw::link("top/out", "..")],
            Some("refused: Source archive symlink escapes its root"),
        ),
        (
            "link-parent-deeper",
            vec![Raw::link("top/a/b/out", "../../../outside/file")],
            Some("refused: Source archive symlink escapes its root"),
        ),
        (
            // Harmless when it is made, leading outside once its parts exist.
            "link-that-changes",
            vec![
                Raw::link("top/a", "b/c/../.."),
                Raw::directory("top/b/"),
                Raw::link("top/b/c", "."),
                Raw::file("top/a/escaped", 0o644, b"x"),
            ],
            Some("refused: Source archive path escapes its root"),
        ),
        (
            "file-twice",
            vec![
                Raw::file("top/file", 0o644, b"first\n"),
                Raw::file("top/file", 0o644, b"second\n"),
            ],
            None,
        ),
        (
            "file-over-link",
            vec![
                Raw::link("top/name", "elsewhere"),
                Raw::file("top/name", 0o644, b"written through a link?\n"),
            ],
            None,
        ),
        (
            "link-over-file",
            vec![
                Raw::file("top/name", 0o644, b"first\n"),
                Raw::link("top/name", "elsewhere"),
            ],
            None,
        ),
        (
            "directory-over-file",
            vec![
                Raw::file("top/name", 0o644, b"first\n"),
                Raw::directory("top/name/"),
            ],
            None,
        ),
        (
            "file-below-file",
            vec![
                Raw::file("top/name", 0o644, b"first\n"),
                Raw::file("top/name/child", 0o644, b"second\n"),
            ],
            None,
        ),
        ("link-to-nothing", vec![Raw::link("top/name", "")], None),
    ];
    let scripts = repository().unwrap().join("scripts");
    for (name, members, expected) in &cases {
        let mut results = Vec::new();
        for side in [&pair.python, &pair.rust] {
            let place = side.join("extract").join(name);
            fs::create_dir_all(&place).unwrap();
            let archive = place.join("archive.tar.gz");
            put(&archive, gzip(&tar(members)));
            let target = place.join("root/target");
            let result = if side == &pair.python {
                let outcome = pair.interpret(&[
                    "-c",
                    EXTRACT,
                    scripts.to_str().unwrap(),
                    archive.to_str().unwrap(),
                    target.to_str().unwrap(),
                ]);
                assert_eq!(outcome.code, 0, "{name}: {}", outcome.output);
                outcome.output.trim_end().to_string()
            } else {
                match archive::extract(&archive, &target) {
                    Ok(()) => "extracted".to_string(),
                    Err(error) => format!("refused: {error}"),
                }
            };
            results.push(result.replace(side.to_str().unwrap(), "<SIDE>"));
            // Nothing beside the target, whatever the archive asked for.
            assert_eq!(
                fs::read_dir(place.join("root")).unwrap().count(),
                1,
                "{name}"
            );
        }
        assert_eq!(results[1], results[0], "{name}");
        if let Some(expected) = expected {
            assert_eq!(results[1], *expected, "{name}");
        } else {
            assert!(
                results[1].starts_with("refused: [Errno "),
                "{name}: {}",
                results[1]
            );
        }
        pair.same_files(name, &format!("extract/{name}/root"));
    }
    assert!(!Path::new("/tmp/fenix-tools-escaped").exists());
    assert!(
        pair.rust
            .join(format!("extract/long-name/root/target/{}", &long[4..]))
            .is_file()
    );
    // The target itself must be new.
    let archive = pair.lab.base.join("archive.tar.gz");
    put(&archive, wine_archive("top"));
    let existing = pair.rust.join("extract/plain/root/target");
    assert_eq!(
        archive::extract(&archive, &existing)
            .unwrap_err()
            .to_string(),
        format!("[Errno 17] File exists: '{}'", existing.display())
    );
    let outcome = pair.interpret(&[
        "-c",
        EXTRACT,
        scripts.to_str().unwrap(),
        archive.to_str().unwrap(),
        pair.python
            .join("extract/plain/root/target")
            .to_str()
            .unwrap(),
    ]);
    assert_eq!(
        outcome.output.trim_end(),
        format!(
            "refused: [Errno 17] File exists: '{}'",
            pair.python.join("extract/plain/root/target").display()
        )
    );
}

#[test]
fn build_agrees_from_the_patches_to_the_recorded_hashes() {
    let Some(pair) = Pair::new() else { return };
    // The real patch, which prints what it does.
    let (_, output) = pair.same("prepare", "build.py", "build", &["--prepare-only"]);
    assert_eq!(
        output,
        "patching file README\npatching file dlls/new/added.c\n"
    );
    let entries = pair.same_files("prepare", "repo");
    assert!(entries > 80, "{entries}");
    assert_eq!(
        read(&pair.rust.join("repo/build/wine/README")),
        "wine readme\nline 2\nline three\n"
    );

    let (code, output) = pair.same("existing work", "build.py", "build", &["--prepare-only"]);
    assert_eq!(
        (code, output.as_str()),
        (
            1,
            "fenix-tools: Use an empty build directory; existing work is never overwritten\n"
        )
    );

    // The whole build with the stand-ins: first refused for its hashes, then recorded, then matching.
    let (code, output) = pair.same(
        "build",
        "build.py",
        "build",
        &["--build-dir", "{side}/repo/work/one", "--jobs", "3"],
    );
    assert_eq!(code, 1);
    assert!(
        output.ends_with("review before --record-build and repin Flightdeck.\n"),
        "{output}"
    );
    pair.same_files("build", "repo");
    let (code, _) = pair.same(
        "record",
        "build.py",
        "build",
        &["--record-build", "--build-dir", "{side}/repo/work/two"],
    );
    assert_eq!(code, 0);
    pair.same_files("record", "repo");
    assert_eq!(
        read(&pair.rust.join("repo/bundle.json")),
        read(&pair.python.join("repo/bundle.json"))
    );
    let (code, _) = pair.same(
        "recorded",
        "build.py",
        "build",
        &["--build-dir", "{side}/repo/work/three"],
    );
    assert_eq!(code, 0);
    pair.same_files("recorded", "repo");

    // A patch that does not apply ends the build with patch's own status.
    pair.on_both(|side| {
        put(
            &side.join("repo/patches/second.patch"),
            "--- a/README\n+++ b/README\n@@ -1 +1 @@\n-not what is there\n+x\n",
        );
        Tree {
            root: side.join("repo"),
        }
        .seal(None);
    });
    let (code, output) = pair.same(
        "failing patch",
        "build.py",
        "build",
        &["--prepare-only", "--build-dir", "{side}/repo/work/four"],
    );
    assert_eq!(code, 1);
    assert!(
        output.contains("' returned non-zero exit status 1.\n"),
        "{output}"
    );
    pair.same_files("failing patch", "repo");

    for (name, message) in [
        (
            "patches/first.patch",
            "Patch checksum mismatch: first.patch",
        ),
        (
            "sources/wine-source.tar.gz",
            "Source checksum mismatch: wine-source.tar.gz",
        ),
    ] {
        pair.on_both(|side| {
            let path = side.join("repo").join(name);
            let mut content = fs::read(&path).unwrap();
            content.push(b'\n');
            put(&path, content);
        });
        let (code, output) = pair.same(
            name,
            "build.py",
            "build",
            &["--prepare-only", "--build-dir", "{side}/repo/work/five"],
        );
        assert_eq!((code, output), (1, format!("fenix-tools: {message}\n")));
    }
    pair.same_files("refused", "repo");
}

#[test]
fn build_proton_agrees_from_the_patches_to_the_recorded_hashes() {
    let Some(pair) = Pair::new() else { return };
    let proton = |what: &str, arguments: &[&str]| {
        let mut all = vec![VARIANT];
        all.extend_from_slice(arguments);
        let found = pair.same(what, "build-proton.py", "build-proton", &all);
        pair.same_files(what, "repo");
        found
    };
    let (code, output) = proton(
        "prepare",
        &["--build-dir", "{side}/repo/work/one", "--prepare-only"],
    );
    assert_eq!((code, output.as_str()), (0, "patching file README\n"));
    assert_eq!(
        read(&pair.rust.join("repo/work/one/wine/README")),
        "wine readme\nline two\nline three\nported to this Proton\n"
    );
    let (code, output) = proton(
        "existing directory",
        &["--build-dir", "{side}/repo/work/one", "--prepare-only"],
    );
    assert_eq!(
        (code, output.as_str()),
        (1, "fenix-tools: Use a fresh build directory\n")
    );

    let (code, output) = proton(
        "build",
        &["--build-dir", "{side}/repo/work/two", "--jobs", "7"],
    );
    assert_eq!(
        (code, output.as_str()),
        (
            1,
            "patching file README\nfenix-tools: Build hashes differ; review before recording a new build\n"
        )
    );
    let (code, _) = proton(
        "record",
        &["--build-dir", "{side}/repo/work/three", "--record-build"],
    );
    assert_eq!(code, 0);
    assert_eq!(
        read(&pair.rust.join("repo/bundle.json")),
        read(&pair.python.join("repo/bundle.json"))
    );
    let (code, _) = proton("recorded", &["--build-dir", "{side}/repo/work/four"]);
    assert_eq!(code, 0);

    let found = pair.same(
        "unknown variant",
        "build-proton.py",
        "build-proton",
        &["no-such-proton", "--build-dir", "{side}/repo/work/five"],
    );
    assert_eq!(found, (1, "fenix-tools: 'no-such-proton'\n".to_string()));

    pair.edit_lock(|lock| lock["variants"][VARIANT]["protocol"] = 865.into());
    let (code, output) = proton(
        "protocol",
        &["--build-dir", "{side}/repo/work/six", "--prepare-only"],
    );
    assert_eq!(
        (code, output.as_str()),
        (
            1,
            "patching file README\nfenix-tools: Unexpected Wine server protocol\n"
        )
    );
    pair.edit_lock(|lock| lock["variants"][VARIANT]["protocol"] = 931.into());
    for (name, message) in [
        ("patches/proton/x.patch", "Proton port checksum mismatch"),
        ("sources/x-wine.tar.gz", "Wine source checksum mismatch"),
    ] {
        pair.on_both(|side| {
            let path = side.join("repo").join(name);
            let mut content = fs::read(&path).unwrap();
            content.push(b'\n');
            put(&path, content);
        });
        let (code, output) = proton(
            name,
            &["--build-dir", "{side}/repo/work/seven", "--prepare-only"],
        );
        assert_eq!((code, output), (1, format!("fenix-tools: {message}\n")));
        pair.on_both(|side| fs::remove_dir_all(side.join("repo/work/seven")).unwrap_or(()));
    }
}

/// Runners, the runtime and the display for both sides of a smoke test.
fn smoke_pair() -> Option<Pair> {
    let mut pair = Pair::new()?;
    let mut states = None;
    pair.on_both(|side| {
        runner(&side.join("runner"), "1700000000 synthetic-runner-1.0");
        runner(&side.join("runner-x"), "1800000000 synthetic-proton-x");
        Tree {
            root: side.join("repo"),
        }
        .seal(Some((&side.join("runner"), &side.join("runner-x"))));
        steam_runtime(&side.join("SteamLinuxRuntime_sniper"));
    });
    for side in [&pair.python, &pair.rust] {
        states = Some(windows(side));
    }
    let display = Display::start(states.unwrap());
    pair.extra.push(("DISPLAY", display.name.clone()));
    pair.display = Some(display);
    Some(pair)
}

fn set(pair: &mut Pair, name: &'static str, value: &str) {
    pair.extra.retain(|(known, _)| *known != name);
    pair.extra.push((name, value.to_string()));
}

#[test]
fn smoke_agrees_from_the_copied_runner_to_the_last_wineserver() {
    let Some(mut pair) = smoke_pair() else { return };
    let smoke = |pair: &Pair, work: &str, more: &[&str]| {
        let directory = format!("{{side}}/{work}");
        let mut arguments = vec!["--runner", "{side}/runner", "--work", directory.as_str()];
        arguments.extend_from_slice(more);
        let found = pair.same(work, "smoke.py", "smoke", &arguments);
        pair.same_files(work, work);
        found
    };
    // Python talks to the display through libX11, the tool through x11rb.
    let (code, output) = smoke(
        &pair,
        "whole",
        &[
            "--steam-runtime",
            "{side}/SteamLinuxRuntime_sniper",
            "--cache",
            "{side}/cache",
        ],
    );
    assert_eq!(code, 0, "{output}");
    assert!(
        output.contains("X11 scope: FenixDisplay.exe Fenix Settings\n"),
        "{output}"
    );
    assert_eq!(
        pair.display.as_ref().unwrap().asked.lock().unwrap().len(),
        2 * 16,
        "each side asked about every reported window"
    );
    let (code, output) = smoke(&pair, "whole", &[]);
    assert_eq!(
        (code, output.as_str()),
        (1, "fenix-tools: Use a fresh smoke directory\n")
    );

    // The overlay of a Proton with its runner, at the default places.
    let found = pair.same(
        "variant",
        "smoke.py",
        "smoke",
        &["--runner", "{side}/runner-x", "--variant", VARIANT],
    );
    assert_eq!(found.0, 0, "{}", found.1);
    pair.same_files("variant", "repo/build");
    let found = pair.same(
        "wrong runner",
        "smoke.py",
        "smoke",
        &["--runner", "{side}/runner", "--variant", VARIANT],
    );
    assert_eq!(
        found,
        (
            1,
            "fenix-tools: The Wine runner version does not match this Fenix build.\n".to_string()
        )
    );
    let found = pair.same(
        "unknown variant",
        "smoke.py",
        "smoke",
        &["--runner", "{side}/runner", "--variant", "no-such-proton"],
    );
    assert_eq!(found.0, 1);
    let found = pair.same(
        "no runner",
        "smoke.py",
        "smoke",
        &["--runner", "{side}/absent"],
    );
    assert_eq!(found.0, 1);

    // Failures on the way: the runtime, the loader, a probe.
    set(&mut pair, "FAKE_CONTAINER_FAILS", "1");
    let (code, output) = smoke(
        &pair,
        "container",
        &["--steam-runtime", "{side}/SteamLinuxRuntime_sniper"],
    );
    assert_eq!(code, 1);
    assert!(
        output.ends_with("The patched runner does not start inside SteamLinuxRuntime_sniper\n"),
        "{output}"
    );
    set(&mut pair, "FAKE_CONTAINER_FAILS", "");
    set(&mut pair, "FAKE_LOADER_REGRESSION", "1");
    let (code, output) = smoke(&pair, "loader", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("fenix-tools: Loader spelling regression\n"),
        "{output}"
    );
    set(&mut pair, "FAKE_LOADER_REGRESSION", "");
    set(
        &mut pair,
        "FAKE_FAILING_PROBE",
        "audio-notification-probe.exe",
    );
    let (code, output) = smoke(&pair, "probe", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("' returned non-zero exit status 3.\n"),
        "{output}"
    );
    set(&mut pair, "FAKE_FAILING_PROBE", "");

    // What the display says decides.
    let states = pair.display.as_ref().unwrap().windows.clone();
    states.lock().unwrap().insert(0x40_0002, 2);
    let (code, output) = smoke(&pair, "mapped", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("fenix-tools: Unexpected X11 state: helper=1, Win32=1, mapped=2\n"),
        "{output}"
    );
    states.lock().unwrap().insert(0x40_0002, 0);
    states.lock().unwrap().insert(0x40_0009, 0);
    let (code, output) = smoke(&pair, "unmapped", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("fenix-tools: Unexpected X11 state: helper=0, Win32=1, mapped=0\n"),
        "{output}"
    );
    states.lock().unwrap().insert(0x40_0009, 2);
    set(&mut pair, "FAKE_PROBE_STATUS", "5");
    let (code, output) = smoke(&pair, "status", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("fenix-tools: Incomplete window mapping probe\n"),
        "{output}"
    );
    set(&mut pair, "FAKE_PROBE_STATUS", "0");

    // No display, and one that nobody listens on.
    set(&mut pair, "DISPLAY", "");
    let (code, output) = smoke(&pair, "unset", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with(
            "fenix-tools: X11 window checks require DISPLAY (Xwayland or Xvfb is sufficient)\n"
        ),
        "{output}"
    );
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    set(
        &mut pair,
        "DISPLAY",
        &format!("127.0.0.1:{}", closed - 6000),
    );
    let (code, output) = smoke(&pair, "closed", &[]);
    assert_eq!(code, 1);
    assert!(
        output.ends_with("fenix-tools: Cannot open test X11 display\n"),
        "{output}"
    );

    // A library the runner did not need.
    set(&mut pair, "DISPLAY", "");
    pair.on_both(|side| {
        put(
            &side.join("repo/payload/files/lib/wine/x86_64-unix/dwrite.so"),
            "NEEDED libfreetype.so.6\nNEEDED libgcc_s.so.1\nNEEDED libbz2.so.1.0\nNEEDED libc.so.6\n",
        );
        Tree {
            root: side.join("repo"),
        }
        .seal(Some((&side.join("runner"), &side.join("runner-x"))));
    });
    let (code, output) = smoke(&pair, "libraries", &[]);
    assert_eq!(
        (code, output.as_str()),
        (
            1,
            "fenix-tools: files/lib/wine/x86_64-unix/dwrite.so needs libbz2.so.1.0, libfreetype.so.6, \
             which the runner it replaces does not\n"
        )
    );
}

/// What Python's `tarfile` and `zipfile` read from an archive: one line of
/// JSON per member, with everything that describes it.
const LIST: &str = r#"
import hashlib, json, sys, tarfile, zipfile
path = sys.argv[1]
if path.endswith(".zip"):
    with zipfile.ZipFile(path) as archive:
        assert archive.testzip() is None
        for item in archive.infolist():
            print(json.dumps([item.filename, list(item.date_time), item.external_attr, item.compress_type,
                              item.create_system, item.flag_bits, item.file_size, item.CRC, item.extra.hex(),
                              item.comment.hex(), item.internal_attr, hashlib.sha256(archive.read(item)).hexdigest(),
                              item.create_version, item.extract_version]))
else:
    with tarfile.open(path) as archive:
        for item in archive.getmembers():
            data = archive.extractfile(item).read() if item.isfile() else b""
            print(json.dumps([item.name, item.mode, item.uid, item.gid, item.uname, item.gname, item.mtime,
                              item.size, item.type.decode(), item.linkname, item.pax_headers, item.devmajor,
                              item.devminor, hashlib.sha256(data).hexdigest()]))
"#;

/// The members of an archive as Python reads them, by name, in their order.
fn listed(pair: &Pair, archive: &Path) -> Vec<(String, String)> {
    let outcome = pair.interpret(&["-c", LIST, archive.to_str().unwrap()]);
    assert_eq!(outcome.code, 0, "{}", outcome.output);
    outcome
        .output
        .lines()
        .map(|line| {
            let member = fenix_patch::json::loads(line).unwrap();
            (member[0].as_str().unwrap().to_string(), line.to_string())
        })
        .collect()
}

/// The header blocks of every member of a tar archive, by name, in their
/// order: an extended header counts to the member it describes.
fn tar_headers(archive: &Path) -> Vec<(String, Vec<u8>)> {
    let mut plain = Vec::new();
    GzDecoder::new(File::open(archive).unwrap())
        .read_to_end(&mut plain)
        .unwrap();
    assert_eq!(plain.len() % 10240, 0, "{}", archive.display());
    let mut members = Vec::new();
    let (mut at, mut pending, mut long) = (0, Vec::new(), None);
    while plain[at..at + 512].iter().any(|byte| *byte != 0) {
        let block = &plain[at..at + 512];
        let size =
            usize::from_str_radix(std::str::from_utf8(&block[124..135]).unwrap(), 8).unwrap();
        let data = &plain[at + 512..at + 512 + size];
        let end = at + 512 + size.div_ceil(512) * 512;
        pending.extend_from_slice(block);
        if block[156] == b'x' {
            pending.extend_from_slice(&plain[at + 512..end]);
            let record = std::str::from_utf8(data).unwrap();
            long = Some(
                record
                    .split_once(" path=")
                    .unwrap()
                    .1
                    .trim_end_matches('\n')
                    .to_string(),
            );
        } else {
            let short = block[..100].split(|byte| *byte == 0).next().unwrap();
            let name = long
                .take()
                .unwrap_or_else(|| String::from_utf8(short.to_vec()).unwrap());
            members.push((name, std::mem::take(&mut pending)));
        }
        at = end;
    }
    assert!(plain[at..].iter().all(|byte| *byte == 0));
    members
}

fn common<T: Clone + PartialEq + std::fmt::Debug>(
    what: &str,
    python: &[(String, T)],
    rust: &[(String, T)],
) -> usize {
    let names = |members: &[(String, T)]| -> Vec<String> {
        members.iter().map(|(name, _)| name.clone()).collect()
    };
    let (in_python, in_rust) = (names(python), names(rust));
    let shared = |members: &[(String, T)], other: &[String]| -> Vec<(String, T)> {
        members
            .iter()
            .filter(|(name, _)| other.contains(name))
            .cloned()
            .collect()
    };
    let (left, right) = (shared(python, &in_rust), shared(rust, &in_python));
    assert_eq!(right, left, "{what}");
    left.len()
}

/// What the tool put into its two archives, and how much of it Python has as well.
struct Compared {
    source: Vec<String>,
    binary: Vec<String>,
    common_source: usize,
    common_binary: usize,
}

/// Compare the releases of both sides: what they printed, the two small
/// files, and every member that both archives have. What only one side
/// ships must be what was decided: Python's own files there, the Rust
/// sources, the installer and the licences of its crates here.
fn compare_releases(pair: &Pair, python: (&Outcome, &Path), rust: (&Outcome, &Path)) -> Compared {
    let stem = format!("fenix-a320-linux-patch-{VERSION}");
    let ((python, python_out), (rust, rust_out)) = (python, rust);
    assert_eq!(
        (python.code, rust.code),
        (0, 0),
        "{}\n{}",
        python.output,
        rust.output
    );

    // The printed lines: name, size, "bytes", the ZIP first.
    let sizes = |output: &str, directory: &Path| {
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "{output}");
        for (line, suffix) in lines.iter().zip(["-linux-x86_64.zip", "-source.tar.gz"]) {
            let name = format!("{stem}{suffix}");
            let size = fs::metadata(directory.join(&name)).unwrap().len();
            assert_eq!(*line, format!("{name} {size} bytes"));
        }
    };
    sizes(&python.output, python_out);
    sizes(&rust.output, rust_out);

    // SHA256SUMS and flightdeck-release.json: the same but for the digests.
    let masked = |directory: &Path, name: &str| {
        let mut text = read(&directory.join(name));
        for archive in ["-linux-x86_64.zip", "-source.tar.gz"] {
            let digest = fsx::digest(&directory.join(format!("{stem}{archive}"))).unwrap();
            text = text.replace(&digest, &format!("<sha256 of {archive}>"));
        }
        text
    };
    for name in ["SHA256SUMS", "flightdeck-release.json"] {
        assert_eq!(masked(rust_out, name), masked(python_out, name), "{name}");
    }
    assert_eq!(
        masked(rust_out, "SHA256SUMS"),
        format!(
            "<sha256 of -linux-x86_64.zip>  {stem}-linux-x86_64.zip\n<sha256 of -source.tar.gz>  {stem}-source.tar.gz\n"
        )
    );
    assert!(
        masked(rust_out, "flightdeck-release.json")
            .contains("\"sha256\": \"<sha256 of -linux-x86_64.zip>\""),
        "the digest of the binary archive"
    );
    let files = |directory: &Path| {
        let mut names: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.contains(&stem) || !name.contains("fenix-a320-linux-patch-"))
            .collect();
        names.sort();
        names
    };
    assert_eq!(files(rust_out), files(python_out));

    // The source archive: header blocks byte for byte, and what Python reads.
    let source = format!("{stem}-source.tar.gz");
    let (python_headers, rust_headers) = (
        tar_headers(&python_out.join(&source)),
        tar_headers(&rust_out.join(&source)),
    );
    let common_source = common("tar headers", &python_headers, &rust_headers);
    let (python_members, rust_members) = (
        listed(pair, &python_out.join(&source)),
        listed(pair, &rust_out.join(&source)),
    );
    assert_eq!(
        common("tar members", &python_members, &rust_members),
        common_source
    );
    let names = |members: &[(String, String)]| -> Vec<String> {
        members
            .iter()
            .map(|(name, _)| name.strip_prefix(&format!("{stem}/")).unwrap().to_string())
            .collect()
    };
    let only_rust = |name: &String, python: &[String]| {
        python.contains(name)
            || ["Cargo.toml", "Cargo.lock", "fenix-patch"].contains(&name.as_str())
            || ["installer/", "tools/", "licenses/rust/"]
                .iter()
                .any(|folder| name.starts_with(folder))
    };
    let (in_python, source) = (names(&python_members), names(&rust_members));
    for name in &in_python {
        assert!(
            source.contains(name)
                || name.starts_with("fenix_patch/")
                || name.starts_with("scripts/"),
            "only Python ships the source {name}"
        );
    }
    for name in &source {
        assert!(
            only_rust(name, &in_python),
            "only Rust ships the source {name}"
        );
        assert_ne!(name, "fenix-patch", "the installer is no source");
    }

    // The binary archive, as Python reads both.
    let binary = format!("{stem}-linux-x86_64.zip");
    let (python_members, rust_members) = (
        listed(pair, &python_out.join(&binary)),
        listed(pair, &rust_out.join(&binary)),
    );
    let common_binary = common("zip members", &python_members, &rust_members);
    let (in_python, binary) = (names(&python_members), names(&rust_members));
    for name in &in_python {
        assert!(
            binary.contains(name)
                || name.starts_with("fenix_patch/")
                || name.starts_with("scripts/"),
            "only Python ships {name}"
        );
    }
    for name in &binary {
        assert!(only_rust(name, &in_python), "only Rust ships {name}");
    }
    Compared {
        source,
        binary,
        common_source,
        common_binary,
    }
}

#[test]
fn release_agrees_on_everything_that_did_not_change_by_design() {
    let Some(pair) = Pair::new() else { return };
    let installer = pair.rust.join("fenix-patch");
    script(&installer, "");
    fs::write(
        &installer,
        tiny_program(&format!("fenix-patch {VERSION}\n")),
    )
    .unwrap();
    cargo_metadata(&pair.rust, &pair.rust.join("repo"));
    let long = format!("docs/{}/{}.md", "a".repeat(40), "b".repeat(60));
    pair.on_both(|side| {
        for (name, content) in [
            (long.as_str(), "a name beyond the classic header\n"),
            ("docs/Gr\u{fc}\u{df}e.md", "a name outside ASCII\n"),
            ("docs/a-b/x.md", "sorted by folder, not by text\n"),
            ("docs/a/y.md", "sorted by folder, not by text\n"),
            ("tests/__pycache__/x.cpython-314.pyc", "skipped\n"),
            ("native/empty.c", ""),
        ] {
            put(&side.join("repo").join(name), content);
        }
        script(&side.join("repo/native/tool.sh"), "#!/bin/sh\n");
        fs::set_permissions(
            side.join("repo/docs/steam.md"),
            std::os::unix::fs::PermissionsExt::from_mode(0o4750),
        )
        .unwrap();
    });
    let release = |pair: &Pair| -> (Outcome, Outcome) {
        let python = pair.script("release.py", &[]);
        let rust = pair.tools(&[
            "release",
            "--installer",
            installer.to_str().unwrap(),
            "--output",
            pair.rust.join("out").to_str().unwrap(),
        ]);
        pair.calls(&pair.rust);
        (python, rust)
    };
    let (python, rust) = release(&pair);
    let compared = compare_releases(
        &pair,
        (&python, &pair.python.join("repo/dist")),
        (&rust, &pair.rust.join("out")),
    );
    assert!(compared.common_source >= 40, "{}", compared.common_source);
    assert!(compared.common_binary >= 65, "{}", compared.common_binary);
    for expected in [
        long.as_str(),
        "docs/Gr\u{fc}\u{df}e.md",
        "sources/x-wine.tar.gz",
        "integration/launch-msfs.sh",
        "Fenix A320 Linux Patch.desktop",
    ] {
        assert!(
            compared.source.iter().any(|name| name == expected),
            "{expected}"
        );
    }
    for expected in [
        "payload/files/bin/wineserver".to_string(),
        format!("payload/variants/{VARIANT}/files/bin/wineserver"),
        "integration/FenixWindowGuard.exe".to_string(),
        "fenix-patch".to_string(),
        "licenses/rust/INVENTORY.json".to_string(),
    ] {
        assert!(compared.binary.contains(&expected), "{expected}");
    }

    // Every refusal, in the same words.
    let refused = |what: &str| -> String {
        let (python, rust) = release(&pair);
        let expected = pair.translated(&python);
        let found = (
            rust.code,
            rust.output.replace(pair.rust.to_str().unwrap(), "<SIDE>"),
        );
        assert_eq!(found, expected, "{what}");
        assert_eq!(found.0, 1, "{what}");
        found.1
    };
    let with = |name: &str, content: &[u8], what: &str| -> String {
        let original = fs::read(pair.rust.join("repo").join(name)).ok();
        pair.on_both(|side| put(&side.join("repo").join(name), content));
        let message = refused(what);
        pair.on_both(|side| match &original {
            Some(original) => put(&side.join("repo").join(name), original),
            None => fs::remove_file(side.join("repo").join(name)).unwrap(),
        });
        message
    };
    for (name, message) in [
        (
            "payload/files/lib/wine/x86_64-windows/d2d1.dll",
            "Patch checksum mismatch: files/lib/wine/x86_64-windows/d2d1.dll",
        ),
        (
            "payload/variants/proton-x/files/lib/wine/x86_64-unix/ntdll.so",
            "Patch checksum mismatch: variants/proton-x/files/lib/wine/x86_64-unix/ntdll.so",
        ),
        (
            "integration/FenixMCDURefresh.exe",
            "Integration checksum mismatch: FenixMCDURefresh.exe",
        ),
        (
            "integration/launch-msfs.sh",
            "Integration checksum mismatch: launch-msfs.sh",
        ),
        (
            "sources/wine-source.tar.gz",
            "Source checksum mismatch: wine-source.tar.gz",
        ),
        (
            "sources/x-wine.tar.gz",
            "Source checksum mismatch: x-wine.tar.gz",
        ),
        (
            "patches/first.patch",
            "Source checksum mismatch: first.patch",
        ),
        (
            "patches/proton/x.patch",
            "Source checksum mismatch: proton/x.patch",
        ),
    ] {
        assert_eq!(
            with(name, b"changed\n", name),
            format!("fenix-tools: {message}\n")
        );
    }

    // Assembled here, so that this file is no private content itself.
    let home = format!("/home/{}/{}", "someone", "Work");
    let runtimes = format!(
        "/home/{}/.local/share/{}/runtimes",
        "some-one_2", "flightdeck"
    );
    let key = format!("-----BEGIN {} KEY-----", "PRIVATE");
    let token = format!("XBL3.0 x={};{}", "1234567890", "aB3_-".repeat(6));
    for (name, content) in [
        ("docs/notes.md", format!("built in {home}/fenix\n")),
        ("native/window-guard.c", format!("// see {runtimes}/x\n")),
        ("tests/key.pem", format!("{key}\nMIIE\n")),
        (
            ".github/workflows/ci.yml",
            format!("env:\n  TOKEN: {token}\n"),
        ),
        ("README.md", format!("\0binary\0{home}\0")),
        ("licenses/notes.txt", format!("{home}shop\n")),
    ] {
        assert_eq!(
            with(name, content.as_bytes(), name),
            format!("fenix-tools: Private content in export: {name}\n")
        );
    }
    // Close to the patterns: accepted by both.
    let short = format!("XBL3.0 x={};{}", "1", "a".repeat(29));
    pair.on_both(|side| {
        put(
            &side.join("repo/docs/close.md"),
            format!(
                "/home/{}/Work\n/home/Work\n/home//Work\n{short}\n-----BEGIN PUBLIC KEY-----\n",
                "a b"
            ),
        );
    });
    let (python, rust) = release(&pair);
    assert_eq!(
        (python.code, rust.code),
        (0, 0),
        "{}\n{}",
        python.output,
        rust.output
    );

    for link in ["docs/linked.md", "tests/deep/er/link", "native/dangling"] {
        pair.on_both(|side| {
            let path = side.join("repo").join(link);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink("/nonexistent/target", path).unwrap();
        });
        assert_eq!(refused(link), "fenix-tools: Symlink in source export\n");
        pair.on_both(|side| fs::remove_file(side.join("repo").join(link)).unwrap());
    }
    for name in ["README.md", ".gitignore", "Fenix A320 Linux Patch.desktop"] {
        pair.on_both(|side| {
            fs::rename(side.join("repo").join(name), side.join("kept")).unwrap();
        });
        let message = format!("fenix-tools: Missing source: <SIDE>/repo/{name}\n");
        assert_eq!(refused(name), message);
        pair.on_both(|side| symlink(side.join("kept"), side.join("repo").join(name)).unwrap());
        assert_eq!(refused(name), message);
        pair.on_both(|side| {
            fs::remove_file(side.join("repo").join(name)).unwrap();
            fs::rename(side.join("kept"), side.join("repo").join(name)).unwrap();
        });
    }
    pair.edit_lock(|lock| lock["version"] = "0.0.0-other".into());
    assert_eq!(
        refused("version"),
        "fenix-tools: Installer and bundle versions differ\n"
    );
}

/// Where the pinned source archives are: `sources/` of this repository or of
/// the checkout next to it, or the directory `FENIX_SOURCES` names.
fn pinned_sources(names: &[String]) -> Option<PathBuf> {
    let worktree = repository().unwrap();
    let candidates = [
        std::env::var_os("FENIX_SOURCES").map(PathBuf::from),
        Some(worktree.join("sources")),
        worktree
            .parent()
            .map(|parent| parent.join("fenix-a320-linux-patch/sources")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|directory| names.iter().all(|name| directory.join(name).is_file()))
}

#[test]
#[ignore = "needs the pinned source archives (222 MB) and 3 GB of disk: extracts and patches each Wine source with both"]
fn the_pinned_sources_are_prepared_alike() {
    let Some(pair) = Pair::new() else { return };
    let worktree = repository().unwrap();
    let lock = fenix_patch::json::loads(&read(&worktree.join("bundle.json"))).unwrap();
    let variants: Vec<String> = lock["variants"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut names: Vec<String> = lock["sources"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    for variant in &variants {
        names.extend(
            lock["variants"][variant]["sources"]
                .as_object()
                .unwrap()
                .keys()
                .cloned(),
        );
    }
    let Some(sources) = pinned_sources(&names) else {
        notice(
            "the pinned source archives were not found (set FENIX_SOURCES); the comparison of the prepared sources is skipped",
        );
        return;
    };
    // The real manifest and patches in place of the synthetic ones; the real patch and perl.
    pair.on_both(|side| {
        let repo = side.join("repo");
        fs::copy(worktree.join("bundle.json"), repo.join("bundle.json")).unwrap();
        fs::remove_dir_all(repo.join("patches")).unwrap();
        copy_tree(&worktree.join("patches"), &repo.join("patches"));
        fs::remove_dir_all(repo.join("sources")).unwrap();
        fs::create_dir(repo.join("sources")).unwrap();
        for name in &names {
            let copied = process::run_checked(
                &Cmd::new(argv![
                    "cp",
                    "-a",
                    "--reflink=auto",
                    "--",
                    sources.join(name),
                    repo.join("sources").join(name)
                ]),
                &|| Ok(()),
            );
            copied.unwrap();
        }
    });
    let mut compared = BTreeMap::new();
    let (code, output) = pair.same("build", "build.py", "build", &["--prepare-only"]);
    assert_eq!(code, 0, "{output}");
    compared.insert("xodus".to_string(), pair.same_files("build", "repo/build"));
    pair.on_both(|side| fs::remove_dir_all(side.join("repo/build")).unwrap());
    for variant in &variants {
        let (code, output) = pair.same(
            variant,
            "build-proton.py",
            "build-proton",
            &[variant, "--build-dir", "{side}/prepared", "--prepare-only"],
        );
        assert_eq!(code, 0, "{variant}: {output}");
        compared.insert(variant.clone(), pair.same_files(variant, "prepared"));
        pair.on_both(|side| fs::remove_dir_all(side.join("prepared")).unwrap());
    }
    for (name, entries) in &compared {
        notice(&format!("{name}: {entries} entries prepared alike"));
        assert!(*entries > 10_000, "{name}: {entries}");
    }
}

/// A checkout that has what a release is made of but Git does not track:
/// the built payload, the helper programs and the pinned source archives.
/// This repository itself, the checkout next to it, or what `FENIX_BUILT` names.
fn built_checkout(lock: &fenix_patch::json::Value) -> Option<PathBuf> {
    let worktree = repository().unwrap();
    let candidates = [
        std::env::var_os("FENIX_BUILT").map(PathBuf::from),
        Some(worktree.clone()),
        worktree
            .parent()
            .map(|parent| parent.join("fenix-a320-linux-patch")),
    ];
    let payload = lock["files"].as_object().unwrap().keys().next().unwrap();
    candidates.into_iter().flatten().find(|checkout| {
        checkout.join("payload").join(payload).is_file()
            && checkout.join("integration/FenixWindowGuard.exe").is_file()
            && lock["sources"]
                .as_object()
                .unwrap()
                .keys()
                .all(|name| checkout.join("sources").join(name).is_file())
    })
}

#[test]
#[ignore = "needs the built payload and the pinned sources (470 MB) and 2 GB of disk: releases the real repository with both"]
fn the_real_repository_is_released_alike() {
    let Some(pair) = Pair::new() else { return };
    let worktree = repository().unwrap();
    let lock = fenix_patch::json::loads(&read(&worktree.join("bundle.json"))).unwrap();
    let Some(built) = built_checkout(&lock) else {
        notice(
            "no checkout with payload, helper programs and sources was found (set FENIX_BUILT); the comparison of real releases is skipped",
        );
        return;
    };
    // What Git tracks, as a fresh checkout has it.
    let mut list = Cmd::new(argv!["git", "ls-files", "-z"]);
    list.cwd = Some(&worktree);
    list.output = Output::CaptureOutput;
    let Ok(tracked) = process::run_checked(&list, &|| Ok(())) else {
        notice("git could not list the tracked files; the comparison of real releases is skipped");
        return;
    };
    let tracked: Vec<String> = String::from_utf8(tracked.stdout)
        .unwrap()
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let copy = |from: &Path, to: &Path| {
        let cmd = Cmd::new(argv!["cp", "-a", "--reflink=auto", "--", from, to]);
        process::run_checked(&cmd, &|| Ok(())).unwrap();
    };
    pair.on_both(|side| {
        let repo = side.join("repo");
        fs::remove_dir_all(&repo).unwrap();
        for name in &tracked {
            fs::create_dir_all(repo.join(name).parent().unwrap()).unwrap();
            copy(&worktree.join(name), &repo.join(name));
        }
        copy(&built.join("payload"), &repo.join("payload"));
        copy(&built.join("sources"), &repo.join("sources"));
        for name in lock["integration"].as_object().unwrap().keys() {
            if name.ends_with(".exe") {
                copy(
                    &built.join("integration").join(name),
                    &repo.join("integration").join(name),
                );
            }
        }
    });
    // The real dependencies of the installer, as Cargo describes them here.
    let mut env = pair.lab.env(&pair.rust);
    env.set("PATH", std::env::var_os("PATH").unwrap_or_default());
    let world = World::new(&worktree, env);
    let graph = fenix_tools::licenses::metadata(&fenix_tools::Context::host(&world)).unwrap();
    put(
        &pair.rust.join("cargo-metadata.json"),
        fenix_patch::json::dumps(&graph, None),
    );
    let installer = pair.rust.join("fenix-patch");
    script(&installer, "");
    fs::write(
        &installer,
        tiny_program(&format!("fenix-patch {VERSION}\n")),
    )
    .unwrap();

    let python = pair.script("release.py", &[]);
    let rust = pair.tools(&[
        "release",
        "--installer",
        installer.to_str().unwrap(),
        "--output",
        pair.rust.join("out").to_str().unwrap(),
    ]);
    let compared = compare_releases(
        &pair,
        (&python, &pair.python.join("repo/dist")),
        (&rust, &pair.rust.join("out")),
    );
    notice(&format!(
        "real release: {} of {} sources and {} of {} files of the binary archive are in Python's as well, and equal",
        compared.common_source,
        compared.source.len(),
        compared.common_binary,
        compared.binary.len()
    ));
    notice(rust.output.trim_end());
    let crates = compared
        .binary
        .iter()
        .filter_map(|name| name.strip_prefix("licenses/rust/")?.split_once('/'))
        .map(|(folder, _)| folder)
        .collect::<std::collections::BTreeSet<_>>();
    notice(&format!("licence texts of {} crates", crates.len()));
    assert!(crates.len() >= 60, "{crates:?}");
    for name in lock["sources"].as_object().unwrap().keys() {
        assert!(
            compared.source.contains(&format!("sources/{name}")),
            "{name}"
        );
    }
    for name in lock["files"].as_object().unwrap().keys() {
        assert!(
            compared.binary.contains(&format!("payload/{name}")),
            "{name}"
        );
    }
    assert!(compared.common_source > 50 && compared.common_binary > 100);
}
