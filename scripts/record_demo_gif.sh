#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ -n "${WSL_DISTRO_NAME:-}" ] || command -v powershell.exe >/dev/null 2>&1; then
    if command -v wslpath >/dev/null 2>&1; then
        WIN_PS1="$(wslpath -w "${SCRIPT_DIR}/record_demo_gif.ps1")"
    else
        DISTRO="${WSL_DISTRO_NAME:-Ubuntu}"
        WIN_PS1="\\\\wsl.localhost\\${DISTRO}$(printf '%s' "${SCRIPT_DIR}/record_demo_gif.ps1" | tr '/' '\\')"
    fi

    if command -v powershell.exe >/dev/null 2>&1; then
        PS_EXE="powershell.exe"
    else
        PS_EXE="/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
    fi

    exec "${PS_EXE}" -NoProfile -ExecutionPolicy Bypass -File "${WIN_PS1}" "$@"
else
    REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
    VENV_PY="${REPO_ROOT}/.venv-demo/bin/python"
    if [ ! -x "${VENV_PY}" ]; then
        "${SCRIPT_DIR}/setup_demo_venv.sh"
    fi
    exec "${VENV_PY}" "${SCRIPT_DIR}/record_demo_gif.py" "$@"
fi

