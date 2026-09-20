# Frontend-Qualität lokal prüfen

Der Frontend-Qualitätslauf benötigt keine GitHub Actions und keine externen Dienste.

```powershell
.\scripts\test-frontend.ps1
```

Für den lokalen Coverage-Bericht:

```powershell
.\scripts\test-frontend.ps1 -Coverage
```

Der Lauf prüft TypeScript, Vitest/JSDOM und den Vite-Produktionsbuild. Die Komponententests decken unter anderem Navigation und Darstellung des globalen Aktivitätsmonitors ab.

## Visual-Regression-Checkliste

Kernseiten nach UI-Änderungen lokal unter diesen Ansichten prüfen:

| Ansicht | Zielgröße |
| --- | --- |
| Desktop | 1440 × 900 |
| Kleiner Desktop/Tablet quer | 1024 × 768 |
| Tablet hoch | 768 × 1024 |
| Mobil | 390 × 844 |

Zu prüfen sind Dashboard, Nodes, Containerliste, Containerdetail, Workflows und Secrets. Dabei insbesondere Navigation per Tastatur, sichtbarer Fokus, Tabellen-Overflow, Aktionsbuttons, Fehlerzustände und `prefers-reduced-motion` kontrollieren. Änderungen an diesen Zuständen bleiben damit lokal reproduzierbar und im Review nachvollziehbar.
