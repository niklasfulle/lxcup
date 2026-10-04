$ErrorActionPreference = 'Stop'
$setupPath = Join-Path $PSScriptRoot '..\frontend\public\windows-agent-setup.ps1'
$setupPath = (Resolve-Path -LiteralPath $setupPath).Path
$tokens = $null
$parseErrors = $null
$scriptAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $setupPath,
    [ref]$tokens,
    [ref]$parseErrors
)
if ($parseErrors.Count -gt 0) {
    $parseErrors | ForEach-Object { Write-Error $_.Message }
    throw 'Windows setup script has PowerShell syntax errors.'
}

$peFunction = $scriptAst.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Assert-WindowsAmd64Pe'
}, $true)
if (-not $peFunction) { throw 'PE validation function was not found.' }
Invoke-Expression $peFunction.Extent.Text

function New-TestPortableExecutable([string]$Path, [uint16]$Machine, [uint16]$Magic) {
    $bytes = [byte[]]::new(0x100)
    $bytes[0] = 0x4D
    $bytes[1] = 0x5A
    [BitConverter]::GetBytes([int32]0x80).CopyTo($bytes, 0x3C)
    [BitConverter]::GetBytes([uint32]0x00004550).CopyTo($bytes, 0x80)
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 0x84)
    [BitConverter]::GetBytes($Magic).CopyTo($bytes, 0x98)
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "lxcup-pe-test-$([guid]::NewGuid().ToString('N'))"
[void](New-Item -ItemType Directory -Path $temporaryDirectory)
try {
    $validPath = Join-Path $temporaryDirectory 'valid.exe'
    New-TestPortableExecutable $validPath 0x8664 0x020B
    Assert-WindowsAmd64Pe $validPath

    $wrongMachinePath = Join-Path $temporaryDirectory 'wrong-machine.exe'
    New-TestPortableExecutable $wrongMachinePath 0x014C 0x020B
    $wrongMachineRejected = $false
    try { Assert-WindowsAmd64Pe $wrongMachinePath }
    catch { $wrongMachineRejected = $_.Exception.Message -like '*amd64*' }
    if (-not $wrongMachineRejected) { throw 'PE validator did not reject a non-amd64 executable.' }

    $wrongMagicPath = Join-Path $temporaryDirectory 'wrong-magic.exe'
    New-TestPortableExecutable $wrongMagicPath 0x8664 0x010B
    $wrongMagicRejected = $false
    try { Assert-WindowsAmd64Pe $wrongMagicPath }
    catch { $wrongMagicRejected = $_.Exception.Message -like '*PE32+*' }
    if (-not $wrongMagicRejected) { throw 'PE validator did not reject a PE32 executable.' }
}
finally {
    Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host 'Windows setup syntax and PE format tests passed.'
