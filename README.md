# lxcup

Webbasierter Update-Manager für Proxmox-LXC-Container.

## Workspace

```text
crates/
├── lxcup-core         Domain- und Geschäftslogik
├── lxcup-apt          APT-Scanner und Parser
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

Die MVP-Grundlagen sind umgesetzt: Discovery vorhandener LXC-Container, APT-
Scanning, sichere Update-Pläne, REST-/SSE-API, React-Dashboard, Execution-
Lebenszyklus, Snapshots/Healthchecks sowie lokale Test- und Quality-Gates.

Die API- und Safety-Grenzen sind vorbereitet. Für den Produktivbetrieb müssen
als Nächstes der Serverzustand vollständig an PostgreSQL angebunden, die Linux-
und Windows-Agenten integriert und echte Update-Ausführungen gegen dedizierte
Zielsysteme aktiviert werden.

Empfohlene Reihenfolge:

1. PostgreSQL-Repositories mit den Backend-Services verdrahten.
2. Linux-Agent mit sicherem Transport, Logs und Metriken integrieren.
3. Windows-Agent mit demselben Agent-Vertrag ergänzen.
4. Execution und Proxmox-Snapshot-Workflow gegen Test-LXCs ausführen.
5. Authentifizierung, Rollen/Rechte, Deployment und Monitoring ergänzen.

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

Eine optionale SonarQube-Analyse wird mit `scripts/sonarqube.ps1` gestartet,
wenn der lokale SonarQube-Scanner eingerichtet ist. GitHub Actions werden nicht
verwendet.

Ein opt-in Integrationstest gegen einen ausdrücklich dedizierten Test-LXC wird
so gestartet:

```powershell
$env:DATABASE_TEST_URL = "postgres://lxcup_test:<password>@localhost:5434/lxcup_test"
$env:PROXMOX_TEST_BASE_URL = "https://pve-test:8006"
$env:PROXMOX_TEST_TOKEN_ID = "<test-token-id>"
$env:PROXMOX_TEST_TOKEN_SECRET = "<test-token-secret>"
$env:LXCUP_INTEGRATION_NODE = "pve-test"
$env:LXCUP_INTEGRATION_VMID = "101"
.\scripts\run-integration-tests.ps1
```

Der Test wird nicht automatisch ausgeführt und verwendet keine produktiven
Credentials oder produktiven Zielcontainer.

## Entwicklungsprinzip

Verändernde Aktionen folgen immer:

```text
SCAN → PLAN → BESTÄTIGUNG → APPLY
```
