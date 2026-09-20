[CmdletBinding()]
param(
    [string]$ServerUrl = "http://127.0.0.1:8080",
    [string]$AgentUrl = "http://127.0.0.1:8090",
    [int]$ContainerId = 101,
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"

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

$repoRoot = Split-Path -Parent $PSScriptRoot
Import-DotEnv -Path (Join-Path $repoRoot ".env")

$serverUri = [Uri]::new($ServerUrl)
if ($serverUri.Host -notin @("127.0.0.1", "localhost")) {
    throw "ServerUrl muss für diesen lokalen Smoke-Test auf localhost zeigen."
}
$agentUri = [Uri]::new($AgentUrl)
if ($agentUri.Host -notin @("127.0.0.1", "localhost")) {
    throw "AgentUrl muss auf den lokalen SSH-Tunnel zeigen."
}

$agentToken = $env:LXCUP_INTEGRATION_AGENT_TOKEN
if ([string]::IsNullOrWhiteSpace($agentToken)) {
    $agentToken = $env:LXCUP_AGENT_TOKEN
}
if ([string]::IsNullOrWhiteSpace($agentToken)) {
    throw "LXCUP_INTEGRATION_AGENT_TOKEN oder LXCUP_AGENT_TOKEN muss gesetzt sein."
}

$serverBinary = Join-Path $repoRoot "target\debug\lxcup-server.exe"
if (-not (Test-Path -LiteralPath $serverBinary -PathType Leaf)) {
    if ($NoBuild) {
        throw "Server-Binary nicht gefunden: $serverBinary"
    }
    cargo build -p lxcup-server
    if ($LASTEXITCODE -ne 0) {
        throw "Der lxcup-Server konnte nicht gebaut werden."
    }
}

$serverProcess = $null
$stdoutTask = $null
$stderrTask = $null
try {
    $processInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $processInfo.FileName = $serverBinary
    $processInfo.UseShellExecute = $false
    $processInfo.RedirectStandardOutput = $true
    $processInfo.RedirectStandardError = $true
    [void]$processInfo.Environment.Remove("DATABASE_URL")
    $processInfo.Environment["LXCUP_BIND_ADDRESS"] = "127.0.0.1:$($serverUri.Port)"
    $processInfo.Environment["LXCUP_DEV_SEED"] = "true"
    $processInfo.Environment["LXCUP_AUTH_REQUIRED"] = "false"

    $serverProcess = [System.Diagnostics.Process]::new()
    $serverProcess.StartInfo = $processInfo
    if (-not $serverProcess.Start()) {
        throw "Der lxcup-Server konnte nicht gestartet werden."
    }
    $stdoutTask = $serverProcess.StandardOutput.ReadToEndAsync()
    $stderrTask = $serverProcess.StandardError.ReadToEndAsync()

    $ready = $false
    for ($attempt = 0; $attempt -lt 50; $attempt++) {
        if ($serverProcess.HasExited) {
            throw "Der lxcup-Server ist vor dem Readiness-Check beendet worden."
        }
        try {
            $readyResponse = Invoke-RestMethod -Uri "$ServerUrl/health/ready" -TimeoutSec 1
            if ($readyResponse.status -eq "ready") {
                $ready = $true
                break
            }
        }
        catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $ready) {
        throw "Der lxcup-Server wurde innerhalb des Readiness-Timeouts nicht bereit."
    }

    $registrationBody = @{
        endpoint = $AgentUrl
        token = $agentToken
    } | ConvertTo-Json -Compress
    $registration = Invoke-RestMethod `
        -Method Post `
        -Uri "$ServerUrl/api/v1/containers/$ContainerId/agent" `
        -ContentType "application/json" `
        -Body $registrationBody
    if ($registration.data.info.info.agent_id -ne "lxcup-test-agent") {
        throw "Unerwartete Agent-ID nach Registrierung: $($registration.data.info.info.agent_id)"
    }

    $health = Invoke-RestMethod -Uri "$ServerUrl/api/v1/containers/$ContainerId/agent/health"
    if (-not $health.data.healthy) {
        throw "Der registrierte Agent meldet sich nicht gesund."
    }
    $metricsBefore = Invoke-RestMethod -Uri "$ServerUrl/api/v1/containers/$ContainerId/agent/metrics"

    $scan = Invoke-RestMethod `
        -Method Post `
        -Uri "$ServerUrl/api/v1/containers/$ContainerId/scans" `
        -ContentType "application/json" `
        -Body "{}"
    $scanId = [string]$scan.data.id
    $scanResult = Invoke-RestMethod -Method Post -Uri "$ServerUrl/api/v1/scans/$scanId/run"
    if ($scanResult.data.status -ne "succeeded") {
        $diagnosticBody = @{
            action = "scan"
            packages = @()
            idempotency_key = "diagnostic-scan-$([guid]::NewGuid().ToString('N'))"
        } | ConvertTo-Json -Compress
        $diagnostic = Invoke-RestMethod `
            -Method Post `
            -Uri "$AgentUrl/command" `
            -Headers @{ Authorization = "Bearer $agentToken" } `
            -ContentType "application/json" `
            -Body $diagnosticBody
        throw "Server-Scan fehlgeschlagen. Direkter Agent-Scan: success=$($diagnostic.success), exit_code=$($diagnostic.exit_code), stderr=$($diagnostic.stderr)"
    }
    $metricsAfter = Invoke-RestMethod -Uri "$ServerUrl/api/v1/containers/$ContainerId/agent/metrics"
    if ([int64]$metricsAfter.data.commands_total -le [int64]$metricsBefore.data.commands_total) {
        throw "Der Scan hat die Agent-Metrik commands_total nicht erhöht."
    }

    Write-Host "Server-Agent-Integration erfolgreich."
    Write-Host "Agent: $($health.data.info.agent_id)"
    Write-Host "Scan: $($scanResult.data.status)"
    Write-Host "Agent commands_total: $($metricsAfter.data.commands_total)"
}
catch {
    if ($stderrTask -and $serverProcess -and $serverProcess.HasExited) {
        $serverError = $stderrTask.GetAwaiter().GetResult()
        if (-not [string]::IsNullOrWhiteSpace($serverError)) {
            Write-Error "Server-Log:`n$serverError"
        }
    }
    throw
}
finally {
    if ($serverProcess -and -not $serverProcess.HasExited) {
        $serverProcess.Kill()
        $serverProcess.WaitForExit()
    }
    if ($serverProcess) {
        $serverProcess.Dispose()
    }
}
