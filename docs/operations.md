# Betriebs-Runbook

Dieses Runbook beschreibt die beobachtbaren Zustände des Compose-Stacks. Es
enthält keine Zugangsdaten und darf deshalb in der Dokumentation versioniert
werden.

## Bereitschaft und Abhängigkeiten

```powershell
docker compose ps
Invoke-WebRequest http://127.0.0.1:8080/health/live
Invoke-WebRequest http://127.0.0.1:8080/health/ready
Invoke-WebRequest http://127.0.0.1:8080/metrics
```

`/health/live` zeigt, dass der API-Prozess antwortet. `/health/ready` prüft
zusätzlich die Datenbankverbindung und meldet deaktivierte Ziele als
`degraded`. Compose prüft PostgreSQL, API, Artefakt-Service und Worker über
eigene Healthchecks; ein Worker-Healthcheck bestätigt nur den laufenden
Prozess, die fachliche Verfügbarkeit kommt aus dem Worker-Heartbeat.

## Wichtige Signale

| Signal | Bedeutung | Alarmbedingung |
| --- | --- | --- |
| `lxcup_worker_heartbeat_age_seconds` | Alter des letzten Worker-Polls | `> 10` Sekunden oder `-1` |
| `lxcup_ansible_queue_age_seconds` | Alter des ältesten wartenden Jobs | steigt trotz verfügbarer Worker weiter an |
| `lxcup_ansible_jobs_failed` | Anzahl fehlgeschlagener Jobs | Anstieg seit dem letzten Check |
| `lxcup_http_requests_failed_total` | HTTP-Fehlerzähler | unerwarteter Anstieg |

Die Werte enthalten keine Secret-Werte. Job-, Ziel- und Worker-Korrelationen
stehen in den strukturierten Workflow-Ereignissen und werden nur über deren
öffentliche IDs referenziert.

## Typische Störungen

### Worker nicht verfügbar

1. `docker compose ps lxcup-worker` prüfen.
2. `/metrics` auf `lxcup_worker_heartbeat_age_seconds` prüfen.
3. `docker compose logs --since 10m lxcup-worker` prüfen.
4. Falls die Secret-Konfiguration fehlt, nur die Variablennamen und den
   Containerstatus prüfen; niemals Secret-Inhalte ausgeben.
5. Nach der Korrektur den Worker neu starten und den Heartbeat abwarten:

```powershell
docker compose restart lxcup-worker
```

### Datenbank nicht bereit

1. `docker compose ps postgres` und den PostgreSQL-Healthcheck prüfen.
2. `/health/ready` muss wieder `200` mit `status=ready` liefern.
3. Bei Migrationen zuerst die Container-Logs prüfen, bevor ein weiterer
   Start versucht wird.

### Artefakte nicht erreichbar

1. `docker compose ps artifacts` prüfen.
2. Den freigegebenen Manifest-Endpunkt aus dem Worker-Log ohne Credentials
   testen.
3. Manifest-Version und SHA-256 niemals manuell überschreiben; das Artefakt
   muss erneut freigegeben werden.

## Nachbearbeitung eines fehlgeschlagenen Jobs

Die Workflow-Detailseite zeigt Status, Fehlercode, Worker-Ausgabe und den
vollständigen technischen Log. Für einen sicheren Wiederholungsversuch muss
zuerst die Ursache behoben werden; die Ziel-Exklusivität verhindert doppelte
aktive Jobs. Bei `reconcile_required` wird zuerst der reale Zielzustand
geprüft, bevor erneut angewendet wird.

## Produktionsauthentifizierung

Für einen produktiven Start muss `LXCUP_ENV=production` gesetzt sein. Der
Server verweigert den Start, wenn `DATABASE_URL`, `LXCUP_SECRET_MASTER_KEY`
oder drei unterschiedliche Rollen-Token fehlen. Die Viewer-, Operator- und
Admin-Tokens müssen jeweils mindestens 16 Zeichen lang sein. Mit
`LXCUP_AUTH_TOKEN_TTL_SECONDS` kann die Token-Lebensdauer begrenzt werden
(Standard: 8 Stunden). Viewer dürfen nur
lesen, Operatoren konfigurieren und Workflows ausführen, Administratoren
zusätzlich Secrets und destruktive Aktionen verwalten. Logout erfolgt durch
Widerruf oder Rotation des verwendeten Tokens; Secret- und Tokenwerte werden
nie geloggt oder in API-Antworten ausgegeben.

## Compose-Onboarding-E2E-Test

Der Test startet den Compose-Stack und ein eigenes Debian-/systemd-SSH-Ziel im
Profil `onboarding-e2e`. Das Testziel ist nur im Compose-Netz erreichbar. Das
Skript liest dessen SSH-Host-Key direkt aus dem Container, legt SSH-Passwort,
Host-Key und Agent-Token als Secrets an und registriert ein separates
Linux-Server-Ziel. Es wartet auf erfolgreiches Agent-Deployment, Heartbeat,
Healthcheck und Paketinventar. Außerdem prüft es idempotente Einreihung und
startet den Worker nach dem Deployment einmal neu.

```powershell
.\scripts\test-onboarding-e2e.ps1
```

Voraussetzung ist Docker Compose. Der Test erzeugt eine eigene zufällige
Compose-Projekt-ID, temporäre Zugangsdaten, eine dedizierte Datenbank und ein
dediziertes Postgres-Volume. Veröffentliche Ports sind standardmäßig nur an
`127.0.0.1` gebunden und lassen sich über `-ControllerPort`, `-PostgresPort`,
`-TestPostgresPort` und `-FrontendPort` ändern. Der SSH-Container läuft
privilegiert, damit darin systemd getestet werden kann, und ist ausschließlich
für lokale Tests gedacht. Bei Erfolg oder Fehler fährt das Skript nur sein
eigenes Compose-Projekt herunter, entfernt dessen Volume und löscht die
temporäre Umgebungsdatei. Mit `-SkipWorkerRestart` lässt sich der
Worker-Neustart gezielt überspringen.
