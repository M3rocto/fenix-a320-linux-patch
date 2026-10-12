// SPDX-License-Identifier: MIT
//! Temporary scaffolding for the move from Python to Rust: the same synthetic
//! installations are handed to `python3 -m fenix_patch` and to `fenix-patch`,
//! and everything they print and leave behind is compared.
//!
//! Delete this file together with the Python package `fenix_patch/`.
//! Without `python3` or without the package every test here is skipped and says so.
#![allow(clippy::unwrap_used)]
use fenix_patch::{
    Result,
    cli::{self, Cli},
    core::{self, Wine},
    fsx,
    json::{self, Value, json},
    steam,
    system::{Host, Progress, System},
    vdf, web,
};
use regex::Regex;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};

/// Every fixture file carries this modification time, so that a preserved
/// time can be told from a new one.
const FIXTURE_TIME: i64 = 1_500_000_000;
const CHILD: &str = "FENIX_PARITY_CHILD";

fn worktree() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn notice(text: &str) {
    // Written directly, so that the test harness does not swallow it.
    let _ = writeln!(std::io::stderr().lock(), "parity: {text}");
}

fn tool(name: &str) -> Option<PathBuf> {
    fsx::which(name, std::env::var_os("PATH"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Python,
    Rust,
}

const SIDES: [Side; 2] = [Side::Python, Side::Rust];

/// Two identical installer directories with their own worlds below them.
struct Lab {
    _temp: tempfile::TempDir,
    sandbox: PathBuf,
    python: PathBuf,
    problems: Vec<String>,
    /// When a step last could have saved a copy of Steam's configuration.
    saved: Option<std::time::Instant>,
    /// Normalised contents of files that did not change since they were last read.
    seen: RefCell<HashMap<PathBuf, (i64, i64, u64, String)>>,
}

#[derive(Default, Clone)]
struct Options {
    stdin: String,
    env: Vec<(String, String)>,
    /// Replace `prepare_geometry`, which needs Microsoft's downloads and real Wine, by nothing on both sides.
    stub_geometry: bool,
    /// Python ends with a traceback or argparse's usage text here; only the exit status is compared.
    stderr_differs: bool,
    /// The help text is argparse's on one side and clap's on the other.
    stdout_differs: bool,
}

/// Marks an environment variable that shall not be set at all.
const UNSET: &str = "<unset>";

#[derive(Debug)]
struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Lab {
    /// `None` when there is no Python to compare with.
    fn new() -> Option<Self> {
        let Some(python) = tool("python3") else {
            notice("python3 was not found; the comparison with the Python installer is skipped");
            return None;
        };
        if !worktree().join("fenix_patch/__main__.py").is_file() {
            notice(
                "the Python package fenix_patch is gone; delete installer/tests/parity.rs as well",
            );
            return None;
        }
        let temp = tempfile::Builder::new()
            .prefix("parity-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let sandbox = temp.path().canonicalize().unwrap();
        let lab = Self {
            _temp: temp,
            sandbox,
            python,
            problems: Vec::new(),
            saved: None,
            seen: RefCell::new(HashMap::new()),
        };
        // Only these programs can be reached: no browser, no file dialog, no real Wine.
        fs::create_dir(lab.sandbox.join("bin")).unwrap();
        for name in [
            "sh", "cp", "env", "sort", "sed", "grep", "readlink", "ls", "tr", "getconf", "sleep",
        ] {
            symlink(
                tool(name).unwrap_or_else(|| panic!("{name} is needed")),
                lab.sandbox.join("bin").join(name),
            )
            .unwrap();
        }
        for side in SIDES {
            fs::create_dir_all(lab.base(side).join("world/home")).unwrap();
            fs::create_dir_all(lab.base(side).join("world/tmp")).unwrap();
        }
        let package = lab.base(Side::Python).join("fenix_patch");
        fs::create_dir(&package).unwrap();
        for entry in fs::read_dir(worktree().join("fenix_patch")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "py") {
                fs::copy(&path, package.join(path.file_name().unwrap())).unwrap();
            }
        }
        let binary = lab.base(Side::Rust).join("fenix-patch");
        if fs::hard_link(env!("CARGO_BIN_EXE_fenix-patch"), &binary).is_err() {
            fs::copy(env!("CARGO_BIN_EXE_fenix-patch"), &binary).unwrap();
        }
        Some(lab)
    }

    /// The installer's own directory of one side; `py` and `rs` are equally long.
    fn base(&self, side: Side) -> PathBuf {
        self.sandbox.join(match side {
            Side::Python => "py",
            Side::Rust => "rs",
        })
    }

    fn environment(&self, side: Side, options: &Options) -> Vec<(String, String)> {
        let world = self.base(side).join("world");
        let mut env = vec![
            ("HOME".to_string(), world.join("home").display().to_string()),
            (
                "XDG_DATA_HOME".to_string(),
                world.join("home/.local/share").display().to_string(),
            ),
            (
                "PATH".to_string(),
                self.sandbox.join("bin").display().to_string(),
            ),
            ("LC_ALL".to_string(), "C.UTF-8".to_string()),
            (
                "TMPDIR".to_string(),
                world.join("tmp").display().to_string(),
            ),
            (
                "FAKE_LOG".to_string(),
                world.join("calls.log").display().to_string(),
            ),
        ];
        for (name, value) in &options.env {
            env.retain(|(known, _)| known != name);
            if value != UNSET {
                env.push((
                    name.clone(),
                    value.replace("{B}", &self.base(side).display().to_string()),
                ));
            }
        }
        env
    }

    /// The command line of one side, started in its installer directory.
    fn command(&self, side: Side, args: &[&str], options: &Options) -> Command {
        let base = self.base(side);
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.replace("{B}", &base.display().to_string()))
            .collect();
        let mut command = match (side, options.stub_geometry) {
            (Side::Python, false) => {
                let mut command = Command::new(&self.python);
                command.args(["-B", "-m", "fenix_patch"]).args(&args);
                command
            }
            (Side::Python, true) => {
                let mut command = Command::new(&self.python);
                command
                    .args(["-B", "-c", PYTHON_WITHOUT_GEOMETRY])
                    .args(&args);
                command
            }
            (Side::Rust, false) => {
                let mut command = Command::new(base.join("fenix-patch"));
                command.args(&args);
                command
            }
            // This test program itself, as the installer with one step replaced.
            (Side::Rust, true) => {
                let mut command = Command::new(std::env::current_exe().unwrap());
                command.args(["installer_without_geometry", "--exact", "--nocapture"]);
                command
            }
        };
        command
            .current_dir(&base)
            .env_clear()
            .envs(self.environment(side, options));
        if side == Side::Rust && options.stub_geometry {
            let request = json!({
                "root": base.display().to_string(), "args": args,
                "out": base.join("child.out").display().to_string(), "err": base.join("child.err").display().to_string(),
            });
            command.env(CHILD, json::dumps(&request, None));
        }
        command
    }

    fn run(&self, side: Side, args: &[&str], options: &Options) -> Outcome {
        let base = self.base(side);
        // Standard input is a file, so that a program which inherits it can be told from one that gets /dev/null.
        let input = self.sandbox.join(format!("stdin-{side:?}"));
        fs::write(&input, &options.stdin).unwrap();
        let output = self
            .command(side, args, options)
            .stdin(File::open(&input).unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        let code = output.status.code().unwrap_or(-1);
        if side == Side::Rust && options.stub_geometry {
            // The test harness owns the real output of this process; the installer wrote into files.
            let read = |name: &str| {
                let content = fs::read(base.join(name)).unwrap_or_else(|_| {
                    panic!("the Rust installer did not start: {}", text(&output.stderr))
                });
                fs::remove_file(base.join(name)).unwrap();
                text(&content)
            };
            return Outcome {
                code,
                stdout: read("child.out"),
                stderr: read("child.err"),
            };
        }
        Outcome {
            code,
            stdout: text(&output.stdout),
            stderr: text(&output.stderr),
        }
    }

    fn normal(&self, side: Side, text: &str) -> String {
        let base = self.base(side).display().to_string();
        // Also as Wine is given it: on drive Z: with backslashes.
        let text = text
            .replace(&base, "<BASE>")
            .replace(&base.replace('/', "\\"), "<BASE>");
        normalise(
            &text.replace(
                &self
                    .sandbox
                    .join(format!("stdin-{side:?}"))
                    .display()
                    .to_string(),
                "<STDIN>",
            ),
        )
    }

    /// Run one command line on both sides and compare what it printed, how it ended and what it left behind.
    fn step(&mut self, label: &str, args: &[&str], options: &Options) -> (Outcome, Outcome) {
        // A copy of Steam's configuration is named after the second it is made in. Once
        // one exists, the next step that can make one waits for another second, so that
        // both sides end up with the same number of copies.
        let saves = args
            .iter()
            .any(|arg| matches!(*arg, "select" | "restore" | "gui" | "--text"));
        if saves
            && self.has_saved_configuration()
            && let Some(remaining) = self
                .saved
                .and_then(|saved| Duration::from_millis(1100).checked_sub(saved.elapsed()))
        {
            std::thread::sleep(remaining);
        }
        let python = self.run(Side::Python, args, options);
        let rust = self.run(Side::Rust, args, options);
        if std::env::var_os("PARITY_TRACE").is_some() {
            // For reading along: what each side answered.
            for (side, outcome) in [(Side::Python, &python), (Side::Rust, &rust)] {
                notice(&format!(
                    "{label} [{side:?}] {args:?} -> {}\n{}{}",
                    outcome.code,
                    self.normal(side, &outcome.stdout),
                    self.normal(side, &outcome.stderr)
                ));
            }
        }
        if python.code != rust.code {
            self.problems.push(format!("{label}: exit status {} (Python) and {} (Rust)\n  Python stderr: {}\n  Rust stderr: {}", python.code, rust.code, python.stderr, rust.stderr));
        }
        if !options.stdout_differs {
            self.same_text(
                &format!("{label}: standard output"),
                &python.stdout,
                &rust.stdout,
            );
        }
        if !options.stderr_differs {
            self.same_text(
                &format!("{label}: standard error"),
                &python.stderr,
                &rust.stderr,
            );
        }
        self.same_trees(label);
        if saves {
            self.saved = Some(std::time::Instant::now());
        }
        // What the synthetic programs recorded has been compared; the next step starts a new record.
        self.both(|base| {
            let _ = fs::remove_file(base.join("world/calls.log"));
        });
        (python, rust)
    }

    fn has_saved_configuration(&self) -> bool {
        let world = self.base(Side::Python).join("world");
        let Ok(places) = fs::read_dir(world) else {
            return false;
        };
        places.flatten().any(|place| {
            let states = place.path().join("steamapps").join(steam::STATE);
            fs::read_dir(states)
                .into_iter()
                .flatten()
                .flatten()
                .any(|state| {
                    fs::read_dir(state.path())
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with("steam-config-before-")
                        })
                })
        })
    }

    fn same_text(&mut self, label: &str, python: &str, rust: &str) {
        let (python, rust) = (
            self.normal(Side::Python, python),
            self.normal(Side::Rust, rust),
        );
        if python != rust {
            self.problems.push(format!(
                "{label} differs{}",
                first_differences(&python, &rust)
            ));
        }
    }

    fn same_trees(&mut self, label: &str) {
        let python = self.snapshot(Side::Python);
        let rust = self.snapshot(Side::Rust);
        if python == rust {
            return;
        }
        let mut report = format!("{label}: the file trees differ");
        let names: std::collections::BTreeSet<&String> = python.keys().chain(rust.keys()).collect();
        for name in names {
            match (python.get(name), rust.get(name)) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), Some(b)) => {
                    for ((python_kind, python_content), (rust_kind, rust_content)) in
                        a.iter().zip(b)
                    {
                        if python_kind != rust_kind {
                            report.push_str(&format!(
                                "\n  {name}: Python has {python_kind}, Rust has {rust_kind}"
                            ));
                        }
                        if python_content != rust_content {
                            report.push_str(&format!(
                                "\n  {name}: content differs{}",
                                first_differences(python_content, rust_content)
                            ));
                        }
                    }
                    if a.len() != b.len() {
                        report.push_str(&format!(
                            "\n  {name}: {} time(s) in Python, {} in Rust",
                            a.len(),
                            b.len()
                        ));
                    }
                }
                (Some(a), None) => {
                    report.push_str(&format!("\n  {name}: only Python has it ({})", a[0].0))
                }
                (None, Some(b)) => {
                    report.push_str(&format!("\n  {name}: only Rust has it ({})", b[0].0))
                }
                (None, None) => {}
            }
        }
        self.problems.push(report);
    }

    /// Every file below one side's directory: kind, permission bits, whether its time is the fixture's, and content.
    /// Names that only differ in their random part end up under one name with several entries.
    fn snapshot(&self, side: Side) -> BTreeMap<String, Vec<(String, String)>> {
        let base = self.base(side);
        let mut entries: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        let mut pending = vec![base.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                let relative = path
                    .strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if directory == base
                    && matches!(
                        relative.as_str(),
                        "fenix_patch" | "fenix-patch" | "__pycache__"
                    )
                {
                    continue;
                }
                let info = fs::symlink_metadata(&path).unwrap();
                let time = if info.mtime() == FIXTURE_TIME {
                    "kept"
                } else {
                    "new"
                };
                let time = format!("{time}{}", attributes(&path));
                let mode = info.mode() & 0o7777;
                let entry = if info.file_type().is_symlink() {
                    (
                        format!("link, time {time}"),
                        self.normal(side, &fs::read_link(&path).unwrap().to_string_lossy()),
                    )
                } else if info.is_dir() {
                    pending.push(path);
                    (format!("directory {mode:o}, time {time}"), String::new())
                } else {
                    let stamp = (info.mtime(), info.mtime_nsec(), info.size());
                    let known = self
                        .seen
                        .borrow()
                        .get(&path)
                        .filter(|known| (known.0, known.1, known.2) == stamp)
                        .map(|known| known.3.clone());
                    let content = known.unwrap_or_else(|| {
                        let content = match String::from_utf8(fs::read(&path).unwrap()) {
                            Ok(text) => self.normal(side, &text),
                            Err(error) => format!("binary {}", fsx::digest_bytes(error.as_bytes())),
                        };
                        self.seen
                            .borrow_mut()
                            .insert(path.clone(), (stamp.0, stamp.1, stamp.2, content.clone()));
                        content
                    });
                    (format!("file {mode:o}, time {time}"), content)
                };
                entries
                    .entry(self.normal(side, &relative))
                    .or_default()
                    .push(entry);
            }
        }
        for same in entries.values_mut() {
            same.sort();
        }
        entries
    }

    /// Change both worlds in the same way.
    fn both(&self, change: impl Fn(&Path)) {
        for side in SIDES {
            change(&self.base(side));
        }
    }

    fn finish(self) {
        assert!(
            self.problems.is_empty(),
            "{} difference(s) between Python and Rust:\n\n{}",
            self.problems.len(),
            self.problems.join("\n\n")
        );
    }

    /// Evaluate a Python expression over `fenix_patch` with one JSON argument and return its JSON result.
    fn python_eval(&self, body: &str, argument: &Value) -> Value {
        self.python_eval_with(body, argument, &Options::default())
    }

    fn python_eval_with(&self, body: &str, argument: &Value, options: &Options) -> Value {
        let script = format!(
            "import json, sys\nfrom pathlib import Path\nfrom fenix_patch import core, steam, vdf, web, guide\nargument = json.loads(sys.stdin.read())\n{body}\nprint(json.dumps(result))\n"
        );
        let mut child = Command::new(&self.python)
            .args(["-B", "-c", &script])
            .current_dir(self.base(Side::Python))
            .env_clear()
            .envs(self.environment(Side::Python, options))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json::dumps(argument, None).as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "Python failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        json::loads(&String::from_utf8(output.stdout).unwrap()).unwrap()
    }
}

/// The extended attributes of a file that a user can set, as text.
fn attributes(path: &Path) -> String {
    let mut names = vec![0u8; 4096];
    let Ok(length) = rustix::fs::llistxattr(path, names.as_mut_slice()) else {
        return String::new();
    };
    let mut found = Vec::new();
    for name in names[..length]
        .split(|byte| *byte == 0)
        .filter(|name| name.starts_with(b"user."))
    {
        let name = String::from_utf8_lossy(name).into_owned();
        let mut value = vec![0u8; 4096];
        let length = rustix::fs::lgetxattr(path, name.as_str(), value.as_mut_slice()).unwrap_or(0);
        found.push(format!(
            "{name}={}",
            String::from_utf8_lossy(&value[..length])
        ));
    }
    found.sort();
    if found.is_empty() {
        String::new()
    } else {
        format!(", attributes {}", found.join(" "))
    }
}

/// Mark a fixture file with an extended attribute, where the file system has them.
fn tag(path: &Path, value: &str) {
    let _ = rustix::fs::setxattr(
        path,
        "user.fenix.parity",
        value.as_bytes(),
        rustix::fs::XattrFlags::empty(),
    );
}

/// The first lines in which two texts differ.
fn first_differences(python: &str, rust: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (python.split('\n').collect(), rust.split('\n').collect());
    let mut out = String::new();
    let mut shown = 0;
    for index in 0..a.len().max(b.len()) {
        let (left, right) = (a.get(index), b.get(index));
        if left != right {
            out.push_str(&format!(
                "\n    line {}:\n      Python: {:?}\n      Rust:   {:?}",
                index + 1,
                left.unwrap_or(&"<none>"),
                right.unwrap_or(&"<none>")
            ));
            shown += 1;
            if shown == 8 {
                out.push_str("\n    …");
                break;
            }
        }
    }
    out
}

/// The Python installer with `prepare_geometry` replaced by nothing.
const PYTHON_WITHOUT_GEOMETRY: &str = "import sys\nfrom unittest.mock import patch\nfrom fenix_patch import core, __main__\nwith patch.object(core, 'prepare_geometry'):\n    sys.exit(__main__.main(sys.argv[1:]))\n";

struct WithoutGeometry(Host);

impl System for WithoutGeometry {
    fn root(&self) -> &Path {
        self.0.root()
    }

    fn prepare_geometry(
        &self,
        _wine: &dyn Wine,
        _cache: &Path,
        _bundle: &Path,
        _progress: Progress<'_>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Downloads that write the address into the file instead of fetching it.
struct WithoutNetwork(Host);

impl System for WithoutNetwork {
    fn root(&self) -> &Path {
        self.0.root()
    }

    fn download(
        &self,
        url: &str,
        destination: &Path,
        _expected: &str,
        _max_size: u64,
    ) -> Result<PathBuf> {
        put(destination, url);
        Ok(destination.to_path_buf())
    }
}

/// The steps that need Microsoft's downloads, run directly with synthetic downloads.
fn setup_steps(root: &Path, first_version: &str) -> Value {
    let system = WithoutNetwork(Host::new(root.to_path_buf()));
    let steps = root.join("world/steps");
    let messages = std::sync::Mutex::new(Vec::new());
    let progress = |message: &str| {
        messages.lock().unwrap().push(message.to_string());
        Ok(())
    };
    let log = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(steps.join("setup.log"))
        .unwrap();
    let wine = core::RealWine::new(&system, &steps.join("prefix"), &steps.join("runner"), &log);
    let mut results = vec![
        outcome(steam::remove_shared_mono(&wine).map(|()| Value::Null)),
        outcome(
            core::prepare_framework(
                &system,
                &wine,
                &steps.join("cache"),
                &progress,
                first_version,
            )
            .map(|()| Value::Null),
        ),
        outcome(
            core::prepare_geometry(&system, &wine, &steps.join("cache"), root, &progress)
                .map(|()| Value::Null),
        ),
        outcome(core::has_framework(&steps.join("prefix")).map(Value::Bool)),
    ];
    results.push(json!(messages.lock().unwrap().clone()));
    Value::Array(results)
}

const PYTHON_SETUP_STEPS: &str = "from unittest.mock import patch\nsteps = Path('world/steps').resolve()\ndef fetch(url, destination, expected, progress=None, max_size=None):\n    destination.parent.mkdir(parents=True, exist_ok=True)\n    destination.write_text(url)\n    return destination\nmessages = []\nwith open(steps / 'setup.log', 'ab') as log, patch.object(core, 'download', side_effect=fetch):\n    wine = core.Wine(steps / 'prefix', steps / 'runner', log)\n    result = [outcome(lambda: steam.remove_shared_mono(wine)), outcome(lambda: core.prepare_framework(wine, steps / 'cache', messages.append, argument)), outcome(lambda: core.prepare_geometry(wine, steps / 'cache', Path('.').resolve(), messages.append)), outcome(lambda: core.has_framework(steps / 'prefix'))]\nresult.append(messages)";

/// Not a test of its own: the Rust installer with one step replaced, started
/// by the comparisons as a separate process with its own environment.
#[test]
fn installer_without_geometry() {
    let Some(request) = std::env::var_os(CHILD) else {
        return;
    };
    let request = json::loads(request.to_str().unwrap()).unwrap();
    let root = PathBuf::from(request["root"].as_str().unwrap());
    if let Some(first_version) = request["steps"].as_str() {
        fs::write(
            request["out"].as_str().unwrap(),
            json::dumps(&setup_steps(&root, first_version), None),
        )
        .unwrap();
        std::process::exit(0);
    }
    let system: Arc<dyn System> = Arc::new(WithoutGeometry(Host::new(root)));
    let mut cli = Cli {
        system: system.clone(),
        background: system,
        interrupt: Arc::new(AtomicBool::new(false)),
        stdout: Box::new(File::create(request["out"].as_str().unwrap()).unwrap()),
        stderr: Box::new(File::create(request["err"].as_str().unwrap()).unwrap()),
    };
    let args = request["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| OsString::from(arg.as_str().unwrap()));
    let code = cli::run(
        &mut cli,
        std::iter::once(OsString::from("fenix-patch")).chain(args),
    );
    drop(cli);
    std::process::exit(code);
}

/// What is different on every run: times, random names, and the numbers of file systems and files.
fn normalise(text: &str) -> String {
    static STAMP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{8}T\d{6}(-[0-9a-f]{8})?").unwrap());
    static IDENTITY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#""original_prefix_id": \[\s*\d+,\s*\d+\s*\]"#).unwrap());
    static RESTORED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"restored-[0-9a-f]{8}\b").unwrap());
    static TEMPORARY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"fenix-geometry-[a-z0-9_]{8}").unwrap());
    static PIPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"pipe:\[\d+\]").unwrap());
    let text = STAMP.replace_all(text, |found: &regex::Captures<'_>| {
        if found.get(1).is_some() {
            "<STAMP>"
        } else {
            "<TIME>"
        }
    });
    let text = IDENTITY.replace_all(&text, "\"original_prefix_id\": <ID>");
    let text = RESTORED.replace_all(&text, "restored-<ID>");
    let text = TEMPORARY.replace_all(&text, "fenix-geometry-<TMP>");
    let text = PIPE.replace_all(&text, "pipe:[<N>]");
    // Exactly 32 hexadecimal digits are a random name; the 64 of a digest are not.
    let bytes = text.as_bytes();
    let hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if hex(bytes[index]) {
            let end = (index..bytes.len())
                .find(|position| !hex(bytes[*position]))
                .unwrap_or(bytes.len());
            out.push_str(if end - index == 32 {
                "<UUID>"
            } else {
                &text[index..end]
            });
            index = end;
        } else {
            let end = (index..bytes.len())
                .find(|position| hex(bytes[*position]))
                .unwrap_or(bytes.len());
            out.push_str(&text[index..end]);
            index = end;
        }
    }
    out
}

fn put(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let _ = fs::remove_file(path);
    fs::write(path, content).unwrap();
}

fn put_mode(path: &Path, content: &str, mode: u32) {
    put(path, content);
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Give everything below `root` the fixture's time, links included.
fn settle(root: &Path) {
    let time = rustix::fs::Timespec {
        tv_sec: FIXTURE_TIME,
        tv_nsec: 0,
    };
    let times = rustix::fs::Timestamps {
        last_access: time,
        last_modification: time,
    };
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let info = fs::symlink_metadata(&path).unwrap();
        if info.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
        rustix::fs::utimensat(
            rustix::fs::CWD,
            &path,
            &times,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();
    }
}

/// A shell script that records how it was started, then does `tail`.
fn fake(program: &str, tail: &str) -> String {
    format!(
        r#"#!/bin/sh
# Synthetic {program} for the comparison of the Python and the Rust installer.
fd0=$(readlink /proc/$$/fd/0); fd1=$(readlink /proc/$$/fd/1); fd2=$(readlink /proc/$$/fd/2)
open=$(ls /proc/$$/fd | tr '\n' ' ')
{{
  printf 'program: {program}\nargv:'
  for argument in "$0" "$@"; do printf ' [%s]' "$argument"; done
  printf '\ncwd: %s\nstdin: %s\nstdout: %s\nstderr: %s\nopen files: %s\nenvironment:\n' "$(pwd -P)" "$fd0" "$fd1" "$fd2" "$open"
  env | grep -v -e '^_=' -e '^SHLVL=' -e '^PWD=' -e '^OLDPWD=' -e '^FENIX_PARITY_CHILD=' | sort | sed 's/^/  /'
  printf '%s\n' '---'
}} >> "$FAKE_LOG"
echo "{program} output for $1"
echo "{program} errors for $1" >&2
{tail}
"#
    )
}

fn fake_wine() -> String {
    fake(
        "wine",
        "if [ -n \"$FAKE_WINE_FAIL\" ]; then case \" $* \" in *\"$FAKE_WINE_FAIL\"*) exit 3;; esac; fi\n\
         if [ -n \"$FAKE_WINE_SLEEP\" ]; then case \" $* \" in *\"$FAKE_WINE_SLEEP\"*) sleep 2;; esac; fi\n\
         case \"$1\" in *.exe) exit \"${FAKE_APP_EXIT:-0}\";; esac\nexit 0",
    )
}

fn fake_wineserver(note: &str) -> String {
    fake(
        "wineserver",
        &format!(
            "# {note}\nif [ \"$1\" = -k ]; then exit \"${{FAKE_SERVER_KILL:-1}}\"; fi\nexit 0"
        ),
    )
}

fn fake_proton() -> String {
    fake(
        "proton",
        "if [ \"$1\" = getcompatpath ]; then printf 'tool version' > \"$STEAM_COMPAT_DATA_PATH/version\"; fi\nexit \"${FAKE_PROTON_EXIT:-0}\"",
    )
}

const REGISTRY: &str = "WINE REGISTRY Version 2\n;; All keys relative to \\\\Machine\n\n[Software\\\\Microsoft\\\\NET Framework Setup\\\\NDP\\\\v4\\\\Full] 1700000000\n\"Release\"=dword:00080ff4\n\n[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts] 1700000000\n\"Tahoma (TrueType)\"=\"tahoma.ttf\"\n\n[Software\\\\Wine] 1700000000\n";
const RUNNER_FILES: [&str; 4] = [
    "files/bin/wine",
    "files/bin/wineserver",
    "files/lib/wine/x86_64-windows/ntdll.dll",
    "files/lib/wine/x86_64-unix/ntdll.so",
];
const OVERLAY: [&str; 3] = [
    "files/lib/wine/x86_64-windows/ntdll.dll",
    "files/bin/wineserver",
    "files/lib/wine/x86_64-unix/win32u.so",
];
const INTEGRATION: [&str; 6] = [
    "launch-msfs.sh",
    "xodus-wine-launch",
    "FenixWindowGuard.exe",
    "FenixMCDURefresh.exe",
    "fenix-display-refresh.py",
    "FenixGeometrySetup.exe",
];
const SCRIPTS: [&str; 2] = ["launch-msfs.sh", "xodus-wine-launch"];
const STEAM: [&str; 4] = ["--steam", "msfs2024", "--steam-root", "{B}/world/steam"];
const FLIGHTDECK: [&str; 2] = ["--runtime", "{B}/world/runtime"];

/// A Wine build with everything the installer takes from it.
fn runner(root: &Path, version: &str, proton: bool) {
    put(&root.join("version"), version);
    put_mode(&root.join("files/bin/wine"), &fake_wine(), 0o755);
    put_mode(
        &root.join("files/bin/wineserver"),
        &fake_wineserver("as shipped"),
        0o755,
    );
    put(
        &root.join("files/lib/wine/x86_64-windows/ntdll.dll"),
        "original ntdll.dll",
    );
    put(
        &root.join("files/lib/wine/x86_64-unix/ntdll.so"),
        "original ntdll.so",
    );
    put(
        &root.join("files/lib/wine/x86_64-unix/win32u.so"),
        "original win32u.so",
    );
    for arch in ["x86_64", "i386"] {
        for name in [
            "libvkd3d-1.dll",
            "libvkd3d-shader-1.dll",
            "libvkd3d-utils-1.dll",
        ] {
            put(
                &root.join(format!("files/lib/vkd3d/{arch}-windows/{name}")),
                &format!("{arch} {name}"),
            );
        }
    }
    for name in [
        "arial", "arialbd", "cour", "courbd", "georgia", "times", "micross",
    ] {
        put(
            &root.join(format!("files/share/fonts/{name}.ttf")),
            &format!("font {name}"),
        );
    }
    put(
        &root.join("files/share/wine/fonts/tahoma.ttf"),
        "font tahoma",
    );
    put_mode(
        &root.join("files/share/wine/fonts/tahomabd.ttf"),
        "font tahoma bold",
        0o444,
    );
    if proton {
        put_mode(&root.join("proton"), &fake_proton(), 0o755);
    }
}

/// A Windows profile in which Microsoft .NET Framework 4.8 is already present.
fn profile(prefix: &Path, user: &str, folder: &str) {
    put(&prefix.join("system.reg"), REGISTRY);
    put(&prefix.join("user.reg"), "WINE REGISTRY Version 2\n");
    put(
        &prefix.join("drive_c/windows/system32/ntdll.dll"),
        "profile ntdll.dll",
    );
    put(
        &prefix.join("drive_c/windows/Microsoft.NET/Framework64/v4.0.30319/clr.dll"),
        "clr",
    );
    put(
        &prefix.join("drive_c/users/Public/Documents/readme.txt"),
        "public",
    );
    let settings = prefix
        .join("drive_c/users")
        .join(user)
        .join("AppData/Roaming")
        .join(folder);
    put(&settings.join("UserCfg.opt"), "settings");
    put(
        &settings.join("Packages/Community/addon/layout.json"),
        "large simulator content",
    );
    put_mode(
        &settings.join("Packages/Official/readonly.bin"),
        "read-only content",
        0o444,
    );
    fs::create_dir_all(prefix.join("dosdevices")).unwrap();
    symlink(prefix.join("drive_c"), prefix.join("dosdevices/c:")).unwrap();
    symlink("/", prefix.join("dosdevices/z:")).unwrap();
}

/// Steam with MSFS 2024 in a second library and MSFS 2020 next to the client.
fn steam_world(base: &Path) {
    let root = base.join("world/steam");
    let library = base.join("world/library two");
    put(
        &root.join("steamapps/libraryfolders.vdf"),
        &format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"2\"\n\t{{\n\t\t\"path\"\t\t\"/missing/library\"\n\t}}\n}}\n",
            root.display(),
            library.display()
        ),
    );
    put(
        &library.join("steamapps/appmanifest_2537590.acf"),
        "\"AppState\"\n{\n}\n",
    );
    put(
        &root.join("steamapps/appmanifest_1250410.acf"),
        "\"AppState\"\n{\n}\n",
    );
    put(
        &library.join("steamapps/compatdata/2537590/version"),
        "11.0-100",
    );
    profile(
        &library.join("steamapps/compatdata/2537590/pfx"),
        "steamuser",
        "Microsoft Flight Simulator 2024",
    );
    put(
        &root.join("steamapps/compatdata/1250410/version"),
        "11.0-100",
    );
    profile(
        &root.join("steamapps/compatdata/1250410/pfx"),
        "steamuser",
        "Microsoft Flight Simulator",
    );
    runner(
        &root.join("compatibilitytools.d/pinned-proton"),
        "1700000002 parity-proton-11.0",
        true,
    );
    put_mode(&root.join("config/config.vdf"), STEAM_CONFIG, 0o640);
}

const STEAM_CONFIG: &str = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"440\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t\t\"quoted\"\t\t\"a \\\"brace\\\" { inside\"\n\t\t\t}\n\t\t}\n\t}\n}\n";

/// A Flightdeck runtime as its launcher creates it.
fn flightdeck_world(base: &Path) {
    flightdeck_runtime(base, &base.join("world/runtime"));
}

fn flightdeck_runtime(base: &Path, root: &Path) {
    put(
        &root.join("private/runtime.json"),
        "{\n  \"game_id\": \"msfs2024\"\n}\n",
    );
    for name in SCRIPTS {
        put_mode(
            &root.join("tools").join(name),
            &format!("#!/bin/sh\n# original {name}\n"),
            0o755,
        );
    }
    profile(
        &root.join("local/msfs-prefix"),
        "flight",
        "Microsoft Flight Simulator 2024",
    );
    runner(
        &base.join("world/runner source"),
        "1700000001 parity-base-11.0",
        false,
    );
    symlink(base.join("world/runner source"), root.join("runner")).unwrap();
    tag(&root.join("tools/launch-msfs.sh"), "launch script");
    tag(&root.join("local/msfs-prefix/user.reg"), "registry");
    put(
        &root.join("private/import-manifest.json"),
        "{\n  \"schema\": 3,\n  \"runtime_files\": {\n    \"launch-msfs.sh\": \"old\",\n    \"other\": \"kept\"\n  },\n  \"note\": \"\u{e4}\"\n}\n",
    );
    put(
        &root.join("private/proton-selection.json"),
        "{\"schema\": 1, \"runner\": \"local/proton/runner\", \"version\": \"parity-base-11.0\", \"ratio\": 1.50, \"big\": 123456789012345678901234567890}",
    );
    tag(&root.join("private/import-manifest.json"), "imported");
}

fn digests(base: &Path, names: &[&str]) -> Value {
    let mut map = json::Map::new();
    for name in names {
        map.insert(
            name.to_string(),
            json!(fsx::digest(&base.join(name)).unwrap()),
        );
    }
    Value::Object(map)
}

/// The installer's bundle: manifest, overlay files and helpers, for release 1 or 2.
fn bundle(base: &Path, release: u32) {
    let payload = base.join("payload");
    let _ = fs::remove_dir_all(&payload);
    let _ = fs::remove_dir_all(base.join("integration"));
    for (folder, kind) in [
        (payload.clone(), "base"),
        (payload.join("variants/fixture"), "fixture"),
    ] {
        put(
            &folder.join(OVERLAY[0]),
            &format!("{kind} overlay ntdll.dll, release 1"),
        );
        put_mode(
            &folder.join(OVERLAY[1]),
            &fake_wineserver(&format!("{kind} overlay, release {release}")),
            0o755,
        );
        put(
            &folder.join(OVERLAY[2]),
            &format!("{kind} overlay win32u.so, release {release}"),
        );
    }
    for name in INTEGRATION {
        put(
            &base.join("integration").join(name),
            &format!("integration {name}, release {release}"),
        );
    }
    let overlay_of = |folder: &Path, release: u32| {
        // The digests of an earlier release are those its files had.
        let mut files = digests(folder, &OVERLAY);
        if release == 1 {
            let kind = if folder.ends_with("fixture") {
                "fixture"
            } else {
                "base"
            };
            files[OVERLAY[1]] = json!(fsx::digest_bytes(
                fake_wineserver(&format!("{kind} overlay, release 1")).as_bytes()
            ));
            files[OVERLAY[2]] = json!(fsx::digest_bytes(
                format!("{kind} overlay win32u.so, release 1").as_bytes()
            ));
        }
        files
    };
    let integration_of = |release: u32| {
        let mut map = json::Map::new();
        for name in INTEGRATION {
            map.insert(
                name.to_string(),
                json!(fsx::digest_bytes(
                    format!("integration {name}, release {release}").as_bytes()
                )),
            );
        }
        Value::Object(map)
    };
    let source = base.join("world/runner source");
    let proton = base.join("world/steam/compatibilitytools.d/pinned-proton");
    let original =
        |name: &str| fsx::digest_bytes(format!("#!/bin/sh\n# original {name}\n").as_bytes());
    let mut fixture = json!({
        "runner_version": "1700000002 parity-proton-11.0",
        "runner_files": if proton.is_dir() { digests(&proton, &RUNNER_FILES) } else { json!({"absent": "0"}) },
        "files": overlay_of(&payload.join("variants/fixture"), release),
    });
    let mut lock = json!({
        "format": 1,
        "version": format!("0.0.{release}-parity"),
        "repository": "https://example.invalid/parity",
        "runner_version": "1700000001 parity-base-11.0",
        "runner_files": if source.is_dir() { digests(&source, &RUNNER_FILES) } else { json!({"absent": "0"}) },
        "files": overlay_of(&payload, release),
        "integration": integration_of(release),
        "accepted_scripts": {SCRIPTS[0]: [original(SCRIPTS[0])], SCRIPTS[1]: ["0".repeat(64), original(SCRIPTS[1])]},
    });
    if release > 1 {
        lock["previous_releases"] = json!({"0.0.1-parity": {"files": overlay_of(&payload, 1), "integration": integration_of(1)}});
        fixture["previous_releases"] =
            json!({"0.0.1-parity": {"files": overlay_of(&payload.join("variants/fixture"), 1)}});
    }
    lock["variants"] = json!({"fixture": fixture, "unused": {"runner_version": "9 nothing", "runner_files": {"absent": "0"}, "files": {}}});
    put(
        &base.join("bundle.json"),
        &(json::dumps(&lock, Some(2)) + "\n"),
    );
    settle(&payload);
    settle(&base.join("integration"));
    settle(&base.join("bundle.json"));
}

fn display() -> Options {
    Options {
        env: vec![("DISPLAY".to_string(), ":99".to_string())],
        ..Options::default()
    }
}

fn with(args: &[&'static str], target: &[&'static str]) -> Vec<&'static str> {
    args.iter().chain(target).copied().collect()
}

/// An installed Fenix as its Windows installer leaves it behind.
fn fenix_installed(prefix: &Path, user: &str) {
    put(
        &prefix.join(core::PROGRAM).join("Fenix.exe"),
        "MZ synthetic Fenix",
    );
    put(
        &prefix.join(core::CONFIG).join("fenixConfig.xml"),
        "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<FenixConfig>\r\n  <displayMode>GPU</displayMode>\r\n  <newRender>true</newRender>\r\n  <Volume unit=\"dB &amp; more\">0.4</Volume>\r\n  <Empty />\r\n  <Note><![CDATA[a < b]]></Note>\r\n</FenixConfig>\r\n",
    );
    put(
        &prefix
            .join("drive_c/users")
            .join(user)
            .join("AppData/Local/FenixApp/current/FenixApp.exe"),
        "MZ synthetic manager",
    );
    tag(
        &prefix.join(core::CONFIG).join("fenixConfig.xml"),
        "settings",
    );
}

#[test]
fn steam_installation_that_fails_and_is_restored() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        // The download cache is occupied by a file: the geometry step fails before it reaches the network.
        put(
            &base
                .join("world/library two/steamapps")
                .join(steam::STATE)
                .join("2537590/downloads"),
            "in the way",
        );
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let json = Options {
        env: display().env,
        ..Options::default()
    };
    lab.step(
        "targets",
        &["targets", "--steam-root", "{B}/world/steam"],
        &Options::default(),
    );
    lab.step(
        "status before",
        &with(&["status"], &STEAM),
        &Options::default(),
    );
    lab.step(
        "install without a display",
        &with(&["install"], &STEAM),
        &Options::default(),
    );
    let (python, _) = lab.step("install", &with(&["install", "--json"], &STEAM), &json);
    assert_eq!(python.code, 1, "{python:?}");
    assert!(python.stderr.contains("File exists"), "{python:?}");
    lab.step(
        "status after the failure",
        &with(&["status"], &STEAM),
        &Options::default(),
    );
    lab.step("install again", &with(&["install"], &STEAM), &display());
    lab.step(
        "configure",
        &with(&["configure"], &STEAM),
        &Options::default(),
    );
    lab.step("select", &with(&["select"], &STEAM), &Options::default());
    lab.step("open", &with(&["open"], &STEAM), &Options::default());
    let (python, _) = lab.step(
        "restore",
        &with(&["restore", "--json"], &STEAM),
        &Options::default(),
    );
    assert_eq!(python.code, 0, "{python:?}");
    lab.step(
        "restore again",
        &with(&["restore"], &STEAM),
        &Options::default(),
    );
    lab.step(
        "status after restore",
        &with(&["status"], &STEAM),
        &Options::default(),
    );
    lab.finish();
}

#[test]
fn steam_installation_from_start_to_restore() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let stub = Options {
        stub_geometry: true,
        ..display()
    };
    let plain = Options::default();
    let library = "world/library two/steamapps/compatdata/2537590/pfx";
    let (python, _) = lab.step("install", &with(&["install"], &STEAM), &stub);
    assert_eq!(python.code, 0, "{python:?}");
    lab.step("status", &with(&["status"], &STEAM), &plain);
    lab.step(
        "install again",
        &with(&["install", "--json"], &STEAM),
        &display(),
    );
    lab.step(
        "configure without Fenix",
        &with(&["configure"], &STEAM),
        &plain,
    );
    let (python, _) = lab.step("select", &with(&["select"], &STEAM), &plain);
    if python.stderr.contains("Steam is running") {
        notice(
            "Steam is running on this machine; writing its selection was only compared up to that refusal",
        );
    }
    lab.step("select again", &with(&["select", "--json"], &STEAM), &plain);
    lab.step("status selected", &with(&["status"], &STEAM), &plain);
    lab.both(|base| fenix_installed(&base.join(library), "steamuser"));
    lab.step(
        "configure without readouts",
        &with(&["configure"], &STEAM),
        &plain,
    );
    lab.step("status configured", &with(&["status"], &STEAM), &plain);
    lab.both(|base| {
        put(
            &base
                .join(library)
                .join(core::CONFIG)
                .join("persistancy.xml"),
            "<Persistancy>\n  <fcuReadoutsType>1</fcuReadoutsType>\n</Persistancy>\n",
        )
    });
    lab.step("configure", &with(&["configure", "--json"], &STEAM), &plain);
    lab.step("configure again", &with(&["configure"], &STEAM), &plain);
    lab.step("open", &with(&["open"], &STEAM), &plain);
    lab.step("manager", &with(&["manager", "--json"], &STEAM), &plain);
    lab.both(|base| {
        put(
            &base.join("world/home/Downloads/FenixInstaller.exe"),
            "MZ official installer",
        )
    });
    lab.step(
        "installer",
        &with(
            &[
                "installer",
                "--exe",
                "{B}/world/home/Downloads/FenixInstaller.exe",
            ],
            &STEAM,
        ),
        &plain,
    );
    lab.step(
        "installer from the home directory",
        &with(
            &["installer", "--exe", "~/Downloads/FenixInstaller.exe"],
            &STEAM,
        ),
        &plain,
    );
    lab.both(|base| {
        put(
            &base.join("world/home/Downloads/notes.exe"),
            "not a program",
        )
    });
    lab.step(
        "installer that is none",
        &with(
            &["installer", "--exe", "{B}/world/home/Downloads/notes.exe"],
            &STEAM,
        ),
        &plain,
    );
    lab.step(
        "installer that is missing",
        &with(
            &["installer", "--exe", "{B}/world/home/Downloads/missing.exe"],
            &STEAM,
        ),
        &plain,
    );
    let usage = Options {
        stderr_differs: true,
        ..Options::default()
    };
    let (python, rust) = lab.step(
        "installer without a file",
        &with(&["installer"], &STEAM),
        &usage,
    );
    assert_eq!((python.code, rust.code), (2, 2));
    assert!(
        rust.stderr
            .contains("installer requires --exe /path/to/FenixInstaller.exe"),
        "{rust:?}"
    );
    // A failing Fenix: Python ends with a traceback of CalledProcessError, Rust with the same text as a message.
    let failing = Options {
        env: vec![("FAKE_APP_EXIT".to_string(), "7".to_string())],
        stderr_differs: true,
        ..Options::default()
    };
    let (python, rust) = lab.step("open that fails", &with(&["open"], &STEAM), &failing);
    assert_eq!((python.code, rust.code), (1, 1));
    let message = lab.normal(Side::Rust, rust.stderr.trim_end()).replacen(
        "Fenix patch: ",
        "subprocess.CalledProcessError: ",
        1,
    );
    assert!(
        lab.normal(Side::Python, &python.stderr)
            .trim_end()
            .ends_with(&message),
        "{python:?}\n{rust:?}"
    );
    lab.step("status before restore", &with(&["status"], &STEAM), &plain);
    lab.step("restore", &with(&["restore"], &STEAM), &plain);
    lab.step("status after restore", &with(&["status"], &STEAM), &plain);
    lab.step(
        "configure after restore",
        &with(&["configure"], &STEAM),
        &plain,
    );
    lab.finish();
}

#[test]
fn steam_update_and_second_simulator() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let stub = Options {
        stub_geometry: true,
        ..display()
    };
    let plain = Options::default();
    let other = ["--steam", "msfs2020", "--steam-root", "{B}/world/steam"];
    lab.step("install", &with(&["install"], &STEAM), &stub);
    lab.step(
        "install the second simulator",
        &with(&["install", "--json"], &other),
        &stub,
    );
    lab.step(
        "status of the second simulator",
        &with(&["status"], &other),
        &plain,
    );
    lab.both(|base| bundle(base, 2));
    lab.step("status with an update", &with(&["status"], &STEAM), &plain);
    lab.step(
        "configure before the update",
        &with(&["configure"], &STEAM),
        &plain,
    );
    lab.step(
        "select before the update",
        &with(&["select"], &STEAM),
        &plain,
    );
    lab.step("update", &with(&["install"], &STEAM), &display());
    lab.step(
        "status after the update",
        &with(&["status"], &STEAM),
        &plain,
    );
    lab.step(
        "status of the second simulator after the update",
        &with(&["status"], &other),
        &plain,
    );
    lab.step(
        "update of the second simulator",
        &with(&["install", "--json"], &other),
        &plain,
    );
    lab.step("update again", &with(&["install"], &STEAM), &display());
    lab.step("restore", &with(&["restore"], &STEAM), &plain);
    lab.step(
        "restore the second simulator",
        &with(&["restore"], &other),
        &plain,
    );
    lab.step(
        "targets",
        &["targets", "--steam-root", "{B}/world/steam"],
        &plain,
    );
    lab.finish();
}

#[test]
fn flightdeck_installation_that_fails_and_is_restored() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        flightdeck_world(base);
        put(
            &base.join("world/runtime/private/fenix-downloads"),
            "in the way",
        );
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let plain = Options::default();
    lab.step("status before", &with(&["status"], &FLIGHTDECK), &plain);
    let (python, _) = lab.step("install", &with(&["install"], &FLIGHTDECK), &plain);
    assert_eq!(python.code, 1, "{python:?}");
    assert!(python.stderr.contains("File exists"), "{python:?}");
    lab.step(
        "status after the failure",
        &with(&["status"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "install again",
        &with(&["install", "--json"], &FLIGHTDECK),
        &plain,
    );
    lab.step("configure", &with(&["configure"], &FLIGHTDECK), &plain);
    lab.step("open", &with(&["open"], &FLIGHTDECK), &plain);
    let (python, _) = lab.step("restore", &with(&["restore"], &FLIGHTDECK), &plain);
    assert_eq!(python.code, 0, "{python:?}");
    lab.step(
        "restore again",
        &with(&["restore", "--json"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "status after restore",
        &with(&["status"], &FLIGHTDECK),
        &plain,
    );
    lab.finish();
}

#[test]
fn flightdeck_installation_upgrade_and_restore() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        flightdeck_world(base);
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let stub = Options {
        stub_geometry: true,
        ..Options::default()
    };
    let plain = Options::default();
    let prefix = "world/runtime/local/msfs-prefix";
    let (python, _) = lab.step("install", &with(&["install"], &FLIGHTDECK), &stub);
    assert_eq!(python.code, 0, "{python:?}");
    lab.step("status", &with(&["status"], &FLIGHTDECK), &plain);
    lab.step("install again", &with(&["install"], &FLIGHTDECK), &plain);
    lab.step(
        "configure without Fenix",
        &with(&["configure", "--json"], &FLIGHTDECK),
        &plain,
    );
    lab.both(|base| {
        fenix_installed(&base.join(prefix), "flight");
        put(
            &base.join(prefix).join(core::CONFIG).join("persistancy.xml"),
            "<Persistancy><fcuReadoutsType>1</fcuReadoutsType></Persistancy>",
        );
    });
    lab.step("configure", &with(&["configure"], &FLIGHTDECK), &plain);
    lab.step("open", &with(&["open", "--json"], &FLIGHTDECK), &plain);
    lab.step("manager", &with(&["manager"], &FLIGHTDECK), &plain);
    let usage = Options {
        stderr_differs: true,
        ..Options::default()
    };
    let (python, rust) = lab.step("select", &with(&["select"], &FLIGHTDECK), &usage);
    assert_eq!((python.code, rust.code), (2, 2));
    assert!(
        rust.stderr
            .contains("select applies to Steam simulators only"),
        "{rust:?}"
    );
    lab.both(|base| bundle(base, 2));
    lab.step(
        "status with an update",
        &with(&["status"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "configure before the update",
        &with(&["configure"], &FLIGHTDECK),
        &plain,
    );
    lab.step("update", &with(&["install", "--json"], &FLIGHTDECK), &stub);
    lab.step(
        "status after the update",
        &with(&["status"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "configure after the update",
        &with(&["configure"], &FLIGHTDECK),
        &plain,
    );
    lab.step("restore", &with(&["restore"], &FLIGHTDECK), &plain);
    lab.step(
        "status after restore",
        &with(&["status"], &FLIGHTDECK),
        &plain,
    );
    lab.step("restore again", &with(&["restore"], &FLIGHTDECK), &plain);
    lab.finish();
}

fn env(pairs: &[(&str, &str)]) -> Options {
    Options {
        env: pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        ..Options::default()
    }
}

fn usage() -> Options {
    Options {
        stderr_differs: true,
        ..Options::default()
    }
}

#[test]
fn finding_and_choosing_simulators() {
    let Some(mut lab) = Lab::new() else { return };
    let plain = Options::default();
    lab.both(|base| bundle(base, 1));
    // Nothing is installed yet.
    lab.step("no simulator: targets", &["targets"], &plain);
    lab.step("no simulator: status", &["status"], &plain);
    lab.step(
        "no simulator: status as JSON",
        &["status", "--json"],
        &plain,
    );
    lab.step("no simulator: guided setup", &["gui", "--text"], &plain);
    lab.step("no simulator: guided setup without a display", &[], &plain);
    lab.both(|base| {
        steam_world(base);
        fs::create_dir_all(base.join("world/home/.local/share")).unwrap();
        symlink(
            base.join("world/steam"),
            base.join("world/home/.local/share/Steam"),
        )
        .unwrap();
        fs::create_dir_all(base.join("world/home/.steam")).unwrap();
        // The same installation under a second well-known name counts once.
        symlink(
            base.join("world/steam"),
            base.join("world/home/.steam/root"),
        )
        .unwrap();
        settle(&base.join("world"));
        bundle(base, 1);
    });
    lab.step("steam: targets", &["targets"], &plain);
    lab.step("steam: status needs a choice", &["status"], &plain);
    lab.step(
        "steam: status of one",
        &["status", "--steam", "msfs2024"],
        &plain,
    );
    lab.step(
        "steam: status of the other",
        &["status", "--steam", "msfs2020"],
        &plain,
    );
    lab.both(|base| {
        flightdeck_runtime(
            base,
            &base.join("world/home/.local/share/flightdeck/runtimes/msfs2024"),
        );
        flightdeck_world(base);
        bundle(base, 1);
    });
    lab.step("all: targets", &["targets"], &plain);
    lab.step("all: status needs a choice", &["status", "--json"], &plain);
    lab.step(
        "all: guided setup asks and quits",
        &["gui", "--text"],
        &Options {
            stdin: "3\nq\n".into(),
            ..Options::default()
        },
    );
    lab.step(
        "all: guided setup with the first",
        &["--text"],
        &Options {
            stdin: "\nq\n".into(),
            ..Options::default()
        },
    );
    lab.step(
        "all: guided setup with a wrong number",
        &["gui", "--text"],
        &Options {
            stdin: "7\n".into(),
            ..Options::default()
        },
    );
    lab.step(
        "all: guided setup with a word",
        &["gui", "--text"],
        &Options {
            stdin: "two\n".into(),
            ..Options::default()
        },
    );
    lab.step(
        "all: guided setup at the end of input",
        &["gui", "--text"],
        &plain,
    );
    lab.step(
        "all: guided setup that ends after the choice",
        &["gui", "--text"],
        &Options {
            stdin: "2".into(),
            ..Options::default()
        },
    );
    lab.step(
        "all: guided setup for the runtime",
        &with(&["gui", "--text"], &FLIGHTDECK),
        &Options {
            stdin: "x\nr\nQ\n".into(),
            ..Options::default()
        },
    );
    lab.step(
        "all: other data directory",
        &["targets"],
        &env(&[("XDG_DATA_HOME", "{B}/world/elsewhere//")]),
    );
    lab.step(
        "all: no data directory variable",
        &["targets"],
        &env(&[("XDG_DATA_HOME", UNSET), ("HOME", "{B}/world/home/")]),
    );
    lab.step(
        "all: relative data directory",
        &["targets"],
        &env(&[("XDG_DATA_HOME", "world/./home/.local/share/")]),
    );
    lab.step(
        "runtime and steam",
        &[
            "status",
            "--runtime",
            "{B}/world/runtime",
            "--steam",
            "msfs2024",
        ],
        &plain,
    );
    lab.step("explicit runtime", &with(&["status"], &FLIGHTDECK), &plain);
    lab.step(
        "relative runtime",
        &["status", "--runtime", "world/runtime/"],
        &plain,
    );
    lab.step(
        "runtime below the home directory",
        &[
            "status",
            "--runtime",
            "~/.local/share/flightdeck/runtimes/msfs2024",
        ],
        &plain,
    );
    lab.step(
        "missing runtime",
        &["status", "--runtime", "/nonexistent/runtime"],
        &plain,
    );
    lab.step(
        "missing runtime as JSON",
        &["install", "--json", "--runtime", "/nonexistent/runtime"],
        &plain,
    );
    lab.step(
        "runtime that is none",
        &["status", "--runtime", "{B}/world/home"],
        &plain,
    );
    lab.step(
        "runtime that is a file",
        &["status", "--runtime", "{B}/bundle.json"],
        &plain,
    );
    lab.step("empty runtime", &["status", "--runtime="], &plain);
    lab.step(
        "missing steam directory",
        &["targets", "--steam-root", "/nonexistent/steam"],
        &plain,
    );
    lab.step(
        "steam directory that is none",
        &["status", "--steam-root", "{B}/world/home"],
        &plain,
    );
    lab.step(
        "steam library alone",
        &["targets", "--steam-root", "{B}/world/library two"],
        &plain,
    );
    lab.step(
        "empty steam directory",
        &["targets", "--steam-root="],
        &plain,
    );
    // The command line itself.
    lab.step(
        "abbreviated options",
        &[
            "status",
            "--steam-r",
            "{B}/world/steam",
            "--steam",
            "msfs2024",
            "--j",
        ],
        &plain,
    );
    lab.step(
        "abbreviated runtime",
        &["status", "--run", "{B}/world/runtime", "--no"],
        &plain,
    );
    lab.step(
        "the last value counts",
        &[
            "status",
            "--runtime",
            "/nonexistent/a",
            "--runtime={B}/world/runtime",
            "--json",
            "--json",
        ],
        &plain,
    );
    lab.step(
        "options before the command",
        &["--runtime", "{B}/world/runtime", "status"],
        &plain,
    );
    for (label, args) in [
        ("unknown command", vec!["frobnicate"]),
        ("unknown simulator", vec!["status", "--steam", "msfs2019"]),
        ("two commands", vec!["status", "install"]),
        ("unknown option", vec!["status", "--frobnicate"]),
        ("ambiguous option", vec!["status", "--s", "x"]),
        (
            "ambiguous longer option",
            vec!["status", "--stea", "msfs2024"],
        ),
        ("option without its value", vec!["status", "--runtime"]),
        ("flag with a value", vec!["status", "--json=yes"]),
    ] {
        let (python, _) = lab.step(label, &args, &usage());
        assert_eq!(python.code, 2, "{label}: {python:?}");
    }
    let help = Options {
        stderr_differs: true,
        stdout_differs: true,
        ..Options::default()
    };
    let (python, rust) = lab.step("help", &["--help"], &help);
    assert_eq!((python.code, rust.code), (0, 0));
    for option in [
        "--steam",
        "--steam-root",
        "--proton",
        "--runtime",
        "--bundle",
        "--exe",
        "--json",
        "--text",
        "--no-browser",
    ] {
        assert!(
            python.stdout.contains(option) && rust.stdout.contains(option),
            "{option}"
        );
    }
    lab.finish();
}

/// Hold a lock file the way Flightdeck or another installer does, on both sides.
fn hold(lab: &Lab, relative: &str) -> Vec<File> {
    SIDES
        .iter()
        .map(|side| {
            let path = lab.base(*side).join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .unwrap();
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
            file
        })
        .collect()
}

/// A process that runs in a Wine profile, on both sides.
fn occupy(lab: &Lab, relative: &str) -> Vec<std::process::Child> {
    SIDES
        .iter()
        .map(|side| {
            Command::new("sleep")
                .arg("60")
                .env("WINEPREFIX", lab.base(*side).join(relative))
                .spawn()
                .unwrap()
        })
        .collect()
}

fn release(processes: Vec<std::process::Child>) {
    for mut process in processes {
        process.kill().unwrap();
        process.wait().unwrap();
    }
}

#[test]
fn flightdeck_refusals() {
    let Some(mut lab) = Lab::new() else { return };
    let plain = Options::default();
    let stub = Options {
        stub_geometry: true,
        ..Options::default()
    };
    let fresh = |lab: &Lab| {
        lab.both(|base| {
            let _ = fs::remove_dir_all(base.join("world"));
            fs::create_dir_all(base.join("world/home")).unwrap();
            fs::create_dir_all(base.join("world/tmp")).unwrap();
            flightdeck_world(base);
            settle(&base.join("world"));
            bundle(base, 1);
        });
    };
    let install = with(&["install"], &FLIGHTDECK);
    let status = with(&["status"], &FLIGHTDECK);
    let runtime = "world/runtime";
    fresh(&lab);
    lab.both(|base| {
        put(
            &base.join(runtime).join("private/runtime.json"),
            "{\"game_id\": \"msfs2020\"}",
        )
    });
    lab.step("another game: status", &status, &plain);
    lab.step("another game: install", &install, &plain);
    lab.both(|base| {
        put(
            &base.join(runtime).join("private/runtime.json"),
            "{\"game_id\": null}",
        )
    });
    lab.step("no game: status", &status, &plain);
    lab.both(|base| put(&base.join(runtime).join("private/runtime.json"), "{broken"));
    // Known difference: a JSON syntax error is worded by serde_json instead of Python's json module.
    let differing = Options {
        stdout_differs: true,
        ..Options::default()
    };
    let (python, rust) = lab.step(
        "unreadable runtime description: status",
        &status,
        &differing,
    );
    assert!(
        python.stdout.contains(
            "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"
        ),
        "{python:?}"
    );
    assert!(
        rust.stdout
            .contains("key must be a string at line 1 column 2"),
        "{rust:?}"
    );
    assert_eq!(
        python.stdout.replace(
            "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            "X"
        ),
        rust.stdout
            .replace("key must be a string at line 1 column 2", "X")
    );
    let differing = Options {
        stderr_differs: true,
        ..Options::default()
    };
    let (python, rust) = lab.step(
        "unreadable runtime description: install",
        &install,
        &differing,
    );
    assert_eq!(
        python.stderr,
        "Fenix patch: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)\n"
    );
    assert_eq!(
        rust.stderr,
        "Fenix patch: key must be a string at line 1 column 2\n"
    );
    lab.both(|base| put(&base.join(runtime).join("private/runtime.json"), "{}"));
    lab.step("runtime description without a game", &status, &plain);
    lab.both(|base| {
        fs::remove_file(base.join(runtime).join("runner")).unwrap();
        fs::create_dir(base.join(runtime).join("runner")).unwrap();
    });
    lab.step("runner is no link: status", &status, &plain);
    lab.step("runner is no link: install", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        fs::rename(
            base.join(runtime).join("tools"),
            base.join("world/tools elsewhere"),
        )
        .unwrap();
        symlink(
            base.join("world/tools elsewhere"),
            base.join(runtime).join("tools"),
        )
        .unwrap();
    });
    lab.step("linked tools directory: status", &status, &plain);
    lab.step(
        "linked tools directory: install",
        &with(&["install", "--json"], &FLIGHTDECK),
        &plain,
    );
    fresh(&lab);
    lab.both(|base| {
        fs::remove_file(base.join(runtime).join("local/msfs-prefix/system.reg")).unwrap()
    });
    lab.step("profile without a registry: status", &status, &plain);
    lab.step("profile without a registry: install", &install, &plain);
    lab.both(|base| {
        symlink(
            "user.reg",
            base.join(runtime).join("local/msfs-prefix/system.reg"),
        )
        .unwrap()
    });
    lab.step("profile with a linked registry: install", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        put(
            &base.join("world/runner source/files/bin/wine"),
            "another build",
        )
    });
    lab.step("unknown runner: status", &status, &plain);
    lab.step("unknown runner: install", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        put(
            &base.join("world/runner source/version"),
            "1700000009 something else",
        )
    });
    lab.step("runner of another version: install", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        put(
            &base.join(runtime).join("tools/launch-msfs.sh"),
            "#!/bin/sh\n# my own\n",
        )
    });
    lab.step("custom launch script", &install, &plain);
    fresh(&lab);
    lab.both(|base| put(&base.join("payload/files/bin/wineserver"), "tampered"));
    lab.step("tampered overlay", &install, &plain);
    fresh(&lab);
    lab.both(|base| put(&base.join("integration/FenixWindowGuard.exe"), "tampered"));
    lab.step(
        "tampered helper",
        &with(&["install", "--json"], &FLIGHTDECK),
        &plain,
    );
    fresh(&lab);
    lab.both(|base| {
        let parent = base.join("payload/files/lib/wine/x86_64-windows");
        fs::rename(&parent, base.join("world/outside")).unwrap();
        symlink(base.join("world/outside"), &parent).unwrap();
    });
    lab.step("overlay folder that leads outside", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        fs::remove_file(base.join("payload/files/bin/wineserver")).unwrap();
        symlink(
            base.join("world/runner source/files/bin/wineserver"),
            base.join("payload/files/bin/wineserver"),
        )
        .unwrap();
    });
    lab.step("overlay file that is a link", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        // A complete bundle elsewhere which belongs to another release.
        let other = base.join("world/other bundle");
        fs::create_dir_all(&other).unwrap();
        for name in ["payload", "integration"] {
            Command::new("cp")
                .args(["-a", "--"])
                .arg(base.join(name))
                .arg(other.join(name))
                .status()
                .unwrap();
        }
        put(
            &other.join("bundle.json"),
            &fs::read_to_string(base.join("bundle.json"))
                .unwrap()
                .replace("0.0.1-parity", "0.0.9-parity"),
        );
        let same = base.join("world/same bundle");
        fs::create_dir_all(&same).unwrap();
        for name in ["payload", "integration", "bundle.json"] {
            Command::new("cp")
                .args(["-a", "--"])
                .arg(base.join(name))
                .arg(same.join(name))
                .status()
                .unwrap();
        }
    });
    lab.step(
        "bundle of another release",
        &with(
            &["install", "--bundle", "{B}/world/other bundle"],
            &FLIGHTDECK,
        ),
        &plain,
    );
    lab.step(
        "missing bundle",
        &with(&["install", "--bundle", "{B}/world/no bundle"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "bundle elsewhere",
        &with(
            &["install", "--bundle", "{B}/world/same bundle/"],
            &FLIGHTDECK,
        ),
        &stub,
    );
    lab.step(
        "restore after the bundle elsewhere",
        &with(&["restore"], &FLIGHTDECK),
        &plain,
    );
    fresh(&lab);
    let locks = hold(&lab, "world/runtime/private/play.lock");
    lab.step("runtime in use: install", &install, &plain);
    lab.step(
        "runtime in use: restore",
        &with(&["restore"], &FLIGHTDECK),
        &plain,
    );
    lab.step("runtime in use: status", &status, &plain);
    drop(locks);
    let running = occupy(&lab, "world/runtime/local/msfs-prefix");
    lab.step("simulator running: install", &install, &plain);
    lab.step("simulator running: status", &status, &plain);
    release(running);
    lab.both(|base| put(&base.join(runtime).join("private/proton-switch.json"), "{}"));
    lab.step("interrupted Proton switch", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        put(&base.join("world/elsewhere.lock"), "");
        symlink(
            base.join("world/elsewhere.lock"),
            base.join(runtime).join("private/play.lock"),
        )
        .unwrap();
    });
    lab.step("lock that is a link", &install, &plain);
    lab.both(|base| {
        fs::remove_file(base.join(runtime).join("private/play.lock")).unwrap();
        fs::hard_link(
            base.join("world/elsewhere.lock"),
            base.join(runtime).join("private/play.lock"),
        )
        .unwrap();
    });
    lab.step("lock with a second name", &install, &plain);
    lab.both(|base| {
        fs::remove_file(base.join(runtime).join("private/play.lock")).unwrap();
        fs::create_dir(base.join(runtime).join("private/play.lock")).unwrap();
    });
    lab.step("lock that is a directory", &install, &plain);
    fresh(&lab);
    lab.both(|base| {
        put(
            &base.join(runtime).join("private/runtime.json"),
            &format!("{{\"game_id\": \"msfs2024\"{}}}", " ".repeat(1024 * 1024)),
        )
    });
    lab.step("oversized runtime description", &status, &plain);
    lab.both(|base| {
        put(
            &base.join(runtime).join("private/runtime.json"),
            "{\"game_id\": \"msfs2024\"} \u{ff}",
        )
    });
    lab.both(|base| {
        fs::write(
            base.join(runtime).join("private/runtime.json"),
            b"{\"game_id\": \"msfs2024\", \"note\": \"\xff\"}",
        )
        .unwrap()
    });
    lab.step("runtime description that is not UTF-8", &status, &plain);
    fresh(&lab);
    lab.step(
        "restore without a patch",
        &with(&["restore"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "configure without a patch",
        &with(&["configure", "--json"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "open without a patch",
        &with(&["open"], &FLIGHTDECK),
        &plain,
    );
    lab.both(|base| {
        put(&base.join(runtime).join("private/fenix-compat.json"), "{}");
        fenix_installed(&base.join(runtime).join("local/msfs-prefix"), "flight");
        put(&base.join("world/home/FenixInstaller.exe"), "MZ installer");
    });
    lab.step("earlier local patch: status", &status, &plain);
    lab.step(
        "earlier local patch: open",
        &with(&["open"], &FLIGHTDECK),
        &plain,
    );
    lab.step(
        "earlier local patch: installer",
        &with(
            &["installer", "--exe", "{B}/world/home/FenixInstaller.exe"],
            &FLIGHTDECK,
        ),
        &plain,
    );
    lab.step(
        "earlier local patch: guided setup",
        &with(&["gui", "--text"], &FLIGHTDECK),
        &Options {
            stdin: "\nq\n".into(),
            ..Options::default()
        },
    );
    fresh(&lab);
    // States this installer did not write itself.
    for (label, state) in [
        (
            "migrated profile",
            r#"{"state": "installed", "version": "0.0.1-parity", "migrated": true, "backup": "private/b", "previous_prefix": "local/p", "work": "local/w"}"#,
        ),
        (
            "state without a backup",
            r#"{"state": "preparing", "version": "0.0.1-parity"}"#,
        ),
        (
            "restored state",
            r#"{"state": "restored", "backup": "private/b", "previous_prefix": "local/p", "work": "local/w"}"#,
        ),
        (
            "state that leads outside",
            r#"{"state": "preparing", "backup": "../outside", "previous_prefix": "local/p", "work": "local/w"}"#,
        ),
        (
            "state of an unknown release",
            r#"{"state": "installed", "version": "0.0.0-ancient", "backup": "private", "previous_prefix": "local/p", "work": "local", "configured": true}"#,
        ),
        (
            "state of an unknown variant",
            r#"{"state": "installed", "version": "0.0.1-parity", "variant": "gone", "work": "local"}"#,
        ),
        ("empty state", "{}"),
    ] {
        lab.both(|base| put(&base.join(runtime).join(core::MARKER), state));
        lab.step(&format!("{label}: status"), &status, &plain);
        lab.step(&format!("{label}: install"), &install, &plain);
        lab.step(
            &format!("{label}: configure"),
            &with(&["configure"], &FLIGHTDECK),
            &plain,
        );
        lab.step(
            &format!("{label}: restore"),
            &with(&["restore"], &FLIGHTDECK),
            &plain,
        );
        lab.step(
            &format!("{label}: open"),
            &with(&["open"], &FLIGHTDECK),
            &plain,
        );
    }
    lab.finish();
}

#[test]
fn steam_refusals() {
    let Some(mut lab) = Lab::new() else { return };
    let plain = Options::default();
    let fresh = |lab: &Lab| {
        lab.both(|base| {
            let _ = fs::remove_dir_all(base.join("world"));
            fs::create_dir_all(base.join("world/home")).unwrap();
            fs::create_dir_all(base.join("world/tmp")).unwrap();
            steam_world(base);
            settle(&base.join("world"));
            bundle(base, 1);
        });
    };
    let stub = Options {
        stub_geometry: true,
        ..display()
    };
    let install = with(&["install"], &STEAM);
    let status = with(&["status"], &STEAM);
    let restore = with(&["restore"], &STEAM);
    let compat = "world/library two/steamapps/compatdata/2537590";
    let state = format!("world/library two/steamapps/{}/2537590", steam::STATE);
    let proton = "world/steam/compatibilitytools.d/pinned-proton";
    fresh(&lab);
    lab.both(|base| {
        fs::rename(base.join(compat), base.join("world/moved profile")).unwrap();
        symlink(base.join("world/moved profile"), base.join(compat)).unwrap();
    });
    lab.step("linked profile: status", &status, &plain);
    lab.step("linked profile: install", &install, &display());
    fresh(&lab);
    lab.both(|base| fs::remove_file(base.join(compat).join("pfx/system.reg")).unwrap());
    lab.step("profile never started: status", &status, &plain);
    lab.step(
        "profile never started: targets",
        &["targets", "--steam-root", "{B}/world/steam"],
        &plain,
    );
    fresh(&lab);
    lab.both(|base| {
        put(
            &base.join(proton).join("files/bin/wine"),
            "an updated Proton",
        )
    });
    lab.step(
        "no supported Proton: install",
        &with(&["install", "--json"], &STEAM),
        &display(),
    );
    lab.step("no supported Proton: status", &status, &plain);
    lab.step("no supported Proton: restore", &restore, &plain);
    lab.step(
        "wrong Proton named",
        &with(
            &[
                "install",
                "--proton",
                "{B}/world/steam/compatibilitytools.d/pinned-proton",
            ],
            &STEAM,
        ),
        &display(),
    );
    lab.step("restore after the wrong Proton", &restore, &plain);
    lab.step(
        "missing Proton named",
        &with(&["install", "--proton", "{B}/world/no proton"], &STEAM),
        &display(),
    );
    lab.step(
        "restore after the missing Proton",
        &with(&["restore", "--json"], &STEAM),
        &plain,
    );
    fresh(&lab);
    lab.both(|base| {
        Command::new("cp")
            .args(["-a", "--"])
            .arg(base.join(proton))
            .arg(base.join("world/my proton"))
            .status()
            .unwrap();
        fs::remove_dir_all(base.join(proton)).unwrap();
    });
    lab.step(
        "Proton named",
        &with(&["install", "--proton", "{B}/world/my proton/"], &STEAM),
        &stub,
    );
    lab.step("Proton named: status", &status, &plain);
    lab.step("Proton named: restore", &restore, &plain);
    lab.step("no Proton at all", &install, &display());
    lab.step("no Proton at all: restore", &restore, &plain);
    fresh(&lab);
    let locks = hold(&lab, &format!("{state}/lock"));
    lab.step("patch in use: install", &install, &display());
    lab.step("patch in use: restore", &restore, &plain);
    lab.step("patch in use: select", &with(&["select"], &STEAM), &plain);
    drop(locks);
    let running = occupy(&lab, &format!("{compat}/pfx"));
    lab.step("simulator running: install", &install, &display());
    lab.step("simulator running: status", &status, &plain);
    release(running);
    lab.step("install", &install, &stub);
    let running = occupy(&lab, &format!("{compat}/pfx"));
    lab.step(
        "simulator running: configure",
        &with(&["configure"], &STEAM),
        &plain,
    );
    lab.step(
        "simulator running: select",
        &with(&["select"], &STEAM),
        &plain,
    );
    lab.step("simulator running: restore", &restore, &plain);
    release(running);
    lab.both(|base| fs::remove_file(base.join("world/steam/config/config.vdf")).unwrap());
    lab.step(
        "Steam without a configuration: select",
        &with(&["select"], &STEAM),
        &plain,
    );
    lab.step("Steam without a configuration: status", &status, &plain);
    lab.both(|base| {
        put(
            &base.join("world/steam/config/config.vdf"),
            "\"Something\"\n{\n}\n",
        )
    });
    lab.step(
        "Steam with an unknown configuration: select",
        &with(&["select", "--json"], &STEAM),
        &plain,
    );
    lab.both(|base| {
        put_mode(
            &base.join("world/steam/config/config.vdf"),
            &STEAM_CONFIG.replace('\n', "\r\n"),
            0o600,
        )
    });
    lab.step(
        "Steam configuration with Windows line ends: select",
        &with(&["select"], &STEAM),
        &plain,
    );
    lab.both(|base| {
        put(
            &base.join(
                "world/steam/compatibilitytools.d/Proton-Fenix-A320-fixture/files/bin/wineserver",
            ),
            "tampered",
        )
    });
    lab.step("changed tool: status", &status, &plain);
    lab.step(
        "changed tool: configure",
        &with(&["configure"], &STEAM),
        &plain,
    );
    lab.step(
        "changed tool: open",
        &with(&["open", "--json"], &STEAM),
        &plain,
    );
    lab.step("changed tool: install", &install, &display());
    lab.step("changed tool: restore", &restore, &plain);
    fresh(&lab);
    for (label, marker) in [
        (
            "state of another simulator",
            r#"{"target": "steam", "game": "msfs2020", "state": "installed", "version": "0.0.1-parity", "tool": "x", "variant": "fixture", "work": "w", "previous": "p"}"#,
        ),
        (
            "state of an unknown release",
            r#"{"target": "steam", "game": "msfs2024", "state": "installed", "version": "0.0.0-ancient", "tool": "x", "variant": "fixture", "work": "w", "previous": "p"}"#,
        ),
        (
            "state of an unknown variant",
            r#"{"target": "steam", "game": "msfs2024", "state": "installed", "version": "0.0.1-parity", "tool": "x", "variant": "gone", "work": "w", "previous": "p"}"#,
        ),
        (
            "state with a missing tool",
            r#"{"target": "steam", "game": "msfs2024", "state": "installed", "version": "0.0.1-parity", "tool": "Proton-Fenix-A320-fixture", "variant": "fixture", "work": "w", "previous": "p"}"#,
        ),
        (
            "state that leads outside",
            r#"{"target": "steam", "game": "msfs2024", "state": "installed", "version": "0.0.1-parity", "tool": "../../outside", "variant": "fixture", "work": "../w", "previous": "/p"}"#,
        ),
        ("state without its fields", r#"{"state": "committing"}"#),
        ("empty state", "{}"),
    ] {
        lab.both(|base| put(&base.join(&state).join("state.json"), marker));
        lab.step(&format!("{label}: status"), &status, &plain);
        lab.step(&format!("{label}: install"), &install, &display());
        lab.step(
            &format!("{label}: configure"),
            &with(&["configure"], &STEAM),
            &plain,
        );
        lab.step(
            &format!("{label}: select"),
            &with(&["select"], &STEAM),
            &plain,
        );
        lab.step(&format!("{label}: open"), &with(&["open"], &STEAM), &plain);
        lab.step(&format!("{label}: restore"), &restore, &plain);
        lab.both(|base| {
            let _ = fs::remove_file(base.join(&state).join("state.json"));
        });
    }
    lab.finish();
}

/// What a library call returned: `["ok", value]` or `["error", kind, message]`.
fn outcome<T: Into<Value>>(result: Result<T>) -> Value {
    match result {
        Ok(value) => json!(["ok", value.into()]),
        Err(fenix_patch::Error::Patch(message)) => json!(["error", "PatchError", message]),
        Err(other) => json!(["error", "other", other.to_string()]),
    }
}

const PYTHON_OUTCOME: &str = "def outcome(call):\n    try:\n        return ['ok', call()]\n    except core.PatchError as error:\n        return ['error', 'PatchError', str(error)]\n    except Exception as error:\n        return ['error', 'other', str(error)]\n";

#[test]
fn steam_configuration_edits() {
    let Some(lab) = Lab::new() else { return };
    let crlf = STEAM_CONFIG.replace('\n', "\r\n");
    let (start, end) = (
        STEAM_CONFIG.find("\t\t\t\t\"CompatToolMapping\"").unwrap(),
        STEAM_CONFIG.find("\t\t\t\t\"quoted\"").unwrap(),
    );
    let bare = STEAM_CONFIG.replace(&STEAM_CONFIG[start..end], "");
    let only = STEAM_CONFIG.replace("\"440\"", "\"2537590\"");
    let nested = format!(
        "{}\"x\" \"y\"{}",
        "\"a\"\n{\n".repeat(150),
        "\n}".repeat(150)
    );
    let mut documents: Vec<String> = vec![
        STEAM_CONFIG.to_string(),
        crlf.clone(),
        bare.clone(),
        bare.replace('\n', "\r\n"),
        only.clone(),
        STEAM_CONFIG.trim_end().to_string(),
        only.trim_end().to_string(),
        STEAM_CONFIG.replace("\"CompatToolMapping\"", "\"compattoolmapping\"").replace("\"Valve\"", "\"VALVE\""),
        STEAM_CONFIG.replace("\t\t\t\t\t\"440\"\n", "\t\t\t\t\t// a comment { with braces\n\t\t\t\t\t\"440\"\n"),
        STEAM_CONFIG.replace("\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n", "\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t\t\"2537590\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"other\"\n\t\t\t\t\t}\n"),
        STEAM_CONFIG.replace("\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n", "\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t\t\"2537590\"\t\t\"text\"\n"),
        STEAM_CONFIG.replace("\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n", "\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t\t\"2537590\" { \"name\" \"inline\" }\n"),
        STEAM_CONFIG.replace("\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n", "\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t\t\"2537590\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"\"\n\t\t\t\t\t}\n\t\t\t\t\t\"2537590\"\n\t\t\t\t\t{\n\t\t\t\t\t}\n"),
        STEAM_CONFIG.replace("\"name\"\t\t\"proton_experimental\"", "\"Name\"\t\t\"Upper\"\n\t\t\t\t\t\t\"name\"\t\t\"lower\""),
        STEAM_CONFIG.replace("\"440\"", "\"2537590\"\n\t\t\t\t\t{\n\t\t\t\t\t}\n\t\t\t\t\t\"440\""),
        STEAM_CONFIG.replace("\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{", "\t\t\t\t\"CompatToolMapping\"\t\t\"text\"\n\t\t\t\t\"Other\"\n\t\t\t\t{"),
        STEAM_CONFIG.replace("\t\t\t\t}\n\t\t\t\t\"quoted\"", "\t\t\t\t} \"after\" \"brace\"\n\t\t\t\t\"quoted\""),
        STEAM_CONFIG.replace("\t\t\t\t}\n\t\t\t\t\"quoted\"", "\t\t\t\t\t\"trailing\" \"entry\" }\n\t\t\t\t\"quoted\""),
        STEAM_CONFIG.replace("\"Steam\"\n", "\"Steam\"\n\t\t\t{\n\t\t\t}\n\t\t\t\"steam\"\n"),
        STEAM_CONFIG.replace("\"Valve\"", "Valve"),
        STEAM_CONFIG.replace("\"quoted\"\t\t\"a \\\"brace\\\" { inside\"", "\"quoted\"\t\t\"ends with a backslash \\\\\"\n\t\t\t\t\"unicode\"\t\t\"\u{e4}\u{20ac}\u{1f600}\""),
        STEAM_CONFIG.replace("\t\t\t\t\"quoted\"", "\t\t\t\t\"multi\nline\"\t\t\"value\nwith lines\"\n\t\t\t\t\"quoted\""),
        STEAM_CONFIG.replace('\t', "  "),
        STEAM_CONFIG.replace("\n\t\"Software\"", "\u{1c}\u{a0}\n\t\"Software\""),
        format!("{STEAM_CONFIG}\"Second\"\n{{\n}}\n"),
        format!("{STEAM_CONFIG}trailing words"),
        format!("// leading comment\n{STEAM_CONFIG}// trailing comment"),
        STEAM_CONFIG[..STEAM_CONFIG.len() - 3].to_string(),
        "\"Other\"\n{\n}\n".to_string(),
        "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\t\t\"text\"\n\t\t}\n\t}\n}\n".to_string(),
        "\"InstallConfigStore\" { \"Software\" { \"Valve\" { \"Steam\" { } } } }".to_string(),
        "\"InstallConfigStore\"\n{\n\"Software\"\n{\n\"Valve\"\n{\n\"Steam\"\n{\n}\n}\n}\n}".to_string(),
        "\"InstallConfigStore\"\r\n{\r\n\t\"Software\"\r\n\t{\r\n\t\t\"Valve\"\r\n\t\t{\r\n\t\t\t\"Steam\"\r\n\t\t\t{\r\n\t\t\t}\r\n\t\t}\r\n\t}\r\n}".to_string(),
        String::new(),
        " \n\t".to_string(),
        "{".to_string(),
        "}".to_string(),
        "\"key\"".to_string(),
        "\"key\" \"value\" \"odd\"".to_string(),
        "\"unterminated".to_string(),
        nested,
    ];
    documents
        .push(vdf::set_compat_tool(&bare, "2537590", Some("Proton-Fenix-A320-fixture")).unwrap());
    let mut cases = Vec::new();
    for document in &documents {
        for appid in ["2537590", "440", "1250410", "12x", ""] {
            for tool in [
                Value::Null,
                json!("Proton-Fenix-A320-fixture"),
                json!("Tool B 1.0_x"),
                json!("bad\"name"),
                json!("bad\nname"),
                json!(""),
            ] {
                cases.push(json!({"text": document, "appid": appid, "tool": tool}));
            }
        }
    }
    let script = format!(
        "{PYTHON_OUTCOME}result = [[outcome(lambda: vdf.compat_tool(case['text'], case['appid'])), outcome(lambda: vdf.set_compat_tool(case['text'], case['appid'], case['tool']))] for case in argument]"
    );
    let python = lab.python_eval(&script, &Value::Array(cases.clone()));
    let mut differences = Vec::new();
    for (case, expected) in cases.iter().zip(python.as_array().unwrap()) {
        let (text, appid, tool) = (
            case["text"].as_str().unwrap(),
            case["appid"].as_str().unwrap(),
            case["tool"].as_str(),
        );
        let rust = json!([
            outcome(vdf::compat_tool(text, appid)),
            outcome(vdf::set_compat_tool(text, appid, tool))
        ]);
        if &rust != expected {
            differences.push(format!(
                "{appid:?} {tool:?} in {text:?}\n  Python: {expected}\n  Rust:   {rust}"
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} of {} edits differ:\n{}",
        differences.len(),
        cases.len(),
        differences.join("\n")
    );
    assert!(
        python
            .as_array()
            .unwrap()
            .iter()
            .filter(|result| result[1][0] == "ok")
            .count()
            > 200,
        "too few edits succeeded to mean anything"
    );
}

#[test]
fn json_output() {
    let Some(lab) = Lab::new() else { return };
    let documents = [
        r#"{"b": 1, "a": [true, false, null, {}, [], {"": ""}], "nested": {"z": {"y": [1, [2, [3]]]}}}"#,
        r#"[0, -0, 1.0, -1.5e-10, 1e5, 1E-7, 0.1, 1e15, 1e16, 1e22, 123456789.123456789, 5e-324, 1.7976931348623157e308, 100000000000000000000.0, 2.50, 0.000123, 0.0001, 0.00001]"#,
        r#"[123456789012345678901234567890, -98765432109876543210, 18446744073709551616, 9223372036854775807, -9223372036854775808]"#,
        "[\"\u{e4}\u{f6}\u{fc} \u{20ac} \u{2026} \u{201c}quoted\u{201d} \u{2192} \u{b7} \u{1f600} \u{7f} \u{80} \u{2028}\", \"tab\\tnewline\\nquote\\\" backslash\\\\ slash/ \\b\\f\\r \\u0001 \\u001f \\ud83d\\ude00 \\u00e4\"]",
        r#"{"format": 1, "state": "installed", "work": "local/fenix-patch-20261012T101112-abcdef01", "original_prefix_id": [45, 281474976710657], "variant": null, "proton_before": {"schema": 1, "ratio": 1.50}}"#,
        r#"{"duplicate": 1, "other": 2, "duplicate": 3}"#,
        " \n\t[ 1 ,\r\n 2 ]\n ",
        "\"text\"",
        "17",
        "null",
    ];
    let argument = json!(documents);
    let python = lab.python_eval("result = [[json.dumps(json.loads(text)), json.dumps(json.loads(text), indent=2), json.dumps(json.loads(text), indent=4)] for text in argument]", &argument);
    for (document, expected) in documents.iter().zip(python.as_array().unwrap()) {
        let value = json::loads(document).unwrap();
        let rust = json!([
            json::dumps(&value, None),
            json::dumps(&value, Some(2)),
            json::dumps(&value, Some(4))
        ]);
        assert_eq!(&rust, expected, "{document}");
    }
    // What Python compares as equal, and what it does not.
    let pairs = [
        (
            r#"{"a": 1, "b": [1.0, true]}"#,
            r#"{"b": [1, 1], "a": 1.0}"#,
        ),
        (r#"{"a": 1}"#, r#"{"a": "1"}"#),
        (r#"[1, 2]"#, r#"[2, 1]"#),
        (r#"{"a": null}"#, r#"{}"#),
        (r#"[0, false, ""]"#, r#"[false, 0, ""]"#),
        (r#"12345678901234567890123"#, r#"12345678901234567890124"#),
    ];
    let python = lab.python_eval(
        "result = [json.loads(a) == json.loads(b) for a, b in argument]",
        &json!(pairs),
    );
    for ((a, b), expected) in pairs.iter().zip(python.as_array().unwrap()) {
        assert_eq!(
            &json!(json::equal(
                &json::loads(a).unwrap(),
                &json::loads(b).unwrap()
            )),
            expected,
            "{a} == {b}"
        );
    }
}

#[test]
fn page_and_version() {
    let Some(lab) = Lab::new() else { return };
    let python = lab.python_eval("import fenix_patch\nresult = [web.PAGE, fenix_patch.__version__, web.IDLE_SECONDS, guide.DASHBOARD, core.MARKER, str(core.PROGRAM), str(core.CONFIG), core.ENV, steam.GAME_ENV, steam.GAMES, [[key, *value] for key, value in steam.PROTON_DOWNLOADS.items()], core.DOWNLOADS, core.GEOMETRY_DOWNLOADS, core.GEOMETRY_SHA256, core.GEOMETRY_PATH, core.NEEDS_SIGN_IN, steam.STATE, steam.TOOL_MARKER]", &Value::Null);
    assert_eq!(python[0], web::PAGE);
    assert_eq!(python[1], fenix_patch::VERSION);
    assert_eq!(python[2], web::IDLE_SECONDS);
    assert_eq!(python[3], fenix_patch::guide::DASHBOARD);
    assert_eq!(python[4], core::MARKER);
    assert_eq!(python[5], core::PROGRAM);
    assert_eq!(python[6], core::CONFIG);
    let pairs = |pairs: &[(&str, &str)]| {
        Value::Object(
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), json!(value)))
                .collect(),
        )
    };
    assert_eq!(
        json::dumps(&python[7], None),
        json::dumps(&pairs(&core::ENV), None)
    );
    assert_eq!(
        json::dumps(&python[8], None),
        json::dumps(&pairs(&steam::GAME_ENV), None)
    );
    let games: json::Map<String, Value> = steam::GAMES
        .iter()
        .map(|game| (game.key.to_string(), json!({"appid": game.appid, "title": game.title, "settings": game.settings, "executable": game.executable})))
        .collect();
    assert_eq!(
        json::dumps(&python[9], None),
        json::dumps(&Value::Object(games), None)
    );
    let downloads: Vec<Value> = steam::proton_downloads()
        .iter()
        .map(|pinned| json!([pinned.variant, pinned.url, pinned.sha256, pinned.folder]))
        .collect();
    assert_eq!(python[10], Value::Array(downloads));
    let listed = |entries: &[(&str, &str, &str)]| {
        Value::Object(
            entries
                .iter()
                .map(|(name, url, sha)| (name.to_string(), json!([url, sha])))
                .collect(),
        )
    };
    assert_eq!(
        json::dumps(&python[11], None),
        json::dumps(&listed(&core::DOWNLOADS), None)
    );
    assert_eq!(
        json::dumps(&python[12], None),
        json::dumps(&listed(&core::GEOMETRY_DOWNLOADS), None)
    );
    assert_eq!(python[13], core::GEOMETRY_SHA256);
    assert_eq!(python[14], core::GEOMETRY_PATH);
    assert_eq!(python[15], core::NEEDS_SIGN_IN);
    assert_eq!(python[16], steam::STATE);
    assert_eq!(python[17], steam::TOOL_MARKER);
}

fn utf16(text: &str) -> Vec<u8> {
    [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

/// One profile for `configure_prefix`: the three documents it edits, as bytes or absent.
struct Settings {
    name: &'static str,
    config: Option<Vec<u8>>,
    persisted: Option<Vec<u8>>,
    launch: Option<Vec<u8>>,
    /// Folders below `drive_c/users` that contain the simulator's settings folder.
    users: &'static [&'static str],
}

fn text(content: &str) -> Option<Vec<u8>> {
    Some(content.as_bytes().to_vec())
}

fn settings_cases() -> Vec<Settings> {
    let simple = || text("<Config><displayMode>GPU</displayMode></Config>");
    let case = |name, config, persisted, launch| Settings {
        name,
        config,
        persisted,
        launch,
        users: &["pilot"],
    };
    vec![
        case("plain", simple(), text("<Settings><fcuReadoutsType>1</fcuReadoutsType><Volume>0.4</Volume></Settings>"), None),
        case("no documents", None, None, None),
        case("only the readout document", None, text("<Settings/>"), None),
        case(
            "as written by .NET",
            text("\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<FenixConfig>\r\n  <displayMode>GPU</displayMode>\r\n  <newRender>true</newRender>\r\n  <preferCPU>false</preferCPU>\r\n  <Volume unit=\"dB &amp; more\" note='single \"quotes\"'>0.4</Volume>\r\n  <Empty />\r\n  <AlsoEmpty></AlsoEmpty>\r\n  <Note><![CDATA[a < b && c]]> tail</Note>\r\n  <Path>C:\\Program Files\\x &lt;y&gt; &quot;z&quot; &apos;q&apos; &#xE4; &#8364;</Path>\r\n</FenixConfig>\r\n"),
            text("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<Persistancy>\n\t<fcuReadoutsType>1</fcuReadoutsType>\n\t<other attr=\"a&#10;b\tc\nd\" />\n</Persistancy>\n"),
            text("<?xml version=\"1.0\" encoding=\"Windows-1252\"?>\r\n\r\n<SimBase.Document Type=\"Launch\" version=\"1,0\">\r\n  <Descr>Launch</Descr>\r\n  <Filename>EXE.xml</Filename>\r\n  <Disabled>False</Disabled>\r\n  <Launch.ManualLoad>False</Launch.ManualLoad>\r\n  <Launch.Addon>\r\n    <Name>Other \u{e4}ddon</Name>\r\n    <Disabled>False</Disabled>\r\n    <Path>C:\\Other\\other.exe</Path>\r\n  </Launch.Addon>\r\n</SimBase.Document>\r\n".replace('\u{e4}', "\u{fffd}").as_str()).map(|bytes| String::from_utf8(bytes).unwrap().replace('\u{fffd}', "\u{e4}").chars().map(|c| if c == '\u{e4}' { 0xe4u8 } else { c as u8 }).collect()),
        ),
        case(
            "settings present, nested and repeated",
            text("<Config>\n  <group><displayMode>nested</displayMode></group>\n  <displayMode>first</displayMode>\n  <displayMode>second</displayMode>\n  <DisplayMode>other case</DisplayMode>\n  <multithread/>\n  <preferCPU>  </preferCPU>\n  <newRender>true<inner/>tail</newRender>\n</Config>"),
            text("<Settings>text<fcuReadoutsType a=\"1\"><x/></fcuReadoutsType>tail</Settings>\n\n"),
            text("<SimBase.Document>\n  <Launch.Addon><Name>Fenix one</Name><Path>a</Path></Launch.Addon> tail one\n  <Launch.Addon><Name>Other</Name></Launch.Addon>\n  <Launch.Addon><Path>C:\\FENIXSim\\x.exe</Path><ManualLoad>True</ManualLoad></Launch.Addon> tail two\n  <Launch.Addon><Name>fenix three</Name></Launch.Addon>\n</SimBase.Document>"),
        ),
        case("empty root elements", text("<Config/>"), text("<Settings></Settings>"), text("<SimBase.Document/>")),
        case("launch entry found by its path", simple(), None, text("<SimBase.Document><Launch.Addon><Path>C:\\x\\FenixBootstrapper.exe</Path></Launch.Addon></SimBase.Document>")),
        case("launch entries of others only", simple(), None, text("<SimBase.Document Type=\"Launch\"><Launch.Addon/><Other/></SimBase.Document>")),
        case("UTF-16", Some(utf16("<?xml version=\"1.0\" encoding=\"utf-16\"?>\r\n<Config>\r\n  <displayMode>GPU</displayMode>\r\n  <name>\u{e4}\u{20ac}</name>\r\n</Config>")), None, None),
        case("Latin-1", Some(b"<?xml version='1.0' encoding='ISO-8859-1'?>\n<Config><name>\xe4\xdf</name></Config>".to_vec()), None, None),
        case("Windows-1252", Some(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?>\n<Config><price>\x80 5</price></Config>".to_vec()), None, None),
        case("ASCII", Some(b"<?xml version=\"1.0\" encoding=\"us-ascii\"?><Config><a>b</a></Config>".to_vec()), None, None),
        case("character references", text("<Config a=\"&#13;&#10;&#9;&lt;&amp;\">&#13;\r&#10;&#9;&gt;&#x1F600;</Config>"), None, None),
        case("the xml prefix", text("<Config xml:space=\"preserve\" xml:lang=\"en\">  <a xml:space=\"default\"/>  </Config>"), None, None),
        case("document type", text("<!DOCTYPE Config><Config/>"), None, None),
        case("document type in lower case", text("<!doctype Config><Config/>"), None, None),
        case("entity declaration in a comment", text("<Config><!-- <!entity x \"y\"> --></Config>"), None, None),
        case("document type in the readout document", simple(), text("<!DOCTYPE x><x/>"), None),
        case("document type in the launch document", simple(), None, text("<!DOCTYPE x><SimBase.Document/>")),
        case("settings is a link", None, None, None),
        Settings { name: "no settings folder", config: simple(), persisted: None, launch: None, users: &[] },
        Settings { name: "two settings folders", config: simple(), persisted: None, launch: None, users: &["pilot", "other"] },
        Settings { name: "settings folder of a system user", config: simple(), persisted: None, launch: None, users: &["Public", "Default", "Default User", "pilot"] },
    ]
}

/// Documents which both installers refuse, each in its own way.
fn broken_documents() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("not closed", b"<Config>".to_vec()),
        ("mismatched", b"<Config></config>".to_vec()),
        ("two roots", b"<Config/><Config/>".to_vec()),
        ("text before", b"x<Config/>".to_vec()),
        ("text after", b"<Config/>x".to_vec()),
        ("unquoted attribute", b"<Config a=1/>".to_vec()),
        ("repeated attribute", b"<Config a='1' a='2'/>".to_vec()),
        ("unknown entity", b"<Config>&nbsp;</Config>".to_vec()),
        ("bare ampersand", b"<Config>a & b</Config>".to_vec()),
        ("control character", b"<Config>\x01</Config>".to_vec()),
        ("invalid reference", b"<Config>&#0;</Config>".to_vec()),
        ("invalid UTF-8", b"<Config>\xff</Config>".to_vec()),
        (
            "late declaration",
            b"<Config><?xml version='1.0'?></Config>".to_vec(),
        ),
        ("invalid name", b"<1Config/>".to_vec()),
        (
            "double hyphen in a comment",
            b"<Config><!-- a -- b --></Config>".to_vec(),
        ),
        (
            "unknown encoding",
            b"<?xml version='1.0' encoding='klingon'?><Config/>".to_vec(),
        ),
    ]
}

fn settings_profile(prefix: &Path, case: &Settings, folder: &str) {
    put(&prefix.join(core::PROGRAM).join("Fenix.exe"), "MZ");
    fs::create_dir_all(prefix.join("drive_c/users")).unwrap();
    let write = |path: PathBuf, content: &Option<Vec<u8>>| {
        if let Some(content) = content {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    };
    write(
        prefix.join(core::CONFIG).join("fenixConfig.xml"),
        &case.config,
    );
    write(
        prefix.join(core::CONFIG).join("persistancy.xml"),
        &case.persisted,
    );
    for user in case.users {
        let settings = prefix
            .join("drive_c/users")
            .join(user)
            .join("AppData/Roaming")
            .join(folder);
        fs::create_dir_all(&settings).unwrap();
        write(settings.join("exe.xml"), &case.launch);
    }
    if case.name == "settings is a link" {
        put(&prefix.join("elsewhere.xml"), "<Config/>");
        fs::create_dir_all(prefix.join(core::CONFIG)).unwrap();
        symlink(
            prefix.join("elsewhere.xml"),
            prefix.join(core::CONFIG).join("fenixConfig.xml"),
        )
        .unwrap();
    }
}

#[test]
fn settings_documents() {
    let Some(mut lab) = Lab::new() else { return };
    let cases = settings_cases();
    let broken = broken_documents();
    lab.both(|base| {
        for (number, case) in cases.iter().enumerate() {
            let folder = if number % 2 == 0 {
                "Microsoft Flight Simulator 2024"
            } else {
                "Microsoft Flight Simulator"
            };
            settings_profile(&base.join(format!("world/cases/{number:02}")), case, folder);
        }
        for (number, (_, document)) in broken.iter().enumerate() {
            let case = Settings {
                name: "broken",
                config: Some(document.clone()),
                persisted: None,
                launch: None,
                users: &["pilot"],
            };
            settings_profile(
                &base.join(format!("world/broken/{number:02}")),
                &case,
                "Microsoft Flight Simulator 2024",
            );
        }
        settle(&base.join("world"));
    });
    let folders: Vec<&str> = (0..cases.len())
        .map(|number| {
            if number % 2 == 0 {
                "Microsoft Flight Simulator 2024"
            } else {
                "Microsoft Flight Simulator"
            }
        })
        .collect();
    let script = format!(
        "{PYTHON_OUTCOME}result = [outcome(lambda: list(core.configure_prefix(Path('world/cases/%02d' % number).resolve(), folder))) for number, folder in enumerate(argument[0])]\nresult = [result, [outcome(lambda: list(core.configure_prefix(Path('world/broken/%02d' % number).resolve())))[:2] for number in range(argument[1])]]"
    );
    let python = lab.python_eval(&script, &json!([folders, broken.len()]));
    let base = lab.base(Side::Rust);
    for (number, case) in cases.iter().enumerate() {
        let rust = outcome(
            core::configure_prefix(
                &base.join(format!("world/cases/{number:02}")),
                folders[number],
            )
            .map(|(ready, legacy)| json!([ready, legacy])),
        );
        if lab.normal(Side::Rust, &rust.to_string())
            != lab.normal(Side::Python, &python[0][number].to_string())
        {
            lab.problems.push(format!(
                "{}: Python returned {}, Rust {}",
                case.name, python[0][number], rust
            ));
        }
    }
    for (number, (name, _)) in broken.iter().enumerate() {
        // Python ends with an uncaught ParseError here; the port reports the document by name.
        let rust = core::configure_prefix(
            &base.join(format!("world/broken/{number:02}")),
            core::DEFAULT_FOLDER,
        );
        let message = rust
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        if python[1][number] != json!(["error", "other"])
            || !message.starts_with("fenixConfig.xml is not well-formed XML (")
        {
            lab.problems.push(format!(
                "broken document ({name}): Python {}, Rust {rust:?}",
                python[1][number]
            ));
        }
    }
    lab.same_trees("settings documents");
    lab.finish();
}

#[test]
fn settings_documents_where_the_port_keeps_more() {
    // ElementTree drops comments, processing instructions and unused namespace
    // declarations, and renames namespace prefixes. The port keeps the document as written.
    let Some(lab) = Lab::new() else { return };
    let cases: [(&str, &str, &str); 3] = [
        (
            "<Config><!-- keep me --><displayMode>GPU</displayMode><?app data?><x/></Config>",
            "<Config><displayMode>CPU</displayMode><x /><newRender>false</newRender><preferCPU>true</preferCPU><multithread>true</multithread></Config>",
            "<Config><!-- keep me --><displayMode>CPU</displayMode><?app data?><x /><newRender>false</newRender><preferCPU>true</preferCPU><multithread>true</multithread></Config>",
        ),
        (
            "<Config xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\"><displayMode>GPU</displayMode><newRender>x</newRender><preferCPU>x</preferCPU><multithread>x</multithread></Config>",
            "<Config><displayMode>CPU</displayMode><newRender>false</newRender><preferCPU>true</preferCPU><multithread>true</multithread></Config>",
            "<Config xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\"><displayMode>CPU</displayMode><newRender>false</newRender><preferCPU>true</preferCPU><multithread>true</multithread></Config>",
        ),
        (
            "<Config xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:f=\"urn:fenix\"><displayMode>GPU</displayMode><newRender>x</newRender><preferCPU>x</preferCPU><multithread>x</multithread><f:own xsi:nil=\"true\"/></Config>",
            "<Config xmlns:ns0=\"urn:fenix\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><displayMode>CPU</displayMode><newRender>false</newRender><preferCPU>true</preferCPU><multithread>true</multithread><ns0:own xsi:nil=\"true\" /></Config>",
            "<Config xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:f=\"urn:fenix\"><displayMode>CPU</displayMode><newRender>false</newRender><preferCPU>true</preferCPU><multithread>true</multithread><f:own xsi:nil=\"true\" /></Config>",
        ),
    ];
    let declaration = "<?xml version='1.0' encoding='utf-8'?>\n";
    lab.both(|base| {
        for (number, (document, _, _)) in cases.iter().enumerate() {
            let case = Settings {
                name: "kept",
                config: text(document),
                persisted: None,
                launch: None,
                users: &["pilot"],
            };
            settings_profile(
                &base.join(format!("world/kept/{number}")),
                &case,
                core::DEFAULT_FOLDER,
            );
        }
    });
    lab.python_eval("result = [list(core.configure_prefix(Path('world/kept/%d' % number).resolve())) for number in range(argument)]", &json!(cases.len()));
    for (number, (_, python, rust)) in cases.iter().enumerate() {
        let written = |side: Side| {
            fs::read_to_string(
                lab.base(side)
                    .join(format!("world/kept/{number}"))
                    .join(core::CONFIG)
                    .join("fenixConfig.xml"),
            )
            .unwrap()
        };
        core::configure_prefix(
            &lab.base(Side::Rust).join(format!("world/kept/{number}")),
            core::DEFAULT_FOLDER,
        )
        .unwrap();
        assert_eq!(
            written(Side::Python),
            format!("{declaration}{python}"),
            "Python, document {number}"
        );
        assert_eq!(
            written(Side::Rust),
            format!("{declaration}{rust}"),
            "Rust, document {number}"
        );
    }
}

#[test]
fn proton_archive_extraction() {
    let Some(mut lab) = Lab::new() else { return };
    let (Some(tar), Some(_)) = (tool("tar"), tool("xz")) else {
        notice(
            "tar or xz was not found; the comparison of the Proton archive extraction is skipped",
        );
        return;
    };
    // The same archive for both sides, made by the system's own tar.
    let source = lab.sandbox.join("source");
    let folder = source.join("proton-fixture");
    runner(&folder, "1700000002 parity-proton-11.0", true);
    put_mode(&folder.join("files/bin/setuid-tool"), "tool", 0o4755);
    put_mode(
        &folder.join("files/share/group-writable.txt"),
        "text",
        0o664,
    );
    put_mode(&folder.join("files/share/private.txt"), "text", 0o600);
    put_mode(&folder.join("files/share/no-owner-execute"), "text", 0o655);
    fs::create_dir_all(folder.join("files/share/default_pfx/drive_c/empty directory")).unwrap();
    fs::set_permissions(
        folder.join("files/share/default_pfx"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    symlink(
        "../lib/wine/x86_64-unix/ntdll.so",
        folder.join("files/bin/linked.so"),
    )
    .unwrap();
    symlink("./a/../version", folder.join("unnormalised link")).unwrap();
    symlink("missing target", folder.join("dangling link")).unwrap();
    fs::hard_link(folder.join("version"), folder.join("files/version copy")).unwrap();
    put(
        &folder.join(format!(
            "files/{}/deep.txt",
            "long-directory-name/".repeat(12)
        )),
        "a path longer than a tar header field",
    );
    settle(&source);
    for (format, name) in [("gnu", "gnu.tar.xz"), ("pax", "pax.tar.xz")] {
        let status = Command::new(&tar)
            .args(["--format", format, "-C"])
            .arg(&source)
            .args(["-cJf"])
            .arg(lab.sandbox.join(name))
            .arg("proton-fixture")
            .status()
            .unwrap();
        assert!(status.success());
        let host = Host::new(lab.base(Side::Rust));
        let python = lab.python_eval(
            &format!("{PYTHON_OUTCOME}result = outcome(lambda: str(steam.extract_proton(Path(argument[0]), Path('world/extracted') / argument[1], 'proton-fixture')))"),
            &json!([lab.sandbox.join(name), format]),
        );
        let destination = lab.base(Side::Rust).join("world/extracted").join(format);
        let rust = outcome(
            steam::extract_proton(
                &host,
                &lab.sandbox.join(name),
                &destination,
                "proton-fixture",
            )
            .map(|path| path.display().to_string()),
        );
        assert_eq!(python[0], "ok", "{python}");
        assert_eq!(
            lab.normal(Side::Rust, &rust.to_string()),
            lab.normal(Side::Python, &python.to_string())
                .replace("world/extracted", "<BASE>/world/extracted"),
            "{format}"
        );
        lab.same_trees(&format!("archive in {format} format"));
        let wrong = outcome(
            steam::extract_proton(
                &host,
                &lab.sandbox.join(name),
                &lab.base(Side::Rust).join("world/wrong"),
                "another-folder",
            )
            .map(|path| path.display().to_string()),
        );
        let python = lab.python_eval(
            &format!("{PYTHON_OUTCOME}result = outcome(lambda: str(steam.extract_proton(Path(argument), Path('world/wrong'), 'another-folder')))"),
            &json!(lab.sandbox.join(name)),
        );
        assert_eq!(wrong, python, "{format}");
        lab.both(|base| fs::remove_dir_all(base.join("world/wrong")).unwrap());
    }
    // Archives that try to leave their folder; Python's tarfile writes them for both sides.
    let members = json!([
        ["parent", "../escaped.txt", "file", ""],
        ["absolute link", "proton-fixture/link", "symlink", "/etc"],
        [
            "link out",
            "proton-fixture/link",
            "symlink",
            "../../outside"
        ],
        [
            "hard link out",
            "proton-fixture/hard",
            "hardlink",
            "../outside.txt"
        ],
        ["device", "proton-fixture/device", "device", ""],
        ["pipe", "proton-fixture/pipe", "fifo", ""],
        [
            "through a link",
            "proton-fixture/inner/file.txt",
            "via-link",
            ""
        ],
    ]);
    let python = lab.python_eval(
        &format!(
            "{PYTHON_OUTCOME}import io, tarfile\nresult = []\nfor label, name, kind, target in argument[1]:\n    path = Path(argument[0]) / (label + '.tar.xz')\n    with tarfile.open(path, 'w:xz') as archive:\n        good = tarfile.TarInfo('proton-fixture/version'); good.size = 1; archive.addfile(good, io.BytesIO(b'1'))\n        if kind == 'via-link':\n            link = tarfile.TarInfo('proton-fixture/inner'); link.type = tarfile.SYMTYPE; link.linkname = '..'; archive.addfile(link)\n            link = tarfile.TarInfo('proton-fixture/inner/up'); link.type = tarfile.SYMTYPE; link.linkname = '..'; archive.addfile(link)\n            name = 'proton-fixture/inner/up/escaped.txt'\n        info = tarfile.TarInfo(name)\n        info.type = {{'file': tarfile.REGTYPE, 'via-link': tarfile.REGTYPE, 'symlink': tarfile.SYMTYPE, 'hardlink': tarfile.LNKTYPE, 'device': tarfile.CHRTYPE, 'fifo': tarfile.FIFOTYPE}}[kind]\n        info.linkname = target\n        archive.addfile(info, io.BytesIO(b'') if info.isreg() else None)\n    result.append(outcome(lambda: str(steam.extract_proton(path, Path('world/unsafe') / label, 'proton-fixture')))[:2])"
        ),
        &json!([lab.sandbox, members]),
    );
    let host = Host::new(lab.base(Side::Rust));
    for (member, python) in members
        .as_array()
        .unwrap()
        .iter()
        .zip(python.as_array().unwrap())
    {
        let label = member[0].as_str().unwrap();
        let archive = lab.sandbox.join(format!("{label}.tar.xz"));
        let rust = steam::extract_proton(
            &host,
            &archive,
            &lab.base(Side::Rust).join("world/unsafe").join(label),
            "proton-fixture",
        );
        assert_eq!(
            python,
            &json!(["error", "other"]),
            "Python accepted the archive ‘{label}’"
        );
        assert_eq!(
            rust.unwrap_err(),
            fenix_patch::Error::patch("Unsafe member in the Proton archive"),
            "{label}"
        );
    }
    for side in SIDES {
        for escaped in [
            "world/escaped.txt",
            "world/unsafe/escaped.txt",
            "world/outside",
            "world/outside.txt",
            "escaped.txt",
        ] {
            assert!(
                fs::symlink_metadata(lab.base(side).join(escaped)).is_err(),
                "{side:?} wrote {escaped}"
            );
        }
    }
    lab.finish();
}

/// One guided setup in the browser, started with `--no-browser`.
struct Setup {
    child: std::process::Child,
    output: BufReader<std::process::ChildStdout>,
    banner: String,
    port: u16,
    secret: String,
}

impl Setup {
    fn start(lab: &Lab, side: Side, args: &[&str]) -> Self {
        let mut child = lab
            .command(side, args, &display())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut banner = String::new();
        let mut address = None;
        while address.is_none() {
            let mut line = String::new();
            if output.read_line(&mut line).unwrap() == 0 {
                let mut errors = String::new();
                child
                    .stderr
                    .take()
                    .unwrap()
                    .read_to_string(&mut errors)
                    .unwrap();
                panic!("{side:?}: the setup ended before it printed its address: {banner}{errors}");
            }
            address = line
                .trim_end()
                .strip_prefix("If no window appeared, open: http://127.0.0.1:")
                .map(str::to_string);
            banner.push_str(&line);
        }
        let address = address.unwrap();
        let (port, secret) = address.split_once('/').unwrap();
        Self {
            child,
            output,
            banner,
            port: port.parse().unwrap(),
            secret: secret.trim_end_matches('/').to_string(),
        }
    }

    /// Send raw bytes and return the complete answer. `{S}`, `{P}` stand for the secret and the port.
    fn exchange(&self, request: &str) -> Vec<u8> {
        let request = request
            .replace("{S}", &self.secret)
            .replace("{P}", &self.port.to_string());
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        // Nothing more follows: a server that still waits for input sees the end instead.
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        answer
    }

    /// The answer without what names the server or the moment.
    fn answer(&self, lab: &Lab, side: Side, request: &str) -> String {
        let answer = String::from_utf8_lossy(&self.exchange(request)).into_owned();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
        let head: Vec<&str> = head
            .split("\r\n")
            .filter(|line| !line.starts_with("Server: ") && !line.starts_with("Date: "))
            .collect();
        let text = format!("{}\n\n{body}", head.join("\n"))
            .replace(&self.secret, "<SECRET>")
            .replace(&self.port.to_string(), "<PORT>");
        lab.normal(side, &text)
    }

    fn state(&self) -> Value {
        let answer = self.exchange("GET /{S}/api/state HTTP/1.1\r\nHost: 127.0.0.1:{P}\r\n\r\n");
        let text = String::from_utf8_lossy(&answer).into_owned();
        json::loads(text.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    fn wait_idle(&self) {
        for _ in 0..600 {
            if self.state()["busy"].is_null() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the step did not finish");
    }
}

#[test]
fn guided_setup_in_the_browser() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        put(
            &base
                .join("world/library two/steamapps")
                .join(steam::STATE)
                .join("2537590/downloads"),
            "in the way",
        );
        put(
            &base.join("world/home/Downloads/FenixInstaller-old.exe"),
            "MZ",
        );
        put(&base.join("world/home/Downloads/notes.txt"), "no installer");
        put(&base.join("world/home/Desktop/FENIX setup.EXE"), "MZ");
        put(&base.join("world/home/fenix.exe"), "MZ");
        settle(&base.join("world"));
        // The newest download is offered first.
        let newer = rustix::fs::Timespec {
            tv_sec: FIXTURE_TIME + 100,
            tv_nsec: 0,
        };
        let times = rustix::fs::Timestamps {
            last_access: newer,
            last_modification: newer,
        };
        rustix::fs::utimensat(
            rustix::fs::CWD,
            base.join("world/home/Desktop/FENIX setup.EXE"),
            &times,
            rustix::fs::AtFlags::empty(),
        )
        .unwrap();
        bundle(base, 1);
    });
    let args = with(&["gui", "--no-browser"], &STEAM);
    let setups: Vec<Setup> = SIDES
        .iter()
        .map(|side| Setup::start(&lab, *side, &args))
        .collect();
    let host = "Host: 127.0.0.1:{P}\r\n";
    let json_post = |path: &str, body: &str| {
        format!(
            "POST /{{S}}/{path} HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    let compare = |lab: &mut Lab, label: &str, request: &str| {
        let python = setups[0].answer(lab, Side::Python, request);
        let rust = setups[1].answer(lab, Side::Rust, request);
        if python != rust {
            lab.problems.push(format!(
                "{label}: the answers differ{}",
                first_differences(&python, &rust)
            ));
        }
        python
    };
    let page = compare(
        &mut lab,
        "page",
        &format!("GET /{{S}}/ HTTP/1.1\r\n{host}\r\n"),
    );
    assert!(
        page.starts_with("HTTP/1.0 200 OK\nContent-Type: text/html; charset=utf-8\n")
            && page.ends_with("</html>\n"),
        "{page:.300}"
    );
    let state = compare(
        &mut lab,
        "state",
        &format!("GET /{{S}}/api/state HTTP/1.1\r\n{host}\r\n"),
    );
    assert!(
        state.contains("\"current\": \"install\"") && state.contains("FENIX setup.EXE"),
        "{state}"
    );
    for (label, request) in [
        ("state by name", "GET /{S}/api/state HTTP/1.1\r\nHost: localhost:{P}\r\n\r\n".to_string()),
        ("state with HTTP/1.0", format!("GET /{{S}}/api/state HTTP/1.0\r\n{host}\r\n")),
        ("root", format!("GET / HTTP/1.1\r\n{host}\r\n")),
        ("without the secret", format!("GET /api/state HTTP/1.1\r\n{host}\r\n")),
        ("wrong secret", format!("GET /wrong/api/state HTTP/1.1\r\n{host}\r\n")),
        ("secret without the slash", format!("GET /{{S}} HTTP/1.1\r\n{host}\r\n")),
        ("query", format!("GET /{{S}}/?x=1 HTTP/1.1\r\n{host}\r\n")),
        ("doubled slash", format!("GET //{{S}}/api/state HTTP/1.1\r\n{host}\r\n")),
        ("unknown address", format!("GET /{{S}}/api/unknown HTTP/1.1\r\n{host}\r\n")),
        ("another host", "GET /{S}/api/state HTTP/1.1\r\nHost: attacker.example:{P}\r\n\r\n".to_string()),
        ("host without the port", "GET /{S}/api/state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_string()),
        ("host with a trailing blank", "GET /{S}/api/state HTTP/1.1\r\nHost: 127.0.0.1:{P} \r\n\r\n".to_string()),
        ("host with leading blanks", "GET /{S}/api/state HTTP/1.1\r\nHost:  \t127.0.0.1:{P}\r\n\r\n".to_string()),
        ("host in lower case", "GET /{S}/api/state HTTP/1.1\r\nhost:127.0.0.1:{P}\r\n\r\n".to_string()),
        ("blank before the colon", "GET /{S}/api/state HTTP/1.1\r\nHost : 127.0.0.1:{P}\r\n\r\n".to_string()),
        ("two hosts", "GET /{S}/api/state HTTP/1.1\r\nHost: 127.0.0.1:{P}\r\nHost: attacker.example\r\n\r\n".to_string()),
        ("two hosts, the other first", "GET /{S}/api/state HTTP/1.1\r\nHost: attacker.example\r\nHost: 127.0.0.1:{P}\r\n\r\n".to_string()),
        ("folded host", "GET /{S}/api/state HTTP/1.1\r\nHost: 127.0.0.1:{P}\r\n more\r\n\r\n".to_string()),
        ("host after a line that is no header", "GET /{S}/api/state HTTP/1.1\r\nnonsense\r\nHost: 127.0.0.1:{P}\r\n\r\n".to_string()),
        ("no host", "GET /{S}/api/state HTTP/1.1\r\n\r\n".to_string()),
        ("line feeds only", "GET /{S}/api/state HTTP/1.1\nHost: 127.0.0.1:{P}\n\n".to_string()),
        ("HTTP/0.9", "GET /{S}/api/state\r\n\r\n".to_string()),
        ("HTTP/0.9 with another method", "POST /{S}/api/state\r\n\r\n".to_string()),
        ("PUT", format!("PUT /{{S}}/api/state HTTP/1.1\r\n{host}\r\n")),
        ("HEAD", format!("HEAD /{{S}}/ HTTP/1.1\r\n{host}\r\n")),
        ("OPTIONS", format!("OPTIONS /{{S}}/api/run HTTP/1.1\r\n{host}Origin: http://attacker.example\r\nAccess-Control-Request-Method: POST\r\n\r\n")),
        ("method in lower case", format!("get /{{S}}/ HTTP/1.1\r\n{host}\r\n")),
        ("method with markup", format!("<b> /{{S}}/ HTTP/1.1\r\n{host}\r\n")),
        ("HTTP/2", format!("GET /{{S}}/ HTTP/2.0\r\n{host}\r\n")),
        ("another protocol", format!("GET /{{S}}/ FTP/1.0\r\n{host}\r\n")),
        ("broken version", format!("GET /{{S}}/ HTTP/1.x\r\n{host}\r\n")),
        ("four words", format!("GET /{{S}}/ extra HTTP/1.1\r\n{host}\r\n")),
        ("one word", "GET\r\n\r\n".to_string()),
        ("empty line", "\r\n".to_string()),
        ("nothing", String::new()),
        ("long address", format!("GET /{} HTTP/1.1\r\n{host}\r\n", "a".repeat(70_000))),
        ("long header", format!("GET /{{S}}/ HTTP/1.1\r\n{host}X: {}\r\n\r\n", "a".repeat(70_000))),
        ("many headers", format!("GET /{{S}}/ HTTP/1.1\r\n{host}{}\r\n", "X: 1\r\n".repeat(100))),
        ("as many headers as allowed", format!("GET /{{S}}/api/state HTTP/1.1\r\n{host}{}\r\n", "X: 1\r\n".repeat(98))),
        ("form instead of JSON", format!("POST /{{S}}/api/run HTTP/1.1\r\n{host}Content-Type: text/plain\r\nContent-Length: 21\r\n\r\n{{\"action\": \"install\"}}")),
        ("JSON with a character set", format!("POST /{{S}}/api/run HTTP/1.1\r\n{host}Content-Type: application/json; charset=utf-8\r\nContent-Length: 2\r\n\r\n{{}}")),
        ("no content type", format!("POST /{{S}}/api/quit HTTP/1.1\r\n{host}Content-Length: 2\r\n\r\n{{}}")),
        ("post without the secret", format!("POST /api/quit HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}")),
        ("post to another host", "POST /{S}/api/quit HTTP/1.1\r\nHost: attacker.example\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}".to_string()),
        ("length that is no number", format!("POST /{{S}}/api/select HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: two\r\n\r\n{{}}")),
        ("length that is no number, wrong address", format!("POST /wrong HTTP/1.1\r\n{host}Content-Length: two\r\n\r\n")),
        ("length with blanks and a sign", format!("POST /{{S}}/api/select HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length:  +1_2 \r\n\r\n{{\"index\": 0}}")),
        ("too long", format!("POST /{{S}}/api/select HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: 65537\r\n\r\n")),
        ("far too long", format!("POST /{{S}}/api/select HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: 99999999999999999999999999\r\n\r\n")),
        ("longest allowed", format!("POST /{{S}}/api/select HTTP/1.1\r\n{host}Content-Type: application/json\r\nContent-Length: 65536\r\n\r\n{{\"index\": 0{}}}", " ".repeat(65_524))),
        ("unknown post", json_post("api/unknown", "{}")),
        ("select", json_post("api/select", "{\"index\": 0}")),
        ("select as text", json_post("api/select", "{\"index\": \" 0 \"}")),
        ("select as a fraction", json_post("api/select", "{\"index\": 0.9}")),
        ("select as a truth value", json_post("api/select", "{\"index\": false}")),
        ("select beyond", json_post("api/select", "{\"index\": 1}")),
        ("select below", json_post("api/select", "{\"index\": -1}")),
        ("select nothing", json_post("api/select", "{}")),
        ("select null", json_post("api/select", "{\"index\": null}")),
        ("select a list", json_post("api/select", "{\"index\": []}")),
        ("select a word", json_post("api/select", "{\"index\": \"first\"}")),
        ("select with an empty body", format!("POST /{{S}}/api/select HTTP/1.1\r\n{host}Content-Type: application/json\r\n\r\n")),
        ("body that is a list", json_post("api/select", "[]")),
        ("body with a byte order mark", json_post("api/select", "\u{feff}{\"index\": 0}")),
        ("pick a file without a dialog", json_post("api/pick", "{}")),
    ] {
        compare(&mut lab, label, &request);
    }
    // Wording of JSON syntax errors is the parser's own on each side; the refusal is the same.
    for body in ["{", "x", "{\"index\": 0} trailing", "\u{ff}"] {
        let request = json_post("api/select", body);
        let answers: Vec<String> = SIDES
            .iter()
            .zip(&setups)
            .map(|(side, setup)| setup.answer(&lab, *side, &request))
            .collect();
        assert!(
            answers.iter().all(|answer| answer
                .starts_with("HTTP/1.0 400 Bad Request\nContent-Type: application/json\n")),
            "{body}: {answers:?}"
        );
    }
    let wait = |setups: &[Setup]| setups.iter().for_each(Setup::wait_idle);
    for (label, body) in [
        ("unknown action", "{\"action\": \"format-disk\"}"),
        ("no action", "{}"),
        ("action that is a number", "{\"action\": 5}"),
        ("configure before the patch", "{\"action\": \"configure\"}"),
        (
            "installer without a file",
            "{\"action\": \"installer\", \"executable\": \"  \"}",
        ),
        (
            "installer that is missing",
            "{\"action\": \"installer\", \"executable\": \"'file:///missing%20file.exe'\"}",
        ),
        (
            "installer before the patch",
            "{\"action\": \"installer\", \"executable\": \"~/fenix.exe\"}",
        ),
        ("install", "{\"action\": \"install\"}"),
        ("install again", "{\"action\": \"install\"}"),
        (
            "select before the patch is complete",
            "{\"action\": \"select\"}",
        ),
        ("restore", "{\"action\": \"restore\"}"),
        ("readouts", "{\"action\": \"readouts\"}"),
        ("open", "{\"action\": \"open\"}"),
        ("manager", "{\"action\": \"manager\"}"),
    ] {
        compare(&mut lab, label, &json_post("api/run", body));
        wait(&setups);
        compare(
            &mut lab,
            &format!("state after {label}"),
            &format!("GET /{{S}}/api/state HTTP/1.1\r\n{host}\r\n"),
        );
        lab.same_trees(&format!("browser: {label}"));
        lab.both(|base| {
            let _ = fs::remove_file(base.join("world/calls.log"));
        });
    }
    compare(&mut lab, "quit", &json_post("api/quit", "{}"));
    let mut banners = Vec::new();
    for (side, mut setup) in SIDES.into_iter().zip(setups) {
        let mut rest = String::new();
        setup.output.read_to_string(&mut rest).unwrap();
        let status = setup.child.wait().unwrap();
        assert_eq!(status.code(), Some(0), "{side:?}");
        banners.push(
            lab.normal(side, &format!("{}{rest}", setup.banner))
                .replace(&setup.secret, "<SECRET>")
                .replace(&setup.port.to_string(), "<PORT>"),
        );
    }
    assert_eq!(banners[0], banners[1]);
    assert_eq!(
        banners[1],
        "Fenix A320 Linux Patch — setup is open in your browser.\nIf no window appeared, open: http://127.0.0.1:<PORT>/<SECRET>/\nKeep this running until you are done; Ctrl+C or “Close installer” ends it.\n"
    );
    lab.finish();
}

#[test]
fn guided_setup_in_the_terminal() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        put(
            &base
                .join("world/library two/steamapps")
                .join(steam::STATE)
                .join("2537590/downloads"),
            "in the way",
        );
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let args = with(&["gui", "--text"], &STEAM);
    let answers = |stdin: &str| Options {
        stdin: stdin.to_string(),
        ..display()
    };
    lab.step("look and quit", &args, &answers("q\n"));
    lab.step(
        "install fails, restore, quit",
        &args,
        &answers("\nx\nR\n q \n"),
    );
    lab.step("install fails, input ends", &args, &answers("\n"));
    lab.step(
        "restore, then nothing to restore",
        &args,
        &answers("r\nr\nq\n"),
    );
    lab.both(|base| {
        fs::remove_file(
            base.join("world/library two/steamapps")
                .join(steam::STATE)
                .join("2537590/downloads"),
        )
        .unwrap()
    });
    let stub = |stdin: &str| Options {
        stdin: stdin.to_string(),
        stub_geometry: true,
        ..display()
    };
    // Install, select in Steam, then the installer step asks for a file.
    lab.both(|base| {
        put(
            &base.join("world/home/Downloads/FenixInstaller.exe"),
            "MZ official installer",
        );
        put(&base.join("world/home/Desktop/fenix-older.exe"), "MZ older");
        settle(&base.join("world/home"));
        let newer = rustix::fs::Timespec {
            tv_sec: FIXTURE_TIME + 100,
            tv_nsec: 0,
        };
        let times = rustix::fs::Timestamps {
            last_access: newer,
            last_modification: newer,
        };
        rustix::fs::utimensat(
            rustix::fs::CWD,
            base.join("world/home/Downloads/FenixInstaller.exe"),
            &times,
            rustix::fs::AtFlags::empty(),
        )
        .unwrap();
    });
    lab.step(
        "install, select, installer offered, another file",
        &args,
        &stub(
            "\n\n\n\"file://{B}/world/home/Desktop/fenix-older.exe\"\nq\n"
                .replace("{B}", "")
                .as_str(),
        ),
    );
    lab.step(
        "installer with the offered file",
        &args,
        &answers("\n\nq\n"),
    );
    lab.both(|base| {
        fenix_installed(
            &base.join("world/library two/steamapps/compatdata/2537590/pfx"),
            "steamuser",
        )
    });
    lab.step("open, configure, readouts", &args, &answers("\n\n\n\nq\n"));
    lab.both(|base| {
        put(
            &base
                .join("world/library two/steamapps/compatdata/2537590/pfx")
                .join(core::CONFIG)
                .join("persistancy.xml"),
            "<p/>",
        )
    });
    lab.step("readouts, all done, restore", &args, &answers("\n\nr\nq\n"));
    lab.finish();
}

fn interrupt(child: &std::process::Child) {
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::INT).unwrap();
}

/// Wait until the synthetic Wine of one side has been started with this argument.
fn await_call(lab: &Lab, side: Side, argument: &str) {
    let log = lab.base(side).join("world/calls.log");
    for _ in 0..400 {
        if fs::read_to_string(&log).is_ok_and(|calls| calls.contains(argument)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("{side:?}: {argument} was never run");
}

fn sleeping_wine() -> Options {
    let mut options = display();
    options.env.push((
        "FAKE_WINE_SLEEP".to_string(),
        "Arial (TrueType)".to_string(),
    ));
    options
}

#[test]
fn interrupted_with_control_c() {
    let Some(mut lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        put(
            &base
                .join("world/library two/steamapps")
                .join(steam::STATE)
                .join("2537590/downloads"),
            "in the way",
        );
        settle(&base.join("world"));
        bundle(base, 1);
    });
    // An installation, interrupted while Wine runs: Wine is stopped, the journal stays, the status is 130.
    let mut outcomes = Vec::new();
    for side in SIDES {
        let child = lab
            .command(
                side,
                &with(&["install", "--json"], &STEAM),
                &sleeping_wine(),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        await_call(&lab, side, "[Arial (TrueType)]");
        interrupt(&child);
        let output = child.wait_with_output().unwrap();
        outcomes.push((
            output.status.code(),
            lab.normal(side, &String::from_utf8_lossy(&output.stdout)),
            lab.normal(side, &String::from_utf8_lossy(&output.stderr)),
        ));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[1].0, Some(130));
    assert!(outcomes[1].1.ends_with("{\"message\": \"Setup did not finish. The original profile or its backup is retained. Use Restore.\"}\n"), "{:?}", outcomes[1]);
    // The synthetic Wine that was killed leaves its sleeping helper behind for a moment.
    std::thread::sleep(Duration::from_millis(2200));
    lab.same_trees("interrupted installation");
    lab.both(|base| fs::remove_file(base.join("world/calls.log")).unwrap());
    lab.step(
        "restore after the interruption",
        &with(&["restore"], &STEAM),
        &Options::default(),
    );

    // The guided setup in the terminal, interrupted while it waits for an answer.
    let mut outcomes = Vec::new();
    for side in SIDES {
        let mut child = lab
            .command(side, &with(&["gui", "--text"], &STEAM), &Options::default())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = child.stdout.take().unwrap();
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).ends_with("[q] Quit: ") {
            let mut buffer = [0u8; 4096];
            let count = output.read(&mut buffer).unwrap();
            assert!(
                count > 0,
                "{side:?}: the setup ended early: {}",
                String::from_utf8_lossy(&seen)
            );
            seen.extend_from_slice(&buffer[..count]);
        }
        interrupt(&child);
        output.read_to_end(&mut seen).unwrap();
        let mut errors = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut errors)
            .unwrap();
        outcomes.push((
            child.wait().unwrap().code(),
            lab.normal(side, &String::from_utf8_lossy(&seen)),
            errors,
        ));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!((outcomes[1].0, outcomes[1].2.as_str()), (Some(130), ""));

    // The guided setup in the browser: idle, Ctrl+C simply ends it.
    let args = with(&["gui", "--no-browser"], &STEAM);
    let mut outcomes = Vec::new();
    for side in SIDES {
        let mut setup = Setup::start(&lab, side, &args);
        interrupt(&setup.child);
        let mut rest = String::new();
        setup.output.read_to_string(&mut rest).unwrap();
        outcomes.push((setup.child.wait().unwrap().code(), rest));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(
        outcomes[1],
        (
            Some(0),
            "Keep this running until you are done; Ctrl+C or “Close installer” ends it.\n"
                .to_string()
        )
    );

    // While a step runs, Ctrl+C waits for it; the step itself is not cut short.
    let mut outcomes = Vec::new();
    for side in SIDES {
        let mut child = lab
            .command(side, &args, &sleeping_wine())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut address = None;
        while address.is_none() {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0);
            address = line
                .trim_end()
                .strip_prefix("If no window appeared, open: http://127.0.0.1:")
                .map(str::to_string);
        }
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let address = address.unwrap();
        let (port, secret) = address.split_once('/').unwrap();
        let setup = Setup {
            child,
            output,
            banner: String::new(),
            port: port.parse().unwrap(),
            secret: secret.trim_end_matches('/').to_string(),
        };
        let body = "{\"action\": \"install\"}";
        let answer = setup.exchange(&format!("POST /{{S}}/api/run HTTP/1.1\r\nHost: 127.0.0.1:{{P}}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
        assert!(
            answer.starts_with(b"HTTP/1.0 200 OK"),
            "{}",
            String::from_utf8_lossy(&answer)
        );
        await_call(&lab, side, "[Arial (TrueType)]");
        interrupt(&setup.child);
        let Setup {
            mut child,
            mut output,
            ..
        } = setup;
        let mut rest = String::new();
        output.read_to_string(&mut rest).unwrap();
        outcomes.push((child.wait().unwrap().code(), rest));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(
        outcomes[1],
        (
            Some(0),
            "\nA step is still running; wait for it to finish.\n".to_string()
        )
    );
    lab.same_trees("interrupted browser setup");
    lab.finish();
}

#[test]
fn steps_that_need_downloads() {
    // .NET Framework and Direct2D geometry cannot be installed here: Microsoft's
    // packages and real Wine are missing. What can be compared is every command
    // the two installers give to Wine on the way, with synthetic downloads.
    let Some(mut lab) = Lab::new() else { return };
    let wine = fake(
        "wine",
        r#"case "$1" in
  uninstaller) if [ "$2" = --list ]; then printf '%s\n' '{11111111-2222-3333-4444-555555555555}|Wine Mono Runtime' '{AAAAAAAA-bbbb-CCCC-dddd-EEEEEEEEEEEE}|Wine Mono Windows Support' 'not-a-code|Wine Mono broken' '{99999999-2222-3333-4444-555555555555}|Another product' '{77777777-2222-3333-4444-55555555555}|Wine Mono short'; fi;;
  */NDP48-x86-x64-AllOS-ENU.exe)
    if [ -z "$FAKE_NO_FRAMEWORK" ]; then
      printf '%s\n' '' '[Software\\Microsoft\\NET Framework Setup\\NDP\\v4\\Full] 1700000000' '"Release"=dword:00080ff4' >> "$WINEPREFIX/system.reg"
      printf clr > "$WINEPREFIX/drive_c/windows/Microsoft.NET/Framework64/v4.0.30319/clr.dll"
    fi
    exit 194;;
  */FenixGeometrySetup.exe) target=$(printf '%s' "$5" | sed -e 's/^Z://' | tr '\\' '/'); printf cabinet > "$target";;
esac
if [ -n "$FAKE_WINE_FAIL" ]; then case " $* " in *"$FAKE_WINE_FAIL"*) exit 3;; esac; fi
exit 0"#,
    );
    let build = |lab: &Lab| {
        lab.both(|base| {
            let steps = base.join("world/steps");
            let _ = fs::remove_dir_all(&steps);
            let _ = fs::remove_file(base.join("world/calls.log"));
            put_mode(&steps.join("runner/files/bin/wine"), &wine, 0o755);
            put_mode(
                &steps.join("runner/files/bin/wineserver"),
                &fake_wineserver("steps"),
                0o755,
            );
            put(
                &steps.join("prefix/system.reg"),
                "WINE REGISTRY Version 2\n\n[Software\\\\Wine] 1700000000\n",
            );
            for folder in ["system32", "syswow64"] {
                put(
                    &steps
                        .join("prefix/drive_c/windows")
                        .join(folder)
                        .join("mscoree.dll"),
                    "builtin loader",
                );
            }
            // Placeholders that link into the tool, and one real file.
            for folder in ["Framework", "Framework64"] {
                let directory = steps
                    .join("prefix/drive_c/windows/Microsoft.NET")
                    .join(folder)
                    .join("v4.0.30319");
                put(&directory.join("real.dll"), "real");
                symlink(
                    steps.join("runner/files/bin/wine"),
                    directory.join("mscorlib.dll"),
                )
                .unwrap();
                symlink("missing", directory.join("dangling.dll")).unwrap();
            }
            settle(&base.join("world"));
            bundle(base, 1);
        });
    };
    for (label, first_version, env) in [
        ("installed", "win7", vec![]),
        ("installed, as Flightdeck asks", "winxp", vec![]),
        (
            "not detected afterwards",
            "win7",
            vec![("FAKE_NO_FRAMEWORK", "1")],
        ),
        (
            "first installer fails",
            "win7",
            vec![("FAKE_WINE_FAIL", "dotNetFx40")],
        ),
        (
            "second installer fails",
            "win7",
            vec![("FAKE_WINE_FAIL", "NDP48")],
        ),
        (
            "removal fails",
            "win7",
            vec![("FAKE_WINE_FAIL", "--remove")],
        ),
        (
            "registry cannot be written",
            "win7",
            vec![("FAKE_WINE_FAIL", "OnlyUseLatestCLR")],
        ),
        (
            "server does not stop",
            "win7",
            vec![("FAKE_NO_FRAMEWORK", "1"), ("FAKE_SERVER_KILL", "2")],
        ),
    ] {
        build(&lab);
        let options = env
            .iter()
            .fold(Options::default(), |mut options, (name, value)| {
                options.env.push((name.to_string(), value.to_string()));
                options
            });
        let python = lab.python_eval_with(
            &format!("{PYTHON_OUTCOME}{PYTHON_SETUP_STEPS}"),
            &json!(first_version),
            &options,
        );
        let base = lab.base(Side::Rust);
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["installer_without_geometry", "--exact", "--nocapture"])
            .current_dir(&base)
            .env_clear()
            .envs(lab.environment(Side::Rust, &options));
        let request = json!({"root": base.display().to_string(), "steps": first_version, "out": lab.sandbox.join("steps.json").display().to_string()});
        // Like the Python side, whose standard input is the pipe its argument came through.
        let output = child
            .env(CHILD, json::dumps(&request, None))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
            .wait_with_output()
            .unwrap();
        let rust = json::loads(
            &fs::read_to_string(lab.sandbox.join("steps.json"))
                .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr))),
        )
        .unwrap();
        fs::remove_file(lab.sandbox.join("steps.json")).unwrap();
        // A failed helper program: Python's message names the exception, the port's the same text.
        let (python, rust) = (
            lab.normal(Side::Python, &json::dumps(&python, Some(2))),
            lab.normal(Side::Rust, &json::dumps(&rust, Some(2))),
        );
        if python != rust {
            lab.problems.push(format!(
                "{label}: the steps ended differently{}",
                first_differences(&python, &rust)
            ));
        }
        lab.same_trees(label);
        if label == "installed" {
            assert!(
                python.contains("Geometry dependency checksum mismatch: update.cab")
                    && python.contains(
                        "Installing Microsoft .NET Framework. This can take several minutes"
                    ),
                "{python}"
            );
        }
    }
    lab.finish();
}

#[test]
fn second_control_c_ends_the_browser_setup() {
    let Some(lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        put(
            &base
                .join("world/library two/steamapps")
                .join(steam::STATE)
                .join("2537590/downloads"),
            "in the way",
        );
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let args = with(&["gui", "--no-browser"], &STEAM);
    let mut outcomes = Vec::new();
    for side in SIDES {
        let mut child = lab
            .command(side, &args, &sleeping_wine())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut address = None;
        while address.is_none() {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0);
            address = line
                .trim_end()
                .strip_prefix("If no window appeared, open: http://127.0.0.1:")
                .map(str::to_string);
        }
        let address = address.unwrap();
        let (port, secret) = address.split_once('/').unwrap();
        let setup = Setup {
            child,
            output,
            banner: String::new(),
            port: port.parse().unwrap(),
            secret: secret.trim_end_matches('/').to_string(),
        };
        let body = "{\"action\": \"install\"}";
        setup.exchange(&format!("POST /{{S}}/api/run HTTP/1.1\r\nHost: 127.0.0.1:{{P}}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
        await_call(&lab, side, "[Arial (TrueType)]");
        let Setup {
            mut child,
            mut output,
            ..
        } = setup;
        interrupt(&child);
        // The first one is answered with a notice; the second one ends the wait.
        let mut seen = String::new();
        while !seen.ends_with("wait for it to finish.\n") {
            assert!(output.read_line(&mut seen).unwrap() > 0, "{side:?}: {seen}");
        }
        interrupt(&child);
        output.read_to_string(&mut seen).unwrap();
        let mut errors = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut errors)
            .unwrap();
        outcomes.push((child.wait().unwrap().code(), seen, errors));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[1].0, Some(130));
    assert_eq!(
        outcomes[1].1,
        "Keep this running until you are done; Ctrl+C or “Close installer” ends it.\n\nA step is still running; wait for it to finish.\n"
    );
    // The abandoned step still has its synthetic Wine asleep.
    std::thread::sleep(Duration::from_millis(2500));
}

#[test]
fn locks_exclude_the_other_implementation() {
    // Both installers and Flightdeck take the same locks. One side holds a
    // profile while its synthetic Fenix runs; the other side must be refused.
    let Some(lab) = Lab::new() else { return };
    lab.both(|base| {
        steam_world(base);
        flightdeck_world(base);
        settle(&base.join("world"));
        bundle(base, 1);
    });
    let stub = Options {
        stub_geometry: true,
        ..display()
    };
    for side in SIDES {
        for target in [&STEAM[..], &FLIGHTDECK[..]] {
            let installed = lab.run(side, &with(&["install"], target), &stub);
            assert_eq!(installed.code, 0, "{installed:?}");
        }
        fenix_installed(
            &lab.base(side)
                .join("world/library two/steamapps/compatdata/2537590/pfx"),
            "steamuser",
        );
        fenix_installed(
            &lab.base(side).join("world/runtime/local/msfs-prefix"),
            "flight",
        );
        // The record of the installation must not be mistaken for the call awaited below.
        fs::remove_file(lab.base(side).join("world/calls.log")).unwrap();
    }
    let mut slow = Options::default();
    slow.env
        .push(("FAKE_WINE_SLEEP".to_string(), "ShowSystray".to_string()));
    for (holder, other) in [(Side::Python, Side::Rust), (Side::Rust, Side::Python)] {
        for (target, refusal) in [
            (
                &STEAM[..],
                "Another Fenix patch operation is using this simulator.",
            ),
            (
                &FLIGHTDECK[..],
                "The simulator or another installation is using this runtime.",
            ),
        ] {
            // The other side works on the holder's installation: same paths, its own program.
            let held = lab.base(holder).display().to_string();
            let args: Vec<String> = ["configure"]
                .iter()
                .chain(target)
                .map(|arg| arg.replace("{B}", &held))
                .collect();
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let mut busy = lab
                .command(holder, &with(&["open"], target), &slow)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            await_call(&lab, holder, "[ShowSystray]");
            let refused = lab.run(other, &args, &Options::default());
            assert_eq!(
                (refused.code, refused.stderr.trim_end()),
                (1, format!("Fenix patch: {refusal}").as_str()),
                "{holder:?} holds, {other:?} asks: {refused:?}"
            );
            assert_eq!(busy.wait().unwrap().code(), Some(0));
            fs::remove_file(lab.base(holder).join("world/calls.log")).unwrap();
            let allowed = lab.run(other, &args, &Options::default());
            assert_eq!(
                allowed.code, 0,
                "{holder:?} released, {other:?} asks: {allowed:?}"
            );
        }
    }
}

/// The pinned Proton as an archive on disk instead of a download.
struct ArchivedProton {
    host: Host,
    archive: PathBuf,
}

impl System for ArchivedProton {
    fn root(&self) -> &Path {
        self.host.root()
    }

    fn download(
        &self,
        _url: &str,
        destination: &Path,
        _expected: &str,
        _max_size: u64,
    ) -> Result<PathBuf> {
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(&self.archive, destination).unwrap();
        Ok(destination.to_path_buf())
    }

    fn proton_downloads(&self) -> Vec<steam::ProtonDownload> {
        let pinned = |variant: &str, file: &str, folder: &str| steam::ProtonDownload {
            variant: variant.to_string(),
            url: format!("https://example.invalid/releases/{file}"),
            sha256: "0".to_string(),
            folder: folder.to_string(),
        };
        vec![
            pinned("unknown", "other.tar.xz", "other"),
            pinned("fixture", "proton-fixture.tar.xz", "proton-fixture"),
        ]
    }
}

#[test]
fn pinned_proton_is_downloaded_and_extracted() {
    let Some(mut lab) = Lab::new() else { return };
    let (Some(tar), Some(_)) = (tool("tar"), tool("xz")) else {
        notice("tar or xz was not found; the comparison of the Proton download is skipped");
        return;
    };
    let source = lab.sandbox.join("source");
    runner(
        &source.join("proton-fixture"),
        "1700000002 parity-proton-11.0",
        true,
    );
    settle(&source);
    let archive = lab.sandbox.join("proton-fixture.tar.xz");
    assert!(
        Command::new(&tar)
            .arg("-C")
            .arg(&source)
            .arg("-cJf")
            .arg(&archive)
            .arg("proton-fixture")
            .status()
            .unwrap()
            .success()
    );
    lab.both(|base| {
        steam_world(base);
        settle(&base.join("world"));
        bundle(base, 1);
        // No Proton is installed any more; the manifest still describes the pinned one.
        fs::remove_dir_all(base.join("world/steam/compatibilitytools.d/pinned-proton")).unwrap();
    });
    let script = format!(
        "{PYTHON_OUTCOME}from unittest.mock import patch\nimport shutil\nitem = steam.target('msfs2024', 'world/steam')\ndef fetch(url, destination, expected, progress=None, max_size=None):\n    destination.parent.mkdir(parents=True, exist_ok=True)\n    shutil.copyfile(argument, destination)\n    return destination\nmessages = []\npinned = {{'unknown': ('https://example.invalid/releases/other.tar.xz', '0', 'other'), 'fixture': ('https://example.invalid/releases/proton-fixture.tar.xz', '0', 'proton-fixture')}}\nwith patch.object(steam, 'PROTON_DOWNLOADS', pinned), patch.object(core, 'download', side_effect=fetch):\n    result = [outcome(lambda: [str(value) for value in steam.source_proton(item, None, messages.append, Path('world/downloaded').resolve())]), messages]"
    );
    let python = lab.python_eval(&script, &json!(archive));
    let base = lab.base(Side::Rust);
    let system = ArchivedProton {
        host: Host::new(base.clone()),
        archive,
    };
    let item = steam::target(
        &system,
        "msfs2024",
        Some(base.join("world/steam").as_os_str()),
    )
    .unwrap();
    let messages = std::sync::Mutex::new(Vec::new());
    let progress = |message: &str| {
        messages.lock().unwrap().push(message.to_string());
        Ok(())
    };
    let found = steam::source_proton(
        &system,
        &item,
        None,
        &progress,
        &base.join("world/downloaded"),
    )
    .map(|(path, variant)| json!([path.display().to_string(), variant]));
    let rust = json!([outcome(found), messages.lock().unwrap().clone()]);
    assert_eq!(
        lab.normal(Side::Rust, &json::dumps(&rust, Some(2))),
        lab.normal(Side::Python, &json::dumps(&python, Some(2)))
    );
    assert_eq!(python[0][0], "ok", "{python}");
    assert_eq!(
        python[1],
        json!([
            "Downloading the pinned Proton build proton-fixture (about 350 MB) …",
            "Extracting Proton …"
        ])
    );
    lab.same_trees("downloaded Proton");
    lab.finish();
}

#[test]
fn paths_and_containment() {
    let Some(lab) = Lab::new() else { return };
    // A small maze of links: inside, outside, dangling, looping, relative and absolute.
    lab.both(|base| {
        let maze = base.join("world/maze");
        fs::create_dir_all(maze.join("root/inside/deep")).unwrap();
        fs::create_dir_all(maze.join("outside/secret")).unwrap();
        put(&maze.join("root/inside/file.txt"), "file");
        put(&maze.join("outside/file.txt"), "outside");
        symlink(maze.join("outside"), maze.join("root/escape")).unwrap();
        symlink("../outside", maze.join("root/relative escape")).unwrap();
        symlink("inside/deep", maze.join("root/shortcut")).unwrap();
        symlink("..", maze.join("root/inside/up")).unwrap();
        symlink("../..", maze.join("root/inside/deep/far up")).unwrap();
        symlink(
            maze.join("outside/file.txt"),
            maze.join("root/inside/file link"),
        )
        .unwrap();
        symlink("nowhere/at all", maze.join("root/dangling")).unwrap();
        symlink("loop b", maze.join("root/loop a")).unwrap();
        symlink("loop a", maze.join("root/loop b")).unwrap();
        symlink("self", maze.join("root/self")).unwrap();
        symlink(maze.join("root"), maze.join("root link")).unwrap();
        symlink("/", maze.join("root/slash")).unwrap();
    });
    let relatives = [
        "inside/file.txt",
        "inside/new/file.txt",
        "inside/deep/../file.txt",
        "inside/./file.txt",
        "inside//file.txt",
        "inside/",
        "inside",
        "",
        ".",
        "..",
        "../outside",
        "/etc/passwd",
        "escape/file.txt",
        "escape",
        "relative escape/secret/x",
        "shortcut/x",
        "shortcut",
        "inside/up/x",
        "inside/up/inside/file.txt",
        "inside/deep/far up/x",
        "inside/deep/far up/root/x",
        "inside/file link",
        "inside/file link/x",
        "dangling",
        "dangling/x",
        "dangling/x/y",
        "loop a",
        "loop a/x",
        "self/x",
        "slash/etc",
        "slash/x",
        "missing/a/b/c",
        "a/../../b",
        "inside/deep/x/../../../y",
    ];
    let roots = [
        "world/maze/root",
        "world/maze/root link",
        "world/maze/root/shortcut",
        "world/maze/root/dangling",
        "world/maze/root/loop a",
        "world/maze/missing root",
        "world/maze/root/inside/up",
    ];
    let script = format!(
        "{PYTHON_OUTCOME}import os\nresult = [[outcome(lambda: str(core.contained(Path(root).resolve() if absolute else root, relative))) for relative in argument[1]] for root in argument[0] for absolute in (True, False)]\nresult = [result, [[outcome(lambda: str(Path(root, relative).resolve())), outcome(lambda: str(Path(root, relative).resolve(strict=True)))] for root in argument[0] for relative in argument[1] if not relative.startswith('/')]]"
    );
    let python = lab.python_eval(&script, &json!([roots, relatives.to_vec()]));
    let base = lab.base(Side::Rust);
    let shown = |result: Result<PathBuf>| match result {
        Ok(path) => json!(["ok", path.display().to_string()]),
        Err(fenix_patch::Error::Patch(message)) => json!(["error", "PatchError", message]),
        Err(other) => json!(["error", "other", other.to_string()]),
    };
    let mut differences = Vec::new();
    let mut compare = |what: String, rust: Value, python: &Value| {
        let (rust, python) = (
            lab.normal(Side::Rust, &rust.to_string()),
            lab.normal(Side::Python, &python.to_string()),
        );
        if rust != python {
            differences.push(format!("{what}\n  Python: {python}\n  Rust:   {rust}"));
        }
    };
    let mut row = 0;
    for root in roots {
        for absolute in [true, false] {
            for (column, relative) in relatives.iter().enumerate() {
                // Python ran in its installer directory; a relative root means the same place here.
                let given = if absolute {
                    fsx::resolve(&base.join(root)).unwrap()
                } else {
                    base.join(root)
                };
                let rust = shown(fsx::contained(&given, relative));
                let expected = &python[0][row][column];
                // A relative root is resolved by `contained` itself, so both answers are absolute.
                compare(
                    format!("contained({root:?}, {relative:?}), absolute root: {absolute}"),
                    rust,
                    expected,
                );
            }
            row += 1;
        }
    }
    let mut index = 0;
    for root in roots {
        for relative in relatives
            .iter()
            .filter(|relative| !relative.starts_with('/'))
        {
            let path = base.join(root).join(relative);
            // `Path(root, "")` is the root itself, without a trailing separator.
            let path = if relative.is_empty() {
                base.join(root)
            } else {
                path
            };
            compare(
                format!("resolve({root:?} / {relative:?})"),
                shown(fsx::resolve(&path)),
                &python[1][index][0],
            );
            compare(
                format!("resolve strictly({root:?} / {relative:?})"),
                shown(fsx::resolve_strict(&path)),
                &python[1][index][1],
            );
            index += 1;
        }
    }
    assert!(
        differences.is_empty(),
        "{} differences:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

#[test]
fn text_helpers() {
    let Some(lab) = Lab::new() else { return };
    let texts = [
        "",
        "plain",
        "it's",
        "say \"hi\"",
        "both ' and \"",
        "back\\slash",
        "tab\tnewline\ncarriage\rreturn",
        "\u{0}\u{1}\u{1f}\u{7f}",
        "\u{80}\u{9f}\u{a0}\u{ad}",
        "\u{e4}\u{f6}\u{fc}\u{df} \u{20ac} \u{2026} \u{201c}\u{201d} \u{2192} \u{b7}",
        "\u{200b}\u{200e}\u{2028}\u{2029}\u{feff}",
        "\u{3000}\u{1680}\u{2003}",
        "\u{1f600}\u{10ffff}\u{e000}",
        "/path/with spaces/and 'quotes'.exe",
        " \t\u{1c}\u{1d}\u{1e}\u{1f}\u{85}\u{a0} padded \u{2000}\u{3000}\n",
        "a\u{b}b\u{c}c\u{1c}d\u{1d}e\u{1e}f\u{85}g\u{2028}h\u{2029}i\r\nj\rk\n",
        "  two  words ",
        "12",
        " 12 ",
        "+7",
        "-0",
        "1_000",
        "_1",
        "1__0",
        "1_",
        "0x10",
        "1.5",
        "١٢",
        "twelve",
        "99999999999999999999999",
    ];
    let python = lab.python_eval(
        "def number(text):\n    try:\n        return int(text)\n    except ValueError as error:\n        return str(error)\nresult = [[repr(text), text.strip(), text.split(), text.splitlines(), number(text)] for text in argument]",
        &json!(texts),
    );
    for (text, expected) in texts.iter().zip(python.as_array().unwrap()) {
        assert_eq!(
            json!(fenix_patch::error::repr(text)),
            expected[0],
            "repr of {text:?}"
        );
        assert_eq!(
            json!(fenix_patch::py::strip(text)),
            expected[1],
            "strip of {text:?}"
        );
        assert_eq!(
            json!(fenix_patch::py::split(text)),
            expected[2],
            "split of {text:?}"
        );
        assert_eq!(
            json!(fenix_patch::py::splitlines(text)),
            expected[3],
            "splitlines of {text:?}"
        );
        let number = match fenix_patch::py::int(text) {
            // Beyond 64 bits the port only keeps "very large"; nothing it converts is that long.
            Ok(i64::MAX) => expected[4].clone(),
            Ok(number) => json!(number),
            Err(error) => json!(error.to_string()),
        };
        // Python also reads the digits of other scripts; the port reads ASCII digits.
        if *text != "١٢" {
            assert_eq!(number, expected[4], "int of {text:?}");
        }
    }
    let paths = [
        "",
        ".",
        "./",
        "a",
        "a/",
        "a//b",
        "a/./b",
        "./a",
        "a/..",
        "../a",
        "/",
        "/a/../b/",
        "a/b/.",
        "~",
        "~/x",
        "~/",
        "~root/x",
        "~nobody-here/x",
        "a/~/b",
        ".exe",
        "a.exe",
        "a.EXE",
        "a.",
        "a.tar.xz",
        "..exe",
        "...",
        "a..b",
    ];
    let python = lab.python_eval(
        "import os\nresult = [[str(Path(text)), Path(text).name, str(Path(text).parent), Path(text).suffix, str(Path(text).expanduser()) if text != '~nobody-here/x' else text] for text in argument]",
        &json!(paths),
    );
    let home = lab
        .environment(Side::Python, &Options::default())
        .into_iter()
        .find(|(name, _)| name == "HOME")
        .map(|(_, value)| OsString::from(value));
    for (text, expected) in paths.iter().zip(python.as_array().unwrap()) {
        let path = fsx::path(text);
        assert_eq!(
            json!(path.display().to_string()),
            expected[0],
            "Path({text:?})"
        );
        assert_eq!(json!(fsx::name(&path)), expected[1], "name of {text:?}");
        assert_eq!(
            json!(fsx::parent(&path).display().to_string()),
            expected[2],
            "parent of {text:?}"
        );
        assert_eq!(
            json!(fsx::suffix(&fsx::name(&path))),
            expected[3],
            "suffix of {text:?}"
        );
        assert_eq!(
            json!(fsx::expanduser(text, home.clone()).display().to_string()),
            expected[4],
            "expanduser of {text:?}"
        );
    }
}

#[test]
fn registry_and_library_reading() {
    let Some(lab) = Lab::new() else { return };
    let section = "[Software\\\\Microsoft\\\\NET Framework Setup\\\\NDP\\\\v4\\\\Full]";
    let registries = [
        format!("{section} 1700000000\n\"Release\"=dword:00080ff4\n"),
        format!("{section} 1700000000\n\"Release\"=dword:00080EA8\n"),
        format!("{section} 1700000000\n\"Release\"=dword:00080ea7\n"),
        format!("{section}\r\n\"Install\"=dword:00000001\r\n\"Release\"=dword:00080ff4\r\n"),
        format!("{section}\n\"Release\"=dword:ffffffffffffffffffffffffffffffffffffffff\n"),
        format!("{section}\n\"Release\"=\"528372\"\n"),
        format!("{section}\n\"Version\"=\"4.8\"\n\n[Other]\n\"Release\"=dword:00080ff4\n"),
        format!("[Other]\n\"Release\"=dword:00080ff4\n\n{section}\n\"Release\"=dword:00000001\n\n{section}\n\"Release\"=dword:00080ff4\n"),
        format!("{}\n\"Release\"=dword:00080ff4\n", section.to_lowercase()),
        format!("prefix {section} suffix \"Release\"=dword:00080ff4"),
        "[Software\\\\Microsoft\\\\NET Framework Setup\\\\NDP\\\\v4\\\\Client]\n\"Release\"=dword:00080ff4\n".to_string(),
        String::new(),
    ];
    let fonts = "[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts]";
    let font_registries = [
        format!(
            "{fonts} 1\n\"Tahoma (TrueType)\"=\"tahoma.ttf\"\n\"Tahoma Bold (TrueType)\"=\"tahomabd.ttf\"\n"
        ),
        format!(
            "{fonts} 1\r\n\"Tahoma (TrueType)\"=\"tahoma.ttf\"\r\n\"Tahoma Bold (TrueType)\"=\"\"\r\n"
        ),
        format!(
            "{fonts} 1\n \"Tahoma (TrueType)\"=\"indented.ttf\"\n\"Tahoma Bold (TrueType)\"=\"unterminated\n"
        ),
        format!(" {fonts} 1\n\"Tahoma (TrueType)\"=\"section is indented\"\n"),
        format!(
            "{fonts} 1\n\"Arial (TrueType)\"=\"arial.ttf\"\n\n[Next]\n\"Tahoma (TrueType)\"=\"other section\"\n"
        ),
        format!(
            "{fonts} 1\n\"Tahoma (TrueType)\"=\"a\"x\n\"Tahoma Bold (TrueType)\"=\"b\rc\"\n\"tahoma (truetype)\"=\"lower\"\n"
        ),
        "[Software\\\\Wow6432Node\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts]\n\"Tahoma (TrueType)\"=\"x\"\n"
            .to_string(),
        "no fonts at all".to_string(),
    ];
    lab.both(|base| {
        for (number, registry) in registries.iter().enumerate() {
            let prefix = base.join(format!("world/registry/{number:02}"));
            put(&prefix.join("system.reg"), registry);
            if number != 1 {
                put(&prefix.join("drive_c/windows/Microsoft.NET/Framework64/v4.0.30319/clr.dll"), "clr");
            }
        }
        fs::create_dir_all(base.join("world/registry/none")).unwrap();
        fs::create_dir_all(base.join("world/registry/directory/system.reg")).unwrap();
        fs::create_dir_all(base.join("world/registry/link")).unwrap();
        symlink("../00/system.reg", base.join("world/registry/link/system.reg")).unwrap();
        for (number, registry) in font_registries.iter().enumerate() {
            let prefix = base.join(format!("world/fonts/{number:02}"));
            put(&prefix.join("system.reg"), registry);
            put(&prefix.join("drive_c/windows/Fonts/tahoma.ttf"), "the user's own");
        }
        put(&base.join("world/font runner/files/share/wine/fonts/tahoma.ttf"), "runner tahoma");
        put(&base.join("world/font runner/files/share/wine/fonts/tahomabd.ttf"), "runner tahoma bold");
        // Steam libraries, as its own file lists them.
        for library in ["lib one", "lib \"quoted\"", "lib back\\slash"] {
            fs::create_dir_all(base.join("world/libraries").join(library).join("steamapps")).unwrap();
        }
        fs::create_dir_all(base.join("world/libraries/no steamapps")).unwrap();
        let at = |name: &str| base.join("world/libraries").join(name).display().to_string().replace('\\', "\\\\").replace('"', "\\\"");
        put(
            &base.join("world/libraries/root/steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\" {{ \"PATH\" \"{}\" }}\n\t\"2\" {{ \"path\"\n\"{}\" }}\n\t\"3\" {{ \"path\" \"{}\" \"path\"\u{a0}\u{1c}\"{}\" }}\n\t\"4\" {{ \"path\" \"world/libraries/not found from here\" \"path\" \"{}\" \"path\"\"{}\" }}\n\t\"5\" {{ \"path\" \"{}/\" \"path\" \"\" }}\n}}\n",
                at("lib one"),
                at("lib one"),
                at("lib \"quoted\""),
                at("lib back\\slash"),
                at("lib one"),
                at("no steamapps"),
                at("missing"),
                at("root"),
            ),
        );
        settle(&base.join("world"));
    });
    let script = format!(
        "{PYTHON_OUTCOME}from unittest.mock import Mock\nframework = [outcome(lambda: core.has_framework(Path('world/registry').resolve() / name)) for name in argument[0]]\nfonts = []\nfor number in range(argument[1]):\n    wine = Mock()\n    prefix = Path('world/fonts/%02d' % number).resolve()\n    done = outcome(lambda: core.ensure_ui_fonts(prefix, Path('world/font runner').resolve(), wine))\n    fonts.append([done, [list(call.args) for call in wine.run.call_args_list]])\nresult = [framework, fonts, outcome(lambda: [str(path) for path in steam.libraries(Path('world/libraries/root').resolve())]), outcome(lambda: [str(path) for path in steam.libraries(Path('world/libraries/lib one').resolve())])]"
    );
    let names: Vec<String> = (0..registries.len())
        .map(|number| format!("{number:02}"))
        .chain(["none", "directory", "link"].map(str::to_string))
        .collect();
    let python = lab.python_eval(&script, &json!([names, font_registries.len()]));
    let base = lab.base(Side::Rust);
    let mut rust_framework = Vec::new();
    for name in &names {
        rust_framework.push(outcome(core::has_framework(
            &base.join("world/registry").join(name),
        )));
    }
    assert_eq!(
        lab.normal(Side::Rust, &json::dumps(&json!(rust_framework), Some(1))),
        lab.normal(Side::Python, &json::dumps(&python[0], Some(1)))
    );
    let mut rust_fonts = Vec::new();
    for number in 0..font_registries.len() {
        let recorder = Recorder::default();
        let wine = RecordingWine {
            prefix: base.join(format!("world/fonts/{number:02}")),
            runner: base.join("world/font runner"),
            env: Default::default(),
            calls: &recorder,
        };
        let done = outcome(
            core::ensure_ui_fonts(wine.prefix(), wine.runner(), &wine).map(|()| Value::Null),
        );
        rust_fonts.push(json!([done, recorder.0.lock().unwrap().clone()]));
    }
    assert_eq!(
        lab.normal(Side::Rust, &json::dumps(&json!(rust_fonts), Some(1))),
        lab.normal(Side::Python, &json::dumps(&python[1], Some(1)))
    );
    let libraries = |root: &str| {
        outcome(steam::libraries(&base.join(root)).map(|found| {
            json!(
                found
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
            )
        }))
    };
    assert_eq!(
        lab.normal(Side::Rust, &libraries("world/libraries/root").to_string()),
        lab.normal(Side::Python, &python[2].to_string())
    );
    assert_eq!(
        lab.normal(
            Side::Rust,
            &libraries("world/libraries/lib one").to_string()
        ),
        lab.normal(Side::Python, &python[3].to_string())
    );
    assert!(python[2][1].as_array().unwrap().len() >= 4, "{}", python[2]);
    let mut lab = lab;
    lab.same_trees("fonts copied for the registry");
    lab.finish();
}

/// Records the commands given to Wine, like `unittest.mock.Mock()` in the Python tests.
#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<Vec<String>>>);

struct RecordingWine<'a> {
    prefix: PathBuf,
    runner: PathBuf,
    env: fenix_patch::process::Env,
    calls: &'a Recorder,
}

impl Wine for RecordingWine<'_> {
    fn prefix(&self) -> &Path {
        &self.prefix
    }

    fn runner(&self) -> &Path {
        &self.runner
    }

    fn env(&self) -> &fenix_patch::process::Env {
        &self.env
    }

    fn run(&self, args: &[OsString], _options: &core::RunOptions<'_>) -> Result<()> {
        self.calls.0.lock().unwrap().push(
            args.iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        );
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        Ok(())
    }
}
