# lxcup Domain Context

Dieses Vokabular beschreibt die plattformneutrale Verwaltung einzelner Systeme. Es gilt für Weboberfläche, Controller, Ansible und Agenten.

## Ressourcen

**Verwaltete Ressource**: Ein einzeln und manuell in lxcup registriertes System. Es gehört genau einer Ressourcenart an und hat eine eigene Verbindungskonfiguration.
_Avoid_: Node, Zugangsprofil

**Linux-Server**: Eine verwaltete Linux-Maschine, die keine LXC-Instanz ist.
_Avoid_: Server-Ziel, Host

**LXC-Container**: Eine manuell registrierte LXC-Instanz. Für ihre Verwaltung ist keine Verbindung zu einer Virtualisierungsplattform erforderlich.
_Avoid_: Proxmox-LXC, entdeckter Container

**Windows-System**: Eine manuell registrierte Windows-Maschine.
_Avoid_: Windows-Ziel

**Docker-Container**: Ein Container, den ein Agent innerhalb einer verwalteten Linux-Ressource meldet. Er ist keine eigenständige Verbindung und gehört zu genau einer Host-Ressource.

**Ressourcenart**: Die fachliche Einordnung einer verwalteten Ressource: `Lxc`, `LinuxServer` oder `WindowsServer`.

## Zugang und Verwaltung

**Verbindungskonfiguration**: Adresse, Transport und Secret-Referenzen einer verwalteten Ressource. Sie wird gemeinsam mit der Ressource angelegt und ist kein eigener Navigationsbereich.
_Avoid_: Target, Zugangsprofil

**Transport**: Der Ansible-Verbindungsweg einer Ressource: SSH für LXC und Linux, WinRM für Windows. Zugangsdaten werden ausschließlich über Secret-Referenzen aufgelöst.

**Secret**: Ein im Secret-Store verwalteter Wert, auf den eine Verbindungskonfiguration oder ein Agent über eine Referenz zugreift.

**Pending**: Eine registrierte Ressource, deren Agent sich noch nicht erfolgreich beim Controller gemeldet hat.

**Managed**: Eine Ressource mit bestätigter Agentenverbindung. Sie darf kontrollierte Workflows ausführen.

**Disabled**: Eine bewusst deaktivierte Ressource. Nur lesende Healthchecks sind erlaubt.

**Agent Heartbeat**: Eine vom Agenten initiierte, authentisierte Meldung an den
Controller mit Identität, Metriken und Zeitstempel. Der Controller pollt
Agenten nicht als Grundlage für ihren Verbindungsstatus.

**Ansible Job**: Eine bestätigte, auditierbare Ausführung eines fest registrierten Playbooks gegen eine Ressource. Freiform-Shell und freies Inventory sind nicht zulässig.

## Updates

**Scan**: Eine beobachtende Abfrage nach verfügbaren Updates innerhalb eines
Ziels.

**Available Update**: Ein beim Scan erkanntes Paketupdate mit installierter
Version, Kandidatenversion und Klassifizierung.

**Update Plan**: Die aufgelöste Beschreibung dessen, was eine konkrete
Update-Anfrage ändern würde. Ein Plan wird vor der Ausführung erneut validiert.

**Execution**: Die protokollierte Ausführung eines bestätigten Update Plans.

## Lebenszyklen

**Verwaltete Ressource**: `Pending` → `Managed` nach einem gültigen Agent Heartbeat; `Disabled` ist ein expliziter Betriebszustand.

**Ansible Job**: `Queued` → `Checking`/`Planned`/`Applying` und schließlich
`Succeeded`, `Failed`, `Aborted` oder `ReconcileRequired`.
