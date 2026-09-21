# lxcup Domain Context

Dieses Vokabular beschreibt die plattformneutrale Verwaltung von LXC,
Linux-Servern und Windows-Systemen. Es gilt für Weboberfläche, Controller,
Ansible und Agenten.

## Infrastruktur

**Target (Ziel)**: Eine einzeln verwaltete Maschine: bestehender LXC,
Linux-Server oder Windows-System. lxcup erstellt derzeit keine LXCs; Ziele
werden bewusst durch einen Operator aufgenommen.

**Target Kind**: Die Plattform eines Ziels: `Lxc`, `LinuxServer` oder
`WindowsServer`.

**Transport**: Der Ansible-Verbindungsweg eines Ziels: SSH für LXC und Linux,
WinRM für Windows. Zugangsdaten werden ausschließlich über eine
Secret-Referenz aufgelöst.

**Controller**: Die lxcup-Hauptanwendung mit PostgreSQL als Quelle der
Wahrheit.

## Verwaltung

**Target Onboarding**: Das bewusste Anlegen eines Ziels, einschließlich
Transport- und Secret-Referenzen. Es ersetzt die automatische
Proxmox-Discovery.

**Pending**: Ein angelegtes Ziel, dessen Agent sich noch nicht erfolgreich
beim Controller gemeldet hat.

**Managed**: Ein Ziel mit bestätigter Agentenverbindung. Es darf kontrollierte
Workflows ausführen.

**Disabled**: Ein bewusst deaktiviertes Ziel. Nur lesende Healthchecks sind
erlaubt.

**Agent Heartbeat**: Eine vom Agenten initiierte, authentisierte Meldung an den
Controller mit Identität, Metriken und Zeitstempel. Der Controller pollt
Agenten nicht als Grundlage für ihren Verbindungsstatus.

**Ansible Job**: Eine bestätigte, auditierbare Ausführung eines fest
registrierten Playbooks gegen ein Ziel. Freiform-Shell und freies Inventory
sind nicht zulässig.

## Updates

**Scan**: Eine beobachtende Abfrage nach verfügbaren Updates innerhalb eines
Ziels.

**Available Update**: Ein beim Scan erkanntes Paketupdate mit installierter
Version, Kandidatenversion und Klassifizierung.

**Update Plan**: Die aufgelöste Beschreibung dessen, was eine konkrete
Update-Anfrage ändern würde. Ein Plan wird vor der Ausführung erneut validiert.

**Execution**: Die protokollierte Ausführung eines bestätigten Update Plans.

## Lebenszyklen

**Target**: `Pending` → `Managed` nach einem gültigen Agent Heartbeat;
`Disabled` ist ein expliziter Betriebszustand.

**Ansible Job**: `Queued` → `Checking`/`Planned`/`Applying` und schließlich
`Succeeded`, `Failed`, `Aborted` oder `ReconcileRequired`.
