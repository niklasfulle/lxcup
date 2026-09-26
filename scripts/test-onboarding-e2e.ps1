[CmdletBinding()]
param(
    [int]$ControllerPort = 18080,
    [int]$PostgresPort = 15433,
    [int]$TestPostgresPort = 15434,
    [int]$FrontendPort = 15173,
    [int]$TimeoutSeconds = 900,
    [switch]$SkipWorkerRestart
)

$ErrorActionPreference = "Stop"
$ports = @($ControllerPort, $PostgresPort, $TestPostgresPort, $FrontendPort)
if ($ports | Where-Object { $_ -lt 1024 -or $_ -gt 65535 } | Select-Object -First 1) {
    throw "E2E host ports must be between 1024 and 65535."
}
if (@($ports | Select-Object -Unique).Count -ne $ports.Count) {
    throw "E2E host ports must be distinct."
}
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Push-Location $repoRoot
$previousPassword = $env:LXCUP_E2E_SSH_PASSWORD
$suffix = [guid]::NewGuid().ToString("N").Substring(0, 12)
$composeProject = "lxcup-onboarding-e2e-$suffix"
$envFile = Join-Path ([System.IO.Path]::GetTempPath()) "lxcup-onboarding-e2e-$suffix.env"
$composePrefix = @("--project-name", $composeProject, "--env-file", $envFile)
$controllerUrl = "http://127.0.0.1:$ControllerPort"
$composeAttempted = $false
$password = [guid]::NewGuid().ToString("N") + [guid]::NewGuid().ToString("N")
$env:LXCUP_E2E_SSH_PASSWORD = $password
$masterKeyBytes = [byte[]]::new(32)
[System.Security.Cryptography.RandomNumberGenerator]::Fill($masterKeyBytes)
$masterKey = [Convert]::ToHexString($masterKeyBytes).ToLowerInvariant()
$envLines = @(
    "LXCUP_POSTGRES_DB=lxcup_e2e",
    "LXCUP_POSTGRES_USER=lxcup_e2e",
    "LXCUP_POSTGRES_PASSWORD=$([guid]::NewGuid().ToString('N'))",
    "LXCUP_TEST_POSTGRES_DB=lxcup_e2e_test",
    "LXCUP_TEST_POSTGRES_USER=lxcup_e2e_test",
    "LXCUP_TEST_POSTGRES_PASSWORD=$([guid]::NewGuid().ToString('N'))",
    "LXCUP_SECRET_MASTER_KEY=$masterKey",
    "LXCUP_CONTROLLER_URL=http://lxcup-server:8080",
    "LXCUP_ARTIFACT_BASE_URL=http://artifacts",
    "LXCUP_WORKER_SSH_USER=lxcup",
    "LXCUP_DEV_SEED=false",
    "LXCUP_POSTGRES_PORT=$PostgresPort",
    "LXCUP_POSTGRES_HOST_IP=127.0.0.1",
    "LXCUP_TEST_POSTGRES_PORT=$TestPostgresPort",
    "LXCUP_TEST_POSTGRES_HOST_IP=127.0.0.1",
    "LXCUP_SERVER_PORT=$ControllerPort",
    "LXCUP_SERVER_HOST_IP=127.0.0.1",
    "LXCUP_FRONTEND_PORT=$FrontendPort",
    "LXCUP_FRONTEND_HOST_IP=127.0.0.1",
    "LXCUP_POSTGRES_VOLUME_NAME=lxcup-e2e-postgres-$suffix",
    "LXCUP_SERVICE_ENV_FILE=$($envFile -replace '\\','/')"
)
 $previousEnvironment = @{}

function Invoke-Compose([string[]]$Arguments) {
    $allArguments = @($composePrefix) + $Arguments
    & docker compose @allArguments
    if ($LASTEXITCODE -ne 0) { throw "docker compose $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

function Invoke-Controller([string]$Method, [string]$Path, [object]$Body = $null) {
    $parameters = @{
        Method = $Method
        Uri = "$($controllerUrl.TrimEnd('/'))$Path"
        Headers = $headers
        ErrorAction = "Stop"
    }
    if ($null -ne $Body) {
        $parameters.ContentType = "application/json"
        $parameters.Body = ConvertTo-Json -InputObject $Body -Depth 12 -Compress
    }
    Invoke-RestMethod @parameters
}

function New-TestSecret([string]$Name, [string]$Kind, [string]$Value) {
    $response = Invoke-Controller "Post" "/api/v1/secrets" @{
        name = $Name
        kind = $Kind
        scope = @{ type = "global" }
        value = $Value
    }
    $response.data.metadata.metadata.id
}

try {
    foreach ($line in $envLines) {
        $name, $value = $line -split "=", 2
        $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
        [Environment]::SetEnvironmentVariable($name, $value, "Process")
    }
    [System.IO.File]::WriteAllLines($envFile, $envLines, [System.Text.Encoding]::ASCII)

    $headers = @{ "Content-Type" = "application/json" }

    Write-Host "Starting a disposable, isolated Compose project and SSH test target..."
    $composeAttempted = $true
    Invoke-Compose @("--profile", "onboarding-e2e", "up", "-d", "--build", "postgres", "lxcup-server", "artifacts", "lxcup-worker", "onboarding-ssh-test")

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        try {
            $null = Invoke-Controller "Get" "/health/ready"
            break
        } catch {
            if ((Get-Date) -ge $deadline) { throw "Controller did not become ready within $TimeoutSeconds seconds." }
            Start-Sleep -Seconds 2
        }
    } while ((Get-Date) -lt $deadline)

    $publicKey = ""
    do {
        $composeArgs = @($composePrefix) + @("exec", "-T", "onboarding-ssh-test", "cat", "/etc/ssh/ssh_host_ed25519_key.pub")
        $publicKey = (& docker compose @composeArgs 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $publicKey -match '^ssh-ed25519\s+[A-Za-z0-9+/=]+') { break }
        if ((Get-Date) -ge $deadline) { throw "Could not read the isolated SSH target host key." }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)
    if ($publicKey -notmatch '^ssh-ed25519\s+[A-Za-z0-9+/=]+') {
        throw "Could not read the isolated SSH target host key."
    }
    $knownHosts = "onboarding-ssh-test $publicKey"
    $agentToken = [guid]::NewGuid().ToString("N") + [guid]::NewGuid().ToString("N")
    $credentialSecretId = New-TestSecret "e2e-ssh-password-$suffix" "ssh_password" $password
    $knownHostsSecretId = New-TestSecret "e2e-ssh-known-hosts-$suffix" "ssh_known_hosts" $knownHosts
    $agentTokenSecretId = New-TestSecret "e2e-agent-token-$suffix" "agent_token" $agentToken

    $targetResponse = Invoke-Controller "Post" "/api/v1/targets" @{
        name = "onboarding-e2e-$suffix"
        kind = "linux_server"
        address = "onboarding-ssh-test"
        transport = "ssh"
        ssh_user = "lxcup"
        credential_secret_ref = $credentialSecretId
        ssh_known_hosts_secret_ref = $knownHostsSecretId
        agent_secret_ref = $agentTokenSecretId
    }
    $targetId = $targetResponse.data.id
    $deploymentRequest = @{
        operation = "deploy_agent"
        target_id = $targetId
        mode = "apply"
        parameters = @{ operation = "deploy_agent"; agent_version = "0.3.1" }
        idempotency_key = "onboarding-deploy-$targetId"
        confirmed = $true
    }
    $deployment = Invoke-Controller "Post" "/api/v1/ansible/jobs" $deploymentRequest
    $deploymentId = $deployment.data.id
    $duplicate = Invoke-Controller "Post" "/api/v1/ansible/jobs" $deploymentRequest
    if ($duplicate.data.id -ne $deploymentId) {
        throw "Repeating the onboarding request created a duplicate deployment job."
    }

    Write-Host "Target created. Waiting for agent deployment, heartbeat, health check, and package inventory..."
    $workerRestarted = $false
    $workerStopped = $false
    do {
        Start-Sleep -Seconds 2
        $jobs = (Invoke-Controller "Get" "/api/v1/ansible/jobs").data |
            Where-Object { $_.target.target -eq $targetId }
        $failedJob = $jobs | Where-Object { $_.status -in @("failed", "reconcile_required") } | Select-Object -First 1
        if ($null -ne $failedJob) {
            throw "Onboarding job $($failedJob.id) failed. See $($controllerUrl.TrimEnd('/'))/workflows/$($failedJob.id)"
        }

        $deployJob = $jobs | Where-Object { $_.id -eq $deploymentId } | Select-Object -First 1
        if (-not $SkipWorkerRestart -and -not $workerRestarted -and $deployJob.status -eq "succeeded") {
            Invoke-Compose @("stop", "lxcup-worker")
            $workerStopped = $true
            $healthDeadline = (Get-Date).AddSeconds(90)
            do {
                Start-Sleep -Seconds 1
                $jobs = (Invoke-Controller "Get" "/api/v1/ansible/jobs").data |
                    Where-Object { $_.target.target -eq $targetId }
                $inventoryJob = $jobs | Where-Object { $_.operation -eq "collect_package_inventory" } | Select-Object -First 1
            } while (($null -eq $inventoryJob -or $inventoryJob.status -notin @("queued", "checking", "planned", "applying")) -and (Get-Date) -lt $healthDeadline)
            if ($null -eq $inventoryJob -or $inventoryJob.status -notin @("queued", "checking", "planned", "applying")) {
                throw "The controller did not leave package-inventory work pending while the worker was stopped."
            }
            Invoke-Compose @("start", "lxcup-worker")
            $workerStopped = $false
            $workerRestarted = $true
            Write-Host "Ansible worker restarted with package-inventory work pending; waiting for it to resume."
        }

        $requiredOperations = @("deploy_agent", "health_check", "collect_package_inventory")
        $workflowJobs = foreach ($operation in $requiredOperations) {
            $jobs | Where-Object { $_.operation -eq $operation } | Sort-Object created_at -Descending | Select-Object -First 1
        }
        $complete = $workflowJobs.Count -eq $requiredOperations.Count -and
            @($workflowJobs | Where-Object { $_.status -ne "succeeded" }).Count -eq 0
        if ((Get-Date) -ge $deadline) { throw "Onboarding timed out. Review the related /workflows/<job-id> logs." }
    } while (-not $complete)

    $target = (Invoke-Controller "Get" "/api/v1/targets").data |
        Where-Object { $_.id -eq $targetId } | Select-Object -First 1
    if ($target.state -ne "managed" -or $target.agent_version -ne "0.3.1") {
        throw "The target completed jobs but has not reported a managed 0.3.1 agent heartbeat."
    }
    $inventory = Invoke-Controller "Get" "/api/v1/targets/$targetId/package-inventory"
    if ($inventory.data.status -ne "complete") { throw "The final package inventory is not complete." }

    Write-Host "Onboarding E2E passed for target ${targetId}: deploy, heartbeat, health check, inventory, idempotent resubmission$(if ($workerRestarted) { ', and worker restart' })."
    Write-Host "Workflow logs: $($controllerUrl.TrimEnd('/'))/workflows/$deploymentId"
} catch {
    if ($composeAttempted) {
        Write-Host "Onboarding E2E failed; collecting sanitized diagnostics before cleanup..."
        try {
            $diagnosticTargets = (Invoke-Controller "Get" "/api/v1/targets").data
            $diagnosticJobs = (Invoke-Controller "Get" "/api/v1/ansible/jobs").data
            foreach ($diagnosticTarget in $diagnosticTargets) {
                Write-Host "Target $($diagnosticTarget.id): state=$($diagnosticTarget.state), agent_version=$($diagnosticTarget.agent_version)"
            }
            foreach ($diagnosticJob in $diagnosticJobs) {
                Write-Host "Job $($diagnosticJob.id): operation=$($diagnosticJob.operation), status=$($diagnosticJob.status), error_code=$($diagnosticJob.error_code)"
            }
        } catch {
            Write-Host "Controller diagnostics unavailable: $($_.Exception.Message)"
        }
        $logArgs = @($composePrefix) + @("logs", "--tail", "100", "lxcup-worker", "lxcup-server", "onboarding-ssh-test")
        & docker compose @logArgs
    }
    throw
} finally {
    if ($null -eq $previousPassword) { Remove-Item Env:LXCUP_E2E_SSH_PASSWORD -ErrorAction SilentlyContinue }
    else { $env:LXCUP_E2E_SSH_PASSWORD = $previousPassword }
    foreach ($name in $previousEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], "Process")
    }
    if ($composeAttempted) {
        $cleanupArgs = @($composePrefix) + @("--profile", "onboarding-e2e", "down", "--volumes", "--remove-orphans")
        & docker compose @cleanupArgs *> $null
    }
    Remove-Item -LiteralPath $envFile -Force -ErrorAction SilentlyContinue
    Pop-Location
}
