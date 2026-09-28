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
        docker buildx build --platform linux/amd64 --file deploy/agent-artifact.Dockerfile --output "type=local,dest=$temporaryOutput" .
        if ($LASTEXITCODE -ne 0) { throw "Linux amd64 agent build failed." }
    }
    finally {
        Pop-Location
    }

    $binary = Join-Path $temporaryOutput "linux-amd64"
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Agent build did not produce linux-amd64." }
    New-Item -ItemType Directory -Path $artifactDirectory -Force | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $artifactDirectory "linux-amd64") -Force
    $checksum = (Get-FileHash -LiteralPath (Join-Path $artifactDirectory "linux-amd64") -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest = @{
        version = $Version
        artifacts = @(@{
            platform = "linux-amd64"
            file = "linux-amd64"
            sha256 = $checksum
        })
    } | ConvertTo-Json -Depth 4
    [System.IO.File]::WriteAllText((Join-Path $artifactDirectory "manifest.json"), "$manifest`n", [System.Text.UTF8Encoding]::new($false))
    Write-Host "Created $artifactDirectory with SHA-256 $checksum"
}
finally {
    Remove-Item -LiteralPath $temporaryOutput -Recurse -Force
}
