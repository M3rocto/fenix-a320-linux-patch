#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Rebuild a Fenix overlay against one exact, pinned Proton Wine revision."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import urllib.request

from build import CONFIGURE, ROOT, TARGETS, extract, sha


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("variant")
    parser.add_argument("--build-dir", required=True, type=Path)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--record-build", action="store_true")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 2, 8))
    args = parser.parse_args()
    lock = json.loads((ROOT / "bundle.json").read_text())
    entry = lock["variants"][args.variant]
    build = args.build_dir.resolve()
    if build.exists():
        raise ValueError("Use a fresh build directory")
    source = build / "wine"
    name, expected = next(iter(entry["sources"].items()))
    archive = ROOT / "sources" / name
    if not archive.exists():
        archive.parent.mkdir(parents=True, exist_ok=True)
        url = "https://codeload.github.com/" + entry["wine_repository"] + "/tar.gz/" + entry["wine_revision"]
        with urllib.request.urlopen(url, timeout=60) as response, archive.open("xb") as output:
            shutil.copyfileobj(response, output)
    if sha(archive) != expected:
        raise ValueError("Wine source checksum mismatch")
    extract(archive, source)
    env = dict(os.environ, XDG_CACHE_HOME=str(build / "cache"))
    def run(command, cwd=source):
        subprocess.run(command, cwd=cwd, env=env, check=True)
    for name, expected in entry["patches"].items():
        patch = ROOT / "patches" / name
        if sha(patch) != expected:
            raise ValueError("Proton port checksum mismatch")
        run(["patch", "--batch", "--fuzz=0", "-p1", "-i", str(patch)])
    # Proton regenerates the archived protocol headers during its build. Never
    # copy Xodus's generated requests or change the selected runner's protocol.
    run(["./tools/make_requests"])
    protocol = re.search(r"#define SERVER_PROTOCOL_VERSION (\d+)",
                         (source / "include/wine/server_protocol.h").read_text())
    if not protocol or int(protocol[1]) != entry["protocol"]:
        raise ValueError("Unexpected Wine server protocol")
    if args.prepare_only:
        return
    run(["./tools/make_specfiles"])
    run(["python3", "dlls/winevulkan/make_vulkan", "--xml", str(source / "dlls/winevulkan/vk.xml"),
         "--video-xml", str(source / "dlls/winevulkan/video.xml")])
    run(["autoreconf", "-f"])
    output = build / "objects"
    output.mkdir()
    flags = "-O2 -ffile-prefix-map=" + str(build) + "=/usr/src/fenix-proton"
    run([str(source / "configure"), *CONFIGURE, "CFLAGS=" + flags, "CROSSCFLAGS=" + flags], output)
    run(["make", "depend"], output)
    run(["make", "-j" + str(args.jobs), *TARGETS], output)
    hashes = {}
    for target, relative in TARGETS.items():
        destination = ROOT / "payload/variants" / args.variant / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(output / target, destination)
        run(["x86_64-w64-mingw32-strip" if destination.suffix == ".dll" else "strip",
             "--strip-debug", str(destination)])
        hashes[relative] = sha(destination)
    if args.record_build:
        entry["files"] = hashes
        (ROOT / "bundle.json").write_text(json.dumps(lock, indent=2) + "\n")
    elif hashes != entry["files"]:
        raise ValueError("Build hashes differ; review before recording a new build")


if __name__ == "__main__":
    main()
