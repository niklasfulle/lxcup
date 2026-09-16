# lxcup – Projektplan

## 1. Ziel

`lxcup` ist ein in Rust geschriebener Update-Manager für Proxmox-LXC-Container.

Das Tool soll:

- verfügbare Betriebssystem-Updates innerhalb von LXCs erkennen,
- Security-Updates gesondert kennzeichnen,
- Updates gezielt durch den Administrator auswählen lassen,
- vor der Ausführung einen Dry-Run durchführen,
- tatsächliche Abhängigkeiten und Paketänderungen anzeigen,
- optional Proxmox-Snapshots erstellen,
- Updates erst nach expliziter Bestätigung ausführen,
- Neustartbedarf und Healthchecks auswerten,
- später Docker-Image-Updates innerhalb von LXCs erkennen und gezielt ausführen,
- langfristig mehrere Proxmox-Nodes über einen zentralen Update-Manager verwalten.

Grundsatz:

> Automatisches Scannen ist erlaubt. Verändernde Aktionen werden niemals automatisch ausgeführt.

---

## 2. Zentrales Sicherheitsprinzip

```text
SCAN ≠ PLAN ≠ APPLY
```

Ablauf:

```text
┌─────────────┐
│    SCAN     │
│ Was könnte  │
│ aktualisiert│
│ werden?     │
└──────┬──────┘
       │
       ▼
┌─────────────┐
│    PLAN     │
│ Was würde   │
│ tatsächlich │
│ passieren?  │
└──────┬──────┘
       │
       ▼
 Benutzer bestätigt
       │
       ▼
┌─────────────┐
│    APPLY    │
│ Update wird │
│ ausgeführt  │
└─────────────┘
```

Nicht vorgesehen:

```text
Update gefunden
      ↓
automatisch installieren
```

---

## 3. Langfristiger Funktionsumfang

1. Proxmox-/LXC-Discovery
2. Linux-Paketupdates
3. Security-Update-Klassifizierung
4. Update-Planung per Dry-Run
5. Snapshots
6. Rollback-Unterstützung
7. Healthchecks
8. Reboot-Erkennung
9. Terminal User Interface
10. Docker-Image-Update-Erkennung
11. Docker-Compose-Updates
12. Mehrere Proxmox-Nodes
13. Zentraler Agent-/Controller-Betrieb
14. Web-Dashboard
15. Audit-Logging und Update-Historie

---

# 4. Version 1 – Lokales CLI auf dem Proxmox-Host

Die erste Version läuft direkt auf einem Proxmox-Node.

```text
Proxmox Host
│
├── lxcup
│
├── LXC 101
├── LXC 102
├── LXC 103
└── LXC 104
```

Vorteile:

- kein SSH in jeden LXC nötig,
- keine Agenten innerhalb der LXCs nötig,
- `pct` kann Container auflisten,
- `pct exec` kann Befehle innerhalb eines Containers ausführen,
- Snapshots und Rollbacks können direkt über Proxmox angestoßen werden.

---

# 5. CLI-Kommandos

## Container auflisten

```bash
lxcup list
```

Beispiel:

```text
ID    Name           Status     OS
101   grafana        running    Debian 13
102   uptime-kuma    running    Debian 13
103   docker01       running    Debian 13
104   testserver     stopped    Ubuntu 24.04
```

---

## Updates scannen

```bash
lxcup scan
```

Beispiel:

```text
LXC Update Scan

ID    Container       Updates   Security
101   grafana             4         1
102   uptime-kuma         0         0
103   docker01            8         2
104   testserver          -         -

Total:
3 containers scanned
12 updates available
3 security updates
```

---

## Einzelnen Container anzeigen

```bash
lxcup show 103
```

Beispiel:

```text
Container 103
docker01

OS:
Debian GNU/Linux 13

Status:
running

Updates:

 #   Package              Installed       Candidate        Type
--------------------------------------------------------------------
 1   openssl              3.5.0-1         3.5.1-1          Security
 2   libc6                2.41-8          2.41-9           Security
 3   curl                 8.14.0-1        8.14.1-1         Normal
 4   docker-ce            28.x            29.x             Normal
```

Update-Typen:

```text
Security
Normal
Unknown
```

`Unknown` wird verwendet, wenn die Herkunft des Updates nicht zuverlässig als Security-Update klassifiziert werden kann.

---

# 6. Gezielte Updates

Ein Paket:

```bash
lxcup update 103 openssl
```

Mehrere Pakete:

```bash
lxcup update 103 openssl libc6
```

Nur Security-Updates:

```bash
lxcup update 103 --security
```

Alle aktuell angebotenen Updates dieses Containers:

```bash
lxcup update 103 --all
```

`--all` gilt ausschließlich für den angegebenen LXC.

---

# 7. Dry-Run vor jedem Update

Vor jeder Installation wird eine Simulation durchgeführt.

Intern beispielsweise:

```bash
apt-get --simulate install --only-upgrade openssl libc6
```

Beispielausgabe von `lxcup`:

```text
Requested:

  openssl
  libc6

APT additionally wants to change:

  libssl3
  libc-bin

Install:
  0

Upgrade:
  4

Remove:
  0

Downgrade:
  0
```

Erst dieser aufgelöste Plan wird dem Benutzer zur Bestätigung vorgelegt.

---

# 8. Feste Safety Rules

Standardmäßig blockiert:

```text
Package removal
Package downgrade
Änderung gehaltener Packages
Distribution upgrade
Debian 13 → Debian 14
Ubuntu 24.04 → Ubuntu 26.04
Unauthenticated packages
Beliebige freie Shell-Kommandos
```

Erlaubt:

```text
Upgrade gezielt ausgewählter Pakete
Notwendige Dependencies nach Anzeige
Security Updates
Normale Paketupdates
```

Falls APT ein Paket entfernen möchte:

```text
PLAN BLOCKED

Updating docker-ce would remove:

  containerd
  foo-package

lxcup will not execute this update.
```

In Version 1 wird ein solcher Plan nicht nur bestätigt, sondern grundsätzlich blockiert.

---

# 9. Update-Plan als eigenes Domain-Objekt

Ein Update wird intern als Plan modelliert.

Beispiel:

```rust
UpdatePlan {
    id,
    container_id,
    created_at,
    requested_packages,
    resolved_changes,
    repository_state,
    snapshot_requested,
    plan_hash,
}
```

Beispiel:

```text
Plan: 8d4431

Container:
103 docker01

Requested:
openssl

Actual changes:
openssl 3.5.0 → 3.5.1
libssl3 3.5.0 → 3.5.1

Remove: 0
Install: 0
Upgrade: 2
```

---

# 10. Schutz gegen veraltete Update-Pläne

Vor der eigentlichen Installation wird die Simulation erneut ausgeführt.

Ablauf:

```text
Plan erstellen
      ↓
Hash speichern
      ↓
Benutzer bestätigt
      ↓
Simulation erneut durchführen
      ↓
Plan vergleichen
      │
   ┌──┴──┐
   │     │
gleich anders
   │     │
   ▼     ▼
Update  ABORT
```

Beispiel:

```text
Update plan has changed.

Old:
openssl 3.5.0 → 3.5.1

New:
openssl 3.5.0 → 3.5.2

Please review the new plan.
```

---

# 11. Snapshot-Unterstützung

Optional:

```bash
lxcup update 103 openssl --snapshot
```

Beispiel-Snapshot:

```text
lxcup-103-20260909-154500
```

Beschreibung:

```text
lxcup pre-update snapshot

Packages:
openssl 3.5.0 -> 3.5.1
libssl3 3.5.0 -> 3.5.1
```

Wenn `--snapshot` angegeben wurde und die Snapshot-Erstellung fehlschlägt:

```text
Snapshot creation failed.

Reason:
Storage does not support snapshots.

Update was NOT started.
```

---

# 12. Rollback

Snapshots anzeigen:

```bash
lxcup snapshots 103
```

Rollback:

```bash
lxcup rollback 103 lxcup-103-20260909-154500
```

Bestätigung:

```text
WARNING

Container:
docker01 [103]

Current state will be rolled back to:

lxcup-103-20260909-154500

Continue? [y/N]
```

Kein automatischer Rollback.

---

# 13. Healthchecks

Konfiguration beispielsweise:

```toml
[containers."101"]

[[containers."101".healthchecks]]
type = "http"
url = "http://10.0.0.101:3000/health"

[[containers."101".healthchecks]]
type = "systemd"
service = "grafana-server"
```

Nach einem Update:

```text
Post Update Check

Container     ✓ running
Grafana       ✓ active
HTTP health   ✓ 200
Reboot        ✗ required
```

Bei Fehler:

```text
UPDATE COMPLETED
HEALTHCHECK FAILED

grafana-server is inactive

Snapshot:
lxcup-101-20260909-154500

No rollback was executed automatically.
```

---

# 14. Reboot-Erkennung

Bei Debian/Ubuntu beispielsweise über:

```text
/run/reboot-required
```

Ausgabe:

```text
Update successful.

Reboot required: YES

Restart container now? [y/N]
```

Auch ein Neustart muss explizit bestätigt werden.

---

# 15. Terminal User Interface

Später mit `ratatui`.

Beispiel:

```text
┌──────────────────────────────────────────────────────────────┐
│ lxcup                                            Proxmox pve │
├──────────────────────┬───────────────────────────────────────┤
│ Containers           │ docker01 [103]                        │
│                      │                                       │
│ ✓ 101 grafana     4  │ Available updates                    │
│ ✓ 102 uptime      0  │                                       │
│ > 103 docker01    8  │ [x] openssl     SECURITY             │
│ ✓ 104 wazuh       3  │ [x] libc6       SECURITY             │
│                      │ [ ] curl                              │
│                      │ [ ] docker-ce                         │
│                      │ [ ] containerd                        │
│                      │                                       │
├──────────────────────┼───────────────────────────────────────┤
│ Security updates: 3  │ [Dry Run] [Snapshot + Update]        │
└──────────────────────┴───────────────────────────────────────┘
```

---

# 16. Rust-Projektstruktur

Von Beginn an als Cargo Workspace:

```text
lxcup/
│
├── Cargo.toml
│
└── crates/
    │
    ├── lxcup-core/
    ├── lxcup-proxmox/
    ├── lxcup-apt/
    ├── lxcup-docker/
    ├── lxcup-cli/
    ├── lxcup-agent/
    └── lxcup-server/
```

Für eine frühe Version kann intern zunächst zusätzlich modularisiert werden:

```text
src/
├── cli/
├── core/
├── proxmox/
├── package_manager/
├── docker/
├── health/
├── persistence/
└── tui/
```

---

# 17. Empfohlene Rust-Crates

```text
clap
    CLI Parsing

serde
serde_json
toml
    Datenmodelle und Konfiguration

tokio
    parallele Scans

tracing
tracing-subscriber
    Logging

thiserror
    Domain Errors

reqwest
    HTTP- und Registry-Abfragen

ratatui
crossterm
    Terminal UI

rusqlite oder sqlx
    Persistenz

chrono oder time
    Zeitstempel
```

---

# 18. Keine Shell-Strings

Nicht:

```rust
Command::new("sh")
    .arg("-c")
    .arg(format!("pct exec {} -- apt install {}", id, package))
```

Stattdessen Argumente direkt an den Prozess übergeben:

```rust
Command::new("pct")
    .args([
        "exec",
        "103",
        "--",
        "apt-get",
        "install",
        "--only-upgrade",
        "openssl",
    ])
```

Paketnamen stammen ausschließlich aus vorher erkannten und validierten Updates.

---

# 19. Parallelisierung

Scans können parallel laufen.

Beispiel:

```text
Concurrency Limit: 4
```

```text
101 ─── scan
102 ─── scan
103 ─── scan
104 ─── scan

        ↓ fertig

105 ─── scan
```

Ausführende Updates sollten dagegen bewusst begrenzt oder zunächst seriell verarbeitet werden.

---

# 20. Konfiguration

Pfad:

```text
/etc/lxcup/config.toml
```

Beispiel:

```toml
[general]
scan_concurrency = 4

[snapshot]
prefix = "lxcup"
default = false

[containers."101"]
enabled = true
snapshot_default = true

[containers."102"]
enabled = true

[containers."103"]
enabled = true

[containers."104"]
enabled = false
```

---

# 21. Persistenz

SQLite eignet sich für Historie und Status.

Tabellen:

```text
containers

scans
scan_updates

update_plans
update_plan_items

executions
execution_events

snapshots

docker_services
docker_images
docker_scans
```

Beispiel spätere Historie:

```text
docker01

Last Scan:
09.09.2026 15:40

Last Update:
04.09.2026 19:03

Updates installed:
openssl
libssl3

Result:
Success
```

---

# 22. Docker-Erweiterung

Docker-Updates werden strikt von LXC-Betriebssystem-Updates getrennt.

```text
LXC Package Updates
        ≠
Docker Image Updates
```

Beispiel:

```text
docker01 [103]

OS
├── 4 package updates
└── 1 security update

Docker
├── Grafana       update available
├── PostgreSQL    current
├── Redis         current
└── Uptime Kuma   update available
```

---

# 23. Docker Discovery

Ablauf:

```text
LXC
 ↓
Docker erkannt?
 ↓
laufende Container
 ↓
Compose-Projekte
 ↓
Image-Referenzen
 ↓
lokaler Digest
 ↓
Registry-Digest
```

Für Compose-Projekte kann beispielsweise die aufgelöste Konfiguration ausgewertet werden:

```bash
docker compose config --images
```

---

# 24. Docker-Update-Typ A – Digest Update

Beispiel:

```yaml
image: grafana/grafana:12
```

Lokal:

```text
grafana/grafana:12
sha256:AAA
```

Registry:

```text
grafana/grafana:12
sha256:BBB
```

Ergebnis:

```text
UPDATE AVAILABLE
```

Das ist der empfohlene Docker-MVP.

---

# 25. Docker-Update-Typ B – neue Version

Spätere Erweiterung:

```text
Current:
12.1.0

Available:

PATCH
12.1.1

MINOR
12.2.0

MAJOR
13.0.0
```

Da Registry-Tags nicht zwingend SemVer folgen, muss die Versionserkennung pro Image konfigurierbar sein.

Beispiel:

```toml
[docker.images."grafana/grafana"]
version_strategy = "semver"
allow_patch = true
allow_minor = true
allow_major = false
```

Für PostgreSQL beispielsweise:

```toml
[docker.images."postgres"]
version_strategy = "major-pinned"
```

---

# 26. Docker-Scan

```bash
lxcup docker scan 103
```

Beispiel:

```text
docker01 [103]

Service        Image                    Status
---------------------------------------------------
grafana        grafana/grafana:12       UPDATE
postgres       postgres:17              CURRENT
redis          redis:8                  CURRENT
uptime-kuma    louislam/uptime-kuma:1   UPDATE
```

Details:

```bash
lxcup docker show 103 grafana
```

Beispiel:

```text
Service:
grafana

Configured:
grafana/grafana:12

Local digest:
sha256:AAA

Registry digest:
sha256:BBB

Update available.
```

---

# 27. Docker Update ausführen

Nur explizit:

```bash
lxcup docker update 103 grafana
```

Update-Plan:

```text
Docker Update Plan

LXC:
103 docker01

Compose service:
grafana

Current:
grafana/grafana:12
sha256:AAA

Target:
grafana/grafana:12
sha256:BBB

Actions:

1. Optional LXC snapshot
2. Pull image
3. Recreate grafana service
4. Verify service
5. Run healthcheck

Continue? [y/N]
```

---

# 28. Docker MVP nur für Compose

Unterstützung zunächst:

```text
Docker Compose:
SCAN + UPDATE
```

Standalone-Container:

```text
docker run:
SCAN ONLY
```

Grund:

Ein frei gestarteter Container kann viele Laufzeitparameter besitzen:

```text
Volumes
Ports
Networks
Environment
Secrets
Capabilities
Devices
Restart Policy
```

Compose ist deklarativ und daher wesentlich sicherer automatisiert rekonstruierbar.

---

# 29. Snapshot vor Docker-Updates

Beispiel:

```bash
lxcup docker update 103 grafana --snapshot
```

Ablauf:

```text
Docker Update
      ↓
LXC Snapshot
      ↓
Image Pull
      ↓
Compose Service neu erstellen
      ↓
Healthcheck
```

Bei fehlgeschlagenem Healthcheck:

```text
WARNUNG
```

Kein automatischer Rollback.

---

# 30. Gemeinsame TUI für OS- und Docker-Updates

Beispiel:

```text
┌────────────────────────────────────────────────────────────────────┐
│ lxcup                                         pve01                │
├────────────────────┬───────────────────────────────────────────────┤
│ Containers         │ docker01 [103]                                │
│                    │                                               │
│ grafana        4   │ OS Updates                                    │
│ uptime-kuma    0   │                                               │
│ > docker01     6   │ 2 Security                                    │
│ wazuh          3   │ 4 Normal                                      │
│                    │                                               │
│                    │ Docker Images                                 │
│                    │                                               │
│                    │ Grafana       UPDATE                          │
│                    │ PostgreSQL    CURRENT                         │
│                    │ Redis         CURRENT                         │
│                    │ Uptime Kuma   UPDATE                          │
├────────────────────┴───────────────────────────────────────────────┤
│ [Scan] [OS Updates] [Docker Updates] [Snapshots] [History]         │
└────────────────────────────────────────────────────────────────────┘
```

---

# 31. Zentraler Update-Manager

Langfristige Architektur:

```text
                         lxcup Controller
                        ┌────────────────┐
                        │ Rust / Axum    │
                        │ Web UI         │
                        │ REST API       │
                        │ Database       │
                        └───────┬────────┘
                                │
              ┌─────────────────┼─────────────────┐
              │                 │                 │
              ▼                 ▼                 ▼
          Proxmox 01        Proxmox 02        Proxmox 03
              │                 │                 │
          lxcup-agent       lxcup-agent       lxcup-agent
              │                 │                 │
         ┌────┼────┐       ┌────┼────┐        ┌──┴──┐
        LXC  LXC  LXC     LXC  LXC  LXC      LXC  LXC
```

---

# 32. Warum Agenten?

Der zentrale Controller sollte nicht einfach Root-SSH-Zugänge zu allen Proxmox-Nodes besitzen.

Stattdessen läuft lokal:

```text
lxcup-agent
```

Der Agent darf kontrolliert:

```text
pct list
pct exec
pct snapshot
pct rollback
```

ausführen.

---

# 33. Langfristige Komponenten

```text
lxcup
```

CLI/TUI

```text
lxcup-agent
```

Proxmox-Node-Agent

```text
lxcup-server
```

Zentrale API und Webserver

```text
lxcup-core
```

Gemeinsame Domain- und Update-Logik

---

# 34. Zentrales Dashboard

Beispiel:

```text
Infrastructure Updates

Proxmox Nodes:       2
LXC Containers:     18

OS Updates:          42
Security Updates:     8
Docker Updates:       6
Reboot Required:      3
```

Darunter:

```text
pve01

101 Grafana
OS       2 updates
Docker   1 update

102 Uptime Kuma
OS       Current
Docker   Current

103 Docker
OS       5 updates
Docker   4 updates
```

---

# 35. Was automatisch passieren darf

Erlaubt:

```text
Scannen
Daten sammeln
Registry-Digests prüfen
Security Updates markieren
Dashboard aktualisieren
Benachrichtigungen erzeugen
```

Nicht automatisch:

```text
apt upgrade
apt install
docker pull
docker compose up
reboot
rollback
```

---

# 36. Automatische Scans

Beispiel:

```text
alle 6 Stunden
     ↓
LXC Scan
     ↓
Docker Registry Scan
     ↓
Dashboard aktualisieren
```

Beispiel Benachrichtigung:

```text
5 new security updates detected.

pve01
├── grafana: 2
├── wazuh: 2
└── docker01: 1
```

---

# 37. Audit Log

Beispiel OS-Update:

```text
2026-09-09 17:31:14
USER: niklas
ACTION: OS_UPDATE
NODE: pve01
LXC: 103 docker01
PLAN: e071ab

REQUESTED:
openssl

ACTUAL:
openssl 3.5.0 → 3.5.1
libssl3 3.5.0 → 3.5.1

SNAPSHOT:
lxcup-103-20260909-173100

RESULT:
SUCCESS
```

Beispiel Docker:

```text
ACTION: DOCKER_UPDATE

SERVICE:
grafana

OLD DIGEST:
sha256:AAA

NEW DIGEST:
sha256:BBB

RESULT:
SUCCESS
```

---

# 38. Projektphasen

## Phase 0 – Fundament

- Cargo Workspace
- Domain Models
- Error Handling
- Logging
- Config
- Unit Tests
- `cargo fmt`
- `cargo clippy`
- `cargo test`

---

## Phase 1 – Proxmox Discovery

Ziel:

```bash
lxcup list
```

Implementieren:

- `pct list`
- `pct config`
- Containerstatus
- Hostname
- OS-Erkennung
- Error Handling bei gestoppten Containern

---

## Phase 2 – APT Scanner

Ziel:

```bash
lxcup scan
lxcup show
```

Implementieren:

- `apt update`
- installierte Version
- Candidate-Version
- Security-Klassifizierung
- gehaltene Pakete
- Scan-Ergebnisse persistieren

Noch keine Updates.

---

## Phase 3 – Update Planner

Ziel:

```bash
lxcup plan
```

Implementieren:

- Paketauswahl
- APT Simulation
- Dependency-Auflösung
- Safety Rules
- Plan Hash
- Blockierung gefährlicher Änderungen

Noch keine Installation.

---

## Phase 4 – Update Executor

Ziel:

```bash
lxcup update
```

Implementieren:

- Simulation
- Benutzerbestätigung
- Plan erneut prüfen
- gezieltes Update
- Logging
- Ergebnisstatus

---

## Phase 5 – Snapshot

Implementieren:

```text
--snapshot
snapshots
rollback
```

Zusätzlich:

- aussagekräftige Snapshot-Namen
- Beschreibung
- Abbruch bei fehlgeschlagenem angefordertem Snapshot

---

## Phase 6 – Healthchecks

Implementieren:

```text
HTTP
TCP
systemd
reboot-required
```

Kein automatischer Rollback.

---

## Phase 7 – TUI

Mit `ratatui`.

Funktionen:

- Container auswählen
- Updates auswählen
- Dry Run
- Snapshot
- Update
- Reboot
- History

---

## Phase 8 – Docker Scanner

Ziel:

```bash
lxcup docker scan
lxcup docker show
```

Implementieren:

- Docker erkennen
- laufende Container erkennen
- Compose-Projekte erkennen
- Image Reference
- lokaler Digest
- Registry Digest
- Update Status

Noch keine Docker-Updates.

---

## Phase 9 – Docker Update Manager

Ziel:

```bash
lxcup docker update
```

Nur Compose-managed Services.

Ablauf:

```text
Select
 ↓
Plan
 ↓
Snapshot optional
 ↓
Confirm
 ↓
Pull
 ↓
Recreate selected service
 ↓
Healthcheck
 ↓
Audit log
```

---

## Phase 10 – Docker Version Intelligence

Optional.

Ziel:

```text
Patch
Minor
Major
```

Mit konfigurierbarer Versionierungsstrategie je Image.

---

## Phase 11 – Node Agent

Binary:

```text
lxcup-agent
```

Auf jedem Proxmox-Host.

Funktionen semantisch:

```text
GET inventory
GET scan
GET plans

POST execute-plan
POST snapshot
POST reboot
```

Transport später beispielsweise über HTTPS/gRPC.

---

## Phase 12 – Zentraler Controller

Binary:

```text
lxcup-server
```

Technologie:

```text
Rust
Axum
Tokio
Serde
SQLx
SQLite oder PostgreSQL
```

Spätere Security:

```text
OIDC
mTLS
RBAC
```

---

## Phase 13 – Webinterface

Bereiche:

```text
Dashboard
├── Nodes
├── LXCs
├── OS Updates
├── Docker Updates
├── Security Updates
├── Snapshots
├── Update Plans
├── History
└── Settings
```

---

# 39. Zielarchitektur

```text
         UI
          │
          ▼
    Update Service
          │
          ▼
     lxcup-core
      /       \
     /         \
APT Adapter   Docker Adapter
     \         /
      \       /
   Execution Layer
          │
          ▼
      Proxmox
```

Wichtige Regel:

- TUI führt niemals selbst `apt` aus.
- Webinterface führt niemals selbst `pct` aus.
- CLI führt die Geschäftslogik nicht selbst aus.
- Alle Frontends verwenden denselben Core.

---

# 40. Endzustand

```text
                  LXCUP
         Proxmox Update Manager

                 ┌─────────┐
                 │ Web UI  │
                 └────┬────┘
                      │
                ┌─────▼─────┐
                │Controller │
                └─────┬─────┘
                      │
           ┌──────────┼───────────┐
           │                      │
        pve01                    pve02
           │                      │
     ┌─────┼─────┐          ┌─────┼─────┐
     │     │     │          │     │     │
    101   102   103        201   202   203
     │           │
    APT         APT
                 │
               Docker
              /      \
         Grafana    PostgreSQL
```

Das System kann automatisch erkennen:

```text
LXC Updates
Security Updates
Docker Digest Updates
Docker Versions
Reboot Requirement
Health Status
```

Verändernde Aktionen folgen immer:

```text
User
 ↓
Auswahl
 ↓
Plan
 ↓
Dry Run
 ↓
Bestätigung
 ↓
Snapshot optional
 ↓
Ausführung
```

---

# 41. Empfohlener MVP

Der erste tatsächlich nutzbare Meilenstein sollte nur Folgendes enthalten:

```text
lxcup list
lxcup scan
lxcup show <id>
lxcup plan <id> <package...>
lxcup update <id> <package...>
lxcup update <id> --security
lxcup update <id> --all
lxcup update ... --snapshot
lxcup snapshots <id>
```

Mit diesen Eigenschaften:

- Debian/Ubuntu zunächst als unterstützte Distributionen
- keine Distribution-Upgrades
- keine Paketentfernungen
- keine Downgrades
- Dry-Run verpflichtend
- erneute Plan-Prüfung unmittelbar vor Installation
- explizite Bestätigung
- optionaler Snapshot
- Audit Log
- Reboot-Erkennung

Erst danach sollte Docker ergänzt werden.

---

# 42. Definition of Done für Version 1

Version 1 ist fertig, wenn:

- LXCs zuverlässig erkannt werden,
- gestoppte Container sauber behandelt werden,
- Updates korrekt erkannt werden,
- Paketversionen angezeigt werden,
- Security-Updates klassifiziert werden können,
- einzelne Pakete auswählbar sind,
- APT-Abhängigkeiten vorab angezeigt werden,
- Paketentfernungen blockiert werden,
- Plans bei Änderungen invalidiert werden,
- Snapshots optional möglich sind,
- Installationen ausschließlich nach Bestätigung laufen,
- alle Aktionen protokolliert werden,
- Reboot-Bedarf angezeigt wird,
- Unit Tests für Parser, Planner und Safety Rules existieren,
- Integrationstests gegen eine Testumgebung vorhanden sind,
- `cargo fmt`, `cargo clippy` und `cargo test` erfolgreich durchlaufen.

---

# 43. Spätere Docker Definition of Done

Die Docker-Erweiterung ist fertig, wenn:

- Docker innerhalb eines LXC erkannt wird,
- Compose-Projekte erkannt werden,
- konfigurierte Images ermittelt werden,
- lokale und Registry-Digests verglichen werden,
- Image-Updates angezeigt werden,
- keine Docker-Updates automatisch ausgeführt werden,
- ein Compose-Service gezielt ausgewählt werden kann,
- vor dem Update ein Plan angezeigt wird,
- optional ein LXC-Snapshot erstellt wird,
- nur der ausgewählte Compose-Service aktualisiert wird,
- anschließend Healthchecks laufen,
- jede Aktion im Audit Log landet,
- Standalone-Container zunächst nur gescannt und nicht automatisch rekonstruiert werden.

---

# 44. Projektleitlinie

`lxcup` soll kein autonomes Auto-Update-System sein.

Es soll ein kontrolliertes Administrationswerkzeug sein, das dem Administrator die Arbeit beim Erkennen, Bewerten, Planen und Ausführen von Updates abnimmt, ohne ihm die Entscheidung abzunehmen.

---

# 45. Architekturentscheidung – lxcup als Management-LXC

`lxcup` läuft selbst nicht direkt auf dem Proxmox-Host, sondern als eigener LXC-Container innerhalb der Proxmox-Umgebung.

Beispiel:

```text
Proxmox Host
│
├── LXC 100: lxcup
│   ├── Weboberfläche
│   ├── Backend/API
│   ├── lxcup-core
│   └── PostgreSQL-Verbindung
│
├── LXC 101: grafana
├── LXC 102: uptime-kuma
├── LXC 103: docker01
└── LXC 104: monitoring
```

Die Weboberfläche ist die primäre Bedienoberfläche. CLI und TUI sind optionale spätere Zusatzoberflächen, verwenden aber dieselbe Geschäftslogik.

## 45.1 Kommunikation mit Proxmox

`lxcup` kommuniziert aus seinem Management-LXC über HTTPS mit der Proxmox-REST-API.

```text
Browser
   ↓
lxcup Web/API im Management-LXC
   ↓ HTTPS mit eigenem API-Token
Proxmox REST API
   ↓
Proxmox-LXC und Proxmox-Tasks
```

Der Management-LXC erhält keinen direkten Zugriff auf Proxmox-Hostdateien wie `/etc/pve` und benötigt keine unkontrollierte Host-Shell. Insbesondere soll `lxcup` nicht davon abhängig sein, innerhalb seines eigenen LXC direkt `pct` auf dem Proxmox-Host auszuführen.

Der Zugriff auf Proxmox wird über einen eigenen, möglichst eingeschränkten Proxmox-Benutzer beziehungsweise API-Token durchgeführt. Das Token wird ausschließlich im Backend verwendet und niemals an den Browser übertragen.

## 45.2 Automatische Aufnahme vorhandener LXC-Container

Beim ersten Verbinden mit einem Proxmox-Node liest `lxcup` das vorhandene LXC-Inventar aus und zeigt es in der Weboberfläche an.

```text
Proxmox API
    ↓
LXC-Inventar erkennen
    ↓
Container in lxcup anzeigen
    ↓
Administrator entscheidet:
    ├── verwalten
    ├── ignorieren
    └── deaktivieren
```

Das Aufnehmen eines bestehenden Containers verändert diesen nicht. Es bedeutet zunächst nur, dass der Container in lxcup bekannt ist und gescannt werden darf.

Empfohlene Zustände:

```text
discovered → managed
      ↓
   ignored
      ↓
  disabled
```

Ein erneuter Discovery-Lauf muss neue Container erkennen und entfernte Container als nicht mehr erreichbar oder entfernt markieren, ohne historische Daten zu löschen.

## 45.3 Verantwortlichkeiten der Webanwendung

Die Webanwendung steuert den vollständigen Ablauf:

```text
Container auswählen
        ↓
Scan starten
        ↓
Updates auswählen
        ↓
Update-Plan erstellen
        ↓
Dry-Run anzeigen
        ↓
Benutzer bestätigt
        ↓
Plan erneut validieren
        ↓
Snapshot optional erstellen
        ↓
Update ausführen
        ↓
Healthcheck und Audit-Log
```

Die Weboberfläche führt niemals direkt `apt`, `docker`, `pct` oder andere Systembefehle aus. Sie ruft ausschließlich Backend-Services auf.

## 45.4 PostgreSQL als zentrale Datenbank

Die Hauptanwendung verwendet PostgreSQL als zentrale Datenbank. PostgreSQL ist die dauerhafte Quelle der Wahrheit für die gesamte lxcup-Verwaltung.

Für den Produktivbetrieb wird die bereits vorhandene PostgreSQL-Datenbank in einem separaten PostgreSQL-LXC verwendet.

```text
┌─────────────────────────────┐
│ LXC lxcup                   │
│ Web UI, Backend, lxcup-core │
└──────────────┬──────────────┘
               │ privates Netzwerk / TLS
               ▼
┌─────────────────────────────┐
│ Separater PostgreSQL-LXC    │
│ Produktive zentrale DB      │
└─────────────────────────────┘
```

PostgreSQL wird nicht in den `lxcup`-LXC eingebettet und nicht als Bestandteil der lxcup-Anwendung mitinstalliert. Der `lxcup`-LXC enthält nur die Zugangskonfiguration und den Datenbank-Client.

Zentrale Datenbereiche:

```text
nodes
containers
container_settings
scans
scan_updates
update_plans
update_plan_items
executions
execution_events
snapshots
healthchecks
docker_services
docker_images
users
roles
audit_log
```

Die produktive Datenbank wird über ein dediziertes Datenbankschema beziehungsweise einen dedizierten Datenbankbenutzer für lxcup angesprochen. Zugangsdaten dürfen nicht im Quellcode oder in Git gespeichert werden.

Datenbankmigrationen werden versioniert und beim Deployment kontrolliert ausgeführt. Backups, Replikation und Wiederherstellung der PostgreSQL-Datenbank gehören zur Infrastruktur des separaten Datenbank-LXC.

Die Anwendung darf keine kritischen Zustände ausschließlich im Arbeitsspeicher halten. Update-Pläne, Bestätigungen, Ausführungszustände und Audit-Daten müssen persistent gespeichert werden.

## 45.5 Optionale Agenten mit SQLite

Agenten werden erst für mehrere Proxmox-Nodes oder für eine stärker verteilte Architektur benötigt.

```text
lxcup Controller
   ↓
PostgreSQL
   ↓
lxcup-agent auf jedem Proxmox-Node
   ├── SQLite
   └── lokale Proxmox-Kommunikation
```

Ein Agent darf SQLite für lokale, wiederherstellbare Zustände verwenden:

- lokaler Cache,
- temporäre Scan-Ergebnisse,
- laufende Tasks,
- lokale Warteschlangen,
- Offline-Zustände,
- Wiederaufnahme nach einer Netzwerkunterbrechung.

SQLite im Agenten ist nicht die zentrale Historie. PostgreSQL im Controller bleibt die zentrale Quelle der Wahrheit. Agenten synchronisieren ihre Ergebnisse idempotent mit dem Controller und müssen nach einem Neustart oder einer Unterbrechung weiterarbeiten können.

## 45.6 Single-Node-MVP

Für den ersten nutzbaren Meilenstein wird kein Agent benötigt:

```text
lxcup-LXC
   ├── Weboberfläche
   ├── Backend/API
   ├── lxcup-core
   └── PostgreSQL-Client
          ↓
      Proxmox REST API
          ↓
      verwaltete LXCs
```

Der Single-Node-MVP umfasst:

- Installation von lxcup als eigenem LXC,
- Verbindung zu einem Proxmox-Node,
- automatische Erkennung bestehender LXC-Container,
- Verwaltung von Einschluss und Ausschluss,
- Webansicht für Container und Status,
- APT-Scan,
- Update-Plan und Dry-Run,
- explizite Bestätigung über die Weboberfläche,
- optionale Snapshots,
- Healthchecks,
- Audit-Logging,
- PostgreSQL-Persistenz.

Docker-Updates und mehrere Proxmox-Nodes werden auf dieser Basis später ergänzt.

## 45.7 Erweiterung auf mehrere Proxmox-Nodes

Bei mehreren Nodes wird die Architektur erweitert:

```text
                         lxcup Controller
                    ┌────────────────────┐
                    │ Web UI             │
                    │ Backend/API        │
                    │ lxcup-core         │
                    │ PostgreSQL         │
                    └─────────┬──────────┘
                              │
              ┌───────────────┼───────────────┐
              │               │               │
              ▼               ▼               ▼
         lxcup-agent     lxcup-agent     lxcup-agent
           pve01           pve02           pve03
              │               │               │
             LXC             LXC             LXC
```

Der Controller stellt die zentrale Weboberfläche, Benutzerverwaltung, Planung und Historie bereit. Die Agenten führen ausschließlich kontrollierte lokale Operationen auf ihrem jeweiligen Proxmox-Node aus.

---

# 46. Optionale Erweiterung – Windows-Agent

Neben Proxmox-LXC-Containern soll langfristig auch die Verwaltung von Windows-Systemen möglich sein. Dafür wird ein eigener Windows-Agent vorgesehen.

Der Windows-Agent läuft direkt auf einem Windows-System, zum Beispiel:

```text
Windows Server
└── lxcup-agent als Windows-Dienst
```

Er kann auf folgenden Systemen eingesetzt werden:

- Windows Server als virtuelle Maschine,
- Windows-VM innerhalb von Proxmox,
- physischer Windows-Server,
- später optional Windows-Desktop-Systeme.

Windows-Systeme werden nicht als LXC behandelt. Sie erscheinen in lxcup als eigener Zieltyp.

## 46.1 Gemeinsames Agentenmodell

Der Controller verwaltet unterschiedliche Zieltypen über eine gemeinsame Abstraktion:

```text
Target
├── ProxmoxNode
├── LxcContainer
└── WindowsHost
```

```text
lxcup Controller
       │
       ├── Proxmox API / Linux-Agent
       │      └── LXC-Container
       │
       └── Windows-Agent
              └── Windows-System
```

Die gemeinsame Geschäftslogik bleibt:

```text
SCAN → PLAN → BESTÄTIGUNG → APPLY
```

Die Betriebssystem-spezifischen Schritte werden jedoch durch eigene Adapter umgesetzt.

## 46.2 Aufgaben des Windows-Agenten

Der Windows-Agent kann später folgende Funktionen anbieten:

- Betriebssystem und Windows-Version erkennen,
- installierte Updates erkennen,
- verfügbare Windows-Updates scannen,
- Security-Updates markieren,
- Neustartbedarf erkennen,
- Windows-Dienste prüfen,
- definierte Healthchecks ausführen,
- Update-Pläne lokal ausführen,
- Ausführungsstatus und Logs an den Controller melden.

Optionale Paketquellen wie WinGet, Chocolatey oder andere Paketmanager werden getrennt vom nativen Windows-Update-Adapter behandelt.

## 46.3 Kommunikation

Der Windows-Agent baut ausschließlich eine ausgehende, verschlüsselte Verbindung zum Controller auf.

```text
Windows-Agent
      │
      │ HTTPS oder sichere persistente Verbindung
      ▼
lxcup Controller
      │
      ▼
PostgreSQL
```

Der Controller benötigt keinen direkten eingehenden Remote-Desktop-, WinRM- oder PowerShell-Zugang zum Windows-System.

Für die Authentifizierung sollen langfristig gerätebezogene Credentials, Zertifikate oder mTLS verwendet werden. Ein Agent darf nur für das registrierte Zielsystem und nur für erlaubte Operationen verwendet werden.

## 46.4 Sicherheitsregeln

Der Windows-Agent darf keine beliebigen vom Server gelieferten Shell- oder PowerShell-Strings ausführen.

Stattdessen werden typisierte Operationen verwendet:

```text
ScanWindowsUpdates
CreateWindowsUpdatePlan
ApplyWindowsUpdatePlan
CheckWindowsService
CheckWindowsRebootRequired
RunConfiguredHealthcheck
```

Jeder Plan enthält mindestens:

```text
target_id
requested_updates
resolved_changes
reboot_required
plan_hash
created_at
```

Vor der Ausführung wird der Plan erneut validiert. Die Bestätigung erfolgt ausschließlich über die Weboberfläche oder eine später ausdrücklich autorisierte alternative Oberfläche.

## 46.5 Lokale Agent-Daten

Der Windows-Agent darf für Cache, Offline-Zustände, lokale Tasks und Wiederaufnahme SQLite verwenden. PostgreSQL im Controller bleibt die zentrale Quelle der Wahrheit.

```text
Windows-Agent
├── Agent-Konfiguration
├── lokaler Task-Status
├── Offline-Warteschlange
└── SQLite-Cache
```

Der Agent muss auch bei einer vorübergehenden Unterbrechung der Verbindung sicher weiterarbeiten oder einen laufenden Vorgang eindeutig als unklar markieren können. Ein Update darf nicht aufgrund einer verlorenen Verbindung unkontrolliert erneut gestartet werden.

## 46.6 Produktumfang

Der Windows-Agent ist zunächst eine Architekturkompatibilität und kein Bestandteil des Single-Node-Linux-MVP.

Empfohlene Reihenfolge:

1. Webbasierter Single-Node-MVP für Proxmox-LXC
2. PostgreSQL und zentrale Plan-/Audit-Logik
3. Linux-Agent für mehrere Proxmox-Nodes
4. Windows-Agent mit Inventar und Scan
5. Windows-Update-Planung
6. Explizite Windows-Update-Ausführung
7. Optionale WinGet-/Chocolatey-Integration

Damit bleibt die Hauptanwendung betriebssystemübergreifend erweiterbar, ohne die sichere LXC-/APT-Implementierung unnötig zu verkomplizieren.

---

# 47. Agent-Telemetrie – Logs und Metriken

Alle Agenten liefern strukturierte Logs, Metriken und Zustandsereignisse an den lxcup-Controller.

Das gilt für:

- Linux-Agenten auf Proxmox-Nodes,
- den späteren Windows-Agenten,
- zukünftige weitere Agenten-Typen.

```text
Agent
├── Logs
├── Metriken
├── Heartbeats
└── Zustandsereignisse
        ↓
   lxcup Controller
        ↓
   PostgreSQL + Weboberfläche
```

## 47.1 Strukturierte Logs

Logs werden strukturiert und nicht nur als unformatierter Text übertragen.

Beispiel:

```json
{
  "timestamp": "2026-09-16T18:30:00Z",
  "level": "INFO",
  "agent_id": "agent-pve01",
  "target_id": "lxc-103",
  "component": "apt-scanner",
  "event": "scan_completed",
  "message": "APT scan completed",
  "fields": {
    "updates": 8,
    "security_updates": 2
  }
}
```

Mindestens unterstützte Log-Level:

```text
TRACE
DEBUG
INFO
WARN
ERROR
```

Logs sollen über die Weboberfläche filterbar sein nach:

- Node oder Agent,
- Container oder Windows-Host,
- Zeitraum,
- Log-Level,
- Komponente,
- Plan oder Execution,
- Ereignistyp.

Geheimnisse, API-Tokens, Passwörter und sensible Werte dürfen nicht in Logs geschrieben werden. Befehlsargumente werden vor der Protokollierung bereinigt oder nur als typisierte Operation dargestellt.

## 47.2 Agent-Metriken

Agenten liefern technische und fachliche Metriken.

Technische Metriken:

```text
agent_up
agent_last_heartbeat_timestamp
agent_version_info
agent_uptime_seconds
agent_cpu_usage
agent_memory_usage
agent_disk_usage
agent_queue_size
agent_pending_tasks
agent_connection_state
```

Fachliche Metriken:

```text
scan_total
scan_success_total
scan_failure_total
scan_duration_seconds
updates_detected_total
security_updates_detected_total
plans_created_total
plans_invalidated_total
executions_total
executions_success_total
executions_failure_total
healthchecks_total
healthcheck_failures_total
reboots_required
```

Metriken müssen eine kontrollierte Anzahl von Labels besitzen. Paketnamen, frei eingegebene Fehlermeldungen und andere Werte mit hoher Kardinalität dürfen nicht unkontrolliert als Metrik-Labels verwendet werden.

## 47.3 Heartbeats und Agent-Zustand

Jeder Agent sendet regelmäßig einen Heartbeat an den Controller.

```text
CONNECTED
DEGRADED
OFFLINE
UNKNOWN
```

Ein Heartbeat enthält mindestens:

```text
agent_id
agent_version
host_id
last_seen_at
capabilities
running_task_ids
queue_status
```

Der Controller zeigt in der Weboberfläche an, ob ein Agent erreichbar ist. Ein ausgebliebener Heartbeat darf nicht automatisch als erfolgreicher oder fehlgeschlagener Update-Vorgang interpretiert werden. Ein laufender Vorgang wird stattdessen als `UNKNOWN` beziehungsweise `REQUIRES_RECONCILIATION` markiert und muss sicher aufgelöst werden.

## 47.4 Übertragung und lokale Pufferung

Agenten übertragen Telemetrie über die bereits verwendete ausgehende, verschlüsselte Verbindung zum Controller.

Bei einer vorübergehenden Unterbrechung:

```text
Agent erzeugt Log/Metrik/Ereignis
        ↓
lokale Queue oder SQLite
        ↓ Verbindung wieder verfügbar
Übertragung in ursprünglicher Reihenfolge
        ↓
Controller bestätigt Verarbeitung
```

Die lokale Queue muss begrenzt sein. Bei Überlauf werden alte, niedrig priorisierte Debug-Logs zuerst verworfen. Sicherheits-, Fehler- und Ausführungsereignisse dürfen nicht stillschweigend verloren gehen.

Jedes übertragene Ereignis erhält eine eindeutige ID. Die Verarbeitung im Controller ist idempotent, damit Wiederholungen nach einem Verbindungsabbruch keine doppelten Ereignisse erzeugen.

## 47.5 Speicherung und Anzeige

PostgreSQL speichert die für Betrieb, Audit und Fehleranalyse benötigten Logs, Metriken und Ereignisse.

Für den ersten MVP genügt PostgreSQL als gemeinsame Speicherung. Eine spätere Version kann zusätzlich eine spezialisierte Metriklösung wie Prometheus oder OpenTelemetry ergänzen, ohne die fachliche Historie aus PostgreSQL zu entfernen.

Die Weboberfläche soll mindestens folgende Ansichten anbieten:

- Agent-Übersicht mit Online-/Offline-Status,
- letzte Heartbeats,
- Agent-Versionen,
- CPU-, Speicher- und Festplattenstatus,
- Scan- und Update-Erfolgsraten,
- fehlgeschlagene Ausführungen,
- aktuelle Tasks und Warteschlangen,
- Logs pro Agent, Container oder Windows-Host,
- zeitlicher Verlauf von Fehlern und Update-Aktivität.

## 47.6 Telemetrie ist beobachtend, nicht verändernd

Das Sammeln von Logs und Metriken darf keine Update-Aktion auslösen.

```text
Telemetry
   ≠
Update Execution
```

Ein kritischer Agent-Zustand erzeugt eine Warnung oder einen sichtbaren Fehler im Dashboard. Ein automatischer Neustart, Rollback oder Update wird dadurch nicht ausgelöst, sofern dies nicht später als ausdrücklich konfigurierbare und separat bestätigte Funktion eingeführt wird.

---

# 48. Entwicklungsstrategie

Entwicklung, Integrationstests und Produktion werden strikt voneinander getrennt.

```text
Lokale Entwicklung
        ↓
Proxmox-Testumgebung
        ↓
Produktion
```

## 48.1 Lokale Entwicklung

Für die tägliche Entwicklung läuft die Rust-Anwendung lokal auf dem Entwicklungsrechner.

```text
Entwicklungsrechner
├── lxcup Backend lokal
├── Weboberfläche im Entwicklungsmodus
├── PostgreSQL-Dev-Instanz
└── Mock- und Testadapter
```

Die PostgreSQL-Dev-Instanz läuft bevorzugt als reproduzierbarer, versionsmäßig festgelegter PostgreSQL-Container. Alternativ kann ein separater Entwicklungs-PostgreSQL-LXC verwendet werden.

Die Entwicklungsdatenbank ist vollständig von der produktiven PostgreSQL-Datenbank getrennt. Es werden keine Produktivdaten und keine Produktiv-Zugangsdaten für lokale Entwicklung verwendet.

Für die lokale Entwicklung werden benötigt:

- versionierte Datenbankmigrationen,
- ein reproduzierbares Datenbankschema,
- Testdaten und Seeds,
- eine `.env.example` ohne echte Geheimnisse,
- lokale Konfiguration außerhalb des Repositorys,
- Mock-Adapter für Proxmox, APT, Docker und Windows.

## 48.2 Adapter statt echter Systeme im Unit-Test

Die Geschäftslogik wird nicht von einer laufenden Proxmox- oder Windows-Umgebung abhängig gemacht.

```text
lxcup-core
   ├── echter Proxmox-Adapter
   ├── echter APT-Adapter
   ├── echter Docker-Adapter
   ├── echter Windows-Adapter
   └── Mock-/Fixture-Adapter für Tests
```

Unit- und die meisten Integrationstests laufen mit kontrollierten Fixtures. Dadurch können gefährliche Aktionen simuliert werden, ohne echte Updates, Reboots oder Snapshots auszuführen.

## 48.3 Isolierte Proxmox-Testumgebung

Zusätzlich zur lokalen Entwicklung wird eine separate Proxmox-Testumgebung benötigt.

```text
Proxmox-Testnode
├── LXC lxcup-test
├── LXC postgres-test
├── LXC debian-test
├── LXC ubuntu-test
└── LXC docker-test
```

Diese Umgebung darf niemals produktive Container, produktive Snapshots oder die produktive PostgreSQL-Datenbank verwenden.

Die Testumgebung wird für echte Integrationstests verwendet:

- Proxmox-API-Verbindung,
- LXC-Discovery,
- Statusabfragen,
- APT-Scans,
- Dry-Runs,
- Snapshot-Erstellung,
- Healthchecks,
- Reboot-Erkennung,
- Docker-Compose-Scans,
- Agent-Kommunikation.

Verändernde Tests dürfen nur auf ausdrücklich dafür vorgesehenen Test-LXCs laufen.

## 48.4 Staging vor Produktion

Vor einem produktiven Deployment läuft lxcup zunächst in einem eigenen Staging-LXC und verwendet eine eigene Staging-Datenbank.

```text
LXC lxcup-staging
        ↓
PostgreSQL-staging
```

Staging verwendet dieselbe Deployment-Struktur und möglichst dieselben Versionen wie Produktion, aber eigene Zugangsdaten, Datenbanken, Nodes und Container.

## 48.5 Produktionsumgebung

Die Produktion besteht aus:

```text
LXC lxcup
        ↓ privates Netzwerk / TLS
bestehender PostgreSQL-LXC
```

Die produktive Datenbank wird ausschließlich von der produktiven lxcup-Instanz verwendet. Migrationen und Deployments werden vorab in der Test- oder Staging-Umgebung geprüft.

## 48.6 Teststufen

Die Entwicklung folgt mehreren Teststufen:

```text
Unit Tests
    ↓
Core- und Adapter-Integrationstests
    ↓
PostgreSQL-Integrationstests
    ↓
Proxmox-Testumgebung
    ↓
Staging
    ↓
Produktion
```

Mindestens erforderlich sind:

- Parser- und Modelltests,
- Tests für Safety Rules,
- Planner-Tests,
- Plan-Hash- und Invalidierungstests,
- Datenbankmigrationstests,
- API- und Authentifizierungstests,
- Mock-Adapter-Tests,
- Integrationstests gegen Test-LXCs,
- Agent-Reconnect- und Offline-Queue-Tests.

## 48.7 Sicherheitsregeln für Entwicklung

- Keine Entwicklungsanwendung verbindet sich mit der Produktionsdatenbank.
- Keine echten Produktiv-API-Tokens in lokalen Konfigurationsdateien.
- Keine echten Paket- oder Docker-Updates in Unit-Tests.
- Keine Tests mit automatischem Zugriff auf Produktiv-Snapshots.
- Produktive Container dürfen nicht als Testziele konfiguriert werden.
- Test- und Produktionsdatenbanken verwenden getrennte Benutzer und Credentials.
- Entwicklungslogs dürfen keine Secrets oder vollständigen Zugangsdaten enthalten.

---

# 49. Frontend-Technologie

Für die Weboberfläche wird React mit TypeScript und Vite verwendet.

```text
React
├── TypeScript
├── Vite
├── React Router
├── TanStack Query
├── TanStack Table
├── Tailwind CSS / UI-Komponenten
└── später Diagramm- und Telemetrie-Komponenten
```

## 49.1 Warum React und Vite

`lxcup` ist eine interaktive Administrationsoberfläche und keine öffentlich indexierte Website. Die wichtigen Anforderungen sind:

- viele Tabellen und Filter,
- Statusanzeigen,
- Update-Auswahl,
- komplexe Bestätigungsdialoge,
- Logs und Ausführungsfortschritt,
- Dashboard-Karten und Metriken,
- wiederverwendbare Formulare,
- gute TypeScript-Unterstützung.

React bietet dafür ein komponentenbasiertes Modell. Vite übernimmt den schnellen Entwicklungsserver mit Hot Module Replacement und erzeugt optimierte statische Produktionsdateien.

Ein zusätzliches Next.js-Backend wird zunächst nicht benötigt. Das Rust-Backend bleibt die zentrale API, Geschäftslogik und Sicherheitsgrenze.

## 49.2 Frontend und Rust-Backend

Während der Entwicklung laufen Frontend und Backend getrennt:

```text
Vite Dev Server
      │
      │ REST / SSE über Proxy
      ▼
Rust Backend / Axum
      │
      ▼
PostgreSQL und Agenten
```

Für Produktion wird das Frontend als statische Anwendung gebaut. Die erzeugten Dateien können vom Rust-Backend oder von einem vorgeschalteten Webserver ausgeliefert werden.

Der Browser erhält keine direkten Zugangsdaten für PostgreSQL, Proxmox oder Agenten.

## 49.3 Datenzugriff

TanStack Query verwaltet den Serverzustand im Frontend:

- API-Abfragen,
- Caching,
- Aktualisierung,
- Mutation-Zustände,
- Fehler- und Ladezustände,
- erneutes Laden nach Änderungen.

Lokaler UI-Zustand bleibt davon getrennt. Beispielsweise gehören geöffnete Dialoge, Tab-Auswahl und temporäre Checkbox-Auswahl in den lokalen Frontend-Zustand, während Container, Scans und Update-Pläne aus der API kommen.

## 49.4 Tabellen und Verwaltungsansichten

TanStack Table wird für komplexe Tabellen verwendet:

- Containerübersicht,
- verfügbare Updates,
- Update-Planänderungen,
- Ausführungen,
- Logs,
- Agenten,
- Snapshots,
- Docker-Services,
- Windows-Systeme.

Erforderliche Tabellenfunktionen sind mindestens:

- Sortierung,
- Filterung,
- Pagination,
- Spaltenauswahl,
- Zeilenauswahl,
- Status-Badges,
- Detailansichten.

## 49.5 Live-Status und Fortschritt

Für laufende Scans, Updates, Agentenstatus und Logs wird zunächst Server-Sent Events (SSE) verwendet.

```text
Rust Backend
      ↓ SSE
Browser / React
```

SSE reicht für die überwiegend vom Server zum Browser fließenden Ereignisse aus:

- Task gestartet,
- Task fortgeschritten,
- Log-Ereignis,
- Healthcheck abgeschlossen,
- Agent offline,
- Update beendet.

WebSockets bleiben eine spätere Option, falls bidirektionale Echtzeitkommunikation für Agenten oder interaktive Terminals erforderlich wird.

## 49.6 Frontend-Sicherheitsregeln

- Das Frontend enthält keine Proxmox- oder Datenbank-Secrets.
- Aktionen werden serverseitig authentifiziert und autorisiert.
- Sicherheitskritische Bestätigungen werden im Backend geprüft.
- Das Frontend darf keine freien Shell-Kommandos anfordern.
- API-Fehler werden verständlich angezeigt, ohne sensible Backend-Details offenzulegen.
- Jede wichtige Aktion zeigt Ziel, Plan, erwartete Änderungen und Ausführungsstatus.

## 49.7 Frontend-Teststrategie

Vorgesehen sind:

- Komponenten- und Utility-Tests,
- API-Client-Tests,
- Tests für Update-Auswahl und Bestätigungsdialoge,
- Tests für Fehler- und Offline-Zustände,
- Browser-End-to-End-Tests für den vollständigen Scan-/Plan-/Apply-Ablauf.

Die kritischen Sicherheitsabläufe werden zusätzlich gegen ein Test-Backend und nicht nur gegen Mock-Daten getestet.

---

# 50. Offene Entscheidungen

Die folgenden Punkte müssen vor oder während der Implementierung noch verbindlich festgelegt werden.

## 50.1 Vor dem ersten produktiven Code

### MVP-Grenze

- Welche Funktionen gehören zwingend in den ersten lauffähigen Web-MVP?
- Sind zunächst nur Debian und Ubuntu erlaubt?
- Wird im MVP ausschließlich ein Proxmox-Node unterstützt?
- Soll der `lxcup`-Container selbst zunächst vom Update-Scan ausgeschlossen werden?

### Backend- und API-Vertrag

- Welche REST-Endpunkte benötigt das Frontend?
- Welche DTOs und Statuswerte werden zwischen Frontend und Backend verwendet?
- Wird die API mit OpenAPI dokumentiert und daraus ein TypeScript-Client erzeugt?
- Welche SSE-Ereignisse werden für Tasks, Logs und Agentenstatus definiert?

### Authentifizierung und Berechtigungen

- Gibt es im ersten MVP einen lokalen Administrator oder direkt mehrere Benutzer?
- Werden Sessions, JWT oder ein Reverse-Proxy-Login verwendet?
- Welche Rollen werden benötigt, zum Beispiel Administrator, Operator und Read-only?
- Wird Zwei-Faktor-Authentifizierung oder OIDC erst später ergänzt?

### Proxmox-Zugriff

- Wird ein Proxmox-Cluster oder zunächst nur ein einzelner Node registriert?
- Welche minimalen Proxmox-API-Berechtigungen benötigt lxcup?
- Wie werden API-Tokens und TLS-Zertifikate konfiguriert?
- Welche Operationen laufen direkt über die Proxmox-API und welche über Agenten?

### Datenmodell

- Welche Tabellen und Beziehungen werden für den ersten MVP benötigt?
- Welche Zustände besitzen Container, Scans, Pläne und Ausführungen?
- Wie werden historische Daten, Logs und Metriken aufbewahrt und gelöscht?
- Welche Datenbankmigrationstechnologie wird verwendet?

## 50.2 Während des MVP

### Update-Ausführung

- Wie wird mit laufenden APT-Prozessen und Paketmanager-Locks umgegangen?
- Wie werden gestoppte Container behandelt?
- Wird vor einem Update automatisch `apt update` ausgeführt oder nur gescannt?
- Wie genau funktioniert die Security-Klassifizierung für Debian und Ubuntu?
- Welche Timeout-, Abbruch- und Wiederaufnahme-Regeln gelten?

### Jobs und Parallelisierung

- Wie viele Scans dürfen parallel laufen?
- Werden Updates immer seriell ausgeführt?
- Wie werden doppelte oder gleichzeitig gestartete Pläne verhindert?
- Wie werden abgebrochene Tasks nach einem Backend-Neustart rekonstruiert?

### Frontend und UX

- Welche Dashboard-Ansicht ist die Startseite?
- Welche Schritte benötigt der Benutzer für eine Update-Bestätigung?
- Wie werden gefährliche Änderungen visuell hervorgehoben?
- Welche Tabellen-, Filter- und Suchfunktionen sind für den MVP zwingend?
- Welche Sprache(n) soll die Weboberfläche unterstützen?

### Deployment

- Wie wird der Rust-Server im `lxcup`-LXC betrieben, zum Beispiel systemd oder Container-Service?
- Liefert Axum die Frontend-Dateien aus oder wird ein Reverse Proxy verwendet?
- Wie werden Frontend, Backend und Datenbankmigrationen gemeinsam versioniert?
- Wie wird ein Rollback eines lxcup-Deployments durchgeführt?

## 50.3 Spätere Entscheidungen

- Agent-Protokoll und mTLS für mehrere Proxmox-Nodes
- Windows-Agent und genaue Windows-Update-Quellen
- Docker-Compose-Update-Strategie
- Prometheus/OpenTelemetry zusätzlich zu PostgreSQL
- OIDC, RBAC und Zwei-Faktor-Authentifizierung
- Benachrichtigungen per E-Mail, Webhook oder anderen Kanälen
- Webinterface für mehrere Nodes und Mandanten
- zentrale Update-Zeitpläne und Wartungsfenster

## 50.4 Empfohlene Entscheidungsreihenfolge

1. MVP-Grenze und Zielsysteme
2. Proxmox-Zugriff und Sicherheitsmodell
3. Domain- und Datenmodell
4. REST- und SSE-API
5. Authentifizierung und Rollen
6. Update- und Jobzustände
7. Deployment und Testumgebung
8. Frontend-Navigation und Detail-UX

Erst danach sollten wir die konkreten Rust-Crates, Datenbankmigrationen und Frontend-Seiten implementieren.

---

# 51. Projektwerkzeuge

## 51.1 GitHub

GitHub ist die zentrale Plattform für Repository, Issues, Pull Requests und Projektplanung.

Repository:

```text
https://github.com/niklasfulle/lxcup
```

Die bestätigten MVP-Arbeitspakete werden als getrennte GitHub-Issues mit klarer Beschreibung, Akzeptanzkriterien und Abhängigkeiten angelegt.

## 51.2 SonarQube

SonarQube wird für Codequalität, Sicherheitsanalyse, Coverage und weitere statische Prüfungen verwendet.

Geplante Nutzung:

- Analyse von Rust-Backend und Frontend,
- Prüfung auf Bugs und Sicherheitsprobleme,
- Überwachung der Testabdeckung,
- Prüfung von Duplikationen,
- Prüfung von Dependency-Risiken,
- Qualitäts-Gate vor produktiven Releases.

GitHub-Issues und SonarQube-Ergebnisse werden getrennt behandelt: GitHub beschreibt geplante Arbeit, SonarQube liefert technische Qualitäts- und Sicherheitsbefunde.

---

# 52. Git- und Branch-Workflow

Die Entwicklung erfolgt über issuebezogene Branches und Pull Requests. Direkte Entwicklungs-Commits auf `main` sind nicht vorgesehen.

## 52.1 Branch-Grundregeln

```text
main
 ├── feat/issue-13-rust-workspace
 ├── feat/issue-17-domain-entities
 ├── feat/issue-21-postgres-schema
 └── feat/issue-25-proxmox-api
```

Regeln:

- `main` enthält nur integrierten und geprüften Code.
- Jede Umsetzung erfolgt auf einem eigenen Branch.
- Ein Branch gehört möglichst genau zu einem GitHub-Issue.
- Ein Unterticket erhält einen eigenen Branch.
- Parent-Issues werden über mehrere gemergte Unterticket-Branches umgesetzt.
- Branches bleiben kurzlebig und werden nach dem Merge gelöscht.

## 52.2 Branch-Namenskonvention

Format:

```text
<typ>/issue-<nummer>-<kurzer-name>
```

Erlaubte Typen:

```text
feat      neue Funktion
fix       Fehlerbehebung
refactor  Strukturänderung ohne neue Funktion
test      Tests und Testinfrastruktur
docs      Dokumentation
chore     Wartung und Tooling
```

Beispiele:

```text
feat/issue-13-rust-workspace
feat/issue-21-postgres-schema
test/issue-24-postgres-integration
docs/issue-50-open-decisions
```

## 52.3 Pull-Request-Ablauf

```text
Issue auswählen
      ↓
Branch von main erstellen
      ↓
Implementieren und testen
      ↓
Branch zu GitHub pushen
      ↓
Pull Request erstellen
      ↓
CI und SonarQube prüfen
      ↓
Review und Korrekturen
      ↓
Squash-Merge nach main
      ↓
Branch löschen
```

Jeder Pull Request enthält mindestens:

- Referenz auf das zugehörige Issue,
- kurze Zusammenfassung,
- Testbeschreibung,
- bekannte Einschränkungen,
- Hinweise auf Datenbankmigrationen oder Konfigurationsänderungen.

Ein abgeschlossenes Issue wird über die Pull-Request-Beschreibung automatisch verknüpft, zum Beispiel:

```text
Closes #13
```

## 52.4 Qualitätsregeln für Pull Requests

- Kein Merge bei fehlgeschlagenem Build.
- Kein Merge bei fehlgeschlagenen Tests.
- Kein Merge bei fehlgeschlagenem Clippy- oder Format-Check.
- SonarQube-Befunde werden vor dem Merge geprüft.
- Sicherheitsrelevante Änderungen benötigen eine zusätzliche Prüfung.
- Große, thematisch gemischte Pull Requests sollen aufgeteilt werden.
- Datenbankmigrationen werden im Pull Request ausdrücklich beschrieben.

## 52.5 Entwicklungs- und Release-Branches

Für die aktuelle Projektphase wird kein dauerhafter `develop`-Branch benötigt. Kurzlebige Feature-Branches mit Pull Requests nach `main` halten den Ablauf übersichtlich.

Später können zusätzliche Branches eingeführt werden, falls Staging und Releases das erfordern:

```text
main       produktionsnaher stabiler Stand
staging    geprüfter Stand für Staging
release/*  vorbereitete Version
```

Diese Branches werden erst eingeführt, wenn der entsprechende Deployment-Prozess tatsächlich benötigt wird.

---
