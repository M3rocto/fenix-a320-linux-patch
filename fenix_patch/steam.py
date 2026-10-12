# SPDX-License-Identifier: MIT
"""Transactional installation for the Steam editions of MSFS 2020 and 2024.

Steam updates its own Proton builds, while the overlay belongs to one exact
Wine build. A separate compatibility tool is therefore created from a pinned
Proton and selected only for the simulator. The simulator's Windows profile is
prepared in a copy; the original profile is retained for Restore.
"""
from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import datetime
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile
import uuid

from . import core, vdf
from .core import PatchError, atomic, contained, digest, read_json, regular, write_json

GAMES = {
    "msfs2020": {"appid": "1250410", "title": "Microsoft Flight Simulator (2020)",
                 "settings": "Microsoft Flight Simulator", "executable": "FlightSimulator.exe"},
    "msfs2024": {"appid": "2537590", "title": "Microsoft Flight Simulator 2024",
                 "settings": "Microsoft Flight Simulator 2024", "executable": "FlightSimulator2024.exe"},
}
STATE = "fenix-a320-linux-patch"
TOOL_MARKER = "fenix-a320-tool.json"
# Exact public builds that an overlay in bundle.json was compiled for, in order
# of preference: the Wine 11 generation that Proton Experimental currently uses,
# then the earlier Wine 10 build.
PROTON_DOWNLOADS = {
    "cachyos-11": (
        "https://github.com/CachyOS/proton-cachyos/releases/download/cachyos-11.0-20261005-slr/proton-cachyos-11.0-20261005-slr-x86_64.tar.xz",
        "096bfe73b506d6565b04ecc45214197a4091818f16ed91f5324b4d2082d0a263",
        "proton-cachyos-11.0-20261005-slr-x86_64"),
    "cachyos-10": (
        "https://github.com/CachyOS/proton-cachyos/releases/download/cachyos-10.0-sunset-slr/proton-cachyos-10.0-sunset-slr-x86_64.tar.xz",
        "d6e1baa66937ed58e854b2013c0ef3b3f1fbc51d7be09e34a5ada3442ea9452d",
        "proton-cachyos-10.0-sunset-slr-x86_64"),
}
# The simulator runs with the configuration the overlay was verified with.
GAME_ENV = {
    "WINE_TRACK_WRITECOPY": "apps:Fenix.exe,FenixSystem.exe,FenixDisplay.exe,FenixCDU.exe,FlightSimulator2024.exe,FlightSimulator.exe",
    "WINE_D2D1_DISPLAY_EFFECTS": core.ENV["WINE_D2D1_DISPLAY_EFFECTS"],
    "WINE_D2D1_GEOMETRY_PROVIDER": core.ENV["WINE_D2D1_GEOMETRY_PROVIDER"],
    "WINE_DWRITE_UNHINTED_OUTLINES": core.ENV["WINE_DWRITE_UNHINTED_OUTLINES"],
    "WINE_FENIX_HELPER_WINDOWS": "1",
    "DOTNET_SYSTEM_GLOBALIZATION_USENLS": "1", "DOTNET_ReadyToRun": "0",
    "PROTON_NO_ESYNC": "1", "PROTON_NO_FSYNC": "1",
}


@dataclass(frozen=True)
class Target:
    game: str
    root: Path      # Steam client directory
    library: Path   # Steam library containing the simulator

    @property
    def appid(self):
        return GAMES[self.game]["appid"]

    @property
    def title(self):
        return GAMES[self.game]["title"]

    @property
    def compat(self):
        return self.library / "steamapps/compatdata" / self.appid

    @property
    def prefix(self):
        return self.compat / "pfx"

    @property
    def state(self):
        return self.library / "steamapps" / STATE / self.appid

    @property
    def marker(self):
        return self.state / "state.json"

    @property
    def tools(self):
        return self.root / "compatibilitytools.d"


def steam_roots(explicit=None):
    if explicit:
        root = Path(explicit).expanduser().resolve(strict=True)
        if not (root / "steamapps").is_dir():
            raise PatchError("This is not a Steam directory: it has no steamapps folder.")
        return [root]
    home = Path.home()
    roots = []
    for candidate in (home / ".local/share/Steam", home / ".steam/root", home / ".steam/steam",
                      home / ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                      home / "snap/steam/common/.local/share/Steam"):
        try:
            root = candidate.resolve(strict=True)
        except OSError:
            continue
        if (root / "steamapps").is_dir() and root not in roots:
            roots.append(root)
    return roots


def libraries(root):
    found = [root]
    try:
        text = regular(root / "steamapps/libraryfolders.vdf", 1024 * 1024).read_text(errors="replace")
    except (OSError, PatchError):
        return found
    for value in re.findall(r'"path"\s+"((?:[^"\\]|\\.)*)"', text):
        try:
            path = Path(re.sub(r"\\(.)", r"\1", value)).resolve(strict=True)
        except OSError:
            continue  # An unplugged library
        if (path / "steamapps").is_dir() and path not in found:
            found.append(path)
    return found


def targets(steam_root=None):
    """Installed Steam simulators whose Windows profile or patch state exists."""
    found = []
    for root in steam_roots(steam_root):
        for library in libraries(root):
            for game, info in GAMES.items():
                target = Target(game, root, library)
                # An interrupted exchange leaves no profile in place for a
                # moment; its patch state must still be found for Restore.
                if (library / "steamapps" / ("appmanifest_%s.acf" % info["appid"])).is_file() and \
                        ((target.prefix / "system.reg").is_file() or target.marker.is_file()) and \
                        not any(item.compat == target.compat for item in found):
                    found.append(target)
    return found


def target(game, steam_root=None):
    if game not in GAMES:
        raise PatchError("Unknown simulator: " + str(game))
    found = [item for item in targets(steam_root) if item.game == game]
    if not found:
        raise PatchError("%s was not found in Steam. Install it and start it once, so that Steam creates its "
                         "Windows profile, then close it." % GAMES[game]["title"])
    if len(found) > 1:
        raise PatchError("%s exists in several Steam installations. Choose one with --steam-root." % GAMES[game]["title"])
    return found[0] if found[0].marker.is_file() else checked(found[0])


def checked(item, *, recovery=False):
    for path in (item.compat,) + (() if recovery else (item.prefix, item.prefix / "drive_c",
                 item.prefix / "drive_c/windows", item.prefix / "drive_c/windows/system32")):
        if path.is_symlink() or not path.is_dir() or path.stat().st_uid != os.getuid():
            raise PatchError("The Steam profile needs owned, ordinary directories: " + path.name)
    if not recovery:
        regular(item.prefix / "system.reg")
    return item


@contextmanager
def locked(item, *, idle=True, recovery=False):
    if not recovery or item.compat.exists():
        checked(item, recovery=recovery)
    item.state.mkdir(mode=0o700, parents=True, exist_ok=True)
    if item.state.is_symlink() or item.state.stat().st_uid != os.getuid():
        raise PatchError("Invalid patch state directory")
    fd = os.open(item.state / "lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
            raise PatchError("Invalid patch lock")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise PatchError("Another Fenix patch operation is using this simulator.") from None
        if idle and item.prefix.exists():
            core.ensure_idle(item.prefix)
        yield item
    finally:
        os.close(fd)


def tool_name(variant):
    return "Proton-Fenix-A320-" + variant


def tool_title(variant):
    version = re.sub(r"^\d+\s+", "", core.manifest(variant)["runner_version"])
    return "Proton Fenix A320 (%s)" % version


def tool_state(tool):
    path = tool / TOOL_MARKER
    return read_json(path) if path.is_file() and not path.is_symlink() else None


def is_proton(path):
    return (path / "proton").is_file() and (path / "version").is_file() and (path / "files/bin/wineserver").is_file()


def installed_protons(item):
    folders = [item.tools, Path("/usr/share/steam/compatibilitytools.d")]
    folders += [library / "steamapps/common" for library in libraries(item.root)]
    for folder in folders:
        try:
            entries = sorted(folder.iterdir())
        except OSError:
            continue
        for entry in entries:
            if entry.is_dir() and is_proton(entry) and not (entry / TOOL_MARKER).exists():
                yield entry


def pristine_variant(path):
    try:
        return core.runner_variant(path)
    except PatchError:
        return None


def extract_proton(archive, destination, folder):
    """Extract one pinned Proton archive; reject members outside its folder."""
    with tarfile.open(archive, "r:xz") as stream:
        members = stream.getmembers()
        if hasattr(tarfile, "data_filter"):
            stream.extractall(destination, members=members, filter="data")
        else:
            base = destination.resolve()
            for member in members:
                path = (base / member.name).resolve()
                link = (path.parent / member.linkname).resolve() if member.issym() else \
                    (base / member.linkname).resolve() if member.islnk() else base
                if not path.is_relative_to(base) or not link.is_relative_to(base) or member.isdev():
                    raise PatchError("Unsafe member in the Proton archive")
            stream.extractall(destination, members=members)
        # Proton ships its files read-only. Profiles link to them, so keep
        # that: a Windows installer must not write through into the tool.
        for member in members:
            if member.isreg():
                os.chmod(destination / member.name, member.mode & 0o555)
    result = destination / folder
    if not is_proton(result):
        raise PatchError("The Proton archive has an unexpected layout.")
    return result


def source_proton(item, explicit, progress, download_into):
    """An unmodified Proton that exactly matches one overlay, and its variant."""
    if explicit:
        path = Path(explicit).expanduser().resolve(strict=True)
        variant = pristine_variant(path) if is_proton(path) else None
        if variant is None:
            raise PatchError("This Proton build has no matching Fenix overlay. Supported: " + supported())
        return path, variant
    installed = {}
    for path in installed_protons(item):
        installed.setdefault(pristine_variant(path), path)
    installed.pop(None, None)
    available = core.manifest().get("variants", {})
    for variant, (url, sha, folder) in PROTON_DOWNLOADS.items():
        if variant not in available:
            continue
        if variant in installed:
            progress("Using the installed %s as the base for the Fenix Proton." % installed[variant].name)
            return installed[variant], variant
        progress("Downloading the pinned Proton build %s (about 350 MB) …" % folder)
        archive = core.download(url, item.state / "downloads" / url.rsplit("/", 1)[1], sha, max_size=1024 ** 3)
        progress("Extracting Proton …")
        path = extract_proton(archive, download_into, folder)
        core.verify_runner(path, core.manifest(variant))
        return path, variant
    for variant, path in installed.items():
        progress("Using the installed %s as the base for the Fenix Proton." % path.name)
        return path, variant
    raise PatchError("No supported Proton build is available. Supported: " + supported())


def supported():
    lock = core.manifest()
    return ", ".join(re.sub(r"^\d+\s+", "", entry["runner_version"]) for entry in lock.get("variants", {}).values())


def create_tool(source, tool, variant):
    """Copy an unmodified Proton to its final Steam location; the overlay follows later."""
    tool.parent.mkdir(parents=True, exist_ok=True)
    core.copy_tree(source, tool)
    write_json(tool / TOOL_MARKER, {"format": 1, "state": "staging", "variant": variant,
                                    "version": core.manifest()["version"]})
    name = tool.name
    atomic(tool / "compatibilitytool.vdf", (
        '"compatibilitytools"\n{\n  "compat_tools"\n  {\n    "%s"\n    {\n      "install_path" "."\n'
        '      "display_name" "%s"\n      "from_oslist"  "windows"\n      "to_oslist"    "linux"\n'
        '    }\n  }\n}\n' % (name, tool_title(variant))).encode(), 0o644)
    settings = "# Written by the Fenix A320 Linux Patch. Proton applies these to games using this tool.\n" \
               "user_settings = " + json.dumps(GAME_ENV, indent=4) + "\n"
    atomic(tool / "user_settings.py", settings.encode(), 0o644)


def verify_tool(tool, variant, version):
    state = tool_state(tool)
    if not state or state.get("state") != "ready" or state.get("variant") != variant or state.get("version") != version:
        raise PatchError("The Fenix Proton tool is incomplete or belongs to another patch version: " + tool.name)
    lock = core.manifest(variant)
    core.verify_runner(tool, {**lock, "runner_files": {**lock["runner_files"], **lock["files"]}})


def carried_directories(item):
    """Simulator content inside the profile: moved at commit instead of copied."""
    found = []
    users = item.prefix / "drive_c/users"
    for home in sorted(users.iterdir()) if users.is_dir() else ():
        folder = home / "AppData/Roaming" / GAMES[item.game]["settings"] / "Packages"
        chain = [home, home / "AppData", home / "AppData/Roaming", folder.parent, folder]
        if all(part.is_dir() and not part.is_symlink() for part in chain):
            found.append(str(folder.relative_to(item.compat)))
    return found


def copy_without(source, destination, excluded):
    """cp -a, except that excluded relative directories are left out."""
    if not excluded:
        core.copy_tree(source, destination)
        return
    destination.mkdir(mode=0o700)
    for child in sorted(source.iterdir()):
        below = {Path(*path.parts[1:]) for path in excluded if path.parts[0] == child.name}
        if Path() in below or Path(".") in below:
            continue
        if below and child.is_dir() and not child.is_symlink():
            copy_without(child, destination / child.name, below)
        else:
            core.copy_tree(child, destination / child.name)
    shutil.copystat(source, destination)


def tree_size(base, excluded=()):
    total = 0
    skip = {str(base / path) for path in excluded}
    for folder, directories, files in os.walk(base):
        directories[:] = [name for name in directories if os.path.join(folder, name) not in skip]
        for name in files:
            info = os.lstat(os.path.join(folder, name))
            if stat.S_ISREG(info.st_mode):
                total += info.st_size
    return total


def proton_setup(tool, compat, item, log):
    """Let this Proton create or upgrade the profile exactly as a Steam launch would."""
    env = {key: value for key, value in os.environ.items() if not key.startswith(("WINE", "STEAM_COMPAT", "PROTON_"))}
    env.update(STEAM_COMPAT_DATA_PATH=str(compat), STEAM_COMPAT_CLIENT_INSTALL_PATH=str(item.root))
    result = subprocess.run([str(tool / "proton"), "getcompatpath", "/"], env=env, cwd=compat,
                            stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, timeout=900)
    if result.returncode:
        raise PatchError("Proton could not prepare the Windows profile. See the Fenix setup log.")


def remove_shared_mono(wine):
    """Proton's shared Wine Mono registers itself as .NET Framework 4.

    Microsoft's setup then reports success without installing anything. Remove
    that registration and the builtin loader, as Winetricks does.
    """
    for key in (r"HKLM\Software\Microsoft\NET Framework Setup\NDP\v4", r"HKLM\Software\Microsoft\NET Framework Setup\NDP\v3.5",
                r"HKLM\Software\Microsoft\.NETFramework\v2.0.50727\SBSDisabled"):
        for view in ("/reg:64", "/reg:32"):
            wine.run("reg", "delete", key, "/f", view, accepted=(0, 1))
    for folder in ("system32", "syswow64"):
        contained(wine.prefix, "drive_c/windows/%s/mscoree.dll" % folder).unlink(missing_ok=True)
    # Proton links its placeholder framework files to read-only files in the
    # tool; Windows Installer cannot replace those and fails the whole setup.
    for folder in ("Framework", "Framework64"):
        directory = contained(wine.prefix, "drive_c/windows/Microsoft.NET/%s/v4.0.30319" % folder)
        for path in directory.iterdir() if directory.is_dir() and not directory.is_symlink() else ():
            if path.is_symlink():
                path.unlink()


def require_display():
    # Microsoft's .NET setup creates windows even when silent; under Wine 11
    # it waits forever when there is no display to create them on.
    if not (os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")):
        raise PatchError("Run the installation inside your desktop session: Microsoft's .NET setup does not finish without a display.")


def updatable(state):
    """An installed earlier release whose files this installer knows and can replace."""
    try:
        return state.get("state") == "installed" and \
            state.get("version") in core.manifest(state.get("variant")).get("previous_releases", {})
    except PatchError:
        return False


def update(item, bundle, state, progress):
    """Replace the overlay of an installed earlier release in place. The Windows profile stays as it is."""
    if state.get("target") != "steam" or state.get("game") != item.game:
        raise PatchError("The patch state belongs to another simulator.")
    variant = state["variant"]
    lock = core.manifest(variant)
    version = lock["version"]
    previous = lock["previous_releases"][state["version"]]
    tool = contained(item.tools, state["tool"])
    current = tool_state(tool) or {}
    # The tool may already carry this release: another simulator shares it, or an update was interrupted.
    if current.get("variant") != variant or current.get("state") not in ("ready", "updating") or \
            current.get("version") not in (state["version"], version):
        raise PatchError("The Fenix Proton tool does not belong to this installation: " + tool.name)
    if current["state"] == "ready":
        files = lock["files"] if current["version"] == version else previous["files"]
        core.verify_runner(tool, {**lock, "runner_files": {**lock["runner_files"], **files}})
    for name, sha in previous.get("prefix_files", {}).items():
        if digest(regular(contained(item.prefix, name))) != sha:
            raise PatchError("Installed Fenix dependency changed: " + name)
    for other in tool_users(item, tool.name):
        if other.prefix.exists():
            core.ensure_idle(other.prefix)
    progress("Updating the Fenix Proton from %s to %s …" % (state["version"], version))
    write_json(tool / TOOL_MARKER, {**current, "state": "updating", "version": version})
    core.apply_overlay(item.prefix, tool, bundle, variant)
    write_json(tool / TOOL_MARKER, {**current, "state": "ready", "version": version})
    updated = {**state, "version": version, "updated_from": state["version"]}
    verify_installed(item, updated)
    write_json(item.marker, updated)
    progress("Patch updated. The Windows profile, Fenix and its settings are unchanged.")


def install(item, bundle, progress=lambda _: None, proton=None):
    core.host_check()
    bundle = core.verify_bundle(bundle)
    version = core.manifest()["version"]
    with locked(item):
        if item.marker.exists():
            state = read_json(item.marker)
            if state.get("state") == "installed" and state.get("version") == version:
                verify_installed(item, state)
                progress("This patch version is already installed.")
                return
            if updatable(state):
                update(item, bundle, state, progress)
                return
            raise PatchError("A previous patch transaction exists. Restore it before reinstalling.")
        require_display()
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:8]
        work = item.state / ("work-" + stamp)
        ready = [path for path in sorted(item.tools.glob("Proton-Fenix-A320-*")) if (tool_state(path) or {}).get("state") == "ready"] \
            if not proton and item.tools.is_dir() else []
        carried = carried_directories(item)
        needed = tree_size(item.compat, carried) + 1024 ** 3
        if shutil.disk_usage(item.state).free < needed:
            raise PatchError("Not enough free space for the independent Windows profile copy.")
        state = {"format": 1, "target": "steam", "game": item.game, "version": version, "state": "preparing",
                 "work": work.name, "previous": "before-" + stamp, "carried": carried, "configured": False,
                 "tool": None, "tool_created": False, "variant": None}
        work.mkdir(mode=0o700)
        write_json(item.marker, state)
        try:
            if ready:
                tool = ready[0]
                variant = tool_state(tool)["variant"]
                verify_tool(tool, variant, version)
                state.update(tool=tool.name, variant=variant)
            else:
                source, variant = source_proton(item, proton, progress, work / "proton")
                tool = item.tools / tool_name(variant)
                if tool.exists() or tool.is_symlink():
                    raise PatchError("An unfinished Fenix Proton tool exists: %s. Use Restore, or remove that folder." % tool)
                if shutil.disk_usage(item.root).free < tree_size(source) + 1024 ** 3:
                    raise PatchError("Not enough free space for the Fenix Proton tool.")
                state.update(tool=tool.name, variant=variant, tool_created=True)
                write_json(item.marker, state)
                progress("Creating the separate Fenix Proton tool; your other Proton versions stay unchanged …")
                create_tool(source, tool, variant)
            write_json(item.marker, state)
            shutil.rmtree(work / "proton", ignore_errors=True)
            progress("Copying the Windows profile; the original remains available …")
            staged = work / "compat"
            copy_without(item.compat, staged, {Path(path) for path in carried})
            prefix = staged / "pfx"
            drive = prefix / "dosdevices/c:"
            drive.parent.mkdir(exist_ok=True)
            if drive.is_symlink():
                drive.unlink()
            drive.symlink_to("../drive_c")
            logpath = work / "setup.log"
            with logpath.open("xb") as log:
                logpath.chmod(0o600)
                # Settle Proton's own profile migration first: a later launch
                # then finds its expected version and keeps native .NET files.
                proton_setup(tool, staged, item, log)
                wine = core.Wine(prefix, tool, log)
                try:
                    if not core.has_framework(prefix):
                        remove_shared_mono(wine)
                    # As Windows XP, .NET 4.0 also installs its own dxva2/evr over
                    # Proton's read-only links. The simulator keeps Proton's media stack.
                    core.prepare_framework(wine, item.state / "downloads", progress, "win7")
                    progress("Preparing fonts, graphics dependencies and Fenix settings …")
                    core.graphics_and_fonts(prefix, tool, wine)
                    core.apply_settings(state, prefix, GAMES[item.game]["settings"])
                    core.prepare_geometry(wine, item.state / "downloads", bundle, progress)
                finally:
                    wine.stop()
            core.apply_overlay(prefix, tool, bundle, variant)
            write_json(tool / TOOL_MARKER, {**tool_state(tool), "state": "ready"})
            core.ensure_idle(item.prefix)
            state["state"] = "committing"
            write_json(item.marker, state)
            previous = item.state / state["previous"]
            os.rename(item.compat, previous)
            os.rename(staged, item.compat)
            for path in carried:
                os.rename(previous / path, item.compat / path)
            state["state"] = "installed"
            write_json(item.marker, state)
            progress("Patch installed. Next, let Steam use “%s” for %s: quit Steam and choose “select”, or restart "
                     "Steam and force it under Properties → Compatibility." % (tool_title(variant), item.title))
        except BaseException:
            progress("Setup did not finish. The original profile or its backup is retained. Use Restore.")
            raise


def verify_installed(item, state):
    if state.get("target") != "steam" or state.get("game") != item.game:
        raise PatchError("The patch state belongs to another simulator.")
    lock = core.manifest(state.get("variant"))
    if state.get("version") != lock["version"]:
        if updatable(state):
            raise PatchError("Patch %s is installed. Update it to %s first." % (state["version"], lock["version"]))
        raise PatchError("This installed patch version is not supported by the current installer.")
    tool = contained(item.tools, state["tool"])
    verify_tool(tool, state["variant"], state["version"])
    for name, sha in lock.get("prefix_files", {}).items():
        if digest(regular(contained(item.prefix, name))) != sha:
            raise PatchError("Installed Fenix dependency changed: " + name)


def tool_users(item, name):
    """Other simulators whose patch state refers to this tool."""
    users = []
    for other in targets(str(item.root)):
        if other.compat != item.compat and other.marker.is_file():
            try:
                if read_json(other.marker).get("tool") == name:
                    users.append(other)
            except (OSError, ValueError, PatchError):
                users.append(other)
    return users


def restore(item, progress=lambda _: None):
    with locked(item, idle=False, recovery=True):
        state = read_json(item.marker)
        previous = contained(item.state, state["previous"])
        if item.prefix.exists():
            core.ensure_idle(item.prefix)
        staged = contained(item.state, state["work"]) / "compat/pfx"
        if staged.exists():
            core.ensure_idle(staged)
        state["state"] = "restoring"
        write_json(item.marker, state)
        if previous.exists():
            if item.compat.exists():
                for path in state.get("carried", []):
                    source, destination = contained(item.compat, path), contained(previous, path)
                    if source.is_dir() and not source.is_symlink() and not destination.exists():
                        os.rename(source, destination)
                retained = item.state / ("retained-" + uuid.uuid4().hex)
                os.rename(item.compat, retained)
                state["retained"] = retained.name
                write_json(item.marker, state)
            os.rename(previous, item.compat)
        elif not item.compat.exists():
            raise PatchError("The original profile backup is missing; nothing was removed.")
        selection = ""
        if state.get("tool") and selected_tool(item) == state["tool"]:
            try:
                write_selection(item, state.get("tool_before"))
            except (OSError, ValueError, PatchError):
                selection = " Steam still has the Fenix Proton selected for the simulator: choose another one under Properties → Compatibility."
        removed = False
        if state.get("tool"):
            tool = contained(item.tools, state["tool"])
            # Only ever remove a tool carrying this installer's marker, and only when unused.
            if tool.is_dir() and not tool.is_symlink() and (tool / TOOL_MARKER).is_file() and not tool_users(item, tool.name):
                shutil.rmtree(tool)
                removed = True
        state["state"] = "restored"
        write_json(item.state / ("restored-" + uuid.uuid4().hex[:8] + ".json"), state)
        item.marker.unlink()
        progress(("Original Windows profile restored. The newer profile is retained in %s." % (item.state / state["retained"])
                  if state.get("retained") else
                  "The original Windows profile is in place. The unfinished copy and its setup log remain in %s."
                  % (item.state / state["work"])) +
                 (" The Fenix Proton tool was removed." if removed else "") + selection)


def installed_state(item):
    if not item.marker.exists():
        raise PatchError("Install the compatibility patch first.")
    state = read_json(item.marker)
    if state.get("state") != "installed":
        raise PatchError("Finish patch installation first, or use Restore.")
    verify_installed(item, state)
    return state


def configure(item, progress=lambda _: None):
    with locked(item):
        state = installed_state(item)
        backup = item.state / ("settings-" + uuid.uuid4().hex)
        backup.mkdir(mode=0o700)
        for path in (item.prefix / core.CONFIG).glob("*.xml"):
            if path.name in ("fenixConfig.xml", "persistancy.xml"):
                shutil.copy2(regular(path), backup / path.name)
        message = core.apply_settings(state, item.prefix, GAMES[item.game]["settings"])
        write_json(item.marker, state)
        if not state["configured"]:
            raise PatchError(core.NEEDS_SIGN_IN)
        progress(message)


def windows_app(item, executable=None, progress=lambda _: None, *, manager=False, wait=None):
    with locked(item):
        state = installed_state(item)
        core.run_app(item.prefix, contained(item.tools, state["tool"]), item.state / "fenix-app.log",
                     executable, progress, manager=manager, wait=wait)


def selected_tool(item):
    """The compatibility tool Steam currently forces for this simulator, if readable."""
    try:
        return vdf.compat_tool(regular(item.root / "config/config.vdf", 16 * 1024 * 1024).read_text(), item.appid)
    except (OSError, ValueError, PatchError):
        return None


def steam_running():
    for proc in Path("/proc").iterdir():
        try:
            if proc.name.isdecimal() and proc.stat().st_uid == os.getuid() and \
                    (proc / "comm").read_text().strip() == "steam":
                return True
        except OSError:
            continue
    return False


def write_selection(item, tool):
    """Change only this simulator's entry in Steam's configuration; Steam must be closed."""
    if steam_running():
        raise PatchError("Steam is running and would overwrite the selection. Quit Steam completely "
                         "(Steam → Exit), then try again.")
    path = regular(item.root / "config/config.vdf", 16 * 1024 * 1024)
    text = path.read_bytes().decode()
    changed = vdf.set_compat_tool(text, item.appid, tool)
    if changed != text:
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S")
        atomic(item.state / ("steam-config-before-%s.vdf" % stamp), text.encode(), 0o600)
        atomic(path, changed.encode(), stat.S_IMODE(path.stat().st_mode))


def select_tool(item, progress=lambda _: None):
    """Force the Fenix Proton for the simulator, as Properties → Compatibility does."""
    with locked(item, idle=False):
        state = installed_state(item)
        current = selected_tool(item)
        if current == state["tool"]:
            progress("Steam already uses the Fenix Proton for %s." % item.title)
            return
        write_selection(item, state["tool"])
        state["tool_before"] = current
        state["tool_selected_by_installer"] = True
        write_json(item.marker, state)
        progress("Steam will start %s with “%s”. You can start Steam again." % (item.title, tool_title(state["variant"])))


def snapshot(item):
    result = {"target": "steam", "game": item.game, "profile": str(item.compat), "state": "unavailable",
              "version": core.manifest()["version"], "configured": False, "installed": False,
              "fenix_installed": False, "can_restore": False, "message": ""}
    try:
        result["fenix_installed"] = (item.prefix / core.PROGRAM / "Fenix.exe").is_file()
        try:
            core.manager_path(item.prefix)
            result["manager_installed"] = True
        except (OSError, PatchError):
            result["manager_installed"] = False
        if item.marker.exists():
            state = read_json(item.marker)
            result.update(state=state.get("state", "interrupted"), installed=state.get("state") == "installed",
                          configured=state.get("configured") is True, readouts=core.readouts_set(state),
                          can_restore=True,
                          installed_version=state.get("version"), tool=state.get("tool"),
                          update_available=updatable(state))
            if result["installed"]:
                result["tool_selected"] = selected_tool(item) == state.get("tool")
                if not result["tool_selected"]:
                    result["message"] = "Steam does not use “%s” for %s yet." % (tool_title(state["variant"]), item.title)
                result["tool_title"] = tool_title(state["variant"])
                result["steam_running"] = steam_running()
        else:
            core.host_check()
            checked(item)
            result["state"] = "available"
        try:
            core.ensure_idle(item.prefix)
            result["idle"] = True
        except PatchError:
            result["idle"] = False
    except (OSError, ValueError, KeyError, PatchError) as error:
        result["message"] = str(error)
    return result
