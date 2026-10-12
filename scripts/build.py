#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Rebuild the Wine modules from the exact archived sources and patches."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "dlls/ntdll/x86_64-windows/ntdll.dll": "files/lib/wine/x86_64-windows/ntdll.dll",
    "dlls/ntdll/ntdll.so": "files/lib/wine/x86_64-unix/ntdll.so",
    "server/wineserver": "files/bin/wineserver",
    **{f"dlls/{n}/x86_64-windows/{n}.dll": f"files/lib/wine/x86_64-windows/{n}.dll" for n in ("crypt32", "mmdevapi", "kernelbase", "d2d1", "dwrite")},
    "dlls/dwrite/dwrite.so": "files/lib/wine/x86_64-unix/dwrite.so",
    "dlls/win32u/win32u.so": "files/lib/wine/x86_64-unix/win32u.so",
    "dlls/winex11.drv/winex11.so": "files/lib/wine/x86_64-unix/winex11.so",
}
# configure links ntdll.so against libunwind when the build host has it. The
# pinned runners do not, and neither Steam Linux Runtime ships the library.
CONFIGURE = ("--enable-win64", "--disable-tests", "--enable-silent-rules", "--without-ffmpeg", "--without-unwind")


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def extract(archive, target):
    target.mkdir(parents=True)
    with tarfile.open(archive) as source:
        for item in source.getmembers():
            parts = Path(item.name).parts[1:]
            if not parts: continue
            if ".." in parts or item.name.startswith("/") or not (item.isfile() or item.isdir() or item.issym()):
                raise ValueError("Unexpected member in upstream source archive: " + item.name)
            path = target.joinpath(*parts)
            if not path.parent.resolve().is_relative_to(target.resolve()):
                raise ValueError("Source archive path escapes its root")
            if item.isdir(): path.mkdir(parents=True, exist_ok=True)
            elif item.issym():
                if Path(item.linkname).is_absolute() or not (path.parent / item.linkname).resolve().is_relative_to(target.resolve()):
                    raise ValueError("Source archive symlink escapes its root")
                path.parent.mkdir(parents=True, exist_ok=True)
                path.symlink_to(item.linkname)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(item) as stream, path.open("xb") as output: shutil.copyfileobj(stream, output)
                path.chmod(item.mode & 0o777)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--build-dir", type=Path, default=ROOT / "build", help="Fresh source/object directory")
    parser.add_argument("--record-build", action="store_true", help="Update payload hashes after rebuilding; review and repin consumers before publishing")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 2, 8))
    args = parser.parse_args()
    lock = json.loads((ROOT / "bundle.json").read_text())
    sources = ROOT / "sources"; sources.mkdir(exist_ok=True)
    urls = {"wine-source.tar.gz": "https://codeload.github.com/xodus-gaming/wine/tar.gz/" + lock["wine_revision"],
            "xgameruntime-source.tar.gz": "https://codeload.github.com/xodus-gaming/xgameruntime/tar.gz/" + lock["xgameruntime_revision"]}
    for name, expected in lock["sources"].items():
        path = sources / name
        if not path.exists():
            with urllib.request.urlopen(urls[name], timeout=60) as response, path.open("xb") as output: shutil.copyfileobj(response, output)
        if sha(path) != expected: raise ValueError("Source checksum mismatch: " + name)
    build = args.build_dir.resolve(); build.mkdir(parents=True, exist_ok=True)
    source = build / "wine"
    if source.exists(): raise ValueError("Use an empty build directory; existing work is never overwritten")
    extract(sources / "wine-source.tar.gz", source)
    submodule = source / "dlls/xgameruntime"
    if submodule.exists(): submodule.rmdir()
    extract(sources / "xgameruntime-source.tar.gz", submodule)
    for name, expected in lock["patches"].items():
        path = ROOT / "patches" / name
        if sha(path) != expected: raise ValueError("Patch checksum mismatch: " + name)
        subprocess.run(["patch", "--batch", "--fuzz=0", "-p1", "-i", str(path)], cwd=source, check=True)
    if args.prepare_only: return
    env = dict(os.environ, XDG_CACHE_HOME=str(build / "cache"))
    run = lambda command, cwd=source: subprocess.run(command, cwd=cwd, env=env, check=True)
    # The protocol 931 generated header/trace are supplied by the patch. Do not
    # let make_requests increment the ABI used by the pinned 32-bit clients.
    run(["./tools/make_specfiles"])
    run(["python3", "dlls/winevulkan/make_vulkan", "--xml", str(source / "dlls/winevulkan/vk.xml"), "--video-xml", str(source / "dlls/winevulkan/video.xml")])
    run(["autoreconf", "-f"])
    output = build / "objects"; output.mkdir()
    flags = ("-g -O2 -ffile-prefix-map=" + str(ROOT) + "=/usr/src/fenix-a320-linux-patch"
             " -ffile-prefix-map=" + str(build) + "=/usr/src/fenix-a320-linux-patch/build")
    run([str(source / "configure"), *CONFIGURE, "CFLAGS=" + flags, "CROSSCFLAGS=" + flags], output)
    run(["make", "depend"], output)
    run(["make", "-j" + str(args.jobs), *TARGETS], output)
    for target, relative in TARGETS.items():
        file = ROOT / "payload" / relative; file.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(output / target, file)
        run(["x86_64-w64-mingw32-strip" if file.suffix == ".dll" else "strip", "--strip-debug", str(file)])
    run(["x86_64-w64-mingw32-gcc", "-Os", "-s", "-municode", "-mwindows", "-Wl,--no-insert-timestamp",
         "-o", str(ROOT / "integration/FenixWindowGuard.exe"), str(ROOT / "native/window-guard.c"), "-luser32"])
    run(["x86_64-w64-mingw32-gcc", "-Os", "-s", "-municode", "-Wl,--no-insert-timestamp",
         "-o", str(ROOT / "integration/FenixGeometrySetup.exe"), str(ROOT / "native/geometry-setup.c"), "-lsetupapi"])
    run(["x86_64-w64-mingw32-gcc", "-Os", "-s", "-municode", "-Wl,--no-insert-timestamp",
         "-o", str(ROOT / "integration/FenixMCDURefresh.exe"), str(ROOT / "native/mcdu-refresh.c")])
    current = {name: sha(ROOT / "payload" / name) for name in lock["files"]}
    integration = {name: sha(ROOT / "integration" / name) for name in lock["integration"]}
    if args.record_build:
        lock.update(files=current, integration=integration)
        lock.setdefault("prefix_files", {})["drive_c/windows/system32/FenixWindowGuard.exe"] = integration["FenixWindowGuard.exe"]
        for name in ("FenixMCDURefresh.exe", "fenix-display-refresh.py"):
            lock["prefix_files"]["drive_c/windows/system32/" + name] = integration[name]
        (ROOT / "bundle.json").write_text(json.dumps(lock, indent=2) + "\n")
    elif current != lock["files"] or integration != lock["integration"]:
        raise ValueError("Build finished with different hashes. Compiler/platform/timestamps can differ; review before --record-build and repin Flightdeck.")


if __name__ == "__main__": main()
