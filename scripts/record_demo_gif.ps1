$ErrorActionPreference = "Stop"

# Automatically resolve the repository root from this script's location ($PSScriptRoot)
$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot

try {
    $PythonExe = Join-Path $RepoRoot ".venv-demo-win\Scripts\python.exe"
    $SetupScript = Join-Path $PSScriptRoot "setup_demo_venv.ps1"
    $RecordScript = Join-Path $PSScriptRoot "record_demo_gif.py"

    if (-not (Test-Path $PythonExe)) {
        Write-Host "[record_demo_gif.ps1] Windows venv not found. Running setup_demo_venv.ps1..."
        & $SetupScript
    }

    & $PythonExe $RecordScript @args
} finally {
    Pop-Location
}

