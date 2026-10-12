// SPDX-License-Identifier: MIT
//! A synthetic repository with its runner, stand-ins for the programs the
//! commands start, and a display that answers the one question they ask.
//!
//! Nothing here reaches the network, the user's display or a real Wine.
#![allow(dead_code, clippy::unwrap_used)]
use fenix_patch::{
    Result, VERSION,
    core::Wine,
    fsx,
    json::{self, Map, Value, json},
    process::Env,
    system::{Host, Progress, System},
};
use fenix_tools::{Context, Fetch, OpenDisplay, Programs, build::TARGETS, cli};
use flate2::{Compression, write::GzEncoder};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};
use x11rb::{
    protocol::xproto::{
        Depth, Format, GetGeometryReply, GetInputFocusReply, GetPropertyReply,
        GetWindowAttributesReply, MapState, QueryExtensionReply, Screen, Setup, VisualClass,
        Visualtype,
    },
    x11_utils::Serialize,
};

/// The variables that the stand-ins record when they are set.
const LOGGED: [&str; 7] = [
    "XDG_CACHE_HOME",
    "WINEPREFIX",
    "WINEDLLOVERRIDES",
    "WINE_TRACK_WRITECOPY",
    "WINE_FENIX_HELPER_WINDOWS",
    "DISPLAY",
    "WAYLAND_DISPLAY",
];

/// Every stand-in begins with this: one line per call in `$FAKE_LOG`, with
/// the arguments, the directory and the variables the commands set.
const RECORD: &str = r#"#!/bin/sh
# A stand-in of the tests: it records its call, then plays its part.
{
    printf '%s' "${0##*/}"
    for argument in "$@"; do printf ' [%s]' "$argument"; done
    printf ' cwd=%s' "$PWD"
    for name in XDG_CACHE_HOME WINEPREFIX WINEDLLOVERRIDES WINE_TRACK_WRITECOPY WINE_FENIX_HELPER_WINDOWS DISPLAY WAYLAND_DISPLAY; do
        eval "value=\${$name-<unset>}"
        printf ' %s=%s' "$name" "$value"
    done
    printf '\n'
} >> "$FAKE_LOG"
"#;

const PATCH: &str = r#"printf '%s\n' "${5##*/}" >> applied-patches.txt
"#;

const MAKE: &str = r#"case "$1" in
    -j*)
        shift
        for target in "$@"; do
            mkdir -p "${target%/*}"
            printf 'built %s\n' "$target" > "$target"
        done ;;
esac
"#;

const STRIP: &str = r#"printf 'stripped by %s\n' "${0##*/}" >> "$2"
"#;

const COMPILER: &str = r#"output=; source=; previous=
for argument in "$@"; do
    [ "$previous" = -o ] && output=$argument
    case "$argument" in *.c|*.cpp) source=$argument ;; esac
    previous=$argument
done
printf 'compiled %s\n' "${source##*/}" > "$output"
"#;

const READELF: &str = r#"sed -n 's/^#* *NEEDED \(.*\)$/ 0x0000000000000001 (NEEDED)             Shared library: [\1]/p' "$2"
echo ' 0x000000000000000e (SONAME)             Library soname: [itself.so]'
"#;

const CARGO: &str = r#"[ "$1" = metadata ] && [ -n "$FAKE_CARGO_FAILS" ] && { echo 'error: the lock file needs to be updated' >&2; exit 101; }
cat "$FAKE_CARGO_METADATA"
"#;

const RUNTIME: &str = r#"[ "$1" = -- ] && shift
exec "$@"
"#;

const WINESERVER: &str = r#"# NEEDED libc.so.6
exit "${FAKE_WINESERVER_STATUS:-0}"
"#;

/// Wine: enough of it for every step of the smoke test.
const WINE: &str = r#"program=${1##*/}
case "$1" in
    wineboot)
        mkdir -p "$WINEPREFIX/drive_c/windows/system32" "$WINEPREFIX/drive_c/windows/syswow64" "$WINEPREFIX/drive_c/windows/Fonts"
        printf 'WINE REGISTRY Version 2\n\n[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts] 1700000000\n"Tahoma (TrueType)"="tahoma.ttf"\n' > "$WINEPREFIX/system.reg"
        echo 'wineboot writes this where nobody reads it' ;;
    cmd.exe)
        [ -n "$FAKE_CONTAINER_FAILS" ] && { echo 'wine: could not load kernel32.dll' >&2; exit 53; }
        shift 3
        printf '%s\r\n' "$*" ;;
    */syswow64/cmd.exe)
        printf '%s\r\n' "${3#echo }" ;;
    reg) ;;
    */FenixWindowGuard.exe)
        exec sleep 300 ;;
    *)
        if [ $# -eq 4 ]; then
            line=$(grep -F "$program|$3|" "$FAKE_WINDOWS")
            first=${line#*|*|}; second=${first#*|}; first=${first%%|*}
            printf 'probe %s as "%s", renamed "%s"\r\n' "$program" "$3" "$4"
            printf 'host xid=%s helper=%s logical_visible=1\r\n' ${first}
            echo 'fixme:something on standard error' >&2
            printf 'host xid=%s helper=%s logical_visible=1\r\n' ${second}
            printf 'window_visible=1\r\n'
            exit "${FAKE_PROBE_STATUS:-0}"
        elif [ "$2" = 0 ] || [ "$2" = --disabled ]; then
            :
        elif [ "$program" = Fenix.exe ]; then
            printf 'loaded KERNEL32.DLL\r\n'
            [ -n "$FAKE_LOADER_REGRESSION" ] || printf 'loaded KERNELBASE.dll\r\n'
        else
            printf '%s says hello\r\n' "$program"
            printf '%s notes something\n' "$program" >&2
            [ "$program" != "$FAKE_FAILING_PROBE" ] || exit 3
        fi ;;
esac
"#;

/// The first member of an archive that Git made.
const GIT_COMMENT: &[u8] = b"52 comment=0123456789abcdef0123456789abcdef01234567\n";

/// A line for whoever runs the tests, written past the test harness so that
/// a skipped comparison does not go unnoticed.
pub fn notice(text: &str) {
    let _ = writeln!(std::io::stderr().lock(), "fenix-tools tests: {text}");
}

pub fn put(path: &Path, content: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

pub fn script(path: &Path, content: &str) {
    put(path, content);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn stand_in(path: &Path, part: &str) {
    script(path, &format!("{RECORD}{part}"));
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

pub fn digest(path: &Path) -> String {
    fsx::digest(path).unwrap()
}

/// `{name: digest(base / name)}` in the given order.
pub fn digests<S: AsRef<str>>(base: &Path, names: &[S]) -> Value {
    let mut found = Map::new();
    for name in names {
        let name = name.as_ref();
        found.insert(name.to_string(), Value::String(digest(&base.join(name))));
    }
    Value::Object(found)
}

/// One member of a tar archive, written byte for byte as given.
pub struct Raw<'a> {
    pub name: &'a [u8],
    pub kind: u8,
    pub link: &'a [u8],
    pub mode: u32,
    pub data: &'a [u8],
}

impl<'a> Raw<'a> {
    pub fn file(name: &'a str, mode: u32, data: &'a [u8]) -> Self {
        Self {
            name: name.as_bytes(),
            kind: b'0',
            link: b"",
            mode,
            data,
        }
    }

    pub fn directory(name: &'a str) -> Self {
        Self {
            name: name.as_bytes(),
            kind: b'5',
            link: b"",
            mode: 0o755,
            data: b"",
        }
    }

    pub fn link(name: &'a str, target: &'a str) -> Self {
        Self {
            name: name.as_bytes(),
            kind: b'2',
            link: target.as_bytes(),
            mode: 0o777,
            data: b"",
        }
    }

    pub fn kind(name: &'a str, kind: u8, link: &'a str) -> Self {
        Self {
            name: name.as_bytes(),
            kind,
            link: link.as_bytes(),
            mode: 0o644,
            data: b"",
        }
    }
}

/// A tar archive of exactly these members.
pub fn tar(members: &[Raw<'_>]) -> Vec<u8> {
    let mut archive = Vec::new();
    for member in members {
        let mut block = [0u8; 512];
        block[..member.name.len()].copy_from_slice(member.name);
        let octal = |field: &mut [u8], number: u64| {
            let digits = format!("{number:0width$o}", width = field.len() - 1);
            field[..digits.len()].copy_from_slice(digits.as_bytes());
        };
        octal(&mut block[100..108], u64::from(member.mode));
        octal(&mut block[108..116], 1000);
        octal(&mut block[116..124], 1000);
        octal(&mut block[124..136], member.data.len() as u64);
        octal(&mut block[136..148], 1_700_000_000);
        block[148..156].fill(b' ');
        block[156] = member.kind;
        block[157..157 + member.link.len()].copy_from_slice(member.link);
        block[257..265].copy_from_slice(b"ustar\x0000");
        let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
        block[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        archive.extend_from_slice(&block);
        archive.extend_from_slice(member.data);
        archive.resize(archive.len().div_ceil(512) * 512, 0);
    }
    archive.resize(archive.len() + 1024, 0);
    archive
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// A statically linked x86-64 program of 150 bytes that prints `message`
/// and succeeds, whatever its arguments.
pub fn tiny_program(message: &str) -> Vec<u8> {
    const BASE: u64 = 0x40_0000;
    const HEADERS: u64 = 64 + 56;
    let mut code: Vec<u8> = Vec::new();
    code.extend([0xb8, 1, 0, 0, 0]); // mov eax, 1 (write)
    code.extend([0xbf, 1, 0, 0, 0]); // mov edi, 1 (standard output)
    code.extend([0x48, 0x8d, 0x35, 16, 0, 0, 0]); // lea rsi, [rip + 16]
    code.push(0xba); // mov edx, length
    code.extend((message.len() as u32).to_le_bytes());
    code.extend([0x0f, 0x05]); // syscall
    code.extend([0xb8, 60, 0, 0, 0]); // mov eax, 60 (exit)
    code.extend([0x31, 0xff]); // xor edi, edi
    code.extend([0x0f, 0x05]); // syscall
    code.extend(message.as_bytes());
    let size = HEADERS + code.len() as u64;
    let mut file = Vec::new();
    file.extend(b"\x7fELF\x02\x01\x01\x00");
    file.extend([0u8; 8]);
    file.extend(2u16.to_le_bytes()); // ET_EXEC
    file.extend(62u16.to_le_bytes()); // x86-64
    file.extend(1u32.to_le_bytes());
    file.extend((BASE + HEADERS).to_le_bytes()); // entry
    file.extend(64u64.to_le_bytes()); // program headers
    file.extend(0u64.to_le_bytes()); // no sections
    file.extend(0u32.to_le_bytes());
    file.extend(64u16.to_le_bytes());
    file.extend(56u16.to_le_bytes());
    file.extend(1u16.to_le_bytes());
    file.extend(64u16.to_le_bytes());
    file.extend(0u16.to_le_bytes());
    file.extend(0u16.to_le_bytes());
    file.extend(1u32.to_le_bytes()); // PT_LOAD
    file.extend(5u32.to_le_bytes()); // readable, executable
    file.extend(0u64.to_le_bytes());
    file.extend(BASE.to_le_bytes());
    file.extend(BASE.to_le_bytes());
    file.extend(size.to_le_bytes());
    file.extend(size.to_le_bytes());
    file.extend(0x1000u64.to_le_bytes());
    file.extend(code);
    file
}

/// The directory that holds everything of one test.
pub struct Lab {
    _temp: tempfile::TempDir,
    pub base: PathBuf,
    /// Stand-ins for the programs that are started by name.
    pub bin: PathBuf,
    /// The stand-in for `patch`, apart from the others: some tests use the real one.
    pub patch: PathBuf,
}

impl Lab {
    pub fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("tools-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let base = temp.path().canonicalize().unwrap();
        let bin = base.join("bin");
        for (name, part) in [
            ("make", MAKE),
            ("autoreconf", ""),
            ("python3", ""),
            ("strip", STRIP),
            ("x86_64-w64-mingw32-strip", STRIP),
            ("x86_64-w64-mingw32-gcc", COMPILER),
            ("x86_64-w64-mingw32-g++", COMPILER),
            ("readelf", READELF),
            ("cargo", CARGO),
        ] {
            stand_in(&bin.join(name), part);
        }
        let patch = base.join("patch/patch");
        stand_in(&patch, PATCH);
        Self {
            _temp: temp,
            base,
            bin,
            patch,
        }
    }

    /// Every program by the path of its stand-in.
    pub fn programs(&self) -> Programs {
        let at = |name: &str| self.bin.join(name).into_os_string();
        Programs {
            patch: self.patch.clone().into_os_string(),
            autoreconf: at("autoreconf"),
            python3: at("python3"),
            make: at("make"),
            strip: at("strip"),
            mingw_strip: at("x86_64-w64-mingw32-strip"),
            mingw_gcc: at("x86_64-w64-mingw32-gcc"),
            mingw_gxx: at("x86_64-w64-mingw32-g++"),
            readelf: at("readelf"),
            cargo: at("cargo"),
        }
    }

    /// The environment of this test: no display of the user, and the
    /// stand-ins first in the search path.
    pub fn env(&self, side: &Path) -> Env {
        let mut env = Env(std::env::vars_os()
            .filter(|(name, _)| {
                !matches!(
                    name.to_str(),
                    Some("DISPLAY" | "WAYLAND_DISPLAY" | "XAUTHORITY" | "XDG_CACHE_HOME")
                ) && !name.to_string_lossy().starts_with("WINE")
            })
            .collect());
        let mut search = self.bin.clone().into_os_string();
        search.push(":");
        search.push(std::env::var_os("PATH").unwrap_or_default());
        env.set("PATH", search);
        env.set("FAKE_LOG", side.join("calls.log"));
        env.set("FAKE_WINDOWS", side.join("windows.txt"));
        env.set("FAKE_CARGO_METADATA", side.join("cargo-metadata.json"));
        env
    }
}

/// The machine of a test: the repository at `root`, the test's environment,
/// and Microsoft's geometry packages replaced by a note that they were asked for.
pub struct World {
    host: Host,
    pub environ: Mutex<Env>,
    pub geometry: AtomicUsize,
}

impl World {
    pub fn new(root: &Path, environ: Env) -> Self {
        Self {
            host: Host::new(root.to_path_buf()),
            environ: Mutex::new(environ),
            geometry: AtomicUsize::new(0),
        }
    }

    pub fn set(&self, name: &str, value: impl Into<OsString>) {
        self.environ.lock().unwrap().set(name, value);
    }
}

impl System for World {
    fn root(&self) -> &Path {
        self.host.root()
    }

    fn environ(&self) -> Env {
        self.environ.lock().unwrap().clone()
    }

    fn var(&self, name: &str) -> Option<OsString> {
        self.environ.lock().unwrap().get(name).cloned()
    }

    fn prepare_geometry(
        &self,
        _wine: &dyn Wine,
        cache: &Path,
        bundle: &Path,
        progress: Progress<'_>,
    ) -> Result<()> {
        self.geometry.fetch_add(1, Ordering::SeqCst);
        progress(&format!(
            "geometry stand-in: {} {}",
            cache.display(),
            bundle.display()
        ))
    }
}

/// What a command printed and how it ended. `output` has everything in the
/// order it was written: the command's own lines, its errors, and what the
/// programs it started wrote to the terminal.
#[derive(Debug)]
pub struct Outcome {
    pub code: i32,
    pub output: String,
}

/// How one command of a test is run.
pub struct Run<'a> {
    pub world: &'a World,
    pub programs: Programs,
    pub fetch: Option<Box<Fetch<'a>>>,
    /// `None`: opening a display is an error of the test.
    pub display: Option<&'a OpenDisplay<'a>>,
    pub output: PathBuf,
}

impl Run<'_> {
    pub fn tools(&self, argv: &[&str]) -> Outcome {
        let open = || {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.output)
                .unwrap()
        };
        let context = Context {
            system: self.world,
            programs: self.programs.clone(),
            out: Mutex::new(Box::new(open())),
            err: Mutex::new(Box::new(open())),
            terminal: Some(open()),
            fetch: Box::new(|url| match &self.fetch {
                Some(fetch) => fetch(url),
                None => panic!("a test must not download {url}"),
            }),
            display: Box::new(|name| match self.display {
                Some(display) => display(name),
                None => panic!("a test must not open the display {name}"),
            }),
        };
        let argv = std::iter::once("fenix-tools")
            .chain(argv.iter().copied())
            .map(OsString::from);
        let code = cli::run(&context, argv);
        drop(context);
        let output = read(&self.output);
        fs::remove_file(&self.output).unwrap();
        Outcome { code, output }
    }
}

/// The calls of the stand-ins since the last time, one per line: the
/// arguments, the directory and the recorded variables that were set, with
/// `root` written as `<ROOT>`.
pub fn calls(side: &Path, root: &Path) -> Vec<String> {
    let log = side.join("calls.log");
    let text = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_file(&log);
    text.lines()
        .map(|line| {
            let mut line = line.replace(&*root.to_string_lossy(), "<ROOT>");
            for name in LOGGED {
                line = line.replace(&format!(" {name}=<unset>"), "");
            }
            line
        })
        .collect()
}

fn source_tree<'a>(top: &'a str, names: &'a mut Vec<String>) -> Vec<Raw<'a>> {
    const FILES: [(&str, u32, &str); 12] = [
        ("configure", 0o755, RECORD),
        ("tools/make_specfiles", 0o755, RECORD),
        ("tools/make_requests", 0o755, RECORD),
        (
            "dlls/winevulkan/make_vulkan",
            0o755,
            "# Wine's own generator\n",
        ),
        ("dlls/winevulkan/vk.xml", 0o644, "<registry/>\n"),
        ("dlls/winevulkan/video.xml", 0o644, "<registry/>\n"),
        (
            "include/wine/server_protocol.h",
            0o644,
            "/* generated */\n#define SERVER_PROTOCOL_VERSION 931\n",
        ),
        ("README", 0o644, "wine readme\nline two\nline three\n"),
        ("server/protocol.def", 0o600, "@REQ(new_process)\n"),
        ("tools/setuid-helper", 0o4755, "#!/bin/sh\n"),
        (
            "dlls/ntdll/unix/loader.c",
            0o664,
            "int main(void) { return 0; }\n",
        ),
        ("po/de.po", 0o644, "msgid \"Gr\u{fc}\u{df}e\"\n"),
    ];
    *names = FILES
        .iter()
        .map(|(name, _, _)| format!("{top}/{name}"))
        .chain([
            format!("{top}/"),
            format!("{top}/dlls/xgameruntime/"),
            format!("{top}/documentation/README.link"),
        ])
        .collect();
    let mut members = vec![
        Raw {
            name: b"pax_global_header",
            kind: b'g',
            link: b"",
            mode: 0o666,
            data: GIT_COMMENT,
        },
        Raw::directory(&names[FILES.len()]),
    ];
    for (index, (_, mode, data)) in FILES.iter().enumerate() {
        members.push(Raw::file(&names[index], *mode, data.as_bytes()));
    }
    members.push(Raw::directory(&names[FILES.len() + 1]));
    members.push(Raw::link(&names[FILES.len() + 2], "../README"));
    members
}

/// A source archive as GitHub delivers it: a comment for all members, one
/// top-level folder, files with their modes, an empty directory for the
/// submodule and a symbolic link.
pub fn wine_archive(top: &str) -> Vec<u8> {
    let mut names = Vec::new();
    let members = source_tree(top, &mut names);
    gzip(&tar(&members))
}

pub const VARIANT: &str = "proton-x";

const INTEGRATION: [&str; 6] = [
    "FenixWindowGuard.exe",
    "launch-msfs.sh",
    "xodus-wine-launch",
    "FenixGeometrySetup.exe",
    "FenixMCDURefresh.exe",
    "fenix-display-refresh.py",
];

const PROBES: [&str; 8] = [
    "loader-case-probe.c",
    "writecopy-probe.c",
    "iocp-smoke.c",
    "audio-notification-probe.c",
    "shared-cursor-probe.c",
    "window-guard-probe.c",
    "path-metrics-probe.cpp",
    "stroke-joins-probe.cpp",
];

const PATCH_FIRST: &str = "\
--- a/README
+++ b/README
@@ -1,3 +1,3 @@
 wine readme
-line two
+line 2
 line three
";

const PATCH_SECOND: &str = "\
--- /dev/null
+++ b/dlls/new/added.c
@@ -0,0 +1 @@
+int added;
";

const PATCH_VARIANT: &str = "\
--- a/README
+++ b/README
@@ -1,3 +1,4 @@
 wine readme
 line two
 line three
+ported to this Proton
";

fn module(kind: &str, name: &str) -> String {
    if name.ends_with("/wineserver") {
        format!("{RECORD}{WINESERVER}# {kind}\n")
    } else {
        format!("{kind} {name}\nNEEDED libc.so.6\nNEEDED libm.so.6\n")
    }
}

/// A repository with everything the four commands read, small enough to be
/// made for every test.
pub struct Tree {
    pub root: PathBuf,
}

impl Tree {
    pub fn create(root: &Path) -> Self {
        for (name, content) in [
            ("README.md", "# Synthetic\n"),
            ("LICENSE", "MIT License\n"),
            ("THIRD_PARTY.md", "Wine: LGPL-2.1\n"),
            ("BUILDING.md", "cargo run -p fenix-tools -- build\n"),
            (".gitignore", "target/\n"),
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"installer\", \"tools\"]\n",
            ),
            ("Cargo.lock", "version = 4\n"),
            (
                "installer/Cargo.toml",
                "[package]\nname = \"fenix-patch\"\n",
            ),
            ("installer/src/main.rs", "fn main() {}\n"),
            ("installer/target/debug/fenix-patch", "build output\n"),
            ("tools/src/lib.rs", "// tools\n"),
            ("tools/tests/common/mod.rs", "// fixtures\n"),
            (
                "licenses/LGPL-2.1.txt",
                "GNU LESSER GENERAL PUBLIC LICENSE\n",
            ),
            ("docs/steam.md", "Steam\n"),
            ("docs/deep/er/notes with spaces.txt", "notes\n"),
            ("native/window-guard.c", "int guard;\n"),
            ("native/geometry-setup.c", "int geometry;\n"),
            ("native/mcdu-refresh.c", "int mcdu;\n"),
            (".github/workflows/ci.yml", "on: [push]\n"),
            ("tests/test_display_refresh.py", "import unittest\n"),
            (
                "tests/__pycache__/test_display_refresh.cpython-314.pyc",
                "\0\0",
            ),
            ("tests/stale.pyc", "\0\0"),
            ("patches/first.patch", PATCH_FIRST),
            ("patches/second.patch", PATCH_SECOND),
            ("patches/proton/x.patch", PATCH_VARIANT),
            ("integration/FenixWindowGuard.exe", "MZ guard\n"),
            ("integration/FenixGeometrySetup.exe", "MZ geometry\n"),
            ("integration/FenixMCDURefresh.exe", "MZ mcdu\n"),
            (
                "integration/fenix-display-refresh.py",
                "#!/usr/bin/env python3\n",
            ),
        ] {
            put(&root.join(name), content);
        }
        for name in ["install.sh", "Fenix A320 Linux Patch.desktop"] {
            script(&root.join(name), "#!/bin/sh\nexec ./fenix-patch \"$@\"\n");
        }
        for name in ["launch-msfs.sh", "xodus-wine-launch"] {
            script(&root.join("integration").join(name), "#!/bin/sh\n");
        }
        for name in PROBES {
            put(&root.join("tests").join(name), format!("// {name}\n"));
        }
        for (_, name) in TARGETS {
            let base = root.join("payload").join(name);
            let variant = root.join("payload/variants").join(VARIANT).join(name);
            if name.ends_with("/wineserver") {
                script(&base, &module("payload", name));
                script(&variant, &module("variant payload", name));
            } else {
                put(&base, module("payload", name));
                put(&variant, module("variant payload", name));
            }
        }
        put(
            &root.join("sources/wine-source.tar.gz"),
            wine_archive("wine-0123abc"),
        );
        put(
            &root.join("sources/x-wine.tar.gz"),
            wine_archive("wine-proton-feed"),
        );
        put(
            &root.join("sources/xgameruntime-source.tar.gz"),
            gzip(&tar(&[
                Raw::directory("xgameruntime-64ae/"),
                Raw::file(
                    "xgameruntime-64ae/Makefile.in",
                    0o644,
                    b"MODULE = xgameruntime.dll\n",
                ),
                Raw::file("xgameruntime-64ae/src/runtime.c", 0o644, b"int runtime;\n"),
            ])),
        );
        let tree = Self {
            root: root.to_path_buf(),
        };
        tree.seal(None);
        tree
    }

    /// Write `bundle.json` for the files as they are now. `runner` and
    /// `variant_runner` are the runners it shall name.
    pub fn seal(&self, runners: Option<(&Path, &Path)>) {
        let root = &self.root;
        let names: Vec<&str> = TARGETS.iter().map(|(_, name)| *name).collect();
        let runner_names = [
            "files/bin/wine",
            "files/lib/wine/x86_64-windows/kernel32.dll",
        ];
        let (runner_files, variant_runner_files) = match runners {
            Some((runner, variant)) => (
                digests(runner, &runner_names),
                digests(variant, &runner_names),
            ),
            None => (json!({}), json!({})),
        };
        let lock = json!({
            "format": 1,
            "version": VERSION,
            "repository": "https://example.invalid/fenix-a320-linux-patch",
            "wine_revision": "0123abc",
            "xgameruntime_revision": "64ae",
            "runner_version": "1700000000 synthetic-runner-1.0",
            "runner_files": runner_files,
            "files": digests(&root.join("payload"), &names),
            "patches": digests(&root.join("patches"), &["first.patch", "second.patch"]),
            "sources": digests(
                &root.join("sources"),
                &["wine-source.tar.gz", "xgameruntime-source.tar.gz"],
            ),
            "tested": {"fenix": "2.4.0.4720", "ratio": 1.5, "exact": 1.0, "note": "Gr\u{fc}\u{df}e \u{2026} \"quoted\""},
            "integration": digests(&root.join("integration"), &INTEGRATION),
            "previous_releases": {"0.1.0-preview.1": []},
            "prefix_files": {
                "drive_c/windows/system32/d2d1_geometry.dll": "663f1d59ec1c014b9ea47a6cef71b3d43579e9759a82f3ee2cbd80b8c6d9e85f",
            },
            "variants": {
                VARIANT: {
                    "wine_repository": "Example/wine-proton",
                    "wine_revision": "feed",
                    "runner_version": "1800000000 synthetic-proton-x",
                    "runner_files": variant_runner_files,
                    "files": digests(&root.join("payload/variants").join(VARIANT), &names),
                    "patches": digests(&root.join("patches"), &["proton/x.patch"]),
                    "sources": digests(&root.join("sources"), &["x-wine.tar.gz"]),
                    "protocol": 931,
                },
            },
        });
        put(
            &root.join("bundle.json"),
            json::dumps(&lock, Some(2)) + "\n",
        );
    }

    pub fn lock(&self) -> Value {
        json::loads(&read(&self.root.join("bundle.json"))).unwrap()
    }

    /// Change `bundle.json` without touching the files it describes.
    pub fn edit(&self, change: impl FnOnce(&mut Value)) {
        let mut lock = self.lock();
        change(&mut lock);
        put(
            &self.root.join("bundle.json"),
            json::dumps(&lock, Some(2)) + "\n",
        );
    }
}

/// A Wine runner made of stand-ins, with the files the smoke test takes from it.
pub fn runner(path: &Path, version: &str) {
    put(&path.join("version"), format!("{version}\n"));
    stand_in(&path.join("files/bin/wine"), WINE);
    put(
        &path.join("files/lib/wine/x86_64-windows/kernel32.dll"),
        "MZ kernel32\n",
    );
    for (_, name) in TARGETS {
        if name.ends_with("/wineserver") {
            script(&path.join(name), &module("runner", name));
        } else {
            put(&path.join(name), module("runner", name));
        }
    }
    for folder in ["x86_64-windows", "i386-windows"] {
        for name in [
            "libvkd3d-1.dll",
            "libvkd3d-shader-1.dll",
            "libvkd3d-utils-1.dll",
        ] {
            put(
                &path.join("files/lib/vkd3d").join(folder).join(name),
                format!("{folder} {name}\n"),
            );
        }
    }
    for name in [
        "arial.ttf",
        "arialbd.ttf",
        "cour.ttf",
        "courbd.ttf",
        "georgia.ttf",
        "times.ttf",
        "micross.ttf",
    ] {
        put(&path.join("files/share/fonts").join(name), name);
    }
    for name in ["tahoma.ttf", "tahomabd.ttf"] {
        put(&path.join("files/share/wine/fonts").join(name), name);
    }
}

/// A Steam Linux Runtime whose `run` starts the program outside any container.
pub fn steam_runtime(path: &Path) {
    stand_in(&path.join("run"), RUNTIME);
}

/// What each case of the window-guard table reports and what the display
/// then says: the probe's lookup file and the windows by their map state.
pub fn windows(side: &Path) -> HashMap<u32, u8> {
    let mut lines = String::new();
    let mut states = HashMap::new();
    for (index, (executable, title, _, hidden)) in
        fenix_tools::smoke::WINDOW_GUARD.iter().enumerate()
    {
        let first = 0x40_0000 + 2 * index as u32;
        let report = |window: u32, hidden: bool| format!("{window} {}", u8::from(hidden));
        lines += &format!(
            "{executable}|{title}|{}|{}\n",
            report(first, hidden.0),
            report(first + 1, hidden.1)
        );
        // A helper window stays unmapped; any other is viewable.
        states.insert(first, if hidden.0 { 0 } else { 2 });
        states.insert(first + 1, if hidden.1 { 0 } else { 2 });
    }
    put(&side.join("windows.txt"), lines);
    states
}

/// An X11 server for exactly what the window check and Xlib's start-up ask:
/// the connection set-up, `GetWindowAttributes`, and three requests that
/// libX11 sends while it opens and closes a display. It listens on the
/// loopback interface only and shows nothing.
pub struct Display {
    /// What `DISPLAY` has to be to reach it.
    pub name: String,
    pub windows: Arc<Mutex<HashMap<u32, u8>>>,
    /// The windows that were asked about, in order.
    pub asked: Arc<Mutex<Vec<u32>>>,
}

impl Display {
    pub fn start(windows: HashMap<u32, u8>) -> Self {
        // Display N is TCP port 6000 + N.
        let listener = loop {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            if listener.local_addr().unwrap().port() > 6000 {
                break listener;
            }
        };
        let name = format!("127.0.0.1:{}", listener.local_addr().unwrap().port() - 6000);
        let windows = Arc::new(Mutex::new(windows));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (known, seen) = (windows.clone(), asked.clone());
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let (known, seen) = (known.clone(), seen.clone());
                thread::spawn(move || {
                    let _ = serve(client, &known, &seen);
                });
            }
        });
        Self {
            name,
            windows,
            asked,
        }
    }
}

fn setup() -> Vec<u8> {
    let visual = Visualtype {
        visual_id: 0x21,
        class: VisualClass::TRUE_COLOR,
        bits_per_rgb_value: 8,
        colormap_entries: 256,
        red_mask: 0xff_0000,
        green_mask: 0xff00,
        blue_mask: 0xff,
    };
    let screen = Screen {
        root: 0x100,
        default_colormap: 0x20,
        white_pixel: 0xff_ffff,
        width_in_pixels: 1024,
        height_in_pixels: 768,
        width_in_millimeters: 270,
        height_in_millimeters: 203,
        min_installed_maps: 1,
        max_installed_maps: 1,
        root_visual: 0x21,
        root_depth: 24,
        allowed_depths: vec![Depth {
            depth: 24,
            visuals: vec![visual],
        }],
        ..Screen::default()
    };
    let mut bytes = Setup {
        status: 1,
        protocol_major_version: 11,
        release_number: 1,
        resource_id_base: 0x20_0000,
        resource_id_mask: 0x1f_ffff,
        maximum_request_length: u16::MAX,
        bitmap_format_scanline_unit: 32,
        bitmap_format_scanline_pad: 32,
        min_keycode: 8,
        max_keycode: 255,
        vendor: b"fenix-tools test display".to_vec(),
        pixmap_formats: vec![Format {
            depth: 24,
            bits_per_pixel: 32,
            scanline_pad: 32,
        }],
        roots: vec![screen],
        ..Setup::default()
    }
    .serialize();
    let length = ((bytes.len() - 8) / 4) as u16;
    bytes[6..8].copy_from_slice(&length.to_le_bytes());
    bytes
}

/// A reply as it goes over the wire: at least 32 bytes.
fn reply(serialized: impl AsRef<[u8]>) -> Vec<u8> {
    let mut bytes = serialized.as_ref().to_vec();
    bytes.resize(bytes.len().max(32), 0);
    bytes
}

fn serve(
    mut client: TcpStream,
    windows: &Mutex<HashMap<u32, u8>>,
    asked: &Mutex<Vec<u32>>,
) -> std::io::Result<()> {
    let mut hello = [0u8; 12];
    client.read_exact(&mut hello)?;
    assert_eq!(hello[0], b'l', "byte order of the client");
    let padded = |index: usize| {
        usize::from(u16::from_le_bytes([hello[index], hello[index + 1]])).div_ceil(4) * 4
    };
    let mut authorization = vec![0u8; padded(6) + padded(8)];
    client.read_exact(&mut authorization)?;
    client.write_all(&setup())?;
    let mut sequence: u16 = 0;
    loop {
        let mut head = [0u8; 4];
        client.read_exact(&mut head)?;
        let length = usize::from(u16::from_le_bytes([head[2], head[3]])) * 4;
        let mut body = vec![0u8; length.saturating_sub(4)];
        client.read_exact(&mut body)?;
        sequence = sequence.wrapping_add(1);
        if std::env::var_os("FENIX_TEST_DISPLAY_TRACE").is_some() {
            notice(&format!(
                "display: request {} (#{sequence}, {length} bytes)",
                head[0]
            ));
        }
        match head[0] {
            // GetWindowAttributes
            3 => {
                let window = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
                asked.lock().unwrap().push(window);
                let state = windows.lock().unwrap().get(&window).copied();
                match state {
                    Some(state) => client.write_all(&reply(
                        GetWindowAttributesReply {
                            sequence,
                            length: 3,
                            visual: 0x21,
                            map_state: MapState::from(state),
                            colormap: 0x20,
                            ..GetWindowAttributesReply::default()
                        }
                        .serialize(),
                    ))?,
                    None => {
                        // BadWindow
                        let mut error = [0u8; 32];
                        error[1] = 3;
                        error[2..4].copy_from_slice(&sequence.to_le_bytes());
                        error[4..8].copy_from_slice(&window.to_le_bytes());
                        error[10] = 3;
                        client.write_all(&error)?;
                    }
                }
            }
            // GetGeometry: the second half of Xlib's XGetWindowAttributes.
            14 => {
                let window = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
                if windows.lock().unwrap().contains_key(&window) {
                    client.write_all(&reply(
                        GetGeometryReply {
                            depth: 24,
                            sequence,
                            root: 0x100,
                            width: 640,
                            height: 480,
                            ..GetGeometryReply::default()
                        }
                        .serialize(),
                    ))?;
                } else {
                    // BadDrawable
                    let mut error = [0u8; 32];
                    error[1] = 9;
                    error[2..4].copy_from_slice(&sequence.to_le_bytes());
                    error[4..8].copy_from_slice(&window.to_le_bytes());
                    error[10] = 14;
                    client.write_all(&error)?;
                }
            }
            // GetProperty: Xlib reads the resources of the root window.
            20 => client.write_all(&reply(
                GetPropertyReply {
                    sequence,
                    ..GetPropertyReply::default()
                }
                .serialize(),
            ))?,
            // GetInputFocus: Xlib's way to wait for everything before.
            43 => client.write_all(&reply(
                GetInputFocusReply {
                    sequence,
                    ..GetInputFocusReply::default()
                }
                .serialize(),
            ))?,
            // QueryExtension: there are none.
            98 => client.write_all(&reply(
                QueryExtensionReply {
                    sequence,
                    ..QueryExtensionReply::default()
                }
                .serialize(),
            ))?,
            _ => {}
        }
    }
}

/// `cp -a`: files with their permissions, links as links.
pub fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            symlink(fs::read_link(&source).unwrap(), &target).unwrap();
        } else if kind.is_dir() {
            copy_tree(&source, &target);
        } else {
            fs::copy(&source, &target).unwrap();
        }
    }
}

/// Every entry below `base`: kind, permission bits, and content or link target.
pub fn snapshot(base: &Path) -> Vec<(String, String)> {
    fn walk(base: &Path, directory: &Path, found: &mut Vec<(String, String)>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            let info = fs::symlink_metadata(&path).unwrap();
            let name = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mode = info.permissions().mode() & 0o7777;
            if info.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                found.push((name, format!("link -> {}", target.display())));
            } else if info.is_dir() {
                found.push((name, format!("directory {mode:o}")));
                walk(base, &path, found);
            } else if info.is_file() {
                found.push((name, format!("file {mode:o} {}", digest(&path))));
            } else {
                found.push((name, format!("other {mode:o}")));
            }
        }
    }
    let mut found = Vec::new();
    walk(base, base, &mut found);
    found
}

pub fn file(path: &Path) -> File {
    File::open(path).unwrap()
}

/// What `cargo metadata` would say about a workspace whose installer links
/// four crates, with their sources below `<side>/registry`. Written where
/// the stand-in for Cargo reads it.
///
/// `alpha`, `beta`, `gamma` and `delta` are linked into `fenix-patch`. The
/// others are not: one only builds it, one only tests it, one belongs to
/// what only builds it, and one is used by the tools alone.
pub fn cargo_metadata(side: &Path, root: &Path) -> Value {
    let registry = side.join("registry");
    for (name, content) in [
        ("alpha-1.0.0/LICENSE-APACHE", "Apache License 2.0 (alpha)\n"),
        ("alpha-1.0.0/LICENSE-MIT", "MIT (alpha)\n"),
        (
            "alpha-1.0.0/src/LICENSE-of-a-vendored-part",
            "not at the top\n",
        ),
        ("alpha-1.0.0/README.md", "alpha\n"),
        ("beta-0.2.0/COPYING", "Apache License 2.0 (beta)\n"),
        ("beta-0.2.0/NOTICE.txt", "beta contains work of others\n"),
        ("beta-0.2.0/license.md", "lower case counts\n"),
        ("beta-0.2.0/licensing-notes.md", "no licence by its name\n"),
        ("gamma-3.0.0-rc.1+build/COPYRIGHT.txt", "Copyright gamma\n"),
        ("delta-0.1.0/README.md", "public domain, no text\n"),
        ("buildonly-1.0.0/README.md", "no licence file\n"),
        ("devonly-1.0.0/README.md", "no licence file\n"),
        ("epsilon-1.0.0/README.md", "no licence file\n"),
        ("zeta-1.0.0/README.md", "no licence file\n"),
    ] {
        put(&registry.join(name), content);
    }
    let id = |name: &str, version: &str| {
        format!("registry+https://github.com/rust-lang/crates.io-index#{name}@{version}")
    };
    let own =
        |folder: &str, name: &str| format!("path+file://{}/{folder}#{name}@0.0.1", root.display());
    let package = |name: &str, version: &str, license: Value, license_file: Value| {
        json!({
            "name": name,
            "version": version,
            "id": id(name, version),
            "license": license,
            "license_file": license_file,
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": registry.join(format!("{name}-{version}/Cargo.toml")),
        })
    };
    let dependency = |pkg: String, kinds: &[Option<&str>]| {
        let kinds: Vec<Value> = kinds
            .iter()
            .map(|kind| json!({"kind": kind, "target": null}))
            .collect();
        json!({"name": "x", "pkg": pkg, "dep_kinds": kinds})
    };
    let node = |id: String, deps: Vec<Value>| json!({"id": id, "deps": deps, "features": []});
    let graph = json!({
        "packages": [
            package("zeta", "1.0.0", json!("MIT"), Value::Null),
            package("gamma", "3.0.0-rc.1+build", Value::Null, json!("COPYRIGHT.txt")),
            package("epsilon", "1.0.0", json!("MIT"), Value::Null),
            package("devonly", "1.0.0", json!("MIT"), Value::Null),
            package("delta", "0.1.0", json!("Unlicense"), Value::Null),
            package("buildonly", "1.0.0", json!("MIT"), Value::Null),
            package("beta", "0.2.0", json!("Apache-2.0"), Value::Null),
            package("alpha", "1.0.0", json!("MIT OR Apache-2.0"), Value::Null),
            {
                "name": "fenix-patch", "version": "0.0.1", "id": own("installer", "fenix-patch"),
                "license": "MIT", "license_file": null, "source": null,
                "manifest_path": root.join("installer/Cargo.toml"),
            },
            {
                "name": "fenix-tools", "version": "0.0.1", "id": own("tools", "fenix-tools"),
                "license": "MIT", "license_file": null, "source": null,
                "manifest_path": root.join("tools/Cargo.toml"),
            },
        ],
        "workspace_members": [own("installer", "fenix-patch"), own("tools", "fenix-tools")],
        "resolve": {
            "nodes": [
                node(own("installer", "fenix-patch"), vec![
                    dependency(id("alpha", "1.0.0"), &[None]),
                    dependency(id("beta", "0.2.0"), &[Some("build"), None]),
                    dependency(id("buildonly", "1.0.0"), &[Some("build")]),
                    dependency(id("devonly", "1.0.0"), &[Some("dev")]),
                ]),
                node(own("tools", "fenix-tools"), vec![
                    dependency(own("installer", "fenix-patch"), &[None]),
                    dependency(id("epsilon", "1.0.0"), &[None]),
                ]),
                node(id("alpha", "1.0.0"), vec![dependency(id("gamma", "3.0.0-rc.1+build"), &[None])]),
                node(id("beta", "0.2.0"), vec![dependency(id("alpha", "1.0.0"), &[None])]),
                node(id("gamma", "3.0.0-rc.1+build"), vec![dependency(id("delta", "0.1.0"), &[None])]),
                node(id("delta", "0.1.0"), vec![]),
                node(id("buildonly", "1.0.0"), vec![dependency(id("zeta", "1.0.0"), &[None])]),
                node(id("devonly", "1.0.0"), vec![]),
                node(id("epsilon", "1.0.0"), vec![]),
                node(id("zeta", "1.0.0"), vec![]),
            ],
            "root": null,
        },
        "version": 1,
    });
    put(&side.join("cargo-metadata.json"), json::dumps(&graph, None));
    graph
}
