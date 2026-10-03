[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$cargoManifest = Get-Content -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Raw
$versionMatch = [regex]::Match($cargoManifest, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) { throw "Could not read the workspace version from Cargo.toml." }
$workspaceVersion = $versionMatch.Groups[1].Value
if ([string]::IsNullOrWhiteSpace($Version)) { $Version = $workspaceVersion }
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') { throw "Invalid release version: $Version" }
if ($Version -ne $workspaceVersion) { throw "Artifact version $Version does not match Cargo workspace version $workspaceVersion." }

$temporaryOutput = Join-Path ([System.IO.Path]::GetTempPath()) "lxcup-agent-artifact-$([guid]::NewGuid().ToString('N'))"
$artifactDirectory = Join-Path $repoRoot "artifacts\agent\$Version"
New-Item -ItemType Directory -Path $temporaryOutput | Out-Null
try {
    Push-Location $repoRoot
    try {
        foreach ($architecture in @("amd64", "arm64")) {
            docker buildx build --platform "linux/$architecture" --file deploy/agent-artifact.Dockerfile --output "type=local,dest=$temporaryOutput" .
            if ($LASTEXITCODE -ne 0) { throw "Linux $architecture agent build failed." }
        }
        docker buildx build --platform "linux/amd64" --file deploy/agent-windows-artifact.Dockerfile --output "type=local,dest=$temporaryOutput" .
        if ($LASTEXITCODE -ne 0) { throw "Windows amd64 agent build failed." }
    }
    finally {
        Pop-Location
    }

    $artifacts = @("amd64", "arm64") | ForEach-Object {
        $file = "linux-$_"
        $binary = Join-Path $temporaryOutput $file
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Agent build did not produce $file." }
        @{ platform = $file; file = $file; sha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $windowsFile = "windows-amd64.exe"
    $windowsBinary = Join-Path $temporaryOutput $windowsFile
    if (-not (Test-Path -LiteralPath $windowsBinary -PathType Leaf)) { throw "Agent build did not produce $windowsFile." }
    $artifacts += @{ platform = "windows-amd64"; file = $windowsFile; sha256 = (Get-FileHash -LiteralPath $windowsBinary -Algorithm SHA256).Hash.ToLowerInvariant() }
    $manifest = @{
        version = $Version
        artifacts = $artifacts
    } | ConvertTo-Json -Depth 4
    New-Item -ItemType Directory -Path $artifactDirectory -Force | Out-Null
    foreach ($architecture in @("amd64", "arm64")) {
        $file = "linux-$architecture"
        Copy-Item -LiteralPath (Join-Path $temporaryOutput $file) -Destination (Join-Path $artifactDirectory $file) -Force
    }
    Copy-Item -LiteralPath (Join-Path $temporaryOutput "windows-amd64.exe") -Destination (Join-Path $artifactDirectory "windows-amd64.exe") -Force
    [System.IO.File]::WriteAllText((Join-Path $artifactDirectory "manifest.json"), "$manifest`n", [System.Text.UTF8Encoding]::new($false))
    Write-Host "Created $artifactDirectory with verified Linux amd64/arm64 and Windows amd64 artifacts."
}
finally {
    Remove-Item -LiteralPath $temporaryOutput -Recurse -Force
}
