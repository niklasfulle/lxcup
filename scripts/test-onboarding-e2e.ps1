[CmdletBinding()]
param(
    [string]$ControllerUrl = "http://127.0.0.1:8080",
    [Parameter(Mandatory = $true)] [long]$ContainerId,
    [Parameter(Mandatory = $true)] [string]$TargetId,
    [int]$TimeoutSeconds = 600,
    [string]$AuthToken
)

$ErrorActionPreference = "Stop"
$headers = @{ "Content-Type" = "application/json" }
if (-not [string]::IsNullOrWhiteSpace($AuthToken)) { $headers.Authorization = "Bearer $AuthToken" }
$base = $ControllerUrl.TrimEnd('/')
$idempotency = "e2e-" + [guid]::NewGuid().ToString()
$body = @{ container_id = $ContainerId; target_id = $TargetId; idempotency_key = $idempotency; start_onboarding = $true } | ConvertTo-Json

Write-Host "Starte isolierten Onboarding-Test für Ziel $TargetId."
$enrollment = Invoke-RestMethod -Method Post -Uri "$base/api/v1/enrollments" -Headers $headers -Body $body
$enrollmentId = $enrollment.data.id
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
    Start-Sleep -Seconds 2
    $current = Invoke-RestMethod -Method Get -Uri "$base/api/v1/enrollments/$enrollmentId" -Headers $headers
    $state = $current.data.state
    Write-Host "Enrollment-Status: $state"
    if ($state -eq "failed") { throw "Onboarding fehlgeschlagen. Workflow-Details im Controller unter /workflows prüfen." }
} while ($state -notin @("connected", "failed") -and (Get-Date) -lt $deadline)
if ($state -ne "connected") { throw "Onboarding-Timeout nach $TimeoutSeconds Sekunden." }

$required = @("deploy_agent", "health_check", "collect_package_inventory")
$jobs = @()
do {
    $jobs = (Invoke-RestMethod -Method Get -Uri "$base/api/v1/ansible/jobs" -Headers $headers).data |
        Where-Object { $_.target.target -eq $TargetId -or $_.target.container -eq $ContainerId }
    $missing = @($required | Where-Object { $jobs.operation -notcontains $_ })
    if ($missing) { Start-Sleep -Seconds 2 }
} while ($missing.Count -gt 0 -and (Get-Date) -lt $deadline)
if ($missing.Count -gt 0) { throw "Onboarding ist verbunden, aber folgende Workflows fehlen: $($missing -join ', ')" }
if (@($jobs | Where-Object { $_.status -in @("failed", "reconcile_required") }).Count -gt 0) {
    throw "Mindestens ein Onboarding-Workflow ist fehlgeschlagen; Details stehen unter /workflows."
}
Write-Host "Onboarding-E2E erfolgreich: Agent, Healthcheck und Paketinventar sind eingereiht."
