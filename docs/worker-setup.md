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
Run `.\scripts\build-agent-artifact.ps1` to build the Linux-amd64 agent
and regenerate its versioned SHA-256 manifest. This creates only a local
artifact; it does not create a Git tag or GitHub release.

Lege das Agent-Binary und ein Manifest unter `artifacts/agent/<version>/` ab.
Der Worker akzeptiert nur die im Manifest genannte Datei und SHA-256-Summe.

```json
{
  "version": "<workspace-version-from-Cargo.toml>",
  "artifacts": [{
    "platform": "linux-amd64",
    "file": "linux-amd64",
    "sha256": "<sha256>"
  }]
}
```

## Testablauf

1. Ein Testziel mit SSH-Zugang und Host-Key-Prüfung einbinden.
2. Credential- und Agent-Token-Secret im lxcup-Store anlegen.
3. `docker compose up -d --build` starten. Der `lxcup-worker` ist ein
   regulärer Compose-Service und startet gemeinsam mit Server, Frontend,
   Datenbank und Artefakt-Service.
4. „Agent installieren“ starten und die Workflow-Detailseite öffnen.
5. Erst bei `succeeded` den separaten Healthcheck ausführen.

Ohne vollständige Executor-Konfiguration schlägt der Worker sicher fehl und
protokolliert `worker_unavailable`; er behauptet keinen Erfolg.

Der Worker prüft den versionierten Artifact-Store-Manifest-Endpunkt beim Start
und anschließend alle 15 Sekunden. Bei Nichterreichbarkeit schreibt er eine
Warnung ins Worker-Log und meldet den Zustand per Heartbeat an den Controller;
die Oberfläche zeigt dann „Artifact Store nicht verfügbar“. Der Worker prüft
automatisch weiter und meldet die Wiederherstellung. Compose startet den Worker
erst, wenn der Artifact Store gesund ist, und startet beide Container nach
unerwartetem Prozessende erneut (`restart: unless-stopped`). Ein reiner
`unhealthy`-Status beendet einen noch laufenden Container nicht; siehe dazu das
Betriebs-Runbook.
