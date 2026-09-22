# Interne Agent-Artefakte

Dieser Ordner wird im lokalen Docker-Netz ausschließlich als lesbarer
Downloadbereich bereitgestellt. Lege pro freigegebener Version einen Ordner an:

```text
artifacts/
  agent/
    0.1.0/
      linux-amd64
      manifest.json
```

`manifest.json` enthält die freigegebene Version und die SHA-256-Prüfsumme des
jeweiligen Binaries. Der Worker darf nur Version, Pfad und Prüfsumme aus diesem
Manifest verwenden; das Frontend liefert keine Download-URL.

Beispiel:

```json
{
  "version": "0.1.0",
  "artifacts": [{
    "platform": "linux-amd64",
    "file": "linux-amd64",
    "sha256": "<sha256-des-binaries>"
  }]
}
```
