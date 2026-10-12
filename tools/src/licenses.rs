// SPDX-License-Identifier: MIT
//! The licence texts of the Rust crates that are linked into `fenix-patch`.
//!
//! The installer is one static program, so a release has to carry the notices
//! of everything inside it. The set comes from Cargo itself and the texts from
//! the very sources Cargo compiled.
use crate::Context;
use fenix_patch::{
    Error, Result, argv, fsx,
    json::{self, Dict, Value, json},
    process::{self, Cmd, Output},
    py,
};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

/// The platform the released installer is built for.
pub const TARGET: &str = "x86_64-unknown-linux-musl";
/// The package whose dependencies are shipped.
pub const PACKAGE: &str = "fenix-patch";
/// Where the texts go in both archives.
pub const FOLDER: &str = "licenses/rust";

const PREFIXES: [&str; 5] = ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE"];

// A crate that ships no licence file is accepted only under one of these.
// Each waives every condition, including the one to keep a copyright notice
// or the licence text with copies: 0BSD and MIT-0 grant permission without
// conditions, CC0-1.0 and the Unlicense dedicate the work to the public
// domain. MIT, BSD, ISC, Apache-2.0 and the like all require their notice in
// copies and are deliberately absent: such a crate needs its text added.
const NO_TEXT_REQUIRED: [&str; 4] = ["0BSD", "CC0-1.0", "MIT-0", "Unlicense"];

static COMPONENT: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r"\A[A-Za-z0-9_][A-Za-z0-9_.+-]*\z"));

fn fail(message: impl Into<String>) -> Error {
    Error::Value(message.into())
}

/// One licence file of a crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    /// Its place below [`FOLDER`]: `<name>-<version>/<file>`.
    pub name: String,
    pub source: PathBuf,
}

/// The texts in the order of their names, and `INVENTORY.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Notices {
    pub texts: Vec<Text>,
    pub inventory: Value,
}

fn is_licence(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
}

/// The regular files directly in `folder` that are named like a licence.
fn licence_files(folder: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for path in fsx::sorted_dir(folder)? {
        let name = fsx::name(&path);
        if is_licence(&name) && fsx::lstat(&path)?.file_type().is_file() {
            found.push(name);
        }
    }
    Ok(found)
}

/// `cargo metadata` for the released platform, as Cargo.lock pins it.
pub fn metadata(ctx: &Context<'_>) -> Result<Value> {
    let env = ctx.system.environ();
    let mut cmd = Cmd::new(argv![
        &ctx.programs.cargo,
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--filter-platform",
        TARGET
    ]);
    cmd.env = Some(&env);
    cmd.cwd = Some(ctx.root());
    cmd.output = Output::Capture;
    let finished = ctx.run(&cmd)?;
    if !finished.stderr.is_empty() {
        ctx.warn(String::from_utf8_lossy(&finished.stderr).trim_end_matches('\n'));
    }
    if finished.code != 0 {
        return Err(process::failure(&cmd.argv, finished.code));
    }
    json::loads(&py::decode(finished.stdout)?)
}

/// The packages that end up in the program: the normal dependencies of
/// [`PACKAGE`] and theirs, without what only builds or tests them.
fn linked<'a>(graph: &'a Value) -> Result<Vec<&'a Value>> {
    let list = |value: &'a Value, what: &str| {
        value
            .as_array()
            .ok_or_else(|| fail(format!("cargo metadata lists no {what}")))
    };
    let packages = list(graph.field("packages"), "packages")?;
    let members: BTreeSet<&str> = list(graph.field("workspace_members"), "workspace members")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut roots = packages.iter().filter(|package| {
        package.is("name", PACKAGE)
            && package.field("source").is_null()
            && package.str("id").is_some_and(|id| members.contains(id))
    });
    let (Some(root), None) = (roots.next(), roots.next()) else {
        return Err(fail(format!(
            "cargo metadata does not describe the package {PACKAGE}"
        )));
    };
    let root = root.need_str("id")?;
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in list(graph.field("resolve").field("nodes"), "dependency graph")? {
        let mut normal = Vec::new();
        for dependency in list(node.field("deps"), "dependencies")? {
            // A dependency can be normal and a build dependency at once.
            let kinds = list(dependency.field("dep_kinds"), "dependency kinds")?;
            if kinds.iter().any(|kind| kind.field("kind").is_null()) {
                normal.push(dependency.need_str("pkg")?);
            }
        }
        edges.insert(node.need_str("id")?, normal);
    }
    let mut reached = BTreeSet::from([root]);
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        let dependencies = edges
            .get(id)
            .ok_or_else(|| fail("cargo metadata names a package outside its dependency graph"))?;
        for dependency in dependencies {
            if reached.insert(dependency) {
                pending.push(dependency);
            }
        }
    }
    reached.remove(root);
    let mut found = Vec::new();
    for id in &reached {
        let mut matching = packages.iter().filter(|package| package.is("id", id));
        let (Some(package), None) = (matching.next(), matching.next()) else {
            return Err(fail(
                "cargo metadata names a package outside its dependency graph",
            ));
        };
        found.push(package);
    }
    Ok(found)
}

/// Collect the licence files of every crate linked into the installer.
pub fn collect(graph: &Value) -> Result<Notices> {
    let mut crates = BTreeMap::new();
    for package in linked(graph)? {
        let (name, version) = (package.need_str("name")?, package.need_str("version")?);
        if !COMPONENT.is_match(name) || !COMPONENT.is_match(version) {
            return Err(fail(format!(
                "Unexpected name of a Rust dependency: {name} {version}"
            )));
        }
        let manifest = PathBuf::from(package.need_str("manifest_path")?);
        let folder = fsx::parent(&manifest).to_path_buf();
        let mut files = licence_files(&folder)?;
        // A text the crate itself declares counts whatever it is called.
        if let Some(declared) = package.str("license_file") {
            let path = folder.join(declared);
            let own = fsx::parent(&path) == folder && fsx::lstat(&path)?.file_type().is_file();
            if !own {
                return Err(fail(format!(
                    "Unexpected licence file of a Rust dependency: {name} {version}"
                )));
            }
            files.push(fsx::name(&path));
        }
        files.sort();
        files.dedup();
        let license = package.field("license");
        if files.is_empty()
            && !license
                .as_str()
                .is_some_and(|license| NO_TEXT_REQUIRED.contains(&license))
        {
            return Err(fail(format!(
                "Missing licence text of a Rust dependency: {name} {version} ({})",
                license.as_str().unwrap_or("no licence declared")
            )));
        }
        let identity = (name.to_string(), version.to_string());
        if crates
            .insert(identity, (license.clone(), folder, files))
            .is_some()
        {
            return Err(fail(format!(
                "cargo metadata describes a Rust dependency twice: {name} {version}"
            )));
        }
    }
    let (mut texts, mut inventory) = (Vec::new(), Vec::new());
    for ((name, version), (license, folder, files)) in crates {
        for file in &files {
            texts.push(Text {
                name: format!("{name}-{version}/{file}"),
                source: folder.join(file),
            });
        }
        inventory
            .push(json!({"name": name, "version": version, "license": license, "files": files}));
    }
    Ok(Notices {
        texts,
        inventory: Value::Array(inventory),
    })
}
