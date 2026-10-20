// SPDX-License-Identifier: MIT
//! `release` on a synthetic repository: what the two archives contain and
//! how, and everything that stops a release.
#![allow(clippy::unwrap_used)]
mod common;

use common::{
    Lab, Outcome, Run, Tree, VARIANT, World, calls, cargo_metadata, digest, notice, put, read,
    script, tiny_program,
};
use fenix_patch::{VERSION, json::Value};
use fenix_tools::{Context, licenses, release, repository};
use flate2::read::GzDecoder;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

struct Fixture {
    lab: Lab,
    tree: Tree,
    world: World,
    installer: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let lab = Lab::new();
        let tree = Tree::create(&lab.base.join("repo"));
        cargo_metadata(&lab.base, &tree.root);
        let world = World::new(&tree.root, lab.env(&lab.base));
        let installer = lab.base.join("built/fenix-patch");
        script(&installer, "");
        fs::write(
            &installer,
            tiny_program(&format!("fenix-patch {VERSION}\n")),
        )
        .unwrap();
        Self {
            lab,
            tree,
            world,
            installer,
        }
    }

    fn tools(&self, argv: &[&str]) -> Outcome {
        Run {
            world: &self.world,
            programs: self.lab.programs(),
            fetch: None,
            display: None,
            output: self.lab.base.join("output.txt"),
        }
        .tools(argv)
    }

    fn release(&self, output: &str) -> Outcome {
        self.tools(&[
            "release",
            "--installer",
            self.installer.to_str().unwrap(),
            "--output",
            self.lab.base.join(output).to_str().unwrap(),
        ])
    }

    /// A release that must be refused with exactly this message, before
    /// anything is written.
    fn refused(&self, message: &str) {
        let outcome = self.release("refused");
        assert_eq!(
            (outcome.code, outcome.output.as_str()),
            (1, format!("fenix-tools: {message}\n").as_str())
        );
        assert!(!self.lab.base.join("refused").exists(), "{message}");
    }
}

fn stem() -> String {
    format!("fenix-a320-linux-patch-{VERSION}")
}

/// The source files of the synthetic repository, as they are named in a release.
const SOURCE: [&str; 43] = [
    ".github/workflows/ci.yml",
    ".gitignore",
    "BUILDING.md",
    "Cargo.lock",
    "Cargo.toml",
    "Fenix A320 Linux Patch.desktop",
    "LICENSE",
    "README.md",
    "THIRD_PARTY.md",
    "bundle.json",
    "docs/deep/er/notes with spaces.txt",
    "docs/steam.md",
    "install.sh",
    "installer/Cargo.toml",
    "installer/src/main.rs",
    "integration/fenix-display-refresh.py",
    "integration/launch-msfs.sh",
    "integration/xodus-wine-launch",
    "licenses/LGPL-2.1.txt",
    "native/geometry-setup.c",
    "native/mcdu-refresh.c",
    "native/window-guard.c",
    "patches/first.patch",
    "patches/proton/x.patch",
    "patches/second.patch",
    "sources/wine-source.tar.gz",
    "sources/x-wine.tar.gz",
    "sources/xgameruntime-source.tar.gz",
    "tests/audio-notification-probe.c",
    "tests/iocp-smoke.c",
    "tests/loader-case-probe.c",
    "tests/path-metrics-probe.cpp",
    "tests/shared-cursor-probe.c",
    "tests/stroke-joins-probe.cpp",
    "tests/test_display_refresh.py",
    "tests/window-guard-probe.c",
    "tests/writecopy-probe.c",
    "tools/src/lib.rs",
    "tools/tests/common/mod.rs",
    // Written by the release itself.
    "licenses/rust/INVENTORY.json",
    "licenses/rust/alpha-1.0.0/LICENSE-APACHE",
    "licenses/rust/alpha-1.0.0/LICENSE-MIT",
    "licenses/rust/beta-0.2.0/COPYING",
];

const NOTICES: [&str; 3] = [
    "licenses/rust/beta-0.2.0/NOTICE.txt",
    "licenses/rust/beta-0.2.0/license.md",
    "licenses/rust/gamma-3.0.0-rc.1+build/COPYRIGHT.txt",
];

const INVENTORY: &str = r#"[
  {
    "name": "alpha",
    "version": "1.0.0",
    "license": "MIT OR Apache-2.0",
    "files": [
      "LICENSE-APACHE",
      "LICENSE-MIT"
    ]
  },
  {
    "name": "beta",
    "version": "0.2.0",
    "license": "Apache-2.0",
    "files": [
      "COPYING",
      "NOTICE.txt",
      "license.md"
    ]
  },
  {
    "name": "delta",
    "version": "0.1.0",
    "license": "Unlicense",
    "files": []
  },
  {
    "name": "gamma",
    "version": "3.0.0-rc.1+build",
    "license": null,
    "files": [
      "COPYRIGHT.txt"
    ]
  }
]
"#;

/// Names in the order an archive must have them: by folder, then by name.
fn ordered(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut paths: Vec<PathBuf> = names.into_iter().map(PathBuf::from).collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| path.to_str().unwrap().to_string())
        .collect()
}

fn source_names() -> Vec<String> {
    ordered(
        SOURCE
            .iter()
            .chain(&NOTICES)
            .map(|name| format!("{}/{name}", stem())),
    )
}

/// Where a member of an archive comes from, when it is a file of the repository.
fn origin(fixture: &Fixture, name: &str) -> Option<PathBuf> {
    let relative = name.strip_prefix(&format!("{}/", stem())).unwrap();
    if let Some(text) = relative.strip_prefix("licenses/rust/") {
        return (text != "INVENTORY.json").then(|| fixture.lab.base.join("registry").join(text));
    }
    (relative != "fenix-patch").then(|| fixture.tree.root.join(relative))
}

struct TarMember {
    name: String,
    mode: u32,
    content: Vec<u8>,
}

/// Every member of a source archive; owners, times and kinds are checked on the way.
fn tar_members(path: &Path) -> Vec<TarMember> {
    let mut plain = Vec::new();
    GzDecoder::new(File::open(path).unwrap())
        .read_to_end(&mut plain)
        .unwrap();
    assert_eq!(plain.len() % 10240, 0, "a whole number of records");
    let mut members = Vec::new();
    for entry in tar::Archive::new(&plain[..]).entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_str().unwrap().to_string();
        let header = entry.header();
        assert_eq!(header.entry_type(), tar::EntryType::Regular, "{name}");
        assert_eq!(
            (
                header.uid().unwrap(),
                header.gid().unwrap(),
                header.mtime().unwrap()
            ),
            (0, 0, 0),
            "{name}"
        );
        assert_eq!(
            (header.username().unwrap(), header.groupname().unwrap()),
            (Some(""), Some("")),
            "{name}"
        );
        let mode = header.mode().unwrap();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        members.push(TarMember {
            name,
            mode,
            content,
        });
    }
    members
}

struct ZipMember {
    name: String,
    mode: u32,
    content: Vec<u8>,
}

fn zip_members(path: &Path) -> Vec<ZipMember> {
    let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut members = Vec::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).unwrap();
        let name = member.name().to_string();
        let time = member.last_modified().unwrap();
        assert_eq!(
            (
                time.year(),
                time.month(),
                time.day(),
                time.hour(),
                time.minute(),
                time.second()
            ),
            (2026, 9, 23, 0, 0, 0),
            "{name}"
        );
        assert_eq!(
            member.compression(),
            zip::CompressionMethod::Deflated,
            "{name}"
        );
        let mode = member.unix_mode().unwrap();
        let mut content = Vec::new();
        member.read_to_end(&mut content).unwrap();
        members.push(ZipMember {
            name,
            mode,
            content,
        });
    }
    members
}

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn dxvk_sources_and_modified_source_pins_are_part_of_both_releases() {
    for variant in [false, true] {
        let fixture = Fixture::new();
        let root = &fixture.tree.root;
        put(
            &root.join("sources/dxvk-source.tar.gz"),
            "unchanged DXVK plus pinned submodules\n",
        );
        put(&root.join("patches/dxvk.patch"), "altered DXVK sources\n");
        fixture.tree.edit(|lock| {
            let entry = if variant {
                &mut lock["variants"][VARIANT]
            } else {
                lock
            };
            entry["sources"]["dxvk-source.tar.gz"] =
                digest(&root.join("sources/dxvk-source.tar.gz")).into();
            entry["dxvk"] = fenix_patch::json::json!({
                "patches": { "dxvk.patch": digest(&root.join("patches/dxvk.patch")) },
            });
        });
        let outcome = fixture.release("gpu-dist");
        assert_eq!(outcome.code, 0, "{}", outcome.output);
        let output = fixture.lab.base.join("gpu-dist");
        let source = tar_members(&output.join(format!("{}-source.tar.gz", stem())));
        let binary = zip_members(&output.join(format!("{}-linux-x86_64.zip", stem())));
        for name in ["sources/dxvk-source.tar.gz", "patches/dxvk.patch"] {
            let name = format!("{}/{name}", stem());
            assert!(source.iter().any(|member| member.name == name), "{name}");
            assert!(binary.iter().any(|member| member.name == name), "{name}");
        }
        put(&root.join("patches/dxvk.patch"), "unrecorded change\n");
        fixture.refused("DXVK patch checksum mismatch: dxvk.patch");
    }
}

#[test]
fn both_archives_hold_the_allowlisted_files_in_a_fixed_form() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    // Files that must not get in, a name beyond the classic header, and one outside ASCII.
    let long = format!("docs/{}/{}.md", "a".repeat(40), "b".repeat(60));
    let accented = "docs/Gr\u{fc}\u{df}e.md";
    for (name, content) in [
        ("tools/target/release/fenix-tools", "build output\n"),
        ("installer/tests/target/deep/file", "also build output\n"),
        ("docs/__pycache__/x.txt", "cache\n"),
        (
            "fenix_patch/core.py",
            "the Python installer is not shipped any more\n",
        ),
        ("scripts/release.py", "nor are the scripts\n"),
        ("integration/notes.txt", "not named in bundle.json\n"),
        ("payload/extra.dll", "not named in bundle.json\n"),
        ("sources/unrelated.tar.gz", "not named in bundle.json\n"),
        ("target/debug/fenix-patch", "build output\n"),
        ("build/smoke/log", "work\n"),
        (long.as_str(), "long\n"),
        (accented, "accented\n"),
    ] {
        put(&root.join(name), content);
    }
    fs::set_permissions(
        root.join("docs/steam.md"),
        fs::Permissions::from_mode(0o4750),
    )
    .unwrap();
    // The installer is shipped executable whatever its file says.
    fs::set_permissions(&fixture.installer, fs::Permissions::from_mode(0o700)).unwrap();

    let outcome = fixture.release("dist");
    let out = fixture.lab.base.join("dist");
    let source_path = out.join(format!("{}-source.tar.gz", stem()));
    let binary_path = out.join(format!("{}-linux-x86_64.zip", stem()));
    assert_eq!(
        outcome.output,
        format!(
            "{}-linux-x86_64.zip {} bytes\n{}-source.tar.gz {} bytes\n",
            stem(),
            fs::metadata(&binary_path).unwrap().len(),
            stem(),
            fs::metadata(&source_path).unwrap().len()
        )
    );
    assert_eq!(outcome.code, 0);
    assert_eq!(
        calls(&fixture.lab.base, root),
        [
            "cargo [metadata] [--locked] [--format-version] [1] [--filter-platform] \
          [x86_64-unknown-linux-musl] cwd=<ROOT>"
        ]
    );

    let mut expected = source_names();
    expected.extend([
        format!("{}/{long}", stem()),
        format!("{}/{accented}", stem()),
    ]);
    let expected = ordered(expected);
    let source = tar_members(&source_path);
    assert_eq!(
        source
            .iter()
            .map(|member| member.name.clone())
            .collect::<Vec<_>>(),
        expected
    );
    for member in &source {
        match origin(&fixture, &member.name) {
            Some(file) => {
                assert_eq!(member.content, fs::read(&file).unwrap(), "{}", member.name);
                let mode = if member.name.contains("/licenses/rust/") {
                    0o644
                } else {
                    mode_of(&file)
                };
                assert_eq!(member.mode, mode, "{}", member.name);
            }
            None => {
                assert_eq!(String::from_utf8_lossy(&member.content), INVENTORY);
                assert_eq!(member.mode, 0o644);
            }
        }
    }
    let modes: BTreeMap<&str, u32> = source
        .iter()
        .map(|member| (member.name.rsplit('/').next().unwrap(), member.mode))
        .collect();
    assert_eq!(
        (
            modes["install.sh"],
            modes["launch-msfs.sh"],
            modes["steam.md"],
            modes["bundle.json"]
        ),
        (0o755, 0o755, 0o4750, 0o644)
    );

    let mut expected: Vec<String> = expected;
    let lock = fixture.tree.lock();
    for name in lock["files"].as_object().unwrap().keys() {
        expected.push(format!("{}/payload/{name}", stem()));
        expected.push(format!("{}/payload/variants/{VARIANT}/{name}", stem()));
    }
    for name in [
        "FenixWindowGuard.exe",
        "FenixGeometrySetup.exe",
        "FenixMCDURefresh.exe",
    ] {
        expected.push(format!("{}/integration/{name}", stem()));
    }
    expected.push(format!("{}/fenix-patch", stem()));
    let binary = zip_members(&binary_path);
    assert_eq!(
        binary
            .iter()
            .map(|member| member.name.clone())
            .collect::<Vec<_>>(),
        ordered(expected)
    );
    for member in &binary {
        let (content, mode) = match origin(&fixture, &member.name) {
            Some(file) if member.name.contains("/licenses/rust/") => {
                (fs::read(&file).unwrap(), 0o644)
            }
            // Only the permission bits go into a ZIP.
            Some(file) => (fs::read(&file).unwrap(), mode_of(&file) & 0o777),
            None if member.name.ends_with("/fenix-patch") => {
                (fs::read(&fixture.installer).unwrap(), 0o755)
            }
            None => (INVENTORY.as_bytes().to_vec(), 0o644),
        };
        assert_eq!(member.content, content, "{}", member.name);
        assert_eq!(member.mode, 0o100_000 | mode, "{}", member.name);
    }

    assert_eq!(
        read(&out.join("SHA256SUMS")),
        format!(
            "{}  {}-linux-x86_64.zip\n{}  {}-source.tar.gz\n",
            digest(&binary_path),
            stem(),
            digest(&source_path),
            stem()
        )
    );
    assert_eq!(
        read(&out.join("flightdeck-release.json")),
        format!(
            "{{\n  \"version\": \"{VERSION}\",\n  \"url\": \"https://example.invalid/fenix-a320-linux-patch/releases/download/v{VERSION}/{}-linux-x86_64.zip\",\n  \"sha256\": \"{}\",\n  \"archive_root\": \"{}\"\n}}\n",
            stem(),
            digest(&binary_path),
            stem()
        )
    );
    let mut written: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    written.sort();
    assert_eq!(
        written,
        [
            "SHA256SUMS".to_string(),
            format!("{}-linux-x86_64.zip", stem()),
            format!("{}-source.tar.gz", stem()),
            "flightdeck-release.json".to_string(),
        ]
    );

    // The same repository gives the same archives, byte for byte, whenever
    // it is released and whatever the files' times are.
    let later = filetime_touch(root);
    assert!(later > 0);
    let outcome = fixture.release("again");
    assert_eq!(outcome.code, 0, "{}", outcome.output);
    let again = fixture.lab.base.join("again");
    for name in &written {
        assert_eq!(
            fs::read(out.join(name)).unwrap(),
            fs::read(again.join(name)).unwrap(),
            "{name}"
        );
    }
    // An existing directory is written into again; its parent must exist.
    let outcome = fixture.release("again");
    assert_eq!(outcome.code, 0, "{}", outcome.output);
    let outcome = fixture.release("no/such/parent");
    assert_eq!(outcome.code, 1);
    assert!(
        outcome
            .output
            .contains("[Errno 2] No such file or directory"),
        "{}",
        outcome.output
    );
}

/// Give every file below `root` a new modification time; how many there were.
fn filetime_touch(root: &Path) -> usize {
    let mut count = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let file = File::options().append(true).open(&path).unwrap();
                file.set_modified(
                    std::time::SystemTime::UNIX_EPOCH
                        + std::time::Duration::from_secs(86_400 * 365 * 30 + count as u64),
                )
                .unwrap();
                count += 1;
            }
        }
    }
    count
}

#[test]
fn the_installer_must_be_static_and_of_this_version() {
    let fixture = Fixture::new();
    let given = fixture.installer.display().to_string();
    let good = fs::read(&fixture.installer).unwrap();

    let outcome = fixture.tools(&["release"]);
    assert_eq!(outcome.code, 2, "{}", outcome.output);
    assert!(outcome.output.contains("--installer"), "{}", outcome.output);

    fs::remove_file(&fixture.installer).unwrap();
    fixture.refused(&format!("[Errno 2] No such file or directory: '{given}'"));
    fs::create_dir(&fixture.installer).unwrap();
    fixture.refused(&format!("The installer is not a regular file: {given}"));
    fs::remove_dir(&fixture.installer).unwrap();
    // A link is not followed: what is checked is what is packed.
    put(&fixture.lab.base.join("built/real"), &good);
    symlink("real", &fixture.installer).unwrap();
    fixture.refused(&format!("The installer is not a regular file: {given}"));
    fs::remove_file(&fixture.installer).unwrap();

    script(
        &fixture.installer,
        &format!("#!/bin/sh\necho fenix-patch {VERSION}\n"),
    );
    fixture.refused(&format!(
        "The installer is not an x86-64 ELF executable: {given}"
    ));
    // This crate's own program is one, but linked against the system's C library.
    if cfg!(all(target_arch = "x86_64", target_env = "gnu")) {
        fs::copy(env!("CARGO_BIN_EXE_fenix-tools"), &fixture.installer).unwrap();
        fixture.refused(&format!("The installer is not statically linked: {given}"));
    }
    for message in [
        "fenix-patch 0.0.0-other\n".to_string(),
        format!("fenix-patch {VERSION}-dirty\n"),
        format!("{VERSION}\n"),
        String::new(),
    ] {
        fs::write(&fixture.installer, tiny_program(&message)).unwrap();
        fixture.refused(&format!(
            "The installer does not report version {VERSION}: {given}"
        ));
    }
    // Not executable: it cannot be asked.
    fs::write(&fixture.installer, &good).unwrap();
    fs::set_permissions(&fixture.installer, fs::Permissions::from_mode(0o644)).unwrap();
    fixture.refused(&format!("[Errno 13] Permission denied: '{given}'"));
    fs::set_permissions(&fixture.installer, fs::Permissions::from_mode(0o755)).unwrap();

    // The version of bundle.json must be the one this tool was built with.
    fixture
        .tree
        .edit(|lock| lock["version"] = "0.0.0-other".into());
    fixture.refused("Installer and bundle versions differ");
    assert!(calls(&fixture.lab.base, &fixture.tree.root).is_empty());
}

#[test]
fn a_real_static_installer_passes_the_format_check() {
    // The installer as it is released, when a build of it is at hand.
    let Some(sample) = std::env::var_os("FENIX_STATIC_INSTALLER").map(PathBuf::from) else {
        notice(
            "FENIX_STATIC_INSTALLER is not set; the check of a real static fenix-patch is skipped",
        );
        return;
    };
    let data = fs::read(&sample).unwrap();
    assert_eq!(fenix_tools::elf::check_static(&data), Ok(()));
}

#[test]
fn payload_sources_and_patches_must_be_what_bundle_json_pins() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    for (name, message) in [
        (
            "payload/files/lib/wine/x86_64-unix/win32u.so",
            "Patch checksum mismatch: files/lib/wine/x86_64-unix/win32u.so".to_string(),
        ),
        (
            "payload/variants/proton-x/files/bin/wineserver",
            "Patch checksum mismatch: variants/proton-x/files/bin/wineserver".to_string(),
        ),
        (
            "integration/xodus-wine-launch",
            "Integration checksum mismatch: xodus-wine-launch".to_string(),
        ),
        (
            "sources/xgameruntime-source.tar.gz",
            "Source checksum mismatch: xgameruntime-source.tar.gz".to_string(),
        ),
        (
            "sources/x-wine.tar.gz",
            "Source checksum mismatch: x-wine.tar.gz".to_string(),
        ),
        (
            "patches/second.patch",
            "Source checksum mismatch: second.patch".to_string(),
        ),
        (
            "patches/proton/x.patch",
            "Source checksum mismatch: proton/x.patch".to_string(),
        ),
    ] {
        let original = fs::read(root.join(name)).unwrap();
        put(&root.join(name), [&original[..], b"\n"].concat());
        fixture.refused(&message);
        put(&root.join(name), original);
    }
    // A link to the right content is no payload, and no source archive either.
    for (name, message) in [
        (
            "payload/files/bin/wineserver",
            "Expected a regular file: wineserver".to_string(),
        ),
        (
            "sources/x-wine.tar.gz",
            format!(
                "Missing source: {}",
                root.join("sources/x-wine.tar.gz").display()
            ),
        ),
    ] {
        let kept = fixture.lab.base.join("kept");
        fs::rename(root.join(name), &kept).unwrap();
        symlink(&kept, root.join(name)).unwrap();
        fixture.refused(&message);
        fs::remove_file(root.join(name)).unwrap();
        fs::rename(&kept, root.join(name)).unwrap();
    }
    fs::remove_file(root.join("sources/wine-source.tar.gz")).unwrap();
    fixture.refused(&format!(
        "[Errno 2] No such file or directory: '{}'",
        root.join("sources/wine-source.tar.gz").display()
    ));
}

#[test]
fn links_missing_files_and_private_content_stop_the_export() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let outside = fixture.lab.base.join("outside.md");
    put(&outside, "outside\n");

    for link in [
        "docs/linked.md",
        "tests/deep/er/link",
        "installer/src/dangling",
    ] {
        fs::create_dir_all(root.join(link).parent().unwrap()).unwrap();
        symlink(&outside, root.join(link)).unwrap();
        fixture.refused("Symlink in source export");
        fs::remove_file(root.join(link)).unwrap();
    }
    // A linked directory is not walked into, and a whole folder may not be one.
    symlink(
        fixture.lab.base.join("registry"),
        root.join("docs/registry"),
    )
    .unwrap();
    fixture.refused("Symlink in source export");
    fs::remove_file(root.join("docs/registry")).unwrap();
    fs::rename(root.join("native"), fixture.lab.base.join("native")).unwrap();
    symlink(fixture.lab.base.join("native"), root.join("native")).unwrap();
    fixture.refused("Symlink in source export");
    fs::remove_file(root.join("native")).unwrap();
    fs::rename(fixture.lab.base.join("native"), root.join("native")).unwrap();
    // Build output and Python's caches are not looked at at all.
    for link in [
        "tools/target/link",
        "tests/__pycache__/link",
        "docs/link.pyc",
    ] {
        fs::create_dir_all(root.join(link).parent().unwrap()).unwrap();
        symlink(&outside, root.join(link)).unwrap();
    }

    for name in ["README.md", "Cargo.lock", "Fenix A320 Linux Patch.desktop"] {
        let kept = fixture.lab.base.join("kept");
        fs::rename(root.join(name), &kept).unwrap();
        let message = format!("Missing source: {}", root.join(name).display());
        fixture.refused(&message);
        symlink(&kept, root.join(name)).unwrap();
        fixture.refused(&message);
        fs::remove_file(root.join(name)).unwrap();
        fs::create_dir(root.join(name)).unwrap();
        fixture.refused(&message);
        fs::remove_dir(root.join(name)).unwrap();
        fs::rename(&kept, root.join(name)).unwrap();
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
        ("tools/src/lib.rs", format!("// see {runtimes}/x\n")),
        ("tests/key.pem", format!("{key}\nMIIE\n")),
        (
            ".github/workflows/ci.yml",
            format!("env:\n  TOKEN: {token}\n"),
        ),
        ("LICENSE", format!("\0binary\0{home}\0")),
        (
            "integration/launch-msfs.sh",
            format!("#!/bin/sh\ncd {home}\n"),
        ),
    ] {
        let path = root.join(name);
        let original = fs::read(&path).ok();
        put(&path, &content);
        if name.starts_with("integration/") {
            fixture.tree.seal(None);
        }
        fixture.refused(&format!("Private content in export: {name}"));
        match original {
            Some(original) => put(&path, original),
            None => fs::remove_file(&path).unwrap(),
        }
    }
    fixture.tree.seal(None);
    // The directory's name may go on, as in "Workshop"; that is refused as well.
    put(
        &root.join("docs/close.md"),
        format!("/home/{}/{}shop\n", "someone", "Work"),
    );
    fixture.refused("Private content in export: docs/close.md");
    // Close to the patterns, but not them.
    let short = format!("XBL3.0 x={};{}", "1", "a".repeat(29));
    put(
        &root.join("docs/close.md"),
        format!(
            "/home/{}/Work\n/home/Work\n/home//Work\n{short}\n-----BEGIN PUBLIC KEY-----\n",
            "a b"
        ),
    );
    let outcome = fixture.release("dist");
    assert_eq!(outcome.code, 0, "{}", outcome.output);
}

#[test]
fn every_linked_crate_must_ship_its_licence() {
    let fixture = Fixture::new();
    let (base, root) = (&fixture.lab.base, &fixture.tree.root);
    let registry = base.join("registry");
    let original = cargo_metadata(base, root);
    let rewrite = |change: &dyn Fn(&mut Value)| {
        let mut graph = original.clone();
        change(&mut graph);
        put(&base.join("cargo-metadata.json"), graph.to_string());
    };
    let package = |graph: &mut Value, name: &str| -> usize {
        graph["packages"]
            .as_array()
            .unwrap()
            .iter()
            .position(|package| package["name"] == name)
            .unwrap()
    };

    // The collection by itself: texts in order, each from the crate's own top level.
    let context = Context::host(&fixture.world);
    let notices = licenses::collect(&original).unwrap();
    drop(context);
    assert_eq!(
        notices
            .texts
            .iter()
            .map(|text| format!("licenses/rust/{}", text.name))
            .collect::<Vec<_>>(),
        SOURCE[40..]
            .iter()
            .chain(&NOTICES)
            .copied()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        notices.texts[4].source,
        registry.join("beta-0.2.0/license.md")
    );
    assert_eq!(
        fenix_patch::json::dumps(&notices.inventory, Some(2)) + "\n",
        INVENTORY
    );

    // A crate under a licence that asks for its text, without one.
    rewrite(&|graph| {
        let delta = package(graph, "delta");
        graph["packages"][delta]["license"] = "MIT".into();
    });
    fixture.refused("Missing licence text of a Rust dependency: delta 0.1.0 (MIT)");
    rewrite(&|graph| {
        let delta = package(graph, "delta");
        graph["packages"][delta]["license"] = Value::Null;
    });
    fixture.refused("Missing licence text of a Rust dependency: delta 0.1.0 (no licence declared)");
    // A link or a directory with a fitting name is no text.
    fs::remove_file(registry.join("beta-0.2.0/COPYING")).unwrap();
    for name in ["NOTICE.txt", "license.md"] {
        fs::remove_file(registry.join("beta-0.2.0").join(name)).unwrap();
    }
    symlink("/etc/hostname", registry.join("beta-0.2.0/LICENSE")).unwrap();
    fs::create_dir(registry.join("beta-0.2.0/LICENSES")).unwrap();
    rewrite(&|_| {});
    fixture.refused("Missing licence text of a Rust dependency: beta 0.2.0 (Apache-2.0)");
    fs::remove_file(registry.join("beta-0.2.0/LICENSE")).unwrap();
    put(
        &registry.join("beta-0.2.0/LICENSE"),
        "Apache License 2.0 (beta)\n",
    );

    // The declared file must be the crate's own.
    put(
        &registry.join("gamma-3.0.0-rc.1+build/src/COPYRIGHT.txt"),
        "nested\n",
    );
    let unexpected =
        "Unexpected licence file of a Rust dependency: gamma 3.0.0-rc.1+build".to_string();
    let absent = format!(
        "[Errno 2] No such file or directory: '{}'",
        registry.join("gamma-3.0.0-rc.1+build/absent.txt").display()
    );
    for (declared, message) in [
        ("../alpha-1.0.0/LICENSE-MIT", &unexpected),
        ("src/COPYRIGHT.txt", &unexpected),
        ("src", &unexpected),
        ("absent.txt", &absent),
    ] {
        rewrite(&|graph| {
            let gamma = package(graph, "gamma");
            graph["packages"][gamma]["license_file"] = declared.into();
        });
        fixture.refused(message);
    }
    // Names become folders of the release.
    rewrite(&|graph| {
        let alpha = package(graph, "alpha");
        graph["packages"][alpha]["version"] = "../1.0.0".into();
    });
    fixture.refused("Unexpected name of a Rust dependency: alpha ../1.0.0");
    rewrite(&|graph| {
        let installer = package(graph, "fenix-patch");
        graph["packages"][installer]["name"] = "another".into();
    });
    fixture.refused("cargo metadata does not describe the package fenix-patch");

    // Cargo's own refusal is shown and ends the release.
    rewrite(&|_| {});
    fixture.world.set("FAKE_CARGO_FAILS", "1");
    let outcome = fixture.release("refused");
    assert_eq!(outcome.code, 1);
    assert!(
        outcome
            .output
            .starts_with("error: the lock file needs to be updated\nfenix-tools: Command '['")
            && outcome
                .output
                .ends_with("' returned non-zero exit status 101.\n"),
        "{}",
        outcome.output
    );
    fixture.world.set("FAKE_CARGO_FAILS", "");

    // A folder of that name in the repository would be shipped twice.
    put(&root.join("licenses/rust/INVENTORY.json"), "[]\n");
    fixture.refused("Two files of the release have the same name: licenses/rust/INVENTORY.json");
}

/// A context on the real repository, with the environment of a test.
fn real(run: impl FnOnce(&Context<'_>, &Path)) {
    let root = repository().unwrap();
    let lab = Lab::new();
    let mut env = lab.env(&lab.base);
    env.set("PATH", std::env::var_os("PATH").unwrap_or_default());
    let world = World::new(&root, env);
    run(&Context::host(&world), &root);
}

#[test]
fn the_repository_itself_passes_the_source_checks() {
    real(|_, root| {
        let lock = fenix_patch::json::loads(&read(&root.join("bundle.json"))).unwrap();
        let allowed = release::sources(root, &lock).unwrap();
        release::check_sources(root, &allowed).unwrap();
        let names: Vec<String> = allowed
            .iter()
            .map(|file| {
                file.strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        for expected in [
            "Cargo.toml",
            "Cargo.lock",
            "installer/src/core.rs",
            "tools/src/release.rs",
            "tools/tests/common/mod.rs",
            "patches/proton/cachyos-11.patch",
            "integration/fenix-display-refresh.py",
            ".github/workflows/checks.yml",
        ] {
            assert!(names.iter().any(|name| name == expected), "{expected}");
        }
        for name in &names {
            assert!(
                !name
                    .split('/')
                    .any(|part| part == "target" || part == "__pycache__")
                    && !name.starts_with("fenix_patch/")
                    && !name.starts_with("scripts/")
                    && !name.starts_with("payload/")
                    && !name.ends_with(".exe"),
                "{name}"
            );
        }
    });
}

#[test]
fn the_real_installer_s_dependencies_all_ship_a_licence_text() {
    if fenix_patch::fsx::which("cargo", std::env::var_os("PATH")).is_none() {
        eprintln!(
            "release: cargo was not found; the licences of the real dependencies are not collected"
        );
        return;
    }
    real(|context, _| {
        let notices = licenses::collect(&licenses::metadata(context).unwrap()).unwrap();
        let crates: Vec<&Value> = notices.inventory.as_array().unwrap().iter().collect();
        let names: Vec<&str> = crates
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        println!("licence texts of {} crates:", crates.len());
        for entry in &crates {
            println!(
                "  {} {} [{}] {}",
                entry["name"].as_str().unwrap(),
                entry["version"].as_str().unwrap(),
                entry["license"].as_str().unwrap_or("-"),
                entry["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|file| file.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // What the installer is made of …
        for linked in [
            "clap",
            "regex",
            "ring",
            "rustls",
            "rustix",
            "serde_json",
            "sha2",
            "tar",
            "ureq",
        ] {
            assert!(names.contains(&linked), "{linked}");
        }
        // … but not itself, what only builds it, what only this tool uses,
        // or what another platform would need.
        for other in [
            "fenix-patch",
            "fenix-tools",
            "cc",
            "tempfile",
            "zip",
            "x11rb",
            "flate2",
            "windows-sys",
        ] {
            assert!(!names.contains(&other), "{other}");
        }
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        for entry in &crates {
            assert!(
                !entry["files"].as_array().unwrap().is_empty(),
                "{} ships no licence text",
                entry["name"]
            );
        }
        for text in &notices.texts {
            assert!(
                fs::metadata(&text.source).unwrap().len() > 0,
                "{}",
                text.name
            );
        }
    });
}
