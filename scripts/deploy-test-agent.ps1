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

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$AgentId = "lxcup-test-agent",

    [ValidateRange(1, 65535)]
    [int]$AgentPort = 8090,

    [string]$BinaryPath,

    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

foreach ($commandName in @("ssh", "scp")) {
    if ($null -eq (Get-Command $commandName -ErrorAction SilentlyContinue)) {
        throw "$commandName wurde nicht gefunden. Installiere OpenSSH Client oder aktiviere ihn in Windows."
    }
}

if ($SshUser -notmatch "^[A-Za-z0-9_.@-]+$") {
    throw "SshUser enthält unerlaubte Zeichen."
}
if ($ContainerName -notmatch "^[A-Za-z0-9_.-]+$") {
    throw "ContainerName enthält unerlaubte Zeichen."
}
if ($AgentId -notmatch "^[A-Za-z0-9_.-]+$") {
    throw "AgentId enthält unerlaubte Zeichen."
}

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptRoot
$sshTarget = "$SshUser@$ProxmoxIp"

function Import-DotEnv {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    foreach ($line in Get-Content -LiteralPath $Path) {
        $trimmed = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed) -or $trimmed.StartsWith("#")) {
            continue
        }
        if ($trimmed.StartsWith("export ")) {
            $trimmed = $trimmed.Substring(7).TrimStart()
        }
        if ($trimmed -notmatch "^(?<name>[A-Za-z_][A-Za-z0-9_]*)=(?<value>.*)$") {
            continue
        }

        $name = $Matches.name
        $value = $Matches.value.Trim()
        if (($value.StartsWith('"') -and $value.EndsWith('"')) -or
            ($value.StartsWith("'") -and $value.EndsWith("'"))) {
            $value = $value.Substring(1, $value.Length - 2)
        }
        $existing = (Get-Item "Env:$name" -ErrorAction SilentlyContinue).Value
        if ([string]::IsNullOrWhiteSpace($existing)) {
            [Environment]::SetEnvironmentVariable($name, $value, "Process")
        }
    }
}

Import-DotEnv -Path (Join-Path $repoRoot ".env")

$agentToken = $env:LXCUP_AGENT_TOKEN
if ([string]::IsNullOrWhiteSpace($agentToken)) {
    $secureToken = Read-Host "LXCUP_AGENT_TOKEN eingeben" -AsSecureString
    $tokenPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureToken)
    try {
        $agentToken = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($tokenPointer)
    }
    finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($tokenPointer)
    }
}
if ([string]::IsNullOrWhiteSpace($agentToken)) {
    throw "LXCUP_AGENT_TOKEN darf nicht leer sein."
}

if ([string]::IsNullOrWhiteSpace($BinaryPath)) {
    $BinaryPath = Join-Path $repoRoot "target\release\lxcup-agent"
    if (-not $NoBuild) {
        if ($null -eq (Get-Command docker -ErrorAction SilentlyContinue)) {
            throw "Docker wurde nicht gefunden. Starte Docker Desktop oder übergib -BinaryPath mit einer Linux-Binary."
        }

        Write-Host "Baue Linux-Agent mit Docker..."
        & docker run --rm `
            --mount "type=bind,source=$repoRoot,target=/src" `
            --workdir /src `
            rust:1.85-bookworm `
            cargo build --release -p lxcup-agent
        if ($LASTEXITCODE -ne 0) {
            throw "Der Linux-Agent konnte mit Docker nicht gebaut werden."
        }
    }
}

$BinaryPath = [System.IO.Path]::GetFullPath($BinaryPath)
if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
    throw "Linux-Agent-Binary nicht gefunden: $BinaryPath"
}

function Invoke-Remote {
    param([Parameter(Mandatory = $true)][string]$Command)

    $output = & ssh -o ConnectTimeout=10 $sshTarget $Command 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "SSH-Befehl fehlgeschlagen: $Command`n$($output -join [Environment]::NewLine)"
    }
    return ($output -join [Environment]::NewLine)
}

function Invoke-RemoteContainerScript {
    param([Parameter(Mandatory = $true)][string]$Script)

    # Bash erkennt Here-Doc-Abschlussmarker mit CRLF nicht zuverlässig. Der
    # Windows-String wird deshalb vor der Übertragung auf LF normalisiert.
    $normalizedScript = $Script.Replace("`r`n", "`n").Replace("`r", "")
    $output = $normalizedScript | & ssh -o ConnectTimeout=10 $sshTarget "pct exec $vmid -- /bin/bash -s" 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Setup im LXC fehlgeschlagen:`n$($output -join [Environment]::NewLine)"
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
    throw "Kein LXC mit dem Namen '$ContainerName' gefunden."
}
if ($matches.Count -gt 1) {
    throw "Mehrere LXCs mit dem Namen '$ContainerName' gefunden. Der Name muss eindeutig sein."
}

$target = $matches[0]
$nodeName = [string]$target.node
$vmid = [string]$target.vmid
if ($nodeName -notmatch "^[A-Za-z0-9_.-]+$" -or $vmid -notmatch "^\d+$") {
    throw "Proxmox lieferte ungültige Node-/VMID-Werte."
}

$statusJson = Invoke-Remote -Command "pvesh get /nodes/$nodeName/lxc/$vmid/status/current --output-format json"
$status = $statusJson | ConvertFrom-Json
if ([string]$status.status -ne "running") {
    throw "Der Test-LXC muss laufen. Aktueller Status: $($status.status)"
}

$remoteBinary = "/tmp/lxcup-agent-$([guid]::NewGuid().ToString('N'))"
$agentTokenBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($agentToken))

try {
    Write-Host "Test-LXC '$ContainerName' gefunden: Node=$nodeName, VMID=$vmid"
    Write-Host "Kopiere Linux-Agent nach Proxmox..."
    & scp -q -o ConnectTimeout=10 $BinaryPath ("{0}:{1}" -f $sshTarget, $remoteBinary)
    if ($LASTEXITCODE -ne 0) {
        throw "Die Linux-Agent-Binary konnte nicht nach Proxmox kopiert werden."
    }

    Invoke-Remote -Command "pct push $vmid $remoteBinary /usr/local/bin/lxcup-agent --perms 0755" | Out-Null

    $remoteSetup = @'
set -eu
install -d -m 0750 /etc/lxcup
printf '%s' '__TOKEN_B64__' | base64 --decode > /etc/lxcup/agent.token
chmod 0640 /etc/lxcup/agent.token
install -d -m 0750 /usr/local/libexec
cat > /usr/local/libexec/lxcup-agent-launcher <<'LAUNCHER'
#!/bin/sh
set -eu
export LXCUP_AGENT_TOKEN="$(cat /etc/lxcup/agent.token)"
export LXCUP_AGENT_ID="__AGENT_ID__"
export LXCUP_AGENT_BIND_ADDRESS="0.0.0.0:__AGENT_PORT__"
exec /usr/local/bin/lxcup-agent
LAUNCHER
chmod 0750 /usr/local/libexec/lxcup-agent-launcher

cat > /etc/systemd/system/lxcup-agent.service <<'UNIT'
[Unit]
Description=lxcup test agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/libexec/lxcup-agent-launcher
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable lxcup-agent.service
if ! systemctl restart lxcup-agent.service; then
    echo "lxcup-agent konnte nicht gestartet werden."
    systemctl status lxcup-agent.service --no-pager || true
    journalctl -u lxcup-agent.service -n 50 --no-pager || true
    exit 1
fi
if ! systemctl is-active --quiet lxcup-agent.service; then
    echo "lxcup-agent ist nach dem Start nicht aktiv."
    systemctl status lxcup-agent.service --no-pager || true
    journalctl -u lxcup-agent.service -n 50 --no-pager || true
    exit 1
fi
'@
    $remoteSetup = $remoteSetup.Replace("__TOKEN_B64__", $agentTokenBase64)
    $remoteSetup = $remoteSetup.Replace("__AGENT_ID__", $AgentId)
    $remoteSetup = $remoteSetup.Replace("__AGENT_PORT__", [string]$AgentPort)
    Invoke-RemoteContainerScript -Script $remoteSetup | Out-Null

    $lxcIp = (Invoke-Remote -Command "pct exec $vmid -- hostname -I").Trim().Split(' ', [System.StringSplitOptions]::RemoveEmptyEntries) |
        Where-Object { $_ -match "^[0-9]+(\.[0-9]+){3}$" } |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($lxcIp)) {
        throw "Die LXC-IP konnte nicht automatisch ermittelt werden."
    }

    Write-Host "Agent erfolgreich installiert und gestartet."
    Write-Host "SSH-Tunnel für den Test in einem zweiten Terminal:"
    Write-Host "ssh -N -L $AgentPort`:$lxcIp`:$AgentPort $sshTarget"
}
finally {
    try {
        Invoke-Remote -Command "rm -f $remoteBinary" | Out-Null
    }
    catch {
        Write-Warning "Temporäre Proxmox-Datei konnte nicht entfernt werden: $remoteBinary"
    }
}
