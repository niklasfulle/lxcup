# Kein Proxmox-Control-Plane

lxcup verwaltet Linux-Server, LXC-Container und Windows-Systeme als manuell registrierte Ressourcen und verwendet dafür ausschließlich ihre Verbindungskonfiguration sowie den Agentenkanal. Proxmox-API, Nodes, Umgebungen und die daraus abgeleitete Discovery werden entfernt, weil sie LXC unnötig an eine einzelne Virtualisierungsplattform bindet und die Ressourcenverwaltung für Nutzer unklar macht.
