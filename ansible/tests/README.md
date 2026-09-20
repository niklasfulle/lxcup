# Lokale Ansible-Teststrategie

Die Prüfungen laufen lokal und benötigen keine Produktiv-Credentials:

1. `scripts/test-ansible-policies.ps1` prüft freie Shell-Schritte, Secret-nahe
   Variablen ohne `no_log` und fehlende explizite Playbook-Ziele.
2. `scripts/test-ansible-playbooks.ps1` führt zusätzlich die Ansible-Syntax-
   und Taskprüfung aus, sobald `ansible-playbook` im Worker verfügbar ist.
3. Der Ansible-Worker testet Registry, Inventory, Limits, falsche Secret-
   Referenzen, unreachable-Zielmodell, Timeout-/Output-Limits, Retry,
   Rollback und Reconcile über Rust-Unit-Tests.
4. Der dedizierte Linux-LXC-Test wird ausschließlich opt-in mit dem vorhandenen
   Test-LXC und separaten Credentials ausgeführt.

Für den End-to-End-Lauf werden die Secret-Referenzen aus der `.env` gelesen,
aber keine Secret-Werte in Inventory-Dateien oder das Repository geschrieben.
Windows wird über einen separaten WinRM-Testhost in der Gruppe
`lxcup_windows_targets` geprüft.
