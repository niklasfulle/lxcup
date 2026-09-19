# lxcup

Webbasierter Update-Manager für Proxmox-LXC-Container.

## Workspace

```text
crates/
├── lxcup-core         Domain- und Geschäftslogik
├── lxcup-apt          APT-Scanner und Parser
├── lxcup-agent        Linux-/Windows-Agentvertrag und Agentdienst
├── lxcup-cli          optionale CLI-Oberfläche
├── lxcup-discovery    LXC-Discovery und Reconciliation
├── lxcup-execution    bestätigte Update-Ausführung und Audit-Grenze
├── lxcup-observability Logging- und Fehler-Infrastruktur
├── lxcup-planner      sichere Update-Pläne und Revalidierung
├── lxcup-proxmox      HTTPS-Transport und Proxmox-API-Adapter
├── lxcup-safety       Snapshots, Healthchecks und Reboot-Erkennung
├── lxcup-server       Backend-REST-/SSE-API
└── lxcup-test-support versionierte Fixtures und Integrationstest-Harness
```

Die React-TypeScript-Vite-Weboberfläche liegt unter `frontend/`. Sie wird lokal
mit `npm install` und `npm run dev` gestartet und spricht standardmäßig über den
Vite-Proxy mit `http://127.0.0.1:8080`.

Die `lxcup-test-support`-Crate enthält reproduzierbare Testdaten für typische
MVP-Szenarien. Sie ist für Test- und Integrations-Crates vorgesehen und gehört
nicht zur fachlichen Produktionslogik.

Die Infrastrukturadapter dürfen keine Geschäftslogik aus `lxcup-core`
duplizieren. Linux- und Windows-Agenten werden als getrennte Agent-Prozesse
ergänzt; der Controller und PostgreSQL bleiben die zentrale Quelle der Wahrheit.

## Aktueller Projektstand

Die MVP-Grundlagen und der erste Ende-zu-Ende-Pfad sind umgesetzt: Discovery
vorhandener LXC-Container, APT-Scanning, sichere Update-Pläne, REST-/SSE-API,
React-Dashboard, authentifizierte Linux-/Windows-Agenten, Execution-Lebenszyklus,
Snapshot-Task-Polling, Healthchecks, PostgreSQL-Ergebnisablage und lokale
Test-/Quality-Gates.

Der Server akzeptiert Agenten über den versionierten Vertrag `v1`. Agenten
liefern Health, Metriken, gekürzte Befehlsausgaben und führen nur validierte
Paketargumente aus. Die Weboberfläche kann Scans starten, Ausführungen gegen
registrierte Agenten auslösen und nach Verbindungsverlust reconciliieren.

Empfohlene Reihenfolge:

1. Einen dedizierten Test-LXC-Agenten registrieren und den APPLY-Pfad testen.
2. Snapshot-/Healthcheck-Regeln gegen die echte Testumgebung verifizieren.
3. PostgreSQL-Backup/Restore und Reverse-Proxy im Management-LXC testen.
4. SonarQube im lokalen Quality-Gate ergänzen.

## Lokale Prüfungen

Die Rust-Version ist in `rust-toolchain.toml` festgelegt.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Unter Windows können alle Prüfungen auch gemeinsam ausgeführt werden:

```powershell
.\scripts\verify.ps1
```

Für PostgreSQL-Integrationstests wird ausschließlich eine separate
Development-Datenbank über `DATABASE_TEST_URL` verwendet:

```powershell
$env:DATABASE_TEST_URL = "postgres://<user>:<password>@<host>:5432/lxcup_test"
cargo test -p lxcup-persistence --test postgres_integration -- --nocapture
```

Ohne `DATABASE_TEST_URL` wird der Integrationstest übersprungen. Eine
Produktions-`DATABASE_URL` wird dafür nicht verwendet.

## Lokale PostgreSQL-Datenbank mit Docker Compose

Für die lokale Entwicklung steht eine isolierte PostgreSQL-Instanz über Docker
Compose bereit. Sie verwendet den Host-Port `5433`, damit eine bereits
vorhandene PostgreSQL-Instanz auf Port `5432` nicht gestört wird. Die Daten
liegen in einem benannten Docker-Volume und bleiben nach `docker compose down`
erhalten.

Einmalig die lokale Konfiguration anlegen und das Passwort ergänzen:

```powershell
Copy-Item .env.example .env
# LXCUP_POSTGRES_PASSWORD in .env mit einem lokalen Wert setzen
```

Datenbank starten und Status prüfen:

```powershell
docker compose up -d postgres
docker compose up -d postgres-test
docker compose ps
```

Die Migrationen werden beim Ausführen des PostgreSQL-Integrationstests durch
die Anwendung angewendet. Dazu die Werte aus `.env` als Test-URL setzen:

```powershell
$env:DATABASE_TEST_URL = "postgres://lxcup_dev:<password>@localhost:5433/lxcup_dev"
cargo test -p lxcup-persistence --test postgres_integration -- --nocapture
```

Die Testdatenbank läuft getrennt auf Port `5434` und verwendet die Werte
`LXCUP_TEST_POSTGRES_DB`, `LXCUP_TEST_POSTGRES_USER` und
`LXCUP_TEST_POSTGRES_PASSWORD` aus `.env`:

```powershell
$env:DATABASE_TEST_URL = "postgres://lxcup_test:<password>@localhost:5434/lxcup_test"
```

Die Entwicklungsdatenbank stoppen, ohne ihre Daten zu löschen:

```powershell
docker compose down
```

`docker compose down -v` löscht zusätzlich das lokale PostgreSQL-Volume und
damit alle darin gespeicherten Entwicklungsdaten.

Die lokale Qualitätsprüfung umfasst Rust und Frontend:

```powershell
.\scripts\verify.ps1
```

Eine optionale SonarQube-Analyse wird lokal mit dem projektbezogenen
`@sonar/scan`-Scanner gestartet. Das Skript erzeugt davor Rust- und Frontend-LCOV-
Reports. Der Server muss aus der aktuellen Shell erreichbar sein; bei
Docker-Netzwerken ist `sonarqube` als Hostname nur innerhalb dieses Netzwerks
auflösbar:

Voraussetzungen:

- Im Frontend müssen die Abhängigkeiten installiert sein (`npm ci`); darin ist
  `@sonar/scan` als Dev-Dependency enthalten. Der klassische Scanner bleibt als
  Fallback über `SONAR_SCANNER_HOME` möglich.
- Für `@sonar/scan` v5 wird Node.js 22.12 oder neuer benötigt.
- Für Rust-Coverage: `rustup component add llvm-tools-preview` und
  `cargo install cargo-llvm-cov --version 0.6.21 --locked` für die festgelegte
  Rust-Version 1.85.
- Für Frontend-Coverage müssen die Frontend-Abhängigkeiten installiert sein.

```powershell
.\sonar.ps1 -SonarHostUrl "http://sonarqube:9000" -Token "<token>" -ProjectKey "Lxcup" -ProjectVersion "0.1.0"
```

Alternativ kann der Token vorher als `$env:SONAR_TOKEN` gesetzt werden. Ein
leerer `-Token ""` wird unterstützt, funktioniert aber nur bei aktivierter
anonymer Analyse. Für Rust-Coverage wird `cargo-llvm-cov` benötigt; für
Frontend-Coverage ist `@vitest/coverage-v8` im Projekt hinterlegt. Fehlt eines
der Coverage-Werkzeuge, läuft der Scan mit Warnung ohne den jeweiligen Report
weiter; der SonarScanner selbst ist dagegen zwingend erforderlich.
Die Projektversion ist standardmäßig `0.1.0` und kann über `-ProjectVersion`
überschrieben werden.
Das bisherige Alias-Skript bleibt unter `scripts/sonarqube.ps1` erhalten.
GitHub Actions werden nicht verwendet.

## Agenten und Betriebsendpunkte

Der Agent läuft im verwalteten LXC beziehungsweise als Windows-Dienst. Er wird
mit `LXCUP_AGENT_TOKEN` und `LXCUP_AGENT_BIND_ADDRESS` konfiguriert. Der Server
registriert ihn über `POST /api/v1/containers/{container_id}/agent`; danach
stehen folgende Endpunkte zur Verfügung:

```text
POST /api/v1/scans/{scan_id}/run
POST /api/v1/executions/{execution_id}/run
POST /api/v1/executions/{execution_id}/reconcile
GET  /api/v1/executions/{execution_id}/result
GET  /api/v1/containers/{container_id}/agent/health
GET  /api/v1/containers/{container_id}/agent/metrics
GET  /health/live
GET  /health/ready
GET  /metrics
```

Für eine produktive Bereitstellung siehe [`deploy/README.md`](deploy/README.md).
Die Authentifizierung verwendet Bearer-Tokens mit Viewer-, Operator- und
Admin-Token; ohne gesetzte Authentifizierung bleibt die lokale Entwicklung
kompatibel.

Ein opt-in Integrationstest gegen einen ausdrücklich dedizierten Test-LXC wird
so gestartet:

```powershell
$env:DATABASE_TEST_URL = "postgres://lxcup_test:<password>@localhost:5434/lxcup_test"
$env:PROXMOX_TEST_BASE_URL = "https://pve-test:8006"
$env:PROXMOX_TEST_TOKEN_ID = "<test-token-id>"
$env:PROXMOX_TEST_TOKEN_SECRET = "<test-token-secret>"
$env:PROXMOX_TEST_CA_CERT = "C:\\secrets\\pve-root-ca.pem"
$env:LXCUP_INTEGRATION_NODE = "pve-test"
$env:LXCUP_INTEGRATION_VMID = "101"
.\scripts\run-integration-tests.ps1
```

Der Test wird nicht automatisch ausgeführt und verwendet keine produktiven
Credentials oder produktiven Zielcontainer. `PROXMOX_TEST_CA_CERT` verweist auf
die PEM-Datei der privaten Proxmox-CA; dadurch bleibt die TLS-Prüfung aktiv,
statt Zertifikate pauschal zu akzeptieren.

Alternativ kann der Proxmox-Test über SSH vorbereitet und mit nur der
Proxmox-IP gestartet werden. Der dedizierte LXC muss dafür `lxcup-test` heißen:

```powershell
.\scripts\bootstrap-test-lxc.ps1 -ProxmoxIp "192.168.1.150"
```

Das Skript findet Node und VMID, kopiert die Proxmox-CA nach `%LOCALAPPDATA%\lxcup`
und lädt die übrigen Testwerte aus `.env`. Es verändert keine produktiven
Container und erstellt keine neuen Proxmox-Berechtigungen. Für die Verbindung
verwendet es den Windows-OpenSSH-Client; vorhandene SSH-Schlüssel werden
genutzt, alternativ kann OpenSSH interaktiv nach dem Passwort fragen.
Die angegebene IP wird für die TCP-Verbindung verwendet, während der
Proxmox-Node-Name für TLS/SNI und die Zertifikatsprüfung genutzt wird. Dadurch
funktioniert der Test auch, wenn die IP nicht im Proxmox-Zertifikat enthalten
ist.

Standardmäßig wird `root` verwendet. Falls der direkte Root-Login deaktiviert
ist, kann ein anderer SSH-Benutzer angegeben werden:

```powershell
.\scripts\bootstrap-test-lxc.ps1 -ProxmoxIp "192.168.1.150" -SshUser "admin"
```

Der Benutzer muss `pvesh` ausführen und `/etc/pve/pve-root-ca.pem` lesen dürfen.

## Entwicklungsprinzip

Verändernde Aktionen folgen immer:

```text
SCAN → PLAN → BESTÄTIGUNG → APPLY
```
