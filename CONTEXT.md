# lxcup Domain Context

Dieses Vokabular beschreibt die fachlichen Begriffe der kontrollierten Update-
Verwaltung für Proxmox-LXC-Container. Die Begriffe gelten für Weboberfläche,
Backend, Agenten und spätere Windows-Ziele.

## Infrastruktur

**Node**:
Ein Proxmox-Host, der LXC-Container bereitstellt und von lxcup über die
Proxmox-API verwaltet wird. Nicht `Host` verwenden, wenn der Proxmox-Kontext
gemeint ist.

**Container**:
Ein Proxmox-LXC, der von lxcup entdeckt oder verwaltet wird. Nicht `VM`
verwenden, da lxcup im MVP nur LXC-Container behandelt.

**Management-LXC**:
Der spezielle LXC, in dem die lxcup-Webanwendung und ihr Backend laufen.

## Verwaltung

**Discovery**:
Das Erkennen und Synchronisieren vorhandener Nodes und LXC-Container, ohne
dabei verändernde Update-Aktionen auszuführen.

**Managed**:
Ein Container, den lxcup aktiv für Scans, Pläne und bestätigte Ausführungen
berücksichtigt.

**Ignored**:
Ein entdeckter Container, der bewusst nicht von lxcup verwaltet wird.

## Updates

**Scan**:
Eine beobachtende Abfrage nach verfügbaren Updates innerhalb eines Containers.
Ein Scan verändert den Zielcontainer nicht.

**Available Update**:
Ein beim Scan erkanntes Paketupdate mit installierter Version,
Kandidatenversion und Klassifizierung.

**Update Plan**:
Die aufgelöste Beschreibung dessen, was eine konkrete Update-Anfrage tatsächlich
ändern würde. Ein Plan muss vor der Ausführung erneut validiert werden.

**Execution**:
Die protokollierte Ausführung eines bestätigten Update Plans. Nicht `Update`
verwenden, wenn der gesamte Lauf mit Status und Audit-Historie gemeint ist.

**Security Update**:
Ein Update, das vom Paketmanager oder der Distribution als sicherheitsrelevant
klassifiziert wurde. Wenn die Klassifizierung nicht zuverlässig möglich ist,
wird `Unknown` verwendet.

## Schutz und Betrieb

**Snapshot**:
Ein ausdrücklich angeforderter Proxmox-Zustand vor einer verändernden Aktion.
Ein Snapshot ist keine automatische Rollback-Entscheidung.

**Healthcheck**:
Eine konfigurierte Nachkontrolle nach einer Ausführung, zum Beispiel HTTP,
TCP oder systemd.

**Agent**:
Ein lokaler lxcup-Prozess, der später kontrollierte Operationen für einen
Proxmox-Node oder ein Windows-System ausführt. Ein Agent ist nicht die zentrale
Quelle der Wahrheit; diese bleibt beim Controller und seiner PostgreSQL-Datenbank.

## Lebenszyklen

**Container-Verwaltung**:
Ein entdeckter Container startet als `Discovered` und wird explizit zu
`Managed`, `Ignored` oder `Disabled`. Nur ein deaktivierter Container kann
wieder als `Managed` aktiviert werden; ein deaktivierter Container wird nicht
direkt als `Ignored` behandelt.

**Scan**:
Ein Scan durchläuft `Pending` → `Running` → `Succeeded` oder `Failed`.
Ein abgeschlossener Scan wird nicht nachträglich in einen laufenden Scan
zurückversetzt.

**Update Plan**:
Ein Plan durchläuft `Draft` → `Ready` → `Confirmed` oder wird bei einer
Sicherheitsverletzung `Blocked`. Ein bestätigter oder veralteter Plan kann
nicht erneut ausgeführt werden; eine Planänderung führt zu `Invalidated`.

**Execution**:
Eine Ausführung startet als `Queued`, läuft als `Running` und endet in
`Succeeded`, `Failed`, `Aborted` oder bei unklarer Verbindungslage zunächst in
`Unknown`. Terminalzustände werden nicht wieder geöffnet.
