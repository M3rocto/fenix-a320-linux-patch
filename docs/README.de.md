<p align="center">
  <img src="assets/mark.svg" width="76" alt="Community-Projektlogo von Fenix A320 Linux Patch">
</p>
<h1 align="center">Fenix A320 Linux Patch</h1>
<p align="center">
  Wine-Kompatibilitätskorrekturen und geführte Einrichtung für den Fenix A320 unter Linux.
</p>
<p align="center">
  <a href="#loslegen"><strong>Loslegen</strong></a> &nbsp;·&nbsp;
  <a href="index.md">Dokumentation</a> &nbsp;·&nbsp;
  <a href="#kompatibilität">Kompatibilität</a> &nbsp;·&nbsp;
  <a href="release-notes.md">Änderungen</a> &nbsp;·&nbsp;
  <a href="../README.md">English</a>
</p>

![Eigenes Artwork eines A320-Familienflugzeugs über den Alpen im Abendlicht](assets/fenix-panorama.png)

Dein **gekaufter Fenix A320** im Linux-Simulator: mit passenden Wine-/Proton-Korrekturen, GPU-Displayübertragung und geführter Einrichtung. Nutze die
integrierte Einrichtung in
[Flightdeck](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox)
für dessen Xbox-PC-Versionen oder den separaten Installer für
**Steam-MSFS 2020 und 2024**. Installation, Anmeldung und Aktivierung erfolgen
mit der offiziellen Fenix-Software.

**Aktuell veröffentlicht: [0.1.0-preview.10](https://github.com/marselnenaj/fenix-a320-linux-patch/releases/tag/v0.1.0-preview.10).**
Eine reguläre Version **0.1.0** wird vorbereitet und ist noch nicht veröffentlicht.

**Neu in preview.10:** Kompatibilitätskorrekturen für den offiziellen Installer
1.0.359 und den Start von Fenix v3; GPU-Displayübertragung und die Direct2D-Speicherkorrektur bleiben enthalten. Mit Fenix 3.0.0.7454 wurden auf dem
geprüften Xodus-/MSFS-2024-System funktionierende Cockpitanzeigen erreicht.
Weitere gemeldete v3-Startprobleme sind offen.
[Kompatibilität](#kompatibilität) · [Release-Vorbereitung](release-readiness.md)

## Loslegen

### Mit Flightdeck

1. Auf **Flightdeck 0.2.18 oder neuer** aktualisieren; MSFS und Fenix schließen.
2. **Mods → Fenix A320** öffnen und dem hervorgehobenen Einrichtungsschritt folgen.
3. Den offiziellen Installer aus deinem [Fenix-Konto](https://fenixsim.com/dashboard/)
   starten. Anmelden, den A320 für deinen Simulator installieren und den Abschluss abwarten.
4. Fenix öffnen, anmelden und die Anzeigeeinstellungen anwenden.
   Danach MSFS über Flightdeck starten.

Flightdeck enthält die über eine feste Prüfsumme eingebundene preview.10-Integration. Verwende dort die integrierte Einrichtung; neuere Patch-Versionen
benötigen eine entsprechende Anpassung in Flightdeck.

### Mit Steam

1. MSFS einmal über Steam starten und wieder schließen.
2. Das **Linux-Installer-ZIP** aus dem
   [preview.10-Release](https://github.com/marselnenaj/fenix-a320-linux-patch/releases/tag/v0.1.0-preview.10)
   herunterladen und entpacken.
3. **Fenix A320 Linux Patch** öffnen oder `./install.sh` starten.
4. Durch die Einrichtung führen lassen: Patch installieren,
   **Proton Fenix A320** auswählen, offiziellen Fenix-Installer ausführen,
   anmelden und Anzeigen konfigurieren.

Im offiziellen Installer **den A320 für den ausgewählten Simulator installieren**
und erst nach Abschluss schließen. Allein das Öffnen des Installers installiert
das Flugzeug nicht.

Die geführte Einrichtung läuft lokal im Browser und braucht keine Root-Rechte.
Sie erstellt ein separates Proton-Werkzeug aus einer exakt unterstützten Version.
Bei geschlossenem Steam kann sie es automatisch auswählen; andernfalls unter
**Eigenschaften → Kompatibilität** einstellen. `./install.sh --text` bietet
dieselben Schritte im Terminal. [Steam-Anleitung](steam.md)

## Was der Patch bietet

| Bereich | Funktionen |
| --- | --- |
| **Geführte Einrichtung** | Passenden Runner auswählen, geprüfte Voraussetzungen installieren, offiziellen Installer starten und Einrichtungsstand anzeigen. [Steam-Anleitung](steam.md) |
| **Cockpitanzeigen** | GPU-Displayübertragung auf unterstützten Wine-11-Versionen, mit CPU-Rückfallmodus und Legacy-Readouts. [Displayübertragung](gpu-displays.md) |
| **Kompatibilitätskorrekturen** | Wine-Anpassungen für Installer und v3-Start sowie die Direct2D-Geometrie-Speicherkorrektur. [Fenix v3](fenix-v3.md) · [Speicherkorrektur](d2d-lifetime.md) |
| **Updates und Wiederherstellung** | Geprüfte Upgradepfade mit Erhalt von Flugzeug, Profil, Renderer-Auswahl und ursprünglichem Wiederherstellungspunkt. [Wiederherstellung](mcdu-restart.md) |

## Voraussetzungen und Runner

Du brauchst **x86_64-Linux mit glibc 2.38+**, GNU `cp`, eine Desktop-Sitzung,
einen Webbrowser, eine funktionierende Simulator-Installation und eine gekaufte
Fenix-A320-Lizenz. Internetzugang wird für offizielle Voraussetzungen und die
Geometrie-Abhängigkeit benötigt.

| Exakt unterstützter Runner | Standard für Displays |
| --- | --- |
| Xodus Wine 11.0 / 7c0b4354 | GPU |
| Proton Experimental 11.0 / 20260924 | GPU |
| CachyOS Proton 11.0 / 20261005 | GPU |
| CachyOS Proton 10.0 sunset | CPU |

Der Installer ist eigenständig. Er bereitet über Prüfsummen verifizierte
Microsoft-Voraussetzungen vor, darunter bei Bedarf .NET Framework 4.8 und
die .NET-10-Desktop-Runtime. Der offizielle Fenix-Installer übernimmt seine
versionsabhängigen Voraussetzungen. Andere Runner-Stände benötigen ein
passendes Overlay; die exakten Zuordnungen stehen in [bundle.json](../bundle.json).

## Kompatibilität

**Experimentell.** Native Regressionstests decken alle vier festgelegten Runner
ab; vollständige Simulator- und GPU-Treiber-Kompatibilität erfordert Laufzeittests.

| Fenix | Geprüfte Umgebung | Beobachtetes Verhalten |
| --- | --- | --- |
| 2.4.0.4720 | MSFS 2024 1.8.16.0 · Xodus 11.0 | PFD, ND, ECAM, MCDU und Cockpit-Readouts lokal geprüft. |
| 2.4.0.4720 | MSFS 2024 · CachyOS 11 | Flugbericht mit stabilem Display-Speicher und ohne eingefrorene Anzeigen; die übermittelte Zeitreihe umfasst knapp zwei Stunden. Separates System-Speicherwachstum ist in den Messwerten bestätigt, die Ursache bleibt offen. |
| 3.0.0.7454 | MSFS 2024 1.8.16.0 · Xodus 11.0 | Offizieller Installer 1.0.359, Anmeldung, System-Verbindung und funktionierende Cockpitanzeigen geprüft; sechs begrenzte Starts mit Produktionsmodulen bestanden. |
| 3.0.0.7454 | Steam MSFS 2020 · CachyOS 11 · NVIDIA/PRIME | Installation, Anmeldung, System und CDU laut Rückmeldung funktionsfähig; Display-Initialisierung und Gateway-Start fehlerhaft. |

Vollständige v3-Testflüge stehen aus; eine frühere Wine-Assertion beim Start ist
ungeklärt. [Issue #7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7)
verfolgt die verbleibenden Display-/Gateway-Fehler;
[#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12) das
separate FenixSystem-Speicherwachstum.
[Aktuelle Befunde und gezielte Diagnoseschritte](display-startup.md)

GPU-fähige Overlays verwenden **GPU-Displays** und **Legacy-Readouts**.
Eine bewusste CPU-Auswahl bleibt bei Updates erhalten. Im CPU-Modus ist
Wetterradar nicht verfügbar; GPU-Wetterradar ist ungeprüft. Helligkeit und
Flugmodi bedienst du normal im Cockpit. [Anzeigeeinstellungen](gpu-displays.md)

## Updates und Wiederherstellung

Vor einem Update MSFS und Fenix schließen. Flightdeck führt unterstützte
Patch-Updates vor dem Öffnen der Fenix-Anwendungen aus; der separate Installer
erkennt verifizierte ältere Previews. Updates behalten Flugzeug, Windows-Profil,
Einstellungen und ursprünglichen Wiederherstellungspunkt.

**Wiederherstellen** setzt das frühere Profil ein und behält das neuere in einem
separaten lokalen Verzeichnis. Community-Inhalte außerhalb dieses Profils bleiben
erhalten. Flightdecks separater Xbox-Spielstandspeicher gehört nicht zu diesem Vorgang.

<details>
<summary>Terminalbefehle</summary>

```sh
./install.sh targets
./install.sh status --steam msfs2024
./install.sh install --steam msfs2024
./install.sh configure --steam msfs2024 --renderer cpu
./install.sh configure --steam msfs2024 --renderer gpu
./install.sh restore --steam msfs2024
```

Für die Steam-Version von 2020 `--steam msfs2020` verwenden. Ältere Flightdeck-Python-Versionen bis 0.1.21 akzeptieren `--runtime /pfad/zur/runtime`;
aktuelle Versionen verwenden die integrierte Einrichtung.

</details>

## Dokumentation und Hilfe

Der [Dokumentationsindex](index.md) führt zu Einrichtung, Anzeigeeinstellungen,
Wiederherstellung und technischen Referenzen. Gib in einem
[GitHub-Issue](https://github.com/marselnenaj/fenix-a320-linux-patch/issues)
die Patch-, Fenix-, Simulator- und Runner-Version, GPU/Treiber und den
fehlerhaften Schritt an. Prüfe Logs vor dem Teilen und entferne Konto-,
Aktivierungs- und Lizenzdaten. Die optionale [Speichermessung](system-memory.md)
erfasst vergleichbare Summenwerte ohne Speicherinhalte.

[BUILDING.md](../BUILDING.md) beschreibt Builds und Tests. Releases enthalten
Installer, Hilfsprogramme, Ersatzmodule, Wine-/DXVK-Patches und vollständige
zugehörige Quellarchive. Offizielle Redistributables werden bei den Herstellern
heruntergeladen und gegen festgelegte Prüfsummen geprüft.

## Lizenz

Installer, Helfer und Projekt-Artwork: [MIT](../LICENSE). Wine-Anpassungen und
abgeleitete Module: **LGPL-2.1-or-later**. DXVK: **zlib/libpng**.
Upstream-Hinweise bleiben erhalten. [Drittanbieter-Hinweise](../THIRD_PARTY.md)

Ein unabhängiges Community-Projekt ohne Unterstützung oder offizielle Verbindung
zu Fenix Simulations, Microsoft oder den Upstream-Projekten. Fenix-/Microsoft-Programme, Flugzeuge, Fonts, Kontodaten und private Profile werden nicht
mitgeliefert. Das Cover ist ein eigenes Projekt-Artwork und kein Simulator-Screenshot. [Herkunft des Artworks](artwork.md)
