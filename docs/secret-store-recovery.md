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
