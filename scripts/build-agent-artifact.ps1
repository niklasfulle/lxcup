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
    }
    finally {
        Pop-Location
    }

    $manifest = @{
        version = $Version
        artifacts = @(("amd64", "arm64") | ForEach-Object {
            $file = "linux-$_"
            $binary = Join-Path $temporaryOutput $file
            if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Agent build did not produce $file." }
            @{ platform = $file; file = $file; sha256 = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant() }
        })
    } | ConvertTo-Json -Depth 4
    New-Item -ItemType Directory -Path $artifactDirectory -Force | Out-Null
    foreach ($architecture in @("amd64", "arm64")) {
        $file = "linux-$architecture"
        Copy-Item -LiteralPath (Join-Path $temporaryOutput $file) -Destination (Join-Path $artifactDirectory $file) -Force
    }
    [System.IO.File]::WriteAllText((Join-Path $artifactDirectory "manifest.json"), "$manifest`n", [System.Text.UTF8Encoding]::new($false))
    Write-Host "Created $artifactDirectory with verified amd64 and arm64 artifacts."
}
finally {
    Remove-Item -LiteralPath $temporaryOutput -Recurse -Force
}
