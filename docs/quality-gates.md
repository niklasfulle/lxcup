# Qualitätsgates

Die Testabdeckung ist ein verpflichtendes Qualitätsgate:

- Mindestziel: 80 % für Zeilen und Funktionen.
- Zielwert: 90 %.
- Frontend: Vitest erzwingt mindestens 80 % für Statements, Zeilen, Funktionen und Branches.
- Rust: `scripts/test-coverage.ps1` führt `cargo llvm-cov` für den gesamten Workspace aus.

Lokal ausführen:

```powershell
.\scripts\test-coverage.ps1
```

Das strengere Ziel kann bereits geprüft werden:

```powershell
.\scripts\test-coverage.ps1 -Minimum 90
```

Ein Coverage-Lauf vom 23.09.2026 lag noch unter dem Gate (Rust gesamt ca. 36 % Zeilen, Frontend ca. 26 % Zeilen). Die Schwelle ist deshalb bewusst als sichtbares Gate eingerichtet; die fehlenden Tests müssen vor dem nächsten Release ergänzt werden.
