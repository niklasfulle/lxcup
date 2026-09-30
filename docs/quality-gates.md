# Qualitätsgates

Die Testabdeckung ist ein verpflichtendes Qualitätsgate:

- Mindestziel: 80 % für Zeilen und Funktionen.
- Zielwert: 90 %.
- Zusätzlich muss jede einzelne Rust- und Frontend-Produktionsdatei mindestens 80 % Zeilenabdeckung erreichen.
- Produktions-Quellcodedateien dürfen höchstens 800 Zeilen enthalten; Tests, generierte Dateien und Abhängigkeiten sind ausgenommen.
- Frontend: Vitest erzwingt mindestens 80 % für Statements, Zeilen, Funktionen und Branches.
- Rust: `scripts/test-coverage.ps1` führt `cargo llvm-cov` für den gesamten Workspace aus.
- `scripts/check-source-limits.ps1` prüft die Zeilengrenze unabhängig von den Tests.

Lokal ausführen:

```powershell
.\scripts\test-coverage.ps1
```

Das strengere Ziel kann bereits geprüft werden:

```powershell
.\scripts\test-coverage.ps1 -Minimum 90
```

Die Coverage-Schwellen gelten für Produktionscode. Tests und generierte Dateien werden nicht als Produktionsdateien bewertet.
