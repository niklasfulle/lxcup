# lxcup

Webbasierter Update-Manager für Proxmox-LXC-Container.

## Workspace

```text
crates/
├── lxcup-core    Domain- und Geschäftslogik
├── lxcup-cli     optionale CLI-Oberfläche
├── lxcup-discovery Discovery und Reconciliation von LXC-Containern
├── lxcup-observability gemeinsame Fehler-/Logging-Infrastruktur
├── lxcup-proxmox HTTPS-Transport für die Proxmox-REST-API
├── lxcup-server  Backend/API und spätere Webauslieferung
└── lxcup-test-support versionierte Fixtures für Core- und Adaptertests
```

Die React-TypeScript-Vite-Weboberfläche liegt unter `frontend/`. Sie wird lokal
mit `npm install` und `npm run dev` gestartet und spricht standardmäßig über den
Vite-Proxy mit `http://127.0.0.1:8080`.

Die `lxcup-test-support`-Crate enthält reproduzierbare Testdaten für typische
MVP-Szenarien. Sie ist für Test- und Integrations-Crates vorgesehen und gehört
nicht zur fachlichen Produktionslogik.

Die Infrastrukturadapter für Proxmox, APT, Docker und Windows werden ergänzt,
sobald die zugehörigen MVP-Tickets umgesetzt werden. Sie dürfen keine
Geschäftslogik aus `lxcup-core` duplizieren.

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

## Entwicklungsprinzip

Verändernde Aktionen folgen immer:

```text
SCAN → PLAN → BESTÄTIGUNG → APPLY
```
