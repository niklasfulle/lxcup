# Windows-Agent: Installation, Abnahme und Wiederherstellung

Windows-Agent-Support ist in 0.5.0 auf Windows 11 x64 beschränkt. Die echte
Windows-Abnahme muss auf einem isolierten Testsystem durch den Betreiber
erfolgen; Unit-Tests und ein lokaler Cross-Build ersetzen sie nicht.

## Voraussetzungen

- lxcup 0.5.0 mit dem passenden `windows-amd64`-Artefakt und Manifest.
- Ein erreichbarer Controller mit gültigem HTTPS-Zertifikat. HTTP ist nur für
  Loopback-Entwicklung erlaubt.
- Ein manuell registriertes Windows-System mit Agent-Token; WinRM und
  eingehende Firewallregeln sind nicht erforderlich.
- Windows 11 x64 und Administratorrechte. Das Setup stellt PowerShell 7.4+
  (gegebenenfalls über WinGet) bereit und installiert `Microsoft.WinGet.Client`
  systemweit aus der PowerShell Gallery mit PSResourceGet (ohne den alten
  NuGet-PackageManagement-Provider); der `LocalSystem`-Dienst verwendet dessen
  PowerShell-Befehle statt `winget.exe`.
  Getestete Pakete müssen maschinenweit installiert sein, damit der Dienst sie
  inventarisieren und aktualisieren kann.

## Installieren und prüfen

1. Im Windows-Bereich ein Ziel anlegen und das Agent-Token sicher speichern.
2. Das einzelne `windows-agent-setup.ps1` über die lxcup-Oberfläche
   herunterladen. Es lädt EXE und Manifest bei der Ausführung direkt aus dem
   versionierten Artifact-Store.
3. PowerShell als Administrator öffnen und den für dieses Ziel angezeigten
   Befehl ausführen. Das Skript fragt das Token verdeckt ab. Token nicht als
   Kommandozeilenargument ergänzen.
   Bei einem Host, der mit einem älteren Setup-Skript eingerichtet wurde, das
   aktualisierte Skript einmal erneut ausführen; dadurch werden PowerShell 7.4+
   und das systemweite WinGet-Modul für den Dienst bereitgestellt.
4. Den Dienststatus und die Konfigurationsrechte kontrollieren:

   ```powershell
   Get-Service lxcup-agent
   icacls "$env:ProgramData\lxcup"
   ```

   Der Dienst muss laufen. Zugriff auf das Konfigurationsverzeichnis soll nur
   `SYSTEM` und lokalen Administratoren gewährt sein.

5. In lxcup die Agent-Version, den Heartbeat, Health, Telemetrie und das
   Softwareinventar kontrollieren. Anschließend das Testsystem neu starten und
   bestätigen, dass der Dienst automatisch startet und erneut Heartbeats
   meldet.
6. Verfügbarkeit des systemweit installierten Moduls und des Inventars
   kontrollieren. Der Dienst läuft als `LocalSystem`; ein interaktives
   Benutzerkonto kann einen anderen WinGet-Paketbestand oder andere Quellen
   sehen. Ein reiner Inventarworkflow überspringt die separate Update-Suche.
   Ein erfolgreicher `Get-WinGetPackage`-Aufruf allein genügt nicht: lxcup muss
   anschließend mindestens ein Paket samt installierter Version anzeigen. Die
   Inventarisierung liest die Version aus `InstalledVersion` (einschließlich
   der WinGet-Objektform mit einer verschachtelten `Version`-Eigenschaft). Wenn
   alle gelieferten Datensätze ungültig sind, muss der Workflow fehlschlagen
   statt ein leeres Inventar als erfolgreich zu speichern.
   Bei Fehlern zeigt das Workflowprotokoll einen stabilen Diagnosegrund für
   fehlendes WinGet, Timeout, zu große Ausgabe oder ungültige Paketdaten; rohe
   Agent-Ausgaben werden nicht angezeigt. Die Update-Suche darf nur verfügbare
   Updates und der Apply nur exakt ausgewählte Paket-IDs verwenden.
7. Nur mit ungefährlichen Testpaketen einen Update-Plan, dessen Paket-IDs,
   Policy-Grenzen und explizite Apply-Bestätigung prüfen. Bestätigen, dass nur
   exakt ausgewählte IDs aktualisiert werden und das Resultat im Workflowlog
   erscheint. Der Agent-Endpunkt darf keinen direkten Apply erlauben.

## Bisherige Betreiber-Rückmeldung

Am 6. Oktober 2026 wurde auf dem isolierten Windows-Ziel in lxcup Agent v0.5.0
als verbunden angezeigt. Healthcheck und Paketinventar waren erfolgreich;
lxcup zeigte 107 Pakete, 36 verfügbare Updates sowie aktuelle CPU-, RAM- und
Speichertelemetrie an. Der Betreiber bestätigte anschließend, dass die
ausstehenden manuellen Abnahmepunkte erledigt sind und die zugehörigen Tickets
geschlossen werden können. Detaillierte Test- und Recovery-Ausgaben wurden
nicht im Repository abgelegt.

Die geschlossene Abnahme basiert daher auf der Bestätigung des Betreibers;
Screenshots und Rohprotokolle mit Ziel- oder Secret-Daten werden nicht im Repo
archiviert.

## Upgrade und Wiederherstellung

- Ein Agent-Upgrade wird in lxcup als Workflow „Agent aktualisieren“ gestartet.
  Im Apply-Modus lädt der Agent die vom Controller angegebene Version aus dem
  Artifact-Store, prüft Manifest, Hash und PE-Architektur, legt eine
  versionierte EXE ab, schaltet den Dienst um und bestätigt die neue Version
  über seinen lokalen Health-Endpunkt. Windows-Softwareupdates bleiben davon
  getrennt und verwenden weiterhin `Microsoft.WinGet.Client`.
- Wenn der neue Agent den Healthcheck nicht besteht, stellt der Updater den
  vorherigen Dienstpfad wieder her und meldet den Workflow als fehlgeschlagen.
  Prüfe danach `Get-Service lxcup-agent` und den Windows-
  Anwendungsereignisprotokoll-Eintrag; ändere nicht manuell die
  Manifest-Prüfung.
- Für eine manuelle Rückkehr zu einer vorherigen Version das Setup-Skript mit
  dieser Version erneut ausführen und das Agent-Token erneut verdeckt eingeben.
  Das Setup-Skript selbst hält nach erfolgreichem Start kein dauerhaftes
  Rollback-Binary vor.
- Wenn das Agent-Token rotiert oder widerrufen wurde, den Agent-Token in lxcup
  erneut konfigurieren und das Setup-Skript mit dem neuen Token ausführen.
  Token nicht in Logs oder Diagnoseausgaben kopieren.
- Bis der isolierte Durchlauf dokumentiert und durch den Betreiber bestätigt
  wurde, bleibt GitHub-Issue #227 und damit das Release-Epic #222 offen.
