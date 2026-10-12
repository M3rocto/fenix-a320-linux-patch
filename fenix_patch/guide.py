# SPDX-License-Identifier: MIT
"""What has been done, and the one thing to do next. Shared by both front ends."""
from pathlib import Path
import shutil
import subprocess
import urllib.parse

from . import core

DASHBOARD = "https://fenixsim.com/dashboard/"


def settings_ready(target):
    # Fenix writes this file at sign-in. persistancy.xml can follow later and has its own step.
    return (target.prefix / core.CONFIG / "fenixConfig.xml").is_file()


def installers():
    """Fenix installers the user has probably just downloaded, newest first."""
    found = []
    for folder in (Path.home() / "Downloads", Path.home() / "Desktop", Path.home()):
        try:
            found += [path for path in folder.iterdir()
                      if path.suffix.lower() == ".exe" and "fenix" in path.name.lower() and path.is_file()]
        except OSError:
            continue
    return [str(path) for path in sorted(found, key=lambda path: path.stat().st_mtime, reverse=True)]


def pick_file():
    """A native file dialog when the desktop offers one; None when cancelled or unavailable."""
    for command in (["zenity", "--file-selection", "--title=Official Fenix Installer", "--file-filter=Windows installer | *.exe *.EXE"],
                    ["kdialog", "--getopenfilename", str(Path.home() / "Downloads"), "*.exe"]):
        if shutil.which(command[0]):
            result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            return result.stdout.decode().strip() or None
    return None


def can_pick_file():
    return bool(shutil.which("zenity") or shutil.which("kdialog"))


def overview(target):
    """Steps in order; exactly one is current unless everything is done or setup is blocked."""
    status = target.status()
    state = status.get("state")
    steam = target.kind == "steam"
    broken = state not in ("available", "installed", "legacy")
    patched = status.get("installed") or state == "legacy"
    update = bool(status.get("update_available"))
    steps = [{
        "id": "install", "title": "Install the compatibility patch", "done": bool(patched),
        "text": ("Copies the simulator's Windows profile, installs Microsoft .NET Framework 4.8 and the Fenix fixes. "
                 "The original profile is kept for Restore. " +
                 ("A separate Proton for the simulator is set up; your other games are not affected. " if steam else "") +
                 "Takes 5–15 minutes and needs an internet connection."),
        "button": "Install patch",
    }]
    if update:
        # The later steps stay as they were and count again once the update is in.
        steps[0].update(title="Update the compatibility patch", done=False, button="Update patch",
                        text="Patch %s is installed; this installer brings %s. Fenix, your sign-in and its settings "
                             "are kept. Close the simulator and Fenix first."
                             % (status.get("installed_version"), status.get("version")))
    if steam:
        running = status.get("steam_running")
        steps.append({
            "id": "select", "title": "Let Steam use the Fenix Proton", "done": bool(status.get("tool_selected")),
            "text": ("Steam is running. Quit it completely (Steam → Exit), then continue. " if running else "") +
                    "The installer sets “%s” for %s, as Properties → Compatibility would. Only this simulator is changed."
                    % (status.get("tool_title", "Proton Fenix A320"), target.title),
            "button": "Check again" if running else "Set it in Steam",
            "note": "By hand: restart Steam, open the simulator's Properties → Compatibility, force “%s”."
                    % status.get("tool_title", "Proton Fenix A320"),
        })
    steps += [{
        "id": "installer", "title": "Run the official Fenix Installer", "done": bool(status.get("fenix_installed")),
        "text": "Download the installer from your Fenix account, choose the file here and complete it as on Windows, "
                "including the prerequisites it offers. Close it when it has finished.",
        "button": "Run Fenix Installer", "needs_file": True, "link": DASHBOARD,
    }, {
        "id": "open", "title": "Open Fenix and sign in", "done": bool(status.get("fenix_installed")) and settings_ready(target),
        "text": "Sign in and activate your aircraft. Then close Fenix; its settings are needed for the next step.",
        "button": "Open Fenix",
    }, {
        "id": "configure", "title": "Apply the display settings", "done": status.get("configured") is True,
        "text": "Sets CPU rendering and Legacy FCU readouts, the configuration the cockpit displays were verified with, "
                "and starts Fenix together with the simulator.",
        "button": "Apply settings",
    }]
    if status.get("configured") is True and status.get("readouts") is False:
        steps.append({
            "id": "readouts", "title": "Set the Legacy FCU readouts", "done": False,
            "text": "Fenix has not created the file for this setting yet. Start %s %s, load the Fenix A320 once and "
                    "close the simulator. Then apply the setting; CPU rendering and autostart are already in place."
                    % (target.title.split(" on ")[0], "from Steam" if steam else "through Flightdeck"),
            "button": "Apply setting",
        })
    if state == "legacy":
        # An earlier local setup is active and stays as it is.
        for step in steps:
            step["done"] = True
    current = None
    for step in steps:
        if not broken and current is None and not step["done"]:
            current = step["id"]
        # A later step cannot be complete before an earlier one.
        step["done"] = step["done"] and current is None
    message = status.get("message", "")
    if broken:
        message = message or "A previous setup did not finish. Restore the original profile, then install again."
    elif status.get("idle") is False:
        message = "The simulator or Fenix is running. Close them before continuing."
    return {"title": target.title, "kind": target.kind, "steps": steps, "current": current, "broken": broken,
            "message": message, "can_restore": bool(status.get("can_restore")), "complete": not broken and current is None,
            "finish": "Start %s %s. Fenix starts with it." % (target.title.split(" on ")[0],
                                                             "from Steam" if steam else "through Flightdeck"),
            "update": update}


def perform(target, action, bundle, progress, executable=None, proton=None):
    if action == "install":
        target.install(bundle, progress, proton)
    elif action == "select":
        target.select(progress)
    elif action == "installer":
        # Paths are often pasted with quotes or as a file:// address.
        executable = (executable or "").strip().strip("'\"")
        if executable.startswith("file://"):
            executable = urllib.parse.unquote(urllib.parse.urlparse(executable).path)
        if not executable:
            raise core.PatchError("Choose the Fenix Installer file first.")
        if not Path(executable).expanduser().is_file():
            raise core.PatchError("This file does not exist: " + executable)
        target.app(progress, executable)
        progress("Fenix Installer closed.")
    elif action == "open":
        target.app(progress)
        progress("Fenix closed.")
    elif action == "manager":
        target.app(progress, manager=True)
    elif action in ("configure", "readouts"):
        target.configure(progress)
    elif action == "restore":
        target.restore(progress)
    else:
        raise core.PatchError("Unknown action")


def text_wizard(found, bundle, proton=None):
    """The same guided setup in a terminal."""
    if not found:
        raise core.PatchError("No simulator was found. Start MSFS once in Steam, or pass --steam-root or --runtime.")
    target = found[0]
    if len(found) > 1:
        for number, item in enumerate(found, 1):
            print("  %d  %s" % (number, item.label))
        choice = input("Which simulator? [1] ").strip() or "1"
        if not choice.isdecimal() or not 1 <= int(choice) <= len(found):
            raise core.PatchError("No such simulator.")
        target = found[int(choice) - 1]
    while True:
        view = overview(target)
        print("\nFenix A320 Linux Patch · " + view["title"])
        for number, step in enumerate(view["steps"], 1):
            mark = "✓" if step["done"] else "→" if step["id"] == view["current"] else " "
            print(" %s %d. %s" % (mark, number, step["title"]))
        if view["message"]:
            print("\n! " + view["message"])
        step = next((item for item in view["steps"] if item["id"] == view["current"]), None)
        if view["complete"]:
            print("\nAll done. " + view["finish"])
        elif step:
            print("\n" + step["text"])
            if step.get("note"):
                print(step["note"])
            if step.get("link"):
                print(step["link"])
        prompt = "[Enter] %s · " % step["button"] if step else ""
        answer = input("\n" + prompt + ("[r] Restore · " if view["can_restore"] else "") + "[q] Quit: ").strip().lower()
        if answer == "q":
            return
        action = "restore" if answer == "r" and view["can_restore"] else step["id"] if step and not answer else None
        if action is None:
            continue
        executable = None
        if action == "installer":
            detected = installers()
            executable = input("Fenix Installer file%s: " % (" [%s]" % detected[0] if detected else "")).strip() or \
                (detected[0] if detected else None)
        try:
            perform(target, action, bundle, print, executable, proton)
        except (core.PatchError, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
            print("\nError: " + str(error))
