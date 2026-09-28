[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidatePattern('^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')]
    [string]$Version
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$utf8 = [System.Text.UTF8Encoding]::new($false)

function Update-FirstMatch {
    param(
        [string]$Path,
        [string]$Pattern,
        [string]$Replacement,
        [System.Text.RegularExpressions.RegexOptions]$Options = [System.Text.RegularExpressions.RegexOptions]::Multiline
    )
    $content = [System.IO.File]::ReadAllText($Path)
    $regex = [regex]::new($Pattern, $Options)
    if (-not $regex.IsMatch($content)) { throw "Version field not found in $Path." }
    $updated = $regex.Replace($content, $Replacement, 1)
    [System.IO.File]::WriteAllText($Path, $updated, $utf8)
}

function Update-AllMatches {
    param([string]$Path, [string]$Pattern, [string]$Replacement)
    $content = [System.IO.File]::ReadAllText($Path)
    $regex = [regex]::new($Pattern, [System.Text.RegularExpressions.RegexOptions]::Multiline)
    if ($regex.Matches($content).Count -lt 2) { throw "Expected both package version fields in $Path." }
    [System.IO.File]::WriteAllText($Path, $regex.Replace($content, $Replacement), $utf8)
}

$cargoManifest = Join-Path $repoRoot "Cargo.toml"
Update-FirstMatch $cargoManifest '(\[workspace\.package\]\s*version\s*=\s*")[^"]+' ('${1}' + $Version) ([System.Text.RegularExpressions.RegexOptions]::Multiline -bor [System.Text.RegularExpressions.RegexOptions]::Singleline)

Update-FirstMatch (Join-Path $repoRoot "frontend\package.json") '(?m)(^  "version": ")[^"]+' ('${1}' + $Version)
Update-AllMatches (Join-Path $repoRoot "frontend\package-lock.json") '("name": "lxcup-frontend",\s*"version": ")[^"]+' ('${1}' + $Version)

Push-Location $repoRoot
try {
    cargo check --workspace
    if ($LASTEXITCODE -ne 0) { throw "Cargo could not refresh the workspace lockfile for version $Version." }
}
finally {
    Pop-Location
}

Write-Host "Workspace version set to $Version. Build the Linux agent artifact with the versioned artifact procedure in docs/worker-setup.md."
