# Linux-Agent-Playbooks

Diese Playbooks werden ausschließlich vom validierten lxcup-Ansible-Worker
aufgerufen. Es gibt kein Repository-Inventar und keine Secrets im Repository.

Erwartete Worker-Variablen:

- `lxcup_agent_version`: validierte Agent-Version, zum Beispiel `0.1.0`
- `lxcup_agent_binary_src`: vom Worker bereitgestellte Binary-Quelle
- `lxcup_agent_token`: zur Laufzeit aus dem Secret Store aufgelöst
- `lxcup_agent_id`: persistierte Agent-ID
- `lxcup_agent_bind_address`: standardmäßig `0.0.0.0:8090`

Token- und Konfigurationsaufgaben verwenden `no_log`. Die Rolle legt
versionierte Binaries ab, hält den vorherigen Symlink für Rollback vor,
aktualisiert den systemd-Service und prüft `/health`.

```powershell
.\scripts\test-ansible-playbooks.ps1
```

Die Syntaxprüfung benötigt `ansible-playbook` und verbindet sich nicht mit
einem Zielsystem.

Das Paketupdate-Playbook akzeptiert nur die vom Backend übergebene, gehashte
Planliste. Held Packages, Distribution-Upgrades, Shell-Kommandos und freie
Paketnamen werden vor APT abgewiesen. Im Apply-Modus werden Snapshot und
Bestätigung vorausgesetzt; das Ergebnis enthält Planhash, Änderung und
Reboot-Anforderung.
