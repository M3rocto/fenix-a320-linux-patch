// SPDX-License-Identifier: MIT
//! `fenix-tools build-proton`: rebuild a Fenix overlay against one exact,
//! pinned Proton Wine revision.
use crate::{
    Context,
    archive::extract,
    build::{TARGETS, apply_patch, compile, fetch, install_module, read_lock, value, write_lock},
};
use fenix_patch::{
    Error, Result, argv, fsx,
    json::{self, Dict, Map, Value},
    py,
};
use regex::Regex;
use std::{ffi::OsString, path::PathBuf, sync::LazyLock};

static PROTOCOL: LazyLock<Regex> =
    LazyLock::new(|| py::pattern(r"#define SERVER_PROTOCOL_VERSION (\d+)"));

#[derive(Debug, Clone)]
pub struct Options {
    pub variant: String,
    pub build_dir: PathBuf,
    pub prepare_only: bool,
    pub record_build: bool,
    pub jobs: i64,
}

pub fn run(ctx: &Context<'_>, options: &Options) -> Result<()> {
    let root = ctx.root();
    let mut lock = read_lock(root)?;
    let entry = Value::Object(lock.clone())
        .need("variants")?
        .need(&options.variant)?
        .clone();
    let build = fsx::resolve(&options.build_dir)?;
    if fsx::exists(&build) {
        return Err(value("Use a fresh build directory"));
    }
    let source = build.join("wine");
    let sources = entry.need_table("sources")?;
    let (name, expected) = sources
        .iter()
        .next()
        .ok_or_else(|| Error::other("bundle.json: the variant names no source archive"))?;
    let archive = root.join("sources").join(name);
    if !fsx::exists(&archive) {
        fsx::mkdir_parents(fsx::parent(&archive), 0o777)?;
        let url = format!(
            "https://codeload.github.com/{}/tar.gz/{}",
            entry.need_str("wine_repository")?,
            entry.need_str("wine_revision")?
        );
        fetch(ctx, &url, &archive)?;
    }
    if !json::is_text(expected, &fsx::digest(&archive)?) {
        return Err(value("Wine source checksum mismatch"));
    }
    extract(&archive, &source)?;
    let mut env = ctx.system.environ();
    env.set("XDG_CACHE_HOME", build.join("cache"));
    for (name, expected) in &entry.need_table("patches")? {
        let patch = root.join("patches").join(name);
        if !json::is_text(expected, &fsx::digest(&patch)?) {
            return Err(value("Proton port checksum mismatch"));
        }
        apply_patch(ctx, &patch, &source, &env)?;
    }
    // Proton regenerates the archived protocol headers during its build. Never
    // copy Xodus's generated requests or change the selected runner's protocol.
    ctx.call(argv!["./tools/make_requests"].to_vec(), &env, Some(&source))?;
    let header = fsx::read_text(&source.join("include/wine/server_protocol.h"))?;
    let expected = match PROTOCOL.captures(&header) {
        Some(found) => json::equal(&Value::from(py::int(&found[1])?), entry.need("protocol")?),
        None => false,
    };
    if !expected {
        return Err(value("Unexpected Wine server protocol"));
    }
    if options.prepare_only {
        return Ok(());
    }
    let output = build.join("objects");
    let mut flags = OsString::from("-O2 -ffile-prefix-map=");
    flags.push(&build);
    flags.push("=/usr/src/fenix-proton");
    compile(ctx, &source, &output, &flags, options.jobs, &env)?;
    let mut hashes = Map::new();
    for (target, relative) in TARGETS {
        let destination = root
            .join("payload/variants")
            .join(&options.variant)
            .join(relative);
        install_module(ctx, &output.join(target), &destination, &source, &env)?;
        hashes.insert(
            relative.to_string(),
            Value::String(fsx::digest(&destination)?),
        );
    }
    if options.record_build {
        let entry = lock
            .get_mut("variants")
            .and_then(|variants| variants.get_mut(&options.variant))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| Error::other("bundle.json: the variant is not a JSON object"))?;
        entry.insert("files".to_string(), Value::Object(hashes));
        write_lock(root, lock)
    } else if !json::equal(&Value::Object(hashes), entry.need("files")?) {
        Err(value(
            "Build hashes differ; review before recording a new build",
        ))
    } else {
        Ok(())
    }
}
