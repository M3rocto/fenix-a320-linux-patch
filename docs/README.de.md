# Fenix A320 unter Linux

Wine-Kompatibilitätskorrekturen und eine geführte Einrichtung für den Fenix A320
unter Linux: mit [Flightdeck](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox)
sowie den Steam-Versionen von Microsoft Flight Simulator 2020 und 2024.

[Preview.10 herunterladen](https://github.com/marselnenaj/fenix-a320-linux-patch/releases/tag/v0.1.0-preview.10)
· [Änderungen](release-notes.md) · [Problem melden](https://github.com/marselnenaj/fenix-a320-linux-patch/issues)
· [English](../README.md)

Ein unabhängiges Community-Projekt. Du benötigst den Simulator und eine eigene
Fenix-Lizenz. Installation, Anmeldung und Aktivierung erfolgen mit der offiziellen Fenix-Software.

## Aktueller Stand

**Veröffentlicht ist 0.1.0-preview.10.** Eine reguläre Version **0.1.0** wird
vorbereitet und ist noch nicht veröffentlicht. Bei Fenix v3 sind Display- und
Gateway-Fehler auf dem gemeldeten NVIDIA-/Steam-System offen. Zusätzlich wird
wachsender Speicherbedarf von FenixSystem untersucht.
[Stand der Release-Vorbereitung](release-readiness.md).

| Konfiguration | Prüfstand und offene Punkte |
| --- | --- |
| Fenix 2.4.0.4720 · MSFS 2024 1.8.16.0 · Xodus 11.0 | PFD, ND, ECAM, MCDU und Cockpit-Readouts lokal geprüft; vollständige Flugtests bleiben begrenzt. |
| Fenix 2.4.0.4720 · MSFS 2024 · CachyOS 11 | Gemeldeter Flug über 1 h 53 min ohne eingefrorene Displays, mit stabilem Display-Speicher; [System-Speicherwachstum offen](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12). |
| Fenix 3.0.0.7454 · MSFS 2024 1.8.16.0 · Xodus 11.0 | Offizieller Installer 1.0.359, Anmeldung, System-Verbindung und funktionierende Cockpitanzeigen geprüft. Sechs begrenzte Starts mit Produktionsmodulen ohne erneute Assertion; eine frühere Wine-Assertion bleibt ungeklärt. Kein vollständiger v3-Testflug. |
| Fenix 3.0.0.7454 · Steam MSFS 2020 · CachyOS 11 · NVIDIA/PRIME | Installation, Anmeldung, System und CDU laut Rückmeldung funktionsfähig; [Display-Initialisierung und Gateway-Start fehlerhaft](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7). |

Native Regressionstests decken alle vier in `bundle.json` festgelegten Runner ab.
Sie ersetzen keine vollständigen Simulator-Tests für jeden Runner und jede GPU.
Einzelheiten: [Fenix v3](fenix-v3.md), [GPU-Anzeigen](gpu-displays.md),
[Direct2D-Speicherkorrektur](d2d-lifetime.md).

## Mit Flightdeck einrichten

1. Auf **Flightdeck 0.2.18 oder neuer** aktualisieren; MSFS und Fenix schließen.
2. **Mods → Fenix A320** öffnen und dem hervorgehobenen Schritt folgen.
3. Den offiziellen Installer aus dem [Fenix-Konto](https://fenixsim.com/dashboard/)
   ausführen. Anmelden, den A320 für den gewählten Simulator installieren und
   den vollständigen Abschluss abwarten, bevor du den Installer schließt.
4. Fenix öffnen, anmelden und die Anzeigeeinstellungen anwenden.
   Danach MSFS über Flightdeck starten.

Flightdeck 0.2.18 bindet preview.10 mit festgelegter Prüfsumme ein. Eine neue
Patch-Version erfordert eine ausdrückliche Anpassung dieser Zuordnung in
Flightdeck. Verwende dort die integrierte Einrichtung. Der separate Installer
erkennt nur die Startskripte älterer Flightdeck-Python-Versionen bis 0.1.21.

## Mit Steam einrichten

1. MSFS einmal über Steam starten und wieder schließen.
2. Das oben verlinkte **Linux-Installer-ZIP** herunterladen und entpacken.
3. `./install.sh` starten oder **Fenix A320 Linux Patch** öffnen.
   Die Einrichtung läuft lokal im Browser und benötigt keine Root-Rechte.
4. Patch installieren, **Proton Fenix A320** auswählen, offiziellen Fenix-Installer
   ausführen, anmelden und Anzeigen konfigurieren.

Im offiziellen Installer **den A320 für den ausgewählten Simulator installieren**.
Erst nach Abschluss schließen; allein das Öffnen und Schließen installiert das
Flugzeug nicht.

Die Einrichtung erstellt eine separate Proton-Kopie aus einer exakt unterstützten
Version. Bei geschlossenem Steam kann sie diese automatisch auswählen;
sonst unter **Eigenschaften → Kompatibilität** einstellen.
`./install.sh --text` bietet dieselben Schritte im Terminal.
[Steam-Einrichtung und Grenzen](steam.md).

## Voraussetzungen und Runner

- x86_64-Linux, glibc **2.38+**, GNU `cp`, Desktop-Sitzung und Webbrowser.
- Funktionierende Simulator-Installation und gekaufte Fenix-A320-Lizenz.
- Internetzugang für offizielle Voraussetzungen und die Geometrie-Abhängigkeit.
- Eine der exakt in `bundle.json` festgelegten Wine-/Proton-Versionen.

| Runner | Standard für Displays |
| --- | --- |
| Xodus Wine 11.0 / 7c0b4354 | GPU |
| Proton Experimental 11.0 / 20260924 | GPU |
| CachyOS Proton 11.0 / 20261005 | GPU |
| CachyOS Proton 10.0 sunset | CPU |

Der Installer ist eigenständig und benötigt keine zusätzliche Laufzeitumgebung.
Er bereitet geprüfte Microsoft-Voraussetzungen vor, darunter bei Bedarf .NET
Framework 4.8 und die .NET-10-Desktop-Runtime. Versionsabhängige weitere
Voraussetzungen übernimmt der offizielle Fenix-Installer. Andere Runner-Stände
benötigen ein passendes Overlay.

## Aktualisieren, Anzeigen und Wiederherstellen

Updates behalten Flugzeug, Windows-Profil, Einstellungen und ursprünglichen
Wiederherstellungspunkt. Flightdeck führt unterstützte Patch-Updates vor dem
Öffnen der Fenix-Anwendungen aus. Der separate Installer erkennt verifizierte
ältere Previews.

GPU-fähige Overlays verwenden **GPU-Displays** und **Legacy-Readouts**. Eine
bewusste CPU-Auswahl bleibt bei Updates erhalten. Im CPU-Modus ist Wetterradar
nicht verfügbar; GPU-Wetterradar bleibt ungeprüft. Helligkeit und Flugmodi
bedienst du normal im Cockpit. Siehe [GPU-Transport](gpu-displays.md),
[Display-Neustarts](mcdu-restart.md) und [Fensterbehandlung](hyprland.md).

```sh
./install.sh targets
./install.sh status --steam msfs2024
./install.sh install --steam msfs2024
./install.sh configure --steam msfs2024 --renderer cpu
./install.sh configure --steam msfs2024 --renderer gpu
./install.sh restore --steam msfs2024
```

Für die Steam-Version von 2020 `--steam msfs2020` verwenden. Ältere unterstützte
Flightdeck-Python-Runtimes akzeptieren stattdessen `--runtime /pfad/zur/runtime`.

**Wiederherstellen** setzt das frühere Profil ein und behält das neuere in einem
separaten lokalen Verzeichnis. Community-Inhalte außerhalb des Profils werden
nicht entfernt. Flightdecks separater Xbox-Spielstandspeicher gehört nicht zu
dieser Wiederherstellung.

## Fehler melden

Bei v3-Startproblemen oder schwarzen/weißen Displays helfen die
[aktuellen Befunde und Diagnoseschritte](display-startup.md).
[#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7) verfolgt die
verbleibenden v3-Fehler; [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12)
den gesonderten FenixSystem-Speicherbefund. Eine optionale
[Speichermessung](system-memory.md) erfasst vergleichbare Summenwerte.

Patch-, Fenix-, Simulator- und Runner-Version, Distribution, Desktop,
GPU/Treiber und fehlerhaften Schritt angeben. Logs vor dem Teilen prüfen;
Konto-, Aktivierungs- und Lizenzdaten weglassen. Für eine erste Meldung sind
weder ein komplettes Wine-Profil noch Speicherabbilder erforderlich.

## Quellcode und Lizenzen

Die Releases enthalten Installer, Hilfsprogramme, Wine-/DXVK-Patches, zugehörige
Binärmodule, vollständige Quellarchive und Bauanleitungen. Fenix-/Microsoft-Programme,
Flugzeuge, Fonts, Kontodaten und private Profile werden nicht mitgeliefert.
Offizielle Redistributables werden beim Hersteller heruntergeladen und gegen
festgelegte Prüfsummen geprüft.

Installer und Helfer: **MIT**. Wine-Korrekturen und abgeleitete Module:
**LGPL-2.1-or-later**. DXVK: **zlib/libpng**. Upstream-Hinweise bleiben erhalten.
[BUILDING.md](../BUILDING.md) · [THIRD_PARTY.md](../THIRD_PARTY.md).
