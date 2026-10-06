# Worker-Setup

Der Worker ist ein separater, nicht privilegierter Container. Er erhält
Credentials nur aus dem gemeinsamen verschlüsselten Secret-Store als Read-only
Mount und schreibt niemals Secret-Werte in Jobereignisse.

## Konfiguration

```dotenv
LXCUP_SECRET_MASTER_KEY=<64-hex-zeichen>
LXCUP_ARTIFACT_BASE_URL=http://artifacts
LXCUP_WORKER_SSH_USER=lxcup
```

## Artefakt freigeben

The workspace version in the root `Cargo.toml` is authoritative. To set a
version, run `.\scripts\set-version.ps1 <version>`; this synchronizes the
frontend package metadata and Cargo lockfile. The onboarding E2E script and
Sonar project metadata read the version from Cargo, and the artifact-store
healthcheck checks the local Nginx service without embedding a release number.
Run `.\scripts\build-agent-artifact.ps1` to build Linux amd64/arm64 and
Windows x64 agents and regenerate the versioned SHA-256 manifest. The Windows
setup script is downloaded separately and fetches the executable and manifest
directly from the artifact service.
Docker
Buildx and the pinned Rust build images are required. The script creates only
a local artifact; it does not create a Git tag or GitHub release. No GitHub
Actions workflow is used for building or testing these artifacts.

Lege das Agent-Binary und ein Manifest unter `artifacts/agent/<version>/` ab.
Der Worker akzeptiert nur die im Manifest genannte Datei und SHA-256-Summe.

```json
{
  "version": "<workspace-version-from-Cargo.toml>",
  "artifacts": [
    { "platform": "linux-amd64", "file": "linux-amd64", "sha256": "<sha256>" },
    { "platform": "linux-arm64", "file": "linux-arm64", "sha256": "<sha256>" },
    { "platform": "windows-amd64", "file": "windows-amd64.exe", "sha256": "<sha256>" }
  ]
}
```

## Testablauf

1. Ein Linux-Testziel mit SSH-Zugang und Host-Key-Prüfung einbinden.
2. Credential- und Agent-Token-Secret im lxcup-Store anlegen.
3. `docker compose up -d --build` starten. Der `lxcup-worker` ist ein
   regulärer Compose-Service und startet gemeinsam mit Server, Frontend,
   Datenbank und Artefakt-Service.
4. „Agent installieren“ starten und die Workflow-Detailseite öffnen.
5. Erst bei `succeeded` den separaten Healthcheck ausführen.

Ohne vollständige Executor-Konfiguration schlägt der Worker sicher fehl und
protokolliert `worker_unavailable`; er behauptet keinen Erfolg.

## Windows-Agent lokal installieren

Windows 11 x64 wird ohne WinRM-Zugangsdaten eingebunden. In lxcup ein
Windows-System mit Agent-Token anlegen, das einzelne
`windows-agent-setup.ps1` in den Downloads-Ordner laden und den angezeigten
Befehl in einer als Administrator gestarteten PowerShell ausführen. Das Skript
fragt das Agent-Token verdeckt ab und lädt Version, Manifest und Binärdatei
direkt aus dem Artifact-Store. Es prüft Plattform, PE32+-Architektur und
SHA-256, bevor es den automatisch startenden Windows-Dienst installiert.
Es öffnet keine Firewall. Die konfigurierte öffentliche Controller-URL muss
vom Windows-System erreichbar sein und HTTPS mit gültigem Zertifikat
verwenden. Unverschlüsseltes HTTP ist im Setup-Skript ausschließlich für
Loopback-Tests auf demselben Rechner zulässig. In der lokalen Entwicklung ist
die erreichbare URL über `LXCUP_PUBLIC_URL` konfigurierbar.

Der Agent meldet Heartbeats, Telemetrie und das lokale `winget`-Inventar über
ausgehendes HTTPS. Windows-Workflows werden ebenfalls vom Agenten über die
authentifizierte Claim-/Result-API abgeholt. Windows-Softwareupdates werden
mit `winget list --upgrade-available` geplant und pro genehmigter exakter ID
mit `winget upgrade --id <ID> --exact` angewendet. Das Agent-Binary selbst
wird über den bestätigten Workflow „Agent aktualisieren“ aus dem Artifact-Store
bezogen; Hash und PE-Architektur werden vor der Dienstumschaltung geprüft, und
ein fehlgeschlagener Healthcheck stellt den vorherigen Dienstpfad wieder her.
Controller-Policy, erfolgreicher Plan und erneute Bestätigung gelten weiterhin
für Softwareupdates; direkter Apply über den Agent-Endpunkt ist gesperrt.

Das Setup-Skript legt Token und Konfiguration in
`%ProgramData%\lxcup\agent.env` ab. Das Verzeichnis ist auf `SYSTEM` und lokale
Administratoren beschränkt. Der Dienst läuft als `LocalSystem`, damit
systemweite Paketinstallationen unterstützt werden können; dies verleiht
freigegebenen winget-Updates hohe lokale Rechte. Beschränke Update-Policies
und Agent-Token entsprechend und teste zuerst auf einem isolierten
Windows-11-x64-System. Windows ARM64 ist nicht unterstützt. Manuelle
Abnahme-, Upgrade-, winget-Verfügbarkeits- und Recovery-Schritte sind in
[`windows-agent-acceptance.md`](windows-agent-acceptance.md) festgehalten.

Der Worker prüft den versionierten Artifact-Store-Manifest-Endpunkt beim Start
und anschließend alle 15 Sekunden. Bei Nichterreichbarkeit schreibt er eine
Warnung ins Worker-Log und meldet den Zustand per Heartbeat an den Controller;
die Oberfläche zeigt dann „Artifact Store nicht verfügbar“. Der Worker prüft
automatisch weiter und meldet die Wiederherstellung. Compose startet den Worker
erst, wenn der Artifact Store gesund ist, und startet beide Container nach
unerwartetem Prozessende erneut (`restart: unless-stopped`). Ein reiner
`unhealthy`-Status beendet einen noch laufenden Container nicht; siehe dazu das
Betriebs-Runbook.
