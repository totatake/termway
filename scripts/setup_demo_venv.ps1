$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir
$VenvDir = Join-Path $RepoRoot ".venv-demo-win"

Write-Host "[1/3] Creating Windows Python virtual environment at $VenvDir..."
python -m venv $VenvDir

$PipExe = Join-Path $VenvDir "Scripts\pip.exe"
$PythonExe = Join-Path $VenvDir "Scripts\python.exe"
$ReqFile = Join-Path $ScriptDir "requirements-demo.txt"
$RecordScript = Join-Path $ScriptDir "record_demo_gif.py"

Write-Host "[2/3] Upgrading pip inside venv..."
& $PipExe install --upgrade pip

Write-Host "[3/3] Installing demo recording dependencies into venv..."
& $PipExe install -r $ReqFile

Write-Host ""
Write-Host "Setup complete!"
Write-Host "Run the recorder from PowerShell with:"
Write-Host "  .\scripts\record_demo_gif.ps1 --calibrate"

