# ADR 0001: Agent-first Target Architecture

## Status

Accepted

## Context

lxcup soll vorhandene LXC, Linux-Server und Windows-Systeme einheitlich
verwalten. Eine Abhängigkeit von der Proxmox-API würde die Produktgrenze auf
eine einzelne Virtualisierungsplattform beschränken und eingehende
Verbindungen zum Agenten erfordern.

## Decision

Der Controller verwaltet plattformneutrale Targets. Ansible führt Deployment,
Update und Reparatur über SSH oder WinRM aus. Agenten bauen die dauerhafte
fachliche Verbindung selbst durch authentisierte Heartbeats zum Controller
auf. PostgreSQL ist die Quelle der Wahrheit für Target-Inventar und Audit.

Die Erstellung von LXCs und die Proxmox-API gehören nicht zum aktuellen
Produktumfang.

## Consequences

Die Aufnahme eines Ziels benötigt eine erreichbare SSH- oder WinRM-Verbindung
und zwei Secret-Referenzen: eine für Ansible und eine für den Agenten. Die
Architektur unterstützt dadurch LXC, Linux und Windows gleichermaßen.
Bestehende Proxmox-spezifische Routen und Tests sind Übergangscode und werden
in einem getrennten Abbaupfad entfernt.
