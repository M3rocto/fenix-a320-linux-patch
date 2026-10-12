# SPDX-License-Identifier: MIT
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
import xml.etree.ElementTree as ET

from fenix_patch import core


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.root = self.base / "runtime with spaces"
        self.prefix = self.root / "local/msfs-prefix"
        self.bundle = self.base / "bundle"
        self.runner = self.base / "original runner"
        for path in (self.root / "private", self.root / "tools", self.prefix / "drive_c/windows/system32",
                     self.prefix / "dosdevices", self.runner / "files/bin", self.bundle / "integration"):
            path.mkdir(parents=True)
        self.put(self.root / "private/runtime.json", json.dumps({"game_id": "msfs2024"}))
        self.put(self.prefix / "system.reg", "original registry")
        self.put(self.prefix / "user-private.txt", "synthetic private state")
        (self.prefix / "dosdevices/c:").symlink_to(self.prefix / "drive_c")
        (self.root / "runner").symlink_to(self.runner)
        self.put(self.runner / "version", "fixture version")
        self.put(self.runner / "files/bin/wine", "fixture Wine")
        self.put(self.runner / "files/share/wine/fonts/tahoma.ttf", "fixture Tahoma")
        self.put(self.runner / "files/share/wine/fonts/tahomabd.ttf", "fixture Tahoma Bold")
        files = ("files/lib/wine/x86_64-windows/ntdll.dll", "files/bin/wineserver")
        for name in files:
            self.put(self.runner / name, "old " + name)
            self.put(self.bundle / "payload" / name, "new " + name)
        self.put(self.prefix / "drive_c/windows/system32/ntdll.dll", "old prefix DLL")
        for name in ("launch-msfs.sh", "xodus-wine-launch"):
            self.put(self.root / "tools" / name, "old script " + name)
            self.put(self.bundle / "integration" / name, "new script " + name)
        self.put(self.bundle / "integration/FenixWindowGuard.exe", "synthetic guard")
        self.put(self.bundle / "integration/FenixMCDURefresh.exe", "synthetic refresh helper")
        self.put(self.bundle / "integration/fenix-display-refresh.py", "synthetic refresh supervisor")
        self.lock = {"format": 1, "version": "fixture", "runner_version": "fixture version",
            "files": {name: core.digest(self.bundle / "payload" / name) for name in files},
            "runner_files": {name: core.digest(self.runner / name) for name in (*files, "files/bin/wine")},
            "integration": {p.name: core.digest(p) for p in (self.bundle / "integration").iterdir()},
            "accepted_scripts": {name: [core.digest(self.root / "tools" / name)] for name in ("launch-msfs.sh", "xodus-wine-launch")}}
        self.put(self.bundle / "bundle.json", json.dumps(self.lock))
        self.patches = [patch.object(core, "manifest", return_value=self.lock), patch.object(core, "host_check"),
                        patch.object(core, "Wine"), patch.object(core, "prepare_framework"), patch.object(core, "graphics_and_fonts"),
                        patch.object(core, "prepare_geometry")]
        for item in self.patches: item.start()

    def tearDown(self):
        for item in reversed(self.patches): item.stop()
        self.temp.cleanup()

    def put(self, path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)

    def test_install_is_isolated_idempotent_and_restorable(self):
        core.install(self.root, self.bundle)
        state = core.read_json(self.root / core.MARKER)
        self.assertEqual(state["state"], "installed")
        self.assertEqual((self.runner / "files/bin/wineserver").read_text(), "old files/bin/wineserver")
        self.assertEqual((self.prefix / "drive_c/windows/system32/ntdll.dll").read_text(), "new files/lib/wine/x86_64-windows/ntdll.dll")
        self.assertEqual((self.prefix / "drive_c/windows/system32/FenixMCDURefresh.exe").read_text(), "synthetic refresh helper")
        self.assertEqual((self.prefix / "drive_c/windows/system32/fenix-display-refresh.py").read_text(), "synthetic refresh supervisor")
        self.assertEqual(os.readlink(self.prefix / "dosdevices/c:"), "../drive_c")
        core.install(self.root, self.bundle)
        self.assertEqual(core.read_json(self.root / core.MARKER), state)
        self.put(self.prefix / "new-user-state", "retain this")
        core.restore(self.root)
        self.assertEqual((self.root / "runner").resolve(), self.runner)
        self.assertEqual((self.prefix / "system.reg").read_text(), "original registry")
        self.assertEqual((self.prefix / "drive_c/windows/system32/ntdll.dll").read_text(), "old prefix DLL")
        retained = list((self.root / "local").glob("msfs-prefix.fenix-retained-*"))
        self.assertEqual((retained[0] / "new-user-state").read_text(), "retain this")
        self.assertFalse((self.root / core.MARKER).exists())

    def test_wrong_runner_and_corrupt_bundle_change_nothing(self):
        self.put(self.runner / "files/bin/wine", "different ABI")
        with self.assertRaises(core.PatchError): core.install(self.root, self.bundle)
        self.assertFalse((self.root / core.MARKER).exists())
        self.put(self.bundle / "payload/files/bin/wineserver", "tampered")
        with self.assertRaises(core.PatchError): core.verify_bundle(self.bundle)

    def test_variant_install_and_restore_preserve_proton_selection(self):
        # Exercise manifest selection itself instead of mocking its result.
        self.patches[0].stop()
        with patch.object(core, "ROOT", self.bundle):
            entry = {key: self.lock[key] for key in ("runner_version", "runner_files", "files")}
            for name in entry["files"]:
                self.put(self.bundle / "payload/variants/experimental" / name, "new " + name)
            self.lock["runner_version"] = "another base runner"
            self.lock["variants"] = {"experimental": entry}
            self.put(self.bundle / "bundle.json", json.dumps(self.lock))
            selected = {"schema": 1, "runner": "local/proton-tests/" + "a" * 32 + "/runner",
                        "version": "fixture version", "base_runner": "/original/runner"}
            core.write_json(self.root / "private/proton-selection.json", selected)
            core.install(self.root, self.bundle)
            state = core.read_json(self.root / core.MARKER)
            self.assertEqual(state["variant"], "experimental")
            self.assertEqual(state["proton_before"], selected)
            active = core.read_json(self.root / "private/proton-selection.json")
            self.assertEqual(active["runner"], state["work"] + "/runner")
            self.assertEqual(active["base_runner"], selected["base_runner"])
            core.verify_installed(self.root, state)
            core.restore(self.root)
            self.assertEqual(core.read_json(self.root / "private/proton-selection.json"), selected)

    def test_classic_wine64_entry_point_is_selected(self):
        self.assertEqual(core.wine_binary(self.runner).name, "wine")
        self.put(self.runner / "files/bin/wine64", "64-bit loader")
        self.assertEqual(core.wine_binary(self.runner).name, "wine64")
        self.assertEqual(core.wine_env(self.prefix, self.runner)["WINE_DISABLE_FAST_SYNC"], "1")

    def test_configure_reports_the_readout_setting_separately(self):
        directory = self.prefix / "drive_c/users/test/AppData/Roaming/Microsoft Flight Simulator 2024"
        directory.mkdir(parents=True, exist_ok=True)
        self.assertEqual(core.configure_prefix(self.prefix), (False, False))
        self.put(self.prefix / core.PROGRAM / "Fenix.exe", "synthetic app")
        # Without its sign-in settings nothing counts as configured.
        self.put(self.prefix / core.CONFIG / "persistancy.xml", "<Settings/>")
        self.assertEqual(core.configure_prefix(self.prefix), (False, False))
        (self.prefix / core.CONFIG / "persistancy.xml").unlink()
        # Fenix writes persistancy.xml later; displays and autostart do not wait for it.
        self.put(self.prefix / core.CONFIG / "fenixConfig.xml", "<Config/>")
        self.assertEqual(core.configure_prefix(self.prefix), (True, False))
        self.assertFalse((self.prefix / core.CONFIG / "persistancy.xml").exists())
        self.assertEqual(ET.parse(self.prefix / core.CONFIG / "fenixConfig.xml").findtext("displayMode"), "CPU")
        self.assertIsNotNone(ET.parse(directory / "exe.xml").find("Launch.Addon"))
        state = {}
        self.assertIn("load the aircraft once", core.apply_settings(state, self.prefix))
        self.assertEqual(state, {"configured": True, "readouts": False})
        self.put(self.prefix / core.CONFIG / "persistancy.xml", "<Settings/>")
        self.assertIn("Legacy readouts", core.apply_settings(state, self.prefix))
        self.assertEqual(state, {"configured": True, "readouts": True})
        # States written before the separate key meant both settings.
        self.assertTrue(core.readouts_set({"configured": True}))
        self.assertFalse(core.readouts_set({"configured": False}))

    def prepare_upgrade(self):
        core.install(self.root, self.bundle)
        state = core.read_json(self.root / core.MARKER)
        state["configured"] = True
        core.write_json(self.root / core.MARKER, state)
        self.put(self.prefix / "aircraft-installed-later", "keep aircraft and account state")
        self.lock["previous_releases"] = {self.lock["version"]: {
            "files": dict(self.lock["files"]), "integration": dict(self.lock["integration"])}}
        self.lock["version"] = "fixture-next"
        for name in (*self.lock["files"], "files/lib/wine/x86_64-unix/win32u.so"):
            self.put(self.bundle / "payload" / name, "updated " + name)
            self.lock["files"][name] = core.digest(self.bundle / "payload" / name)
        self.put(self.bundle / "bundle.json", json.dumps(self.lock))
        return state

    def test_upgrade_keeps_aircraft_settings_and_original_restore_point(self):
        previous = self.prepare_upgrade()
        old_runner = (self.root / "runner").resolve()
        self.assertTrue(core.snapshot(self.root)["update_available"])
        core.prepare_framework.reset_mock()
        core.prepare_geometry.reset_mock()
        core.install(self.root, self.bundle)
        core.prepare_framework.assert_not_called()
        core.prepare_geometry.assert_called_once()
        state = core.read_json(self.root / core.MARKER)
        self.assertEqual(state["version"], "fixture-next")
        self.assertTrue(state["configured"])
        for key in ("backup", "previous_prefix", "previous_runner", "original_prefix_id"):
            self.assertEqual(state[key], previous[key])
        self.assertNotEqual((self.root / "runner").resolve(), old_runner)
        self.assertEqual((old_runner / "files/bin/wineserver").read_text(), "new files/bin/wineserver")
        self.assertEqual((self.prefix / "aircraft-installed-later").read_text(), "keep aircraft and account state")
        self.assertFalse(core.snapshot(self.root)["update_available"])
        core.verify_installed(self.root, state)
        core.restore(self.root)
        self.assertEqual((self.root / "runner").resolve(), self.runner)
        self.assertEqual((self.prefix / "system.reg").read_text(), "original registry")
        retained = list((self.root / "local").glob("msfs-prefix.fenix-retained-*"))
        self.assertTrue((retained[0] / "aircraft-installed-later").is_file())

    def test_upgrade_rejects_changed_previous_files_without_changing_state(self):
        previous = self.prepare_upgrade()
        self.put((self.root / "runner").resolve() / "files/bin/wineserver", "custom binary")
        with self.assertRaisesRegex(core.PatchError, "Installed patch file changed"):
            core.install(self.root, self.bundle)
        self.assertEqual(core.read_json(self.root / core.MARKER), previous)
        self.assertTrue((self.prefix / "aircraft-installed-later").is_file())

    def test_geometry_setup_failure_never_commits_the_staging_profile(self):
        previous = self.prepare_upgrade()
        original_runner = (self.root / "runner").resolve()
        with patch.object(core, "prepare_geometry", side_effect=core.PatchError("bad dependency checksum")):
            with self.assertRaisesRegex(core.PatchError, "bad dependency checksum"):
                core.install(self.root, self.bundle)
        self.assertEqual((self.root / "runner").resolve(), original_runner)
        self.assertEqual((self.prefix / "aircraft-installed-later").read_text(), "keep aircraft and account state")
        self.assertEqual(core.read_json(self.root / core.MARKER)["state"], "preparing")
        self.assertEqual(core.read_json(self.root / core.MARKER)["previous_prefix"], previous["previous_prefix"])

    def test_provider_integrity_is_checked_for_new_releases_only(self):
        core.install(self.root, self.bundle)
        state = core.read_json(self.root / core.MARKER)
        self.put(self.prefix / core.GEOMETRY_PATH, "fixture geometry")
        self.lock["prefix_files"] = {core.GEOMETRY_PATH: core.digest(self.prefix / core.GEOMETRY_PATH)}
        core.verify_installed(self.root, state)
        self.put(self.prefix / core.GEOMETRY_PATH, "changed geometry")
        with self.assertRaisesRegex(core.PatchError, "dependency changed"):
            core.verify_installed(self.root, state)

    def test_scoped_geometry_environment_replaces_inherited_global_override(self):
        with patch.dict(os.environ, {"WINE_D2D1_GEOMETRY_PROVIDER": "1", "WINESERVERSOCKET": "stale", "WINE_DLL_FILE_MAP": "stale"}):
            env = core.wine_env(self.prefix, self.runner)
        self.assertEqual(env["WINE_D2D1_GEOMETRY_PROVIDER"], "FenixDisplay.exe")
        self.assertNotIn("WINESERVERSOCKET", env)
        self.assertNotIn("WINE_DLL_FILE_MAP", env)

    def test_interrupted_upgrade_retains_current_profile_and_can_restore(self):
        self.prepare_upgrade()
        rename = os.rename
        def fail_commit(source, target):
            if Path(source).name == "prefix" and Path(target) == self.prefix:
                raise OSError("interrupted upgrade commit")
            return rename(source, target)
        with patch.object(core.os, "rename", side_effect=fail_commit):
            with self.assertRaisesRegex(OSError, "interrupted upgrade"):
                core.install(self.root, self.bundle)
        state = core.read_json(self.root / core.MARKER)
        self.assertEqual(state["state"], "committing")
        self.assertTrue((self.root / state["upgrade_previous_prefix"] / "aircraft-installed-later").is_file())
        core.restore(self.root)
        self.assertEqual((self.prefix / "system.reg").read_text(), "original registry")
        self.assertTrue((self.root / state["upgrade_previous_prefix"] / "aircraft-installed-later").is_file())

    def test_custom_scripts_are_not_overwritten(self):
        self.put(self.root / "tools/launch-msfs.sh", "user custom")
        with self.assertRaises(core.PatchError): core.install(self.root, self.bundle)
        self.assertEqual((self.root / "tools/launch-msfs.sh").read_text(), "user custom")

    def test_runtime_lock_excludes_install(self):
        with (self.root / "private/play.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(core.PatchError): core.install(self.root, self.bundle)
        self.assertFalse((self.root / core.MARKER).exists())

    def test_interactive_waiter_retains_runtime_lock_through_shutdown(self):
        core.install(self.root, self.bundle)
        self.put(self.prefix / core.PROGRAM / "Fenix.exe", "synthetic app")
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        def wait(process):
            self.assertIs(process, child)
            with (self.root / "private/play.lock").open("r+") as lock:
                with self.assertRaises(BlockingIOError): fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            process.terminate()
            process.wait(timeout=3)
            return 0
        try:
            with patch.object(core.subprocess, "Popen", return_value=child):
                core.windows_app(self.root, wait=wait)
            with core.locked(self.root): pass
        finally:
            if child.poll() is None: child.kill()
            child.wait(timeout=3)

    def test_failed_interactive_waiter_reaps_child_and_releases_lock(self):
        core.install(self.root, self.bundle)
        self.put(self.prefix / core.PROGRAM / "Fenix.exe", "synthetic app")
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            with patch.object(core.subprocess, "Popen", return_value=child):
                with self.assertRaisesRegex(RuntimeError, "wait failed"):
                    core.windows_app(self.root, wait=lambda _: (_ for _ in ()).throw(RuntimeError("wait failed")))
            self.assertIsNotNone(child.poll())
            with core.locked(self.root): pass
        finally:
            if child.poll() is None: child.kill()
            child.wait(timeout=3)

    def test_legacy_profile_can_open_installed_fenix_but_cannot_run_an_installer(self):
        self.put(self.root / "private/fenix-compat.json", "{}")
        self.put(self.prefix / core.PROGRAM / "Fenix.exe", "synthetic app")
        with patch.object(core.subprocess, "run") as run:
            core.windows_app(self.root)
            core.Wine.return_value.reg.assert_called_with(r"HKCU\Software\Wine\Explorer", "ShowSystray", "0", "REG_DWORD")
            self.assertEqual(run.call_args.args[0][-1], str(self.prefix / core.PROGRAM / "Fenix.exe"))
            self.assertTrue(any(call.args[-1] == "/reg:64" for call in core.Wine.return_value.run.call_args_list))
            run.reset_mock()
            with self.assertRaises(core.PatchError):
                core.windows_app(self.root, "/not-an-installed-app.exe")
            run.assert_not_called()

    def test_ui_fonts_repair_32_bit_only_registration_and_preserve_existing_files(self):
        self.put(self.prefix / "system.reg", '[Software\\\\Wow6432Node\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts]\n"Tahoma (TrueType)"="old-font.ttf"\n')
        existing = self.prefix / "drive_c/windows/Fonts/tahoma.ttf"
        self.put(existing, "user's existing Tahoma")
        wine = Mock()
        core.ensure_ui_fonts(self.prefix, self.runner, wine)
        self.assertEqual(existing.read_text(), "user's existing Tahoma")
        self.assertEqual((existing.parent / "tahomabd.ttf").read_text(), "fixture Tahoma Bold")
        self.assertEqual(wine.run.call_count, 2)
        self.assertEqual([c.args[c.args.index('/v') + 1] for c in wine.run.call_args_list],
                         ["Tahoma (TrueType)", "Tahoma Bold (TrueType)"])
        self.assertTrue(all(c.args[-1] == "/reg:64" for c in wine.run.call_args_list))

    def test_ui_fonts_leave_registered_fonts_unchanged(self):
        self.put(self.prefix / "system.reg", '[Software\\\\Microsoft\\\\Windows NT\\\\CurrentVersion\\\\Fonts] 1\n"Tahoma (TrueType)"="custom-regular.ttf"\n"Tahoma Bold (TrueType)"="custom-bold.ttf"\n')
        wine = Mock()
        core.ensure_ui_fonts(self.prefix, self.runner, wine)
        wine.run.assert_not_called()
        self.assertFalse((self.prefix / "drive_c/windows/Fonts").exists())

    def test_running_prefix_is_detected(self):
        process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(20)"], env={**os.environ, "WINEPREFIX": str(self.prefix)})
        try:
            with self.assertRaises(core.PatchError): core.install(self.root, self.bundle)
        finally:
            process.terminate(); process.wait(timeout=5)

    def test_failed_preparation_restores_original_without_losing_stage(self):
        with patch.object(core, "graphics_and_fonts", side_effect=core.PatchError("fixture failure")):
            with self.assertRaises(core.PatchError): core.install(self.root, self.bundle)
        self.assertEqual((self.prefix / "system.reg").read_text(), "original registry")
        core.restore(self.root)
        self.assertEqual((self.root / "runner").resolve(), self.runner)
        self.assertTrue(list((self.root / "local").glob("fenix-patch-*/prefix")))

    def test_recovery_between_prefix_renames(self):
        rename = os.rename
        def interrupted(source, target):
            if Path(source).name == "prefix" and Path(target) == self.prefix:
                raise OSError("power loss between renames")
            return rename(source, target)
        with patch.object(core.os, "rename", side_effect=interrupted):
            with self.assertRaises(OSError): core.install(self.root, self.bundle)
        self.assertFalse(self.prefix.exists())
        self.assertTrue(core.snapshot(self.root)["can_restore"])
        core.restore(self.root)
        self.assertEqual((self.prefix / "system.reg").read_text(), "original registry")

    def test_recovery_during_restore_is_repeatable(self):
        core.install(self.root, self.bundle)
        with patch.object(core, "replace_link", side_effect=OSError("restore interrupted")):
            with self.assertRaises(OSError): core.restore(self.root)
        core.restore(self.root)
        self.assertEqual((self.root / "runner").resolve(), self.runner)

    def test_symlink_parent_escape_is_rejected(self):
        outside = self.base / "outside"; outside.mkdir()
        (self.bundle / "payload/files/lib/wine/x86_64-windows/ntdll.dll").unlink()
        parent = self.bundle / "payload/files/lib/wine/x86_64-windows"
        parent.rmdir(); parent.symlink_to(outside)
        with self.assertRaises(core.PatchError): core.verify_bundle(self.bundle)

    def test_configure_preserves_unrelated_xml_and_autostart_entries(self):
        self.put(self.prefix / core.PROGRAM / "Fenix.exe", "synthetic app")
        self.put(self.prefix / core.CONFIG / "fenixConfig.xml", "<Config><displayMode>GPU</displayMode><UserChoice>keep</UserChoice></Config>")
        self.put(self.prefix / core.CONFIG / "persistancy.xml", "<Settings><fcuReadoutsType>1</fcuReadoutsType><Volume>0.4</Volume></Settings>")
        directory = self.prefix / "drive_c/users/test/AppData/Roaming/Microsoft Flight Simulator 2024"
        self.put(directory / "exe.xml", '<SimBase.Document><Launch.Addon><Name>Other addon</Name><Path>C:\\Other.exe</Path></Launch.Addon></SimBase.Document>')
        self.assertEqual(core.configure_prefix(self.prefix), (True, True))
        self.assertEqual(core.configure_prefix(self.prefix), (True, True))
        doc = ET.parse(self.prefix / core.CONFIG / "fenixConfig.xml")
        self.assertEqual(doc.findtext("displayMode"), "CPU")
        self.assertEqual(doc.findtext("UserChoice"), "keep")
        self.assertEqual(ET.parse(self.prefix / core.CONFIG / "persistancy.xml").findtext("fcuReadoutsType"), "0")
        entries = ET.parse(directory / "exe.xml").findall("Launch.Addon")
        self.assertEqual(len(entries), 2)
        self.assertEqual(entries[0].findtext("Name"), "Other addon")


if __name__ == "__main__": unittest.main()
