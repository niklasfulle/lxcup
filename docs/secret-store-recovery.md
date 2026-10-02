# Secret-Store: Backup und Recovery

## Grundregel

Secret-Werte liegen nicht in PostgreSQL, API-Responses, SSE-Ereignissen oder normalen Logs. Der verschlüsselte Dateiprovider schreibt Metadaten als JSON und Werte als `*.enc`-Dateien. Der Master-Key wird ausschließlich außerhalb des Repositories und der Datenbank bereitgestellt.

## Development

1. Lege ein separates Secret-Verzeichnis außerhalb des Repositorys an.
2. Setze `LXCUP_SECRET_MASTER_KEY` als 64-stellige Hex-Zeichenfolge.
3. Sichere das Secret-Verzeichnis und den Master-Key getrennt.
4. Stelle zuerst das Verzeichnis wieder her und starte danach den Dienst mit demselben Key.

Ein falscher oder fehlender Key muss den Zugriff mit einem kontrollierten Fehler abbrechen. Ein Datenbank-Restore allein stellt keine Secret-Werte wieder her.

## Produktion

Backups müssen mindestens zwei getrennte Bestandteile enthalten:

- verschlüsselte `*.enc`-Dateien und die zugehörigen Metadaten-JSONs;
- den Master-Key aus dem externen Secret- oder KMS-System.

Der Master-Key darf nicht in dasselbe Backup, in SQL-Dumps oder in Support-Logs gelangen. Nach einer Wiederherstellung werden zuerst Dateirechte und Verzeichnisbesitz geprüft, danach der Dienst gestartet und anschließend ein lesender Healthcheck ausgeführt.

## Schlüsselrotation

Beim Wechsel wird der neue Key als aktueller Key konfiguriert und der alte Key nur für eine begrenzte Übergangszeit als Previous-Key akzeptiert. `EncryptedFileSecretStore::rewrap` verschlüsselt einzelne Werte mit dem aktuellen Key neu. Nach erfolgreicher Rewrapping-Prüfung wird der alte Key aus der Konfiguration entfernt und sicher widerrufen.

## Prüfen

Die lokale Negativtest-Suite prüft falsche Keys, fehlende Werte, Widerruf, Redaction und Rotation:

```powershell
.\scripts\test-secret-store-security.ps1
```

Die Tests verwenden ausschließlich synthetische Werte.

## Gemeinsamer Backup- und Restore-Test

Admins können ein verschlüsseltes Datenbank- und Secret-Store-Backup unter
**System → Backups** erstellen und herunterladen. Jedes Backup verwendet das
im Erstellungsdialog angegebene Passwort und standardmäßige age-Passphrase-
Verschlüsselung; lxcup speichert das Passwort nicht. Die App behält
verschlüsselte Archive im separaten persistenten Backup-Volume gemäß
`LXCUP_BACKUP_RETENTION_DAYS` (Standard: 30 Tage). Kopiere Downloads zusätzlich
auf ein Offsite-Ziel. Das Archiv enthält keine Live-`.env`-Datei oder Schlüssel;
Compose-Dateien und `.env.example` liegen ausschließlich als versionierte
Vorlage bei. Ohne das Backup-Passwort ist das Archiv nicht wiederherstellbar.

Entschlüssele ein heruntergeladenes App-Backup lokal mit age:

```powershell
age --decrypt --output .\backup.tar .\lxcup-backup-<id>.tar.age
```

age fragt nach dem Backup-Passwort. Entpacke das entschlüsselte TAR-Archiv in
einem zugriffsbeschränkten lokalen Verzeichnis. Der separate
`LXCUP_SECRET_MASTER_KEY` muss ebenfalls extern gesichert sein, um den
Secret-Store nach einer Wiederherstellung lesen zu können.

Für reproduzierbare Stack-Backups und Restore-Tests außerhalb der Anwendung
bleibt das folgende Runbookskript verfügbar:

Ein verschlüsseltes Stack-Backup enthält den PostgreSQL-Custom-Dump, den
bereits verschlüsselten Secret-Store sowie Compose-Datei und `.env.example` als
Konfigurationsvorlage. Während der Erfassung wird der Controller kurz angehalten,
damit Secret-Metadaten und Secret-Dateien konsistent bleiben. Die standortspezifische
`.env`-Datei und Schlüssel bleiben außerhalb des Archivs und müssen aus dem
externen Konfigurations-/Secret-Manager wiederhergestellt werden. Dieses
separate Runbookskript verwendet weiterhin einen öffentlichen Age-Empfänger
und ist unabhängig von der Passphrase-Verschlüsselung der App:

```powershell
$recipient = Read-Host "Öffentlicher age-Empfänger für das separate Stack-Backup"
.\scripts\backup-stack.ps1 -AgeRecipient $recipient
```

Die Aufbewahrung beträgt standardmäßig 30 Tage und kann mit
`-RetentionDays` angepasst werden. Speichere das verschlüsselte Archiv nur auf
einem zugriffsbeschränkten Backup-Ziel, möglichst zusätzlich außerhalb des
Hosts. Der Age-Private-Key und `LXCUP_SECRET_MASTER_KEY` müssen getrennt vom
Archiv in einem Passwortmanager, Secret Manager oder KMS liegen. Das lokale
Verzeichnis `backups` ist kein Ersatz für ein externes/offsite Backup.

Für einen isolierten Restore-Test muss `DATABASE_TEST_URL` auf eine dedizierte,
entbehrliche Testdatenbank zeigen. Das Skript akzeptiert optional
`-AgeIdentity` für Empfänger-verschlüsselte Archive. Bei einem passwort-
verschlüsselten App-Backup wird der Parameter weggelassen und age fragt nach
dem Passwort. Das Skript verlangt weiterhin eine ausdrückliche Bestätigung und
akzeptiert nur Datenbanknamen mit `test` oder `restore`, da `pg_restore --clean`
vorhandene Testdaten ersetzt:

```powershell
.\scripts\restore-backup-test.ps1 -BackupFile .\backups\lxcup-<id>.tar.age -ConfirmIsolatedDatabase
```

Wenn `age` lokal verfügbar ist, aber `pg_restore` und `psql` nicht installiert
sind, können die PostgreSQL-Werkzeuge aus dem bereits laufenden Compose-
PostgreSQL-Container verwendet werden:

```powershell
.\scripts\restore-backup-test.ps1 -BackupFile .\backups\lxcup-<id>.tar.age -ConfirmIsolatedDatabase -PostgresToolsContainer lxcup-postgres-test-1
```

`PostgresToolsContainer` muss auf den Container zeigen, der die in
`DATABASE_TEST_URL` benannte isolierte Restore-Datenbank enthält. Das Skript
prüft die Verbindung zur richtigen Datenbank vor der Integritätsprüfung.

Der Restore-Test spielt die Datenbank wieder ein, führt ausstehende SQLx-
Migrationen aus, prüft die Ziel-/Agent-Secret-Referenzen gegen das
wiederhergestellte Secret-Store-Verzeichnis, validiert die verschlüsselten
aktiven Secret-Werte ohne sie auszugeben und liest Audit-Datensätze. Ein
falscher Master-Key oder ein unvollständiges Archiv lässt den Test fehlschlagen.
Automatisiere Backup und Restore-Test mindestens täglich bzw. wöchentlich; Ziel
ist RPO 24 Stunden und RTO 1 Stunde. Bewahre Backups und beide Schlüssel in
getrennten, zugriffsbeschränkten Systemen auf.
