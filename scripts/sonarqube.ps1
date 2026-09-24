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
    [string]$ProjectName = "lxcup",

    [Parameter(Mandatory = $false)]
    [ValidateNotNullOrEmpty()]
    [string]$ProjectVersion = "0.2.0"
)

$ErrorActionPreference = "Stop"
$rootScript = Join-Path (Split-Path -Parent $PSScriptRoot) "sonar.ps1"
& $rootScript -SonarHostUrl $SonarHostUrl -Token $Token -ProjectKey $ProjectKey -ProjectName $ProjectName -ProjectVersion $ProjectVersion
exit $LASTEXITCODE
