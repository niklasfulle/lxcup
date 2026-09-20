# lxcup Test-LXC-Setup

Diese Anleitung beschreibt die lokale Entwicklung und den opt-in-
Integrationstest gegen den dedizierten Proxmox-LXC `lxcup-test`.

Der Test verwendet ausschließlich die separate Testdatenbank und einen
separaten Proxmox-API-Token. Produktive Credentials und produktive Container
dürfen nicht verwendet werden.

## 1. Voraussetzungen auf Windows

- Rust 1.85 über `rustup`
- Docker Desktop mit Docker Compose
- Windows OpenSSH Client (`ssh` und `scp`)
- PowerShell 7 oder Windows PowerShell
- Zugriff auf das Repository

GitHub Actions werden für diesen Ablauf nicht benötigt.

## 2. Lokale `.env` anlegen

Die Datei `.env` liegt im Repository-Root und wird nicht committed. Sie enthält
Passwörter und Tokens.

```powershell
Copy-Item .env.example .env
```

Mindestens diese Werte werden für die lokale Compose-Datenbank benötigt:

```text
LXCUP_POSTGRES_DB=lxcup_dev
LXCUP_POSTGRES_USER=lxcup_dev
LXCUP_POSTGRES_PASSWORD=<lokales-passwort>
LXCUP_TEST_POSTGRES_DB=lxcup_test
LXCUP_TEST_POSTGRES_USER=lxcup_test
LXCUP_TEST_POSTGRES_PASSWORD=<test-passwort>
```

Für den Proxmox-Integrationstest kommen hinzu:

```text
PROXMOX_TEST_BASE_URL=https://pve:8006
PROXMOX_TEST_TOKEN_ID=lxcup-test@pve!lxcup-integration
PROXMOX_TEST_TOKEN_SECRET=<token-secret>
DATABASE_TEST_URL=postgres://lxcup_test:<test-passwort>@localhost:5434/lxcup_test
LXCUP_INTEGRATION_NODE=pve
LXCUP_INTEGRATION_VMID=113
```

`PROXMOX_TEST_CA_CERT`, `LXCUP_INTEGRATION_NODE` und
`LXCUP_INTEGRATION_VMID` werden normalerweise automatisch durch das
Bootstrap-Skript gesetzt.

## 3. Lokale PostgreSQL-Testdatenbank starten

Die Testdatenbank läuft getrennt von der Entwicklungsdatenbank auf Host-Port
`5434`:

```powershell
docker compose up -d postgres-test
docker compose ps
```

Die Entwicklungsdatenbank läuft separat auf Port `5433`:

```powershell
docker compose up -d postgres
```

Daten bleiben in Docker-Volumes erhalten. `docker compose down -v` löscht die
Volumes und damit die Daten; diesen Befehl nur bewusst verwenden.

## 4. Dedizierten Test-LXC vorbereiten

Der Proxmox-LXC muss exakt `lxcup-test` heißen und laufen. Das Bootstrap-
Skript sucht Node und VMID automatisch. Der aktuell verifizierte Test-LXC ist:

```text
Name: lxcup-test
Node: pve
VMID: 113
```

### 4.1 Proxmox-API-Token

Der Test-Token ist vom Produktions-Token getrennt. Für einen neuen Token kann
der Proxmox-Administrator beispielsweise verwenden:

```bash
pveum user add lxcup-test@pve
pveum user token add lxcup-test@pve lxcup-integration -privsep 1
```

Das Token-Secret wird nur bei der Erstellung angezeigt und gehört anschließend
als `PROXMOX_TEST_TOKEN_SECRET` in `.env`.

### 4.2 Berechtigungen für VM 113

Bei `privsep=1` müssen sowohl der Benutzer als auch der API-Token berechtigt
sein. Für den lesenden Erreichbarkeitstest reicht `PVEAuditor`:

```bash
pveum acl modify /vms/113 \
  -user 'lxcup-test@pve' \
  -role PVEAuditor

pveum acl modify /vms/113 \
  -token 'lxcup-test@pve!lxcup-integration' \
  -role PVEAuditor
```

Die effektiven Berechtigungen prüfen:

```bash
pveum user permissions 'lxcup-test@pve'
pveum user token permissions 'lxcup-test@pve' 'lxcup-integration'
```

Für `/vms/113` muss `VM.Audit` enthalten sein. Die einfachen Anführungszeichen
sind wichtig, weil Bash das `!` sonst als History-Expansion interpretieren kann.

## 5. SSH-Bootstrap ausführen

Das Skript benötigt nur die Proxmox-IP:

```powershell
.\scripts\bootstrap-test-lxc.ps1 -ProxmoxIp "192.168.1.150"
```

Falls Root-SSH deaktiviert ist:

```powershell
.\scripts\bootstrap-test-lxc.ps1 `
  -ProxmoxIp "192.168.1.150" `
  -SshUser "admin"
```

Der SSH-Benutzer muss `pvesh` ausführen und `/etc/pve/pve-root-ca.pem` lesen
dürfen. Das Skript erlaubt SSH-Key-Authentifizierung und interaktive
Passwort-Authentifizierung.

Das Skript erledigt automatisch:

1. SSH-Verbindung zu Proxmox herstellen.
2. LXC `lxcup-test` finden.
3. Node und VMID aus der Proxmox-Ressourcenliste ermitteln.
4. Die private Proxmox-CA nach
   `%LOCALAPPDATA%\lxcup\proxmox-test-ca.pem` kopieren.
5. Den Node-Namen als TLS/SNI-Hostname verwenden.
6. Die angegebene IP separat als TCP-Verbindungsziel verwenden.
7. `.env` laden und den dedizierten Integrationstest starten.

Die IP muss deshalb nicht im Proxmox-Zertifikat enthalten sein. Das Zertifikat
wird weiterhin vollständig geprüft; TLS wird nicht deaktiviert.

## 6. Integrationstest starten

Nach erfolgreichem Bootstrap kann der Test ohne erneuten Build ausgeführt
werden:

```powershell
.\scripts\bootstrap-test-lxc.ps1 `
  -ProxmoxIp "192.168.1.150" `
  -NoBuild
```

Alternativ bei bereits gesetzten Umgebungsvariablen:

```powershell
.\scripts\run-integration-tests.ps1 -ProxmoxIp "<PROXMOX-IP>" -NoBuild
```

Der aktuelle Test prüft:

- Proxmox API-Erreichbarkeit
- private CA und TLS/SNI
- Test-API-Token statt Produktions-Credentials
- Zugriff auf den dedizierten LXC
- korrekten Node und VMID

Erfolgreiche Ausgabe:

```text
test dedicated_lxc_is_reachable_without_using_production_credentials ... ok
test result: ok. 1 passed; 0 failed
```

## 7. Bekannte Fehlerbilder

### SSH `Permission denied (publickey,password)`

Der Proxmox-Host ist erreichbar, aber der SSH-Benutzer konnte sich nicht
authentifizieren. SSH-Key installieren, Passwort-Login erlauben oder
`-SshUser` verwenden.

### TLS `UnknownIssuer`

Die private Proxmox-CA fehlt. Bootstrap erneut ausführen oder
`PROXMOX_TEST_CA_CERT` auf eine gültige PEM-Datei setzen.

### TLS `NotValidForName`

Die Verbindung wurde über eine IP aufgebaut, die nicht im Zertifikat steht.
Bootstrap verwenden; es nutzt den Node-Namen für TLS/SNI und die IP nur für die
TCP-Verbindung.

### HTTP 403 `VM.Audit`

Der API-Token hat keine effektive Berechtigung auf `/vms/113`. Bei
`privsep=1` Benutzer-ACL und Token-ACL setzen und mit `pveum user token
permissions` prüfen.

### `DATABASE_TEST_URL must be set`

Die Testdatenbank-URL fehlt in `.env` oder der Container `postgres-test` läuft
nicht. `.env` prüfen und `docker compose up -d postgres-test` ausführen.

## 8. Nächster Agent-Test

Der Agent-Vertrag ist bereits vorhanden:

```text
GET  /health
GET  /metrics
POST /command
```

Der Server registriert einen Agenten über:

```text
POST /api/v1/containers/{container_id}/agent
```

Danach werden über den Server geprüft:

```text
GET /api/v1/containers/{container_id}/agent/health
GET /api/v1/containers/{container_id}/agent/metrics
```

Nach der Installation bzw. dem Start von `lxcup-agent` im dedizierten
Test-LXC wird der opt-in Health-/Metriktest so ausgeführt:

```powershell
$env:LXCUP_INTEGRATION_AGENT_URL = "http://127.0.0.1:8090"
$env:LXCUP_INTEGRATION_AGENT_TOKEN = "<separates-agent-token>"
cargo test -p lxcup-test-support --test dedicated_agent -- --ignored --nocapture
```

Der AgentClient akzeptiert HTTP nur für `localhost`. Für den entfernten LXC
wird deshalb entweder eine HTTPS-Adresse (zum Beispiel über einen späteren
Reverse-Proxy) oder ein lokaler SSH-Tunnel verwendet:

```powershell
ssh -N -L 8090:<LXC-IP>:8090 root@<PROXMOX-IP>
```

Der Test prüft Health, Protokollversion und Metriken und führt ausschließlich
das harmlose Health-Kommando aus. Erst wenn diese Prüfungen grün sind, wird ein
kontrollierter APPLY-Test mit einem ausdrücklich erlaubten Testpaket
durchgeführt.

Agent-Credentials bleiben getrennt von den Proxmox-Credentials:

```text
LXCUP_AGENT_ID=lxcup-test-agent
LXCUP_AGENT_TOKEN=<separates-agent-token>
LXCUP_AGENT_BIND_ADDRESS=0.0.0.0:8090
```

### Agent automatisch verteilen

Der Agent kann nach einer SSH-Anmeldung am Proxmox-Host per Skript in den
dedizierten LXC installiert werden. `LXCUP_AGENT_TOKEN` wird aus der aktuellen
Umgebung oder aus `.env` gelesen; falls er fehlt, fragt das Skript ihn verdeckt
ab:

```powershell
.\scripts\deploy-test-agent.ps1 -ProxmoxIp "<PROXMOX-IP>"
```

Standardmäßig baut das Skript die Linux-Binary mit dem Docker-Image
`rust:1.85-bookworm`. Alternativ kann eine bereits gebaute Linux-Binary
übergeben werden:

```powershell
.\scripts\deploy-test-agent.ps1 `
  -ProxmoxIp "<PROXMOX-IP>" `
  -BinaryPath ".\target\release\lxcup-agent" `
  -NoBuild
```

Das Skript ermittelt Node und VMID, kopiert die Binary temporär zum Proxmox-
Host, installiert `/usr/local/bin/lxcup-agent`, legt den systemd-Dienst
`lxcup-agent.service` an und startet ihn. Danach gibt es den passenden
SSH-Tunnel-Befehl mit der automatisch ermittelten LXC-IP aus. Dieser Befehl
muss in einem zweiten Terminal geöffnet bleiben, während der Agent-Test läuft.

## 9. Sicherheitsregeln

- `.env` niemals committen oder in Logs ausgeben.
- Proxmox-Testtoken und Agent-Token niemals wiederverwenden.
- Keine produktiven LXC-Container für Integrationstests verwenden.
- Kein `docker compose down -v`, wenn Testdaten erhalten bleiben sollen.
- APPLY erst nach erfolgreichem Health-/Metriktest und nur gegen den dedizierten
  Test-LXC ausführen.
