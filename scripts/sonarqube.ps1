$ErrorActionPreference = "Stop"

if (-not (Get-Command sonar-scanner -ErrorAction SilentlyContinue)) {
    throw "sonar-scanner is not installed or not available on PATH."
}

sonar-scanner -Dsonar.projectKey=lxcup -Dsonar.projectName=lxcup
