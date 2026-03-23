$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$exePath = Join-Path $toolsDir 'mergetopus.exe'

if (-not (Test-Path $exePath)) {
    throw "Expected embedded executable not found: $exePath"
}

Write-Host 'mergetopus executable is embedded in this package and ready to use.'
