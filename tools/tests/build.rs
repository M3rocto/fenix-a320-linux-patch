// SPDX-License-Identifier: MIT
//! `build` and `build-proton` from the first download to the recorded
//! hashes, with stand-ins for every program that would compile something.
#![allow(clippy::unwrap_used)]
mod common;

use common::{Lab, Run, Tree, VARIANT, World, calls, digest, put, read, snapshot};
use fenix_patch::{
    Error,
    download::{Message, Stream},
    json::Value,
};
use std::{
    fs,
    os::unix::fs::symlink,
    path::Path,
    sync::{Mutex, mpsc::sync_channel},
};

const MODULES: &str = "[dlls/ntdll/x86_64-windows/ntdll.dll] [dlls/ntdll/ntdll.so] [server/wineserver] \
[dlls/crypt32/x86_64-windows/crypt32.dll] [dlls/mmdevapi/x86_64-windows/mmdevapi.dll] \
[dlls/kernelbase/x86_64-windows/kernelbase.dll] [dlls/d2d1/x86_64-windows/d2d1.dll] \
[dlls/dwrite/x86_64-windows/dwrite.dll] [dlls/dwrite/dwrite.so] [dlls/win32u/win32u.so] \
[dlls/winex11.drv/winex11.so]";

const INSTALLED: [&str; 11] = [
    "files/lib/wine/x86_64-windows/ntdll.dll",
    "files/lib/wine/x86_64-unix/ntdll.so",
    "files/bin/wineserver",
    "files/lib/wine/x86_64-windows/crypt32.dll",
    "files/lib/wine/x86_64-windows/mmdevapi.dll",
    "files/lib/wine/x86_64-windows/kernelbase.dll",
    "files/lib/wine/x86_64-windows/d2d1.dll",
    "files/lib/wine/x86_64-windows/dwrite.dll",
    "files/lib/wine/x86_64-unix/dwrite.so",
    "files/lib/wine/x86_64-unix/win32u.so",
    "files/lib/wine/x86_64-unix/winex11.so",
];

const CONFIGURE: &str = "[--enable-win64] [--disable-tests] [--enable-silent-rules] [--without-ffmpeg] [--without-unwind]";

struct Fixture {
    lab: Lab,
    tree: Tree,
    world: World,
}

impl Fixture {
    fn new() -> Self {
        let lab = Lab::new();
        let tree = Tree::create(&lab.base.join("repo"));
        let world = World::new(&tree.root, lab.env(&lab.base));
        Self { lab, tree, world }
    }

    fn run(&self) -> Run<'_> {
        Run {
            world: &self.world,
            programs: self.lab.programs(),
            fetch: None,
            display: None,
            output: self.lab.base.join("output.txt"),
        }
    }

    fn calls(&self) -> Vec<String> {
        calls(&self.lab.base, &self.tree.root)
    }
}

/// The commands after the patches, for a build in `<ROOT>/{out}` with `flags`.
fn compile_calls(out: &str, flags: &str, jobs: &str) -> Vec<String> {
    let cache = format!("XDG_CACHE_HOME=<ROOT>/{out}/cache");
    vec![
        format!("make_specfiles cwd=<ROOT>/{out}/wine {cache}"),
        format!(
            "python3 [dlls/winevulkan/make_vulkan] [--xml] [<ROOT>/{out}/wine/dlls/winevulkan/vk.xml] \
             [--video-xml] [<ROOT>/{out}/wine/dlls/winevulkan/video.xml] cwd=<ROOT>/{out}/wine {cache}"
        ),
        format!("autoreconf [-f] cwd=<ROOT>/{out}/wine {cache}"),
        format!(
            "configure {CONFIGURE} [CFLAGS={flags}] [CROSSCFLAGS={flags}] cwd=<ROOT>/{out}/objects {cache}"
        ),
        format!("make [depend] cwd=<ROOT>/{out}/objects {cache}"),
        format!("make [-j{jobs}] {MODULES} cwd=<ROOT>/{out}/objects {cache}"),
    ]
}

fn strip_calls(out: &str, payload: &str) -> Vec<String> {
    INSTALLED
        .iter()
        .map(|name| {
            let strip = if name.ends_with(".dll") {
                "x86_64-w64-mingw32-strip"
            } else {
                "strip"
            };
            format!(
                "{strip} [--strip-debug] [<ROOT>/{payload}/{name}] cwd=<ROOT>/{out}/wine \
                 XDG_CACHE_HOME=<ROOT>/{out}/cache"
            )
        })
        .collect()
}

fn patch_call(patch: &str, out: &str, cache: bool) -> String {
    let cache = if cache {
        format!(" XDG_CACHE_HOME=<ROOT>/{out}/cache")
    } else {
        String::new()
    };
    format!(
        "patch [--batch] [--fuzz=0] [-p1] [-i] [<ROOT>/patches/{patch}] cwd=<ROOT>/{out}/wine{cache}"
    )
}

fn build_calls(out: &str, jobs: &str) -> Vec<String> {
    let flags = format!(
        "-g -O2 -ffile-prefix-map=<ROOT>=/usr/src/fenix-a320-linux-patch \
         -ffile-prefix-map=<ROOT>/{out}=/usr/src/fenix-a320-linux-patch/build"
    );
    let mut expected = vec![
        patch_call("first.patch", out, false),
        patch_call("second.patch", out, false),
    ];
    expected.extend(compile_calls(out, &flags, jobs));
    expected.extend(strip_calls(out, "payload"));
    let gcc = "x86_64-w64-mingw32-gcc [-Os] [-s] [-municode]";
    let tail = format!("cwd=<ROOT>/{out}/wine XDG_CACHE_HOME=<ROOT>/{out}/cache");
    expected.extend([
        format!(
            "{gcc} [-mwindows] [-Wl,--no-insert-timestamp] [-o] [<ROOT>/integration/FenixWindowGuard.exe] \
             [<ROOT>/native/window-guard.c] [-luser32] {tail}"
        ),
        format!(
            "{gcc} [-Wl,--no-insert-timestamp] [-o] [<ROOT>/integration/FenixGeometrySetup.exe] \
             [<ROOT>/native/geometry-setup.c] [-lsetupapi] {tail}"
        ),
        format!(
            "{gcc} [-Wl,--no-insert-timestamp] [-o] [<ROOT>/integration/FenixMCDURefresh.exe] \
             [<ROOT>/native/mcdu-refresh.c] {tail}"
        ),
    ]);
    expected
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn failure(text: &str) -> String {
    format!("fenix-tools: {text}\n")
}

#[test]
fn a_whole_build_runs_the_commands_of_the_script_in_order() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let before = fixture.tree.lock();
    let out = root.join("work/first");
    let outcome =
        fixture
            .run()
            .tools(&["build", "--build-dir", out.to_str().unwrap(), "--jobs", "3"]);
    // The stand-ins build something else than what bundle.json pins.
    assert_eq!(
        outcome.output,
        failure(
            "Build finished with different hashes. Compiler/platform/timestamps can differ; \
             review before --record-build and repin Flightdeck."
        )
    );
    assert_eq!(outcome.code, 1);
    assert_eq!(fixture.calls(), build_calls("work/first", "3"));
    assert_eq!(fixture.tree.lock(), before);
    assert_eq!(
        read(&root.join("payload/files/lib/wine/x86_64-windows/d2d1.dll")),
        "built dlls/d2d1/x86_64-windows/d2d1.dll\nstripped by x86_64-w64-mingw32-strip\n"
    );
    assert_eq!(
        read(&root.join("payload/files/bin/wineserver")),
        "built server/wineserver\nstripped by strip\n"
    );
    assert_eq!(
        read(&root.join("integration/FenixGeometrySetup.exe")),
        "compiled geometry-setup.c\n"
    );
    assert_eq!(
        read(&out.join("wine/applied-patches.txt")),
        "first.patch\nsecond.patch\n"
    );

    // Recording keeps every key where it was and adds the three helper files.
    let outcome = fixture.run().tools(&[
        "build",
        "--record-build",
        "--build-dir",
        root.join("work/second").to_str().unwrap(),
        "--jobs=1",
    ]);
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(fixture.calls(), build_calls("work/second", "1"));
    let text = read(&root.join("bundle.json"));
    let after = fixture.tree.lock();
    assert_eq!(
        text,
        fenix_patch::json::dumps(&after, Some(2)) + "\n",
        "written as json.dumps(lock, indent=2) and a line end"
    );
    // Text outside ASCII is written as escapes, numbers keep their form.
    let escaped = format!(
        "\"note\": \"Gr{0}u00fc{0}u00dfe {0}u2026 {0}\"quoted{0}\"\"",
        '\\'
    );
    assert!(text.contains(&escaped), "{text}");
    assert!(
        text.contains("\"ratio\": 1.5,\n    \"exact\": 1.0,"),
        "{text}"
    );
    assert!(text.contains("\"0.1.0-preview.1\": []"), "{text}");
    assert_eq!(keys(&after), keys(&before));
    assert_eq!(keys(&after["files"]), INSTALLED);
    for name in INSTALLED {
        assert_eq!(
            after["files"][name],
            digest(&root.join("payload").join(name))
        );
    }
    assert_eq!(keys(&after["integration"]), keys(&before["integration"]));
    for name in keys(&after["integration"]) {
        assert_eq!(
            after["integration"][name],
            digest(&root.join("integration").join(name))
        );
    }
    assert_ne!(after["integration"], before["integration"]);
    assert_eq!(
        keys(&after["prefix_files"]),
        [
            "drive_c/windows/system32/d2d1_geometry.dll",
            "drive_c/windows/system32/FenixWindowGuard.exe",
            "drive_c/windows/system32/FenixMCDURefresh.exe",
            "drive_c/windows/system32/fenix-display-refresh.py",
        ]
    );
    for name in [
        "FenixWindowGuard.exe",
        "FenixMCDURefresh.exe",
        "fenix-display-refresh.py",
    ] {
        assert_eq!(
            after["prefix_files"][format!("drive_c/windows/system32/{name}")],
            after["integration"][name]
        );
    }
    assert_eq!(after["variants"], before["variants"]);

    // The same build again now matches; the default directory is ROOT/build.
    let outcome = fixture.run().tools(&["build", "--jobs", "2"]);
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(fixture.calls(), build_calls("build", "2"));
    assert_eq!(read(&root.join("bundle.json")), text);
}

#[test]
fn preparing_stops_after_the_patches_and_refuses_what_is_in_the_way() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let outcome = fixture.run().tools(&["build", "--prepare-only"]);
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(
        fixture.calls(),
        [
            patch_call("first.patch", "build", false),
            patch_call("second.patch", "build", false)
        ]
    );
    let prepared: std::collections::BTreeMap<String, String> =
        snapshot(&root.join("build/wine")).into_iter().collect();
    // Modes come from the archive without the bits beyond rwx; the submodule
    // is the second archive; the link is a link.
    for (name, expected) in [
        ("configure", "file 755 "),
        ("server/protocol.def", "file 600 "),
        ("tools/setuid-helper", "file 755 "),
        ("dlls/ntdll/unix/loader.c", "file 664 "),
        ("dlls/xgameruntime/Makefile.in", "file 644 "),
        ("dlls/xgameruntime/src/runtime.c", "file 644 "),
        ("documentation/README.link", "link -> ../README"),
    ] {
        assert!(
            prepared[name].starts_with(expected),
            "{name}: {}",
            prepared[name]
        );
    }
    assert!(!prepared.contains_key("pax_global_header"));
    assert!(!root.join("build/objects").exists());
    assert!(!root.join("build/cache").exists());

    // Work that exists is never written over.
    let outcome = fixture.run().tools(&["build", "--prepare-only"]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure("Use an empty build directory; existing work is never overwritten")
        )
    );
    // A dangling link in its place is no existing directory, yet nothing replaces it.
    let linked = root.join("linked");
    fs::create_dir(&linked).unwrap();
    symlink("nowhere", linked.join("wine")).unwrap();
    let outcome = fixture.run().tools(&[
        "build",
        "--prepare-only",
        "--build-dir",
        linked.to_str().unwrap(),
    ]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure(&format!(
                "[Errno 17] File exists: '{}'",
                linked.join("wine").display()
            ))
        )
    );
    assert!(fixture.calls().is_empty());

    for (name, key, message) in [
        (
            "patches/second.patch",
            "second.patch",
            "Patch checksum mismatch: second.patch",
        ),
        (
            "sources/xgameruntime-source.tar.gz",
            "xgameruntime-source.tar.gz",
            "Source checksum mismatch: xgameruntime-source.tar.gz",
        ),
    ] {
        let original = fs::read(root.join(name)).unwrap();
        put(&root.join(name), [&original[..], b"\n"].concat());
        let out = root.join("refused").join(key);
        let outcome = fixture.run().tools(&[
            "build",
            "--prepare-only",
            "--build-dir",
            out.to_str().unwrap(),
        ]);
        assert_eq!((outcome.code, outcome.output), (1, failure(message)));
        put(&root.join(name), original);
    }
    // The first patch was applied before the second was found to be wrong.
    assert_eq!(
        fixture.calls(),
        [patch_call("first.patch", "refused/second.patch", false)]
    );
    assert!(!root.join("refused/xgameruntime-source.tar.gz").exists());

    let outcome = fixture
        .run()
        .tools(&["build", "--prepare-only", "--jobs", "many"]);
    assert_eq!(outcome.code, 2);
    assert!(
        outcome
            .output
            .contains("invalid literal for int() with base 10: 'many'"),
        "{}",
        outcome.output
    );
}

/// A download that arrives in pieces and then ends, or fails.
fn stream(parts: Vec<Message>) -> fenix_patch::Result<Stream> {
    let (sender, receiver) = sync_channel(parts.len());
    for part in parts {
        sender.send(part).unwrap();
    }
    Ok(Stream(receiver))
}

#[test]
fn missing_sources_are_downloaded_once_and_never_written_over() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let archive = root.join("sources/wine-source.tar.gz");
    let content = fs::read(&archive).unwrap();
    let asked = Mutex::new(Vec::new());
    let run = |parts: fn(&[u8]) -> Vec<Message>, out: &str| {
        let mut run = fixture.run();
        run.fetch = Some(Box::new(|url| {
            asked.lock().unwrap().push(url.to_string());
            stream(parts(&content))
        }));
        run.tools(&[
            "build",
            "--prepare-only",
            "--build-dir",
            root.join(out).to_str().unwrap(),
        ])
    };
    let whole = |content: &[u8]| {
        let (first, second) = content.split_at(content.len() / 2);
        vec![
            Message::Data(first.to_vec()),
            Message::Data(second.to_vec()),
            Message::End,
        ]
    };

    fs::remove_file(&archive).unwrap();
    let outcome = run(whole, "one");
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(fs::read(&archive).unwrap(), content);
    assert_eq!(
        *asked.lock().unwrap(),
        ["https://codeload.github.com/xodus-gaming/wine/tar.gz/0123abc"]
    );
    // What is there is used as it is.
    let outcome = run(|_| panic!("downloaded again"), "two");
    assert_eq!(outcome.code, 0);

    // Nothing is created when the server does not answer, and what an
    // interrupted download leaves is removed.
    fs::remove_file(&archive).unwrap();
    let outcome = run(
        |_| vec![Message::Failed(Error::os_text("HTTP Error 404: Not Found"))],
        "three",
    );
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("HTTP Error 404: Not Found"))
    );
    assert!(!archive.exists());
    let outcome = run(
        |content| {
            vec![
                Message::Data(content[..100].to_vec()),
                Message::Failed(Error::os_text("<urlopen error connection reset>")),
            ]
        },
        "four",
    );
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("<urlopen error connection reset>"))
    );
    assert!(!archive.exists());

    // A complete download with the wrong content stays for inspection, is
    // refused, and is not replaced by the next run.
    let outcome = run(
        |_| vec![Message::Data(b"not the source".to_vec()), Message::End],
        "five",
    );
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("Source checksum mismatch: wine-source.tar.gz"))
    );
    assert_eq!(fs::read(&archive).unwrap(), b"not the source");
    let outcome = run(|_| panic!("downloaded over an existing file"), "six");
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("Source checksum mismatch: wine-source.tar.gz"))
    );
    assert_eq!(fs::read(&archive).unwrap(), b"not the source");
    // A dangling link is in the way as well: the download is not written through it.
    fs::remove_file(&archive).unwrap();
    symlink(root.join("elsewhere.tar.gz"), &archive).unwrap();
    let outcome = run(whole, "seven");
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure(&format!("[Errno 17] File exists: '{}'", archive.display()))
        )
    );
    assert!(!root.join("elsewhere.tar.gz").exists());
    for out in ["three", "four", "five", "six", "seven"] {
        assert!(!root.join(out).exists(), "{out}");
    }

    // A source that bundle.json names but the build has no address for.
    fs::remove_file(&archive).unwrap();
    put(&archive, &content);
    fixture.tree.edit(|lock| {
        lock["sources"]["other-source.tar.gz"] = "0".into();
    });
    let outcome = run(|_| panic!("no address to download from"), "eight");
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("'other-source.tar.gz'"))
    );
}

fn proton_calls(out: &str, jobs: &str) -> Vec<String> {
    let flags = format!("-O2 -ffile-prefix-map=<ROOT>/{out}=/usr/src/fenix-proton");
    let mut expected = vec![
        patch_call("proton/x.patch", out, true),
        format!("make_requests cwd=<ROOT>/{out}/wine XDG_CACHE_HOME=<ROOT>/{out}/cache"),
    ];
    expected.extend(compile_calls(out, &flags, jobs));
    expected.extend(strip_calls(out, &format!("payload/variants/{VARIANT}")));
    expected
}

fn proton(fixture: &Fixture, out: &Path, more: &[&str]) -> common::Outcome {
    let mut argv = vec![
        "build-proton",
        VARIANT,
        "--build-dir",
        out.to_str().unwrap(),
    ];
    argv.extend_from_slice(more);
    fixture.run().tools(&argv)
}

#[test]
fn a_whole_proton_build_runs_the_commands_of_the_script_in_order() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let before = fixture.tree.lock();
    let outcome = proton(&fixture, &root.join("proton/first"), &["--jobs", "5"]);
    assert_eq!(
        (outcome.code, outcome.output),
        (
            1,
            failure("Build hashes differ; review before recording a new build")
        )
    );
    assert_eq!(fixture.calls(), proton_calls("proton/first", "5"));
    assert_eq!(fixture.tree.lock(), before);
    // The base build's payload and helper programs are not touched.
    assert_eq!(
        before["files"],
        common::digests(&root.join("payload"), &INSTALLED)
    );
    assert_eq!(
        read(&root.join("integration/FenixWindowGuard.exe")),
        "MZ guard\n"
    );

    let outcome = proton(
        &fixture,
        &root.join("proton/second"),
        &["--record-build", "--jobs", "5"],
    );
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(fixture.calls(), proton_calls("proton/second", "5"));
    let text = read(&root.join("bundle.json"));
    let after = fixture.tree.lock();
    assert_eq!(text, fenix_patch::json::dumps(&after, Some(2)) + "\n");
    assert_eq!(keys(&after), keys(&before));
    assert_eq!(
        keys(&after["variants"][VARIANT]),
        keys(&before["variants"][VARIANT])
    );
    assert_eq!(keys(&after["variants"][VARIANT]["files"]), INSTALLED);
    let payload = root.join("payload/variants").join(VARIANT);
    assert_eq!(
        after["variants"][VARIANT]["files"],
        common::digests(&payload, &INSTALLED)
    );
    assert_eq!(
        read(&payload.join("files/lib/wine/x86_64-unix/winex11.so")),
        "built dlls/winex11.drv/winex11.so\nstripped by strip\n"
    );
    let mut rest = (after.clone(), before.clone());
    for lock in [&mut rest.0, &mut rest.1] {
        lock["variants"][VARIANT]["files"] = Value::Null;
    }
    assert_eq!(rest.0, rest.1, "nothing else was recorded");

    let outcome = proton(&fixture, &root.join("proton/third"), &[]);
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(read(&root.join("bundle.json")), text);
}

#[test]
fn a_proton_build_checks_its_directory_source_patch_and_protocol() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let out = root.join("proton/prepared");
    let outcome = proton(&fixture, &out, &["--prepare-only"]);
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(
        fixture.calls(),
        proton_calls("proton/prepared", "0")[..2].to_vec()
    );
    assert!(out.join("wine/configure").is_file());
    assert!(!out.join("objects").exists());

    // Unlike the base build, the directory itself has to be new.
    let outcome = proton(&fixture, &out, &["--prepare-only"]);
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("Use a fresh build directory"))
    );
    let empty = root.join("proton/empty");
    fs::create_dir(&empty).unwrap();
    let outcome = proton(&fixture, &empty, &["--prepare-only"]);
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("Use a fresh build directory"))
    );
    assert!(fixture.calls().is_empty());

    let outcome = fixture.run().tools(&[
        "build-proton",
        "no-such-proton",
        "--build-dir",
        root.join("proton/unknown").to_str().unwrap(),
    ]);
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("'no-such-proton'"))
    );
    let outcome = fixture.run().tools(&["build-proton", VARIANT]);
    assert_eq!(outcome.code, 2, "{}", outcome.output);
    assert!(outcome.output.contains("--build-dir"), "{}", outcome.output);

    for (name, message) in [
        ("sources/x-wine.tar.gz", "Wine source checksum mismatch"),
        ("patches/proton/x.patch", "Proton port checksum mismatch"),
    ] {
        let original = fs::read(root.join(name)).unwrap();
        put(&root.join(name), [&original[..], b"\n"].concat());
        let outcome = proton(&fixture, &root.join("proton/refused").join(name), &[]);
        assert_eq!((outcome.code, outcome.output), (1, failure(message)));
        put(&root.join(name), original);
    }
    assert!(fixture.calls().is_empty());

    // The patched source must speak the protocol of the runner it is for.
    fixture
        .tree
        .edit(|lock| lock["variants"][VARIANT]["protocol"] = 932.into());
    let outcome = proton(&fixture, &root.join("proton/protocol"), &["--prepare-only"]);
    assert_eq!(
        (outcome.code, outcome.output),
        (1, failure("Unexpected Wine server protocol"))
    );
    assert_eq!(
        fixture.calls(),
        proton_calls("proton/protocol", "0")[..2].to_vec()
    );
}

#[test]
fn a_missing_proton_source_comes_from_the_repository_bundle_json_names() {
    let fixture = Fixture::new();
    let root = &fixture.tree.root;
    let archive = root.join("sources/x-wine.tar.gz");
    let content = fs::read(&archive).unwrap();
    fs::remove_dir_all(root.join("sources")).unwrap();
    let asked = Mutex::new(Vec::new());
    let mut run = fixture.run();
    run.fetch = Some(Box::new(|url| {
        asked.lock().unwrap().push(url.to_string());
        stream(vec![Message::Data(content.clone()), Message::End])
    }));
    let out = root.join("proton/downloaded");
    let outcome = run.tools(&[
        "build-proton",
        VARIANT,
        "--prepare-only",
        "--build-dir",
        out.to_str().unwrap(),
    ]);
    assert_eq!((outcome.code, outcome.output.as_str()), (0, ""));
    assert_eq!(
        *asked.lock().unwrap(),
        ["https://codeload.github.com/Example/wine-proton/tar.gz/feed"]
    );
    assert_eq!(fs::read(&archive).unwrap(), content);
    assert!(out.join("wine/README").is_file());
}
