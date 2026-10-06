[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ControllerUrl,

    [Parameter(Mandatory = $true)]
    [guid]$TargetId,

    [Parameter(Mandatory = $true)]
    [string]$Version
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$serviceName = 'lxcup-agent'
$installDirectory = Join-Path $env:ProgramFiles 'lxcup'
$dataDirectory = Join-Path $env:ProgramData 'lxcup'
$agentPath = Join-Path $installDirectory 'lxcup-agent.exe'
$configPath = Join-Path $dataDirectory 'agent.env'
$privateIdentities = @(
    [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18'),
    [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
)
$stagedAgent = $null
$stagedConfig = $null
$temporaryDirectory = $null

function Set-PrivateAcl([string]$Path, [System.Security.Principal.SecurityIdentifier[]]$Identities) {
    $acl = Get-Acl -LiteralPath $Path
    $acl.SetAccessRuleProtection($true, $false)
    $inheritance = [System.Security.AccessControl.InheritanceFlags]::None
    if ((Get-Item -LiteralPath $Path).PSIsContainer) {
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
            [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
    }
    foreach ($identity in $Identities) {
        $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $identity,
            [System.Security.AccessControl.FileSystemRights]::FullControl,
            $inheritance,
            [System.Security.AccessControl.PropagationFlags]::None,
            [System.Security.AccessControl.AccessControlType]::Allow
        )
        [void]$acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

function Assert-WindowsAmd64Pe([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Agent artifact is not a Windows executable.' }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadInt32()
        if ($peOffset -lt 64 -or $peOffset -gt ($stream.Length - 26)) {
            throw 'Agent executable has an invalid PE header.'
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550 -or $reader.ReadUInt16() -ne 0x8664) {
            throw 'Agent artifact is not a PE32+ Windows amd64 executable.'
        }
        $stream.Position = $peOffset + 24
        if ($reader.ReadUInt16() -ne 0x20B) { throw 'Agent artifact is not a PE32+ executable.' }
    }
    finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

try {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Start PowerShell as Administrator and run this setup script again.'
    }
    if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
        throw 'Only Windows amd64 is supported by this release.'
    }
    if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
        throw 'The requested agent version is invalid.'
    }

    $controller = [Uri]::new($ControllerUrl.TrimEnd('/'))
    $localHttpAllowed = $controller.Scheme -eq 'http' -and $controller.IsLoopback
    if ($controller.Scheme -ne 'https' -and -not $localHttpAllowed) {
        throw 'The controller URL must use HTTPS. HTTP is allowed only for loopback development.'
    }
    if ($controller.UserInfo -or $controller.Query -or $controller.Fragment -or $controller.AbsolutePath -ne '/') {
        throw 'Use only the controller origin, without credentials, path, query, or fragment.'
    }

    $artifactBase = "$($controller.AbsoluteUri.TrimEnd('/'))/agent/$Version"
    $packageDirectory = Split-Path -Parent $PSCommandPath
    $packagedManifestPath = Join-Path $packageDirectory 'manifest.json'
    $packagedArtifactPath = Join-Path $packageDirectory 'windows-amd64.exe'
    $usesPackagedArtifact = (Test-Path -LiteralPath $packagedManifestPath -PathType Leaf) -and
        (Test-Path -LiteralPath $packagedArtifactPath -PathType Leaf)
    if ($usesPackagedArtifact) {
        $manifest = Get-Content -LiteralPath $packagedManifestPath -Raw | ConvertFrom-Json
    }
    else {
        $manifest = Invoke-RestMethod -Uri "$artifactBase/manifest.json" -Method Get
    }
    if ($manifest.version -ne $Version -or -not $manifest.artifacts) {
        throw 'The controller returned a manifest for a different or unsupported version.'
    }
    $artifact = @($manifest.artifacts | Where-Object { $_.platform -eq 'windows-amd64' })
    if ($artifact.Count -ne 1 -or $artifact[0].file -ne 'windows-amd64.exe' -or $artifact[0].sha256 -notmatch '^[a-fA-F0-9]{64}$') {
        throw 'The controller manifest has no valid Windows amd64 artifact.'
    }

    $temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "lxcup-agent-$([guid]::NewGuid().ToString('N'))"
    [void](New-Item -ItemType Directory -Path $temporaryDirectory)
    $downloadPath = Join-Path $temporaryDirectory 'windows-amd64.exe'
    if ($usesPackagedArtifact) {
        Copy-Item -LiteralPath $packagedArtifactPath -Destination $downloadPath
    }
    else {
        Invoke-WebRequest -Uri "$artifactBase/windows-amd64.exe" -OutFile $downloadPath -Method Get
    }
    $actualHash = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash
    if (-not [string]::Equals($actualHash, $artifact[0].sha256, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The downloaded agent checksum does not match the manifest.'
    }
    Assert-WindowsAmd64Pe $downloadPath

    $secureToken = Read-Host 'Agent-Token eingeben' -AsSecureString
    if ($secureToken.Length -lt 1) { throw 'An agent token is required.' }
    $tokenPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureToken)
    try {
        $token = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($tokenPointer)
        if ($token -match '[\r\n]') { throw 'The agent token contains unsupported line breaks.' }
        [void](New-Item -ItemType Directory -Force -Path $installDirectory)
        [void](New-Item -ItemType Directory -Force -Path $dataDirectory)
        Set-PrivateAcl $dataDirectory $privateIdentities
        $stagedConfig = Join-Path $dataDirectory "agent-$([guid]::NewGuid().ToString('N')).tmp"
        $config = @(
            "LXCUP_AGENT_TOKEN=$token",
            "LXCUP_AGENT_ID=lxcup-$TargetId",
            "LXCUP_TARGET_ID=$TargetId",
            "LXCUP_CONTROLLER_URL=$($controller.AbsoluteUri.TrimEnd('/'))",
            'LXCUP_AGENT_BIND_ADDRESS=127.0.0.1:8090'
        ) -join "`r`n"
        [System.IO.File]::WriteAllText($stagedConfig, "$config`r`n", [System.Text.UTF8Encoding]::new($false))
        Set-PrivateAcl $stagedConfig $privateIdentities
        $config = $null
        $token = $null
    }
    finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($tokenPointer)
        $secureToken.Dispose()
    }

    $stagedAgent = Join-Path $installDirectory "lxcup-agent-$([guid]::NewGuid().ToString('N')).tmp.exe"
    $previousAgent = Join-Path $installDirectory "lxcup-agent-$([guid]::NewGuid().ToString('N')).previous.exe"
    $previousConfig = Join-Path $dataDirectory "agent-$([guid]::NewGuid().ToString('N')).previous"
    Copy-Item -LiteralPath $downloadPath -Destination $stagedAgent -Force
    $service = Get-CimInstance Win32_Service -Filter "Name='$serviceName'" -ErrorAction SilentlyContinue
    $hadAgent = Test-Path -LiteralPath $agentPath -PathType Leaf
    $hadConfig = Test-Path -LiteralPath $configPath -PathType Leaf
    $serviceWasRunning = $service -and $service.State -eq 'Running'
    $serviceCreated = $false
    try {
        if ($service -and $service.State -ne 'Stopped') { Stop-Service -Name $serviceName -Force -ErrorAction Stop }
        if ($hadAgent) { Move-Item -LiteralPath $agentPath -Destination $previousAgent }
        if ($hadConfig) { Move-Item -LiteralPath $configPath -Destination $previousConfig }
        Move-Item -LiteralPath $stagedAgent -Destination $agentPath
        Move-Item -LiteralPath $stagedConfig -Destination $configPath
        Set-PrivateAcl $configPath $privateIdentities

        if (-not $service) {
            New-Service -Name $serviceName -BinaryPathName "`"$agentPath`"" -DisplayName 'lxcup Agent' -StartupType Automatic | Out-Null
            $serviceCreated = $true
        }
        else {
            $serviceBinaryPath = "binPath= `"$agentPath`""
            & sc.exe config $serviceName $serviceBinaryPath start= auto obj= LocalSystem | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'Could not update the lxcup agent service configuration.' }
        }
        $startupLogPath = Join-Path $dataDirectory 'agent-startup.log'
        Remove-Item -LiteralPath $startupLogPath -Force -ErrorAction SilentlyContinue
        Start-Service -Name $serviceName
        (Get-Service -Name $serviceName).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
    }
    catch {
        Stop-Service -Name $serviceName -Force -ErrorAction SilentlyContinue
        if ($serviceCreated) {
            & sc.exe delete $serviceName | Out-Null
        }
        if (Test-Path -LiteralPath $agentPath) { Remove-Item -LiteralPath $agentPath -Force }
        if (Test-Path -LiteralPath $configPath) { Remove-Item -LiteralPath $configPath -Force }
        if ($hadAgent -and (Test-Path -LiteralPath $previousAgent)) {
            Move-Item -LiteralPath $previousAgent -Destination $agentPath
        }
        if ($hadConfig -and (Test-Path -LiteralPath $previousConfig)) {
            Move-Item -LiteralPath $previousConfig -Destination $configPath
            Set-PrivateAcl $configPath $privateIdentities
        }
        if ($serviceWasRunning) {
            Start-Service -Name $serviceName -ErrorAction SilentlyContinue
        }
        throw
    }
    Remove-Item -LiteralPath $previousAgent, $previousConfig -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    Write-Host "lxcup Agent $Version was installed and the service is running."
}
catch {
    $failureMessage = "Agent setup failed: $($_.Exception.Message)"
    $startupLogPath = Join-Path $dataDirectory 'agent-startup.log'
    if (Test-Path -LiteralPath $startupLogPath -PathType Leaf) {
        $startupDetail = (Get-Content -LiteralPath $startupLogPath -Raw).Trim()
        if ($startupDetail) { $failureMessage += " Agent startup detail: $startupDetail" }
    }
    Write-Error $failureMessage
    if ($stagedAgent -and (Test-Path -LiteralPath $stagedAgent)) {
        Remove-Item -LiteralPath $stagedAgent -Force -ErrorAction SilentlyContinue
    }
    if ($stagedConfig -and (Test-Path -LiteralPath $stagedConfig)) {
        Remove-Item -LiteralPath $stagedConfig -Force -ErrorAction SilentlyContinue
    }
    if ($temporaryDirectory -and (Test-Path -LiteralPath $temporaryDirectory)) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit 1
}
