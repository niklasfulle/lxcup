[CmdletBinding()]
param(
    [string]$AnsiblePlaybook = "ansible-playbook"
)

$ErrorActionPreference = "Stop"

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$null = Set-Location $repositoryRoot
$env:ANSIBLE_CONFIG = Join-Path $repositoryRoot "ansible/ansible.cfg"

if (-not (Get-Command $AnsiblePlaybook -ErrorAction SilentlyContinue)) {
    throw "ansible-playbook wurde nicht gefunden. Installiere Ansible im Worker oder übergebe -AnsiblePlaybook."
}

$playbooks = @(
    "ansible/playbooks/agent-linux.yml",
    "ansible/playbooks/agent-linux-repair.yml",
    "ansible/playbooks/agent-linux-rollback.yml",
    "ansible/playbooks/packages-linux.yml",
    "ansible/playbooks/agent-windows.yml",
    "ansible/playbooks/packages-windows.yml"
)

foreach ($playbook in $playbooks) {
    & $AnsiblePlaybook --syntax-check --list-tasks $playbook
    if ($LASTEXITCODE -ne 0) {
        throw "Syntaxprüfung fehlgeschlagen: $playbook"
    }
}

Write-Host "Alle Linux-Agent-Playbooks sind syntaktisch gültig."
