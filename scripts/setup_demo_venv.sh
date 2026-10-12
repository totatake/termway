#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENV_DIR="${REPO_ROOT}/.venv-demo"

echo "[1/3] Creating Python virtual environment at ${VENV_DIR}..."
python3 -m venv "${VENV_DIR}"

echo "[2/3] Upgrading pip inside venv..."
"${VENV_DIR}/bin/pip" install --upgrade pip

echo "[3/3] Installing demo recording dependencies into venv..."
"${VENV_DIR}/bin/pip" install -r "${SCRIPT_DIR}/requirements-demo.txt"

echo ""
echo "Setup complete!"
echo "Run the recorder with:"
echo "  ./scripts/record_demo_gif.sh --calibrate"

