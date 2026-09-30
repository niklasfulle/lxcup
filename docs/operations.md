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
Frontend und produktiver Caddy besitzen ebenfalls Healthchecks. Der Caddy-
Healthcheck validiert die Konfiguration; sein Upstream ist der Compose-
Servicename `lxcup-server`, nicht localhost im Caddy-Container.

Artifact Store und Worker verwenden `restart: unless-stopped`; der Worker wird
erst gestartet, wenn der Artifact-Store-Healthcheck erfolgreich ist. Ein
expliziter Compose-Neustart des Artifact Stores startet auch den Worker neu.
Docker startet Container bei beendetem Prozess erneut, aber nicht allein wegen
des Status `unhealthy`. Der Worker prüft den Manifest-Endpunkt selbst alle
15 Sekunden, protokolliert Zustandswechsel und übermittelt den Status in seinem
Heartbeat. Eine Warnung in der Oberfläche bedeutet daher: Worker lebt, der
Artifact Store wurde zuletzt aber als nicht verfügbar geprüft.

## Wichtige Signale

| Signal | Bedeutung | Alarmbedingung |
| --- | --- | --- |
| `lxcup_worker_heartbeat_age_seconds` | Alter des letzten Worker-Polls | `> 10` Sekunden oder `-1` |
| `lxcup_worker_available` | Worker-Heartbeat jünger/gleich 10 s (`1`/`0`) | `0` länger als 30 Sekunden |
| `lxcup_ansible_queue_age_seconds` | Alter des ältesten wartenden Jobs | steigt trotz verfügbarer Worker weiter an |
| `lxcup_ansible_jobs_failed` | Anzahl fehlgeschlagener Jobs | Anstieg seit dem letzten Check |
| `lxcup_http_requests_failed_total` | HTTP-Fehlerzähler | unerwarteter Anstieg |

Die Werte enthalten keine Secret-Werte. Job-, Ziel- und Worker-Korrelationen
stehen in den strukturierten Worker-Logs als `worker_id`, `job_id` und `target`;
API-Zugriffe tragen zusätzlich `request_id` und geben sie im Header
`x-request-id` zurück. Die Kennungen enthalten nur IDs, nie Zugangsdaten.

## Alarmregeln

- `/health/ready` liefert länger als 30 Sekunden keinen HTTP-Status `200`.
- `lxcup_worker_available == 0` oder `lxcup_worker_heartbeat_age_seconds > 10`
  länger als 30 Sekunden.
- `lxcup_ansible_jobs_queued > 0` und
  `lxcup_ansible_queue_age_seconds > 120` Sekunden.
- `lxcup_ansible_jobs_failed` steigt; bei produktiven Änderungen zusätzlich
  sofort den Jobstatus `reconcile_required` untersuchen.
- Compose meldet `unhealthy` für PostgreSQL, API, Frontend, Artefakte, Worker
  oder Caddy.

`/metrics` ist ein Prometheus-Textformat ohne Secret-Werte. In Produktion
blockiert der öffentliche Caddy-Eingang diesen Pfad; ein privater Scraper im
Compose-Netz kann `http://lxcup-server:8080/metrics` abrufen. Die Metriken sind
Betriebsinformationen und gehören nicht ins öffentliche Netz.

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
2. `docker compose logs --since 10m lxcup-worker` auf die Warnung zum Artifact
   Store prüfen. Der Worker wiederholt die Prüfung automatisch alle 15 Sekunden.
3. Falls der Artifact Store beendet ist, versucht Docker den Container zu
   starten. Bei `unhealthy` trotz laufendem Prozess die Ursache prüfen; ein
   Healthcheck-Neustart ist keine Docker-Compose-Funktion.
4. Manifest-Version und SHA-256 niemals manuell überschreiben; das Artefakt
   muss erneut freigegeben werden.

## Nachbearbeitung eines fehlgeschlagenen Jobs

Die Workflow-Detailseite zeigt Status, Fehlercode, Worker-Ausgabe und den
vollständigen technischen Log. Für einen sicheren Wiederholungsversuch muss
zuerst die Ursache behoben werden; die Ziel-Exklusivität verhindert doppelte
aktive Jobs. Bei `reconcile_required` wird zuerst der reale Zielzustand
geprüft, bevor erneut angewendet wird.

## Benutzerkonten und Produktionsauthentifizierung

Für einen produktiven Start müssen `LXCUP_ENV=production`, `DATABASE_URL` und
`LXCUP_SECRET_MASTER_KEY` konfiguriert sein. Beim ersten PostgreSQL-Start legt
der Server ein Konto `admin` mit dem temporären Passwort `admin` an. Beim
ersten Login ist ausschließlich der Passwortwechsel erlaubt; ein neues
Passwort muss mindestens 12 Zeichen lang sein. Wechsle es sofort. Spätere
Starts ändern bestehende Konten nicht. Öffentliche Registrierung und
E-Mail-Wiederherstellung gibt es nicht.
Beim Upgrade bleiben registrierte Ressourcen und Agent-Credentials erhalten;
die früheren Viewer-/Operator-/Admin-Controller-Tokens werden für menschliche
API-Aufrufe nicht in Konten umgewandelt und nicht mehr akzeptiert. Das neue
Bootstrap-Konto ist der explizite, erzwungene Zugangspfad; Agenten melden sich
weiterhin separat mit ihrem Ziel-Token an.
Stelle den Dienst während des ersten Starts und des Passwortwechsels nicht
öffentlich oder ohne TLS bereit. Der Bootstrap-Login ist absichtlich ein
bekanntes temporäres Credential und muss unmittelbar rotiert werden.

Admins können Benutzer anlegen, Rollen vergeben, Konten deaktivieren,
Passwörter zurücksetzen und Benutzer löschen. Neu angelegte oder
zurückgesetzte Konten müssen ihr Startpasswort beim nächsten Login wechseln.
Der letzte aktive Admin kann nicht gelöscht, deaktiviert oder herabgestuft
werden. Nur Admins können Benutzerverwaltung, Secrets und das
Benutzer-Aktivitätsprotokoll verwenden.

### Administratorzugang wiederherstellen

Wenn noch ein anderer Admin erreichbar ist, setzt dieser das betroffene Konto
über die Benutzerverwaltung zurück. Gibt es keinen nutzbaren Admin mehr, ist
ein manueller Datenbank-Notfallzugriff erforderlich: Controller anhalten,
zuerst ein vollständiges PostgreSQL-Backup erstellen und dann ausschließlich
die Konten löschen:

```sql
DELETE FROM auth_users;
```

Beim nächsten Controllerstart wird dadurch das einmalige `admin`/`admin`
Bootstrap-Konto neu angelegt und erzwingt den Passwortwechsel. Diese
Notfallmaßnahme entfernt alle Benutzer und Sitzungen. Audit-Ereignisse bleiben
erhalten; ihre Benutzer-Fremdschlüssel werden durch `ON DELETE SET NULL`
entfernt, während der gespeicherte Benutzername/Rollensnapshot erhalten bleibt.
Nur für eine kontrollierte Wiederherstellung mit Datenbankzugriff verwenden.

Passwörter werden als gesalzene PBKDF2-HMAC-SHA256-Hashes gespeichert.
`POST /api/v1/auth/login` setzt ein zufälliges HttpOnly-Session-Cookie;
PostgreSQL speichert nur dessen SHA-256-Hash. Sitzungen laufen nach acht
Stunden ab und werden beim Neuladen über das HttpOnly-Cookie wiederhergestellt.
Schreibzugriffe über Cookies benötigen zusätzlich den CSRF-Header. Logout,
Passwortwechsel/-reset, Rollenänderung, Deaktivierung und Löschen widerrufen
die betroffenen Sitzung(en); Logout löscht beide Browser-Cookies.

Der produktive Browserzugriff muss ausschließlich über TLS erfolgen. Die
Echtzeitverbindung verwendet dieselbe HttpOnly-Session über eine authentifizierte
Same-Origin-HTTP-Stream-Anfrage; Zugangsdaten werden nicht in die URL geschrieben.
`/health/live`, `/health/ready`, `/metrics`, Account-Status/Login und der
Agent-Heartbeat haben eigene Exposure-Regeln. Der Agent-Heartbeat
authentifiziert sich separat mit dem pro Ziel hinterlegten Agent-Token.
Ungültige/abgelaufene Sessions erhalten `401`, fehlende Berechtigungen `403`.
Audit-Einträge enthalten keine Request-Bodies, Passwörter, Token oder Secrets.
Das Audit-Protokoll ist nur für Admins verfügbar und kann nach Benutzer,
Aktionskennung, Ressource sowie Zeitraum gefiltert und seitenweise gelesen
werden. Fehlgeschlagene Loginversuche werden ohne Passwort oder Token und ohne
verifizierte Benutzeridentität als anonyme Ereignisse erfasst.

## Compose-Onboarding-E2E-Test

Der Test startet einen isolierten Compose-Stack, das Frontend und ein eigenes
Debian-/systemd-SSH-Ziel im Profil `onboarding-e2e`. Vor dem Ressourcen-Onboarding
prüft er das einmalige `admin`/`admin`-Bootstrap samt Pflichtwechsel, legt einen
User an und verifiziert Passwortwechsel, Admin/User-API-Rechte, Audit-Redaction,
Frontend-Proxy sowie HTTP 404 für unbekannte Frontend-Routen und API-404-Trennung.
Das Testziel ist nur im Compose-Netz erreichbar. Das Skript liest dessen
SSH-Host-Key direkt aus dem Container, legt SSH-Passwort, Host-Key und Agent-Token
als Secrets an und registriert ein separates Linux-Server-Ziel. Es wartet auf
erfolgreiches Agent-Deployment, Heartbeat, Healthcheck und Paketinventar. Außerdem
prüft es idempotente Einreihung und startet den Worker nach dem Deployment einmal
neu.

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
