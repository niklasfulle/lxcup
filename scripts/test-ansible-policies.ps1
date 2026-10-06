[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$ansibleRoot = Join-Path $repositoryRoot "ansible"
$yamlFiles = Get-ChildItem -Path $ansibleRoot -Recurse -File -Include *.yml, *.yaml

if (-not $yamlFiles) {
    throw "Keine Ansible-YAML-Dateien gefunden."
}

$failures = [System.Collections.Generic.List[string]]::new()
foreach ($file in $yamlFiles) {
    $content = Get-Content -LiteralPath $file.FullName -Raw
    $relative = $file.FullName.Substring($repositoryRoot.Length + 1)

    if ($content -match '(?m)^\s*-\s*(?:ansible\.[^:]+\.)?(shell|win_shell)\s*:') {
        $failures.Add("Freier Shell-Schritt in $relative")
    }

    if ($content -match '(?i)(password|token|private_key|secret)\s*:' -and
        $content -notmatch '(?m)^\s*no_log:\s*true\s*$') {
        $failures.Add("Secret-nahe Variablen ohne no_log in $relative")
    }
}

$playbooks = Get-ChildItem -Path (Join-Path $ansibleRoot "playbooks") -File -Filter *.yml
foreach ($playbook in $playbooks) {
    $content = Get-Content -LiteralPath $playbook.FullName -Raw
    if ($content -notmatch '(?m)^\s*hosts\s*:') {
        $failures.Add("Playbook ohne explizites hosts-Ziel: $($playbook.Name)")
    }
}

$linuxAgentTasks = Get-Content -LiteralPath (Join-Path $ansibleRoot "roles/lxcup_agent_linux/tasks/main.yml") -Raw
$linuxAgentUnit = Get-Content -LiteralPath (Join-Path $ansibleRoot "roles/lxcup_agent_linux/templates/lxcup-agent.service.j2") -Raw
$linuxAgentHandler = Get-Content -LiteralPath (Join-Path $ansibleRoot "roles/lxcup_agent_linux/handlers/main.yml") -Raw
if ($linuxAgentUnit -notmatch '(?m)^WantedBy=multi-user\.target\s*$' -or
    $linuxAgentHandler -notmatch '(?m)^\s*enabled:\s*true\s*$' -or
    $linuxAgentTasks -notmatch '(?s)- name: Enable and start the agent service.*?state: started.*?enabled: true') {
    $failures.Add("Linux-Agent ist nicht dauerhaft für den Systemstart aktiviert.")
}

if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Error $_ }
    throw "Ansible-Policyprüfung fehlgeschlagen."
}

Write-Host "Ansible-Policyprüfung erfolgreich: keine freien Shell-Schritte, Secret-nahe Aufgaben sind redigiert, Ziele sind explizit."
