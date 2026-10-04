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
- Windows 11 x64, Administratorrechte und `winget`, wenn Softwareinventar oder
  Update-Workflows abgenommen werden.

## Installieren und prüfen

1. Im Windows-Bereich ein Ziel anlegen und das Agent-Token sicher speichern.
2. `windows-agent-setup.ps1` über die lxcup-Oberfläche herunterladen.
3. PowerShell als Administrator öffnen und den für dieses Ziel angezeigten
   Befehl ausführen. Das Skript fragt das Token verdeckt ab. Token nicht als
   Kommandozeilenargument ergänzen.
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
6. Verfügbarkeit und Identität von `winget` im tatsächlichen Dienstkontext
   kontrollieren. Der Dienst läuft als `LocalSystem`; ein interaktives
   Benutzerkonto kann eine andere winget-Installation oder Paketquelle sehen.
   Ohne winget oder eindeutig parsebare Paket-IDs muss Suche/Plan fehlschlagen,
   nicht stillschweigend als vollständig gelten.
7. Nur mit ungefährlichen Testpaketen einen Update-Plan, dessen Paket-IDs,
   Policy-Grenzen und explizite Apply-Bestätigung prüfen. Bestätigen, dass nur
   exakt ausgewählte IDs aktualisiert werden und das Resultat im Workflowlog
   erscheint. Der Agent-Endpunkt darf keinen direkten Apply erlauben.

## Upgrade und Wiederherstellung

- Ein Upgrade wird durch erneutes Ausführen des heruntergeladenen Skripts mit
  der neuen, vom Controller unterstützten Version ausgeführt. Es stoppt den
  Dienst, tauscht Binary und Konfiguration aus und startet den Dienst erneut.
- Scheitert der Austausch oder Dienststart, versucht das Skript die zuvor
  vorhandenen Dateien wiederherzustellen und den vorherigen Dienst neu zu
  starten. Prüfe danach `Get-Service lxcup-agent` und den Windows-
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
