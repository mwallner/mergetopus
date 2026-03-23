$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$exePath = Join-Path $toolsDir 'mergetopus.exe'

if (Test-Path $exePath) {
    Remove-Item $exePath -Force
}
