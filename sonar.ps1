[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$SonarHostUrl,

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$Token = $env:SONAR_TOKEN,

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$ProjectKey = "Lxcup",

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$ProjectName = "lxcup"
)

$ErrorActionPreference = "Stop"

function Find-SonarScanner {
    $command = Get-Command sonar-scanner -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    $candidateRoots = @(
        $env:SONAR_SCANNER_HOME,
        "$env:ProgramFiles\sonar-scanner",
        "$env:ProgramFiles\SonarSource\sonar-scanner",
        "$env:LOCALAPPDATA\sonar-scanner"
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($root in $candidateRoots) {
        $candidate = Join-Path $root "bin\sonar-scanner.bat"
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }

    throw "sonar-scanner wurde nicht gefunden. Installiere den SonarScanner CLI oder setze SONAR_SCANNER_HOME."
}

function Test-SonarHost {
    param([string]$Url)

    try {
        $status = Invoke-RestMethod -Uri "$($Url.TrimEnd('/'))/api/system/status" -TimeoutSec 10
        if ($status.status -notin @("UP", "STARTING")) {
            throw "SonarQube meldet den Status '$($status.status)'."
        }
    }
    catch {
        throw "SonarQube unter '$Url' ist nicht erreichbar oder nicht bereit. Prüfe URL, DNS/Docker-Netzwerk und Port 9000. Details: $($_.Exception.Message)"
    }
}

function Invoke-Coverage {
    param([string]$Root)

    $coverageDirectory = Join-Path $Root "coverage"
    New-Item -ItemType Directory -Force -Path $coverageDirectory | Out-Null

    $cargoLlvmCov = Get-Command cargo-llvm-cov -ErrorAction SilentlyContinue
    if ($null -ne $cargoLlvmCov) {
        Write-Host "Erzeuge Rust-LCOV-Report..."
        & cargo llvm-cov --workspace --lcov --output-path (Join-Path $coverageDirectory "rust.lcov")
        if ($LASTEXITCODE -ne 0) { throw "cargo llvm-cov konnte den Rust-Coverage-Report nicht erzeugen." }
    }
    else {
        Write-Warning "cargo-llvm-cov ist nicht installiert; Rust-Coverage wird übersprungen."
    }

    $packageManager = Get-Command npm -ErrorAction SilentlyContinue
    if ($null -eq $packageManager) { $packageManager = Get-Command pnpm -ErrorAction SilentlyContinue }
    if ($null -ne $packageManager) {
        Write-Host "Erzeuge Frontend-LCOV-Report..."
        Push-Location (Join-Path $Root "frontend")
        try {
            & $packageManager.Source run test:coverage
            if ($LASTEXITCODE -ne 0) { throw "Der Frontend-Coverage-Test ist fehlgeschlagen." }
        }
        finally { Pop-Location }
    }
    else {
        Write-Warning "Weder npm noch pnpm ist verfügbar; Frontend-Coverage wird übersprungen."
    }
}

$scanner = Find-SonarScanner
$root = $PSScriptRoot
$properties = Join-Path $root "sonar-project.properties"
if (-not (Test-Path -LiteralPath $properties)) {
    throw "sonar-project.properties wurde im Projektroot nicht gefunden: $root"
}

Test-SonarHost -Url $SonarHostUrl
Invoke-Coverage -Root $root

$oldToken = $env:SONAR_TOKEN
try {
    if ([string]::IsNullOrWhiteSpace($Token)) {
        Remove-Item Env:SONAR_TOKEN -ErrorAction SilentlyContinue
        Write-Warning "Kein Token übergeben. Der Scan funktioniert nur, wenn SonarQube anonyme Analyse erlaubt."
    }
    else {
        $env:SONAR_TOKEN = $Token
    }

    $arguments = @(
        "-Dsonar.host.url=$SonarHostUrl",
        "-Dsonar.projectKey=$ProjectKey",
        "-Dsonar.projectName=$ProjectName",
        "-Dsonar.projectBaseDir=$root"
    )

    Write-Host "Starte SonarQube-Analyse für '$ProjectKey' gegen '$SonarHostUrl'..."
    & $scanner @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "sonar-scanner wurde mit Exitcode $LASTEXITCODE beendet."
    }
}
finally {
    if ($null -eq $oldToken) {
        Remove-Item Env:SONAR_TOKEN -ErrorAction SilentlyContinue
    }
    else {
        $env:SONAR_TOKEN = $oldToken
    }
}
