# Fenix A320 unter Linux

Dieses Community-Paket richtet die getesteten Fenix-Korrekturen für
**MSFS 2024 in Flightdeck** und, als ungetestete erste Fassung, für die
**Steam-Versionen von MSFS 2020 und 2024** ein. Unterstützt werden ausschließlich
die in `bundle.json` festgelegten Wine-/Proton-Stände; andere werden vor
Änderungen abgewiesen.

## Steam (MSFS 2020 / 2024)

Der Installer funktioniert auch ohne Flightdeck mit den Steam-Versionen. Dieser
Weg wurde noch nicht mit einem echten Steam-Simulator geprüft; MSFS 2020 mit
Fenix ist ungetestet.

1. Den Simulator einmal in Steam starten und wieder schließen.
2. `./install.sh` starten (oder *Fenix A320 Linux Patch* doppelklicken). Im
   Browser öffnet sich eine geführte Einrichtung, die nur lokal läuft.
3. Den fünf Schritten folgen: Patch einrichten, Steam auf **Proton Fenix A320**
   umstellen, offiziellen Fenix-Installer ausführen, anmelden, Anzeigen
   einstellen. Erledigte Schritte sind abgehakt; die Einrichtung lässt sich
   schließen und später fortsetzen. Legt Fenix die Datei für die
   Legacy-FCU-Anzeigen erst nach dem ersten Laden des Flugzeugs an, folgt dafür
   ein weiterer Schritt.

Eine frühere Installation (preview.5 oder preview.6) aktualisiert **Patch
aktualisieren** an Ort und Stelle: Profil, Fenix, Anmeldung und Einstellungen
bleiben erhalten. preview.5 startete den Simulator aus Steam nicht, weil eine
Bibliothek im Steam-Container fehlte.

Seit preview.7 ist der Installer ein einzelnes Programm ohne Python.
preview.8 ergänzt GPU-Anzeigen für die festgelegten Xodus- und
Proton-11-Versionen. CachyOS Proton 10 bleibt beim CPU-Modus.
[GPU-Korrekturen und Prüfstand](gpu-displays.md).

Der Installer legt das eigene Proton an, weil Steam Proton Experimental laufend
aktualisiert und die Korrekturen zu genau einem Wine-Stand gehören. Andere
Proton-Versionen und Spiele bleiben unverändert. Bei geschlossenem Steam trägt
er dieses Proton selbst für den Simulator ein; sonst unter *Eigenschaften →
Kompatibilität* erzwingen. `./install.sh --text` führt dieselben Schritte im
Terminal aus.

Der Ordner `Packages` im Profil wird verschoben statt kopiert. Einzelheiten und
Grenzen: [steam.md](steam.md).

## Flightdeck (MSFS 2024)

In Flightdeck **Add-ons → Fenix A320** öffnen. Flightdeck lädt die festgelegte,
geprüfte Patch-Version, richtet sie mit eigenem Code ein und führt durch
dieselben Schritte:

1. Patch einrichten. Flightdeck sichert das Windows-Profil, erstellt eine
   eigene Runner-Kopie und installiert bei Bedarf Microsoft .NET Framework 4.8
   sowie die geprüfte Geometrie-Abhängigkeit für die Routenanzeige.
2. Den offiziellen Fenix-Installer aus dem eigenen Fenix-Konto auswählen und
   die normale Installation einschließlich der angebotenen Voraussetzungen abschließen.
3. Fenix öffnen, anmelden und aktivieren. Anschließend Fenix samt Helfern schließen.
4. Unterstützten Anzeige-Renderer und Legacy-Readouts anwenden. Danach MSFS über Flightdeck starten.
   Fehlen die Readout-Einstellungen von Fenix noch, das Flugzeug einmal laden
   und die Einstellung erneut anwenden.

`./install.sh` aus diesem ZIP richtet eine Runtime des aktuellen Flightdeck
nicht ein: Es ersetzt nur Startskripte, die es kennt, nämlich die der
Flightdeck-Python-Versionen bis 0.1.21, und lehnt andere mit *Custom Flightdeck
launch script detected* ab. Für eine solche ältere Runtime funktioniert es wie
bei Steam.

Du benötigst die gekaufte Fenix-Lizenz, glibc 2.38+, GNU `cp` und einen
Webbrowser für die geführte Einrichtung. Ab preview.7 bringt das
Installationsprogramm alles mit; preview.6 und älter brauchen noch
Python 3.10+. Keine Root-Rechte nötig.

Getestet: Fenix 2.4.0.4720, MSFS 2024 1.8.16.0, Xodus Wine 11.0 und Hyprland.
Die Cockpitanzeigen wurden im laufenden Flugzeug geprüft. Ein vollständiger
Testflug steht noch aus. Der GPU-Modus wurde im Xodus-Cockpit mit PFD, ND, ECAM
und bewegter Geschwindigkeitsanzeige geprüft. NVIDIA und GPU-Wetterradar sind
noch nicht geprüft. Im CPU-Modus ist Wetterradar nicht verfügbar.
Helligkeit und Flugmodi stellst du wie üblich im Cockpit ein.

Beim Upgrade wechselt eine bislang automatisch gesetzte CPU-Konfiguration zu
GPU, sofern die gewählte Version das unterstützt. Bei Problemen setzt
`./install.sh configure --steam msfs2024 --renderer cpu` dauerhaft CPU;
`--renderer gpu` schaltet wieder auf GPU. Für ältere Flightdeck-Runtimes
statt `--steam msfs2024` den Parameter `--runtime /pfad/zur/runtime` verwenden.
Die ausdrückliche Wahl bleibt bei erneuter Einrichtung und Updates erhalten.
Das aktuelle Flightdeck verwendet seine eigene Einrichtung und benötigt eine
Version mit den neuen Bundle-Pins und derselben Renderer-Unterstützung.

„Wiederherstellen“ setzt Runner, Startskripte und das Windows-Profil auf den
Stand vor dem Patch zurück. Das neuere Profil bleibt als lokale Sicherung erhalten.
Außerhalb des Profils installierte Community-Inhalte werden nicht entfernt.
Bei einer unterbrochenen Einrichtung bleibt diese Funktion ebenfalls verfügbar.

Das Downloadpaket enthält keine Fenix-/Microsoft-Programme, Flugzeuge, Fonts,
Kontodaten oder privaten Profile. Es enthält den Installer, Wine-/DXVK-Patches,
die zugehörigen Binärmodule sowie vollständige Quellarchive und Bauanleitung.

Preview.3 ergänzt die Korrekturen für fehlende Routen und überlange Linien.
Unter X11/Xwayland werden die passenden Fenix-Hilfsfenster direkt im Wine-Treiber
vom Desktop ferngehalten. Updates von preview.1 und preview.2 behalten Flugzeug,
Einstellungen und den ursprünglichen Wiederherstellungspunkt.

Nach einem Neustart von Fenix Display frischt ein Helfer festgehaltene dunkle
MCDU-Bilder automatisch auf. Er stellt die Anzeigeeinstellung und Helligkeit
wieder her und sendet keine Seiten- oder Flugplaneingaben. Bei maximaler
Helligkeit ist dafür ein kurzer DIM/BRT-Wechsel nötig.
[Verhalten und Grenzen](mcdu-restart.md).
