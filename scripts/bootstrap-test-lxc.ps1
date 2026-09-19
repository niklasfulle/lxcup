[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$ProxmoxIp,

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$SshUser = "root",

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$ContainerName = "lxcup-test",

    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

foreach ($commandName in @("ssh", "scp")) {
    if ($null -eq (Get-Command $commandName -ErrorAction SilentlyContinue)) {
        throw "$commandName wurde nicht gefunden. Installiere OpenSSH Client oder aktiviere ihn in Windows."
    }
}

$sshTarget = "$SshUser@$ProxmoxIp"

function Invoke-Remote {
    param([string]$Command)

    # BatchMode verhindert die Passwortabfrage. Schlüssel- und interaktive
    # Passwort-Authentifizierung sollen beide möglich sein.
    $output = & ssh -o ConnectTimeout=10 $sshTarget $Command 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "SSH-Befehl fehlgeschlagen: $Command`n$($output -join [Environment]::NewLine)"
    }
    return ($output -join [Environment]::NewLine)
}

$resourceJson = Invoke-Remote -Command "pvesh get /cluster/resources --type vm --output-format json"
try {
    $resources = @($resourceJson | ConvertFrom-Json)
}
catch {
    throw "Die Proxmox-Ressourcen konnten nicht als JSON gelesen werden: $($_.Exception.Message)"
}

$matches = @($resources | Where-Object { $_.type -eq "lxc" -and $_.name -eq $ContainerName })
if ($matches.Count -eq 0) {
    throw "Kein LXC mit dem Namen '$ContainerName' gefunden. Benenne den dedizierten Test-LXC in Proxmox entsprechend."
}
if ($matches.Count -gt 1) {
    throw "Mehrere LXCs mit dem Namen '$ContainerName' gefunden. Der Name muss eindeutig sein."
}

$target = $matches[0]
$nodeName = [string]$target.node
$vmid = [string]$target.vmid

$localConfigDirectory = Join-Path $env:LOCALAPPDATA "lxcup"
New-Item -ItemType Directory -Force -Path $localConfigDirectory | Out-Null
$caPath = Join-Path $localConfigDirectory "proxmox-test-ca.pem"

& scp -q -o ConnectTimeout=10 "${sshTarget}:/etc/pve/pve-root-ca.pem" $caPath
if ($LASTEXITCODE -ne 0) {
    throw "Die Proxmox-CA konnte nicht per SSH nach '$caPath' kopiert werden."
}

$caContent = [System.IO.File]::ReadAllText($caPath)
if ($caContent -notmatch "BEGIN CERTIFICATE") {
    throw "Die heruntergeladene Datei ist keine gültige PEM-Zertifikatsdatei: $caPath"
}

$env:PROXMOX_TEST_BASE_URL = "https://$ProxmoxIp`:8006"
$env:PROXMOX_TEST_CA_CERT = $caPath
$env:LXCUP_INTEGRATION_NODE = $nodeName
$env:LXCUP_INTEGRATION_VMID = $vmid

Write-Host "Test-LXC '$ContainerName' gefunden: Node=$nodeName, VMID=$vmid"
Write-Host "Proxmox-CA gespeichert unter: $caPath"
Write-Host "Starte dedizierten Integrationstest..."

$testScript = Join-Path $PSScriptRoot "run-integration-tests.ps1"
& $testScript -NoBuild:$NoBuild
exit $LASTEXITCODE
