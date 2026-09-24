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

Ein verschlüsseltes Stack-Backup enthält den PostgreSQL-Custom-Dump und den
bereits verschlüsselten Secret-Store. Der Age-Empfänger wird nur als
Kommandozeilenargument verwendet; der private Schlüssel wird nie ausgegeben
oder im Repository gespeichert:

```powershell
.\scripts\backup-stack.ps1 -AgeRecipient $env:LXCUP_BACKUP_AGE_RECIPIENT
```

Die Aufbewahrung beträgt standardmäßig 30 Tage und kann mit
`-RetentionDays` angepasst werden. Speichere das verschlüsselte Archiv nur auf
einem zugriffsbeschränkten Backup-Ziel, möglichst zusätzlich außerhalb des
Hosts. Der Age-Private-Key und `LXCUP_SECRET_MASTER_KEY` müssen getrennt vom
Archiv in einem Passwortmanager, Secret Manager oder KMS liegen. Das lokale
Verzeichnis `backups` ist kein Ersatz für ein externes/offsite Backup.

Für einen regelmäßigen isolierten Restore-Test muss `DATABASE_TEST_URL` auf
eine dedizierte, entbehrliche Testdatenbank zeigen. Das Skript verlangt eine
explizite Bestätigung und akzeptiert nur Datenbanknamen mit `test` oder
`restore`, da `pg_restore --clean` vorhandene Testdaten ersetzt:

```powershell
.\scripts\restore-backup-test.ps1 -BackupFile .\backups\lxcup-<timestamp>.tar.age -AgeIdentity $env:LXCUP_BACKUP_AGE_IDENTITY -ConfirmIsolatedDatabase
```

Der Restore-Test spielt die Datenbank wieder ein, führt ausstehende SQLx-
Migrationen aus, prüft die Ziel-/Agent-Secret-Referenzen gegen das
wiederhergestellte Secret-Store-Verzeichnis, validiert die verschlüsselten
aktiven Secret-Werte ohne sie auszugeben und liest Audit-Datensätze. Ein
falscher Master-Key oder ein unvollständiges Archiv lässt den Test fehlschlagen.
Automatisiere Backup und Restore-Test mindestens täglich bzw. wöchentlich; Ziel
ist RPO 24 Stunden und RTO 1 Stunde. Bewahre Backups und beide Schlüssel in
getrennten, zugriffsbeschränkten Systemen auf.
