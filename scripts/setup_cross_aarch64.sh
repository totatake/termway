#!/usr/bin/env bash
set -euo pipefail

# setup_cross_aarch64.sh
# Sets up cross-compilation toolchain and arm64 multiarch libraries for building termway on Ubuntu.

if [ "$(id -u)" -ne 0 ]; then
    echo "This script must be run as root (or with sudo)." >&2
    exit 1
fi

UBUNTU_CODENAME=$(lsb_release -cs 2>/dev/null || (. /etc/os-release && echo "$UBUNTU_CODENAME"))

echo "Configuring multiarch for arm64 (Ubuntu ${UBUNTU_CODENAME})..."
dpkg --add-architecture arm64

# Configure APT sources for arm64 ports
# Standard Ubuntu archive/security mirrors (archive.ubuntu.com, security.ubuntu.com, azure.archive.ubuntu.com)
# only host amd64/i386. When multiarch arm64 is enabled, apt will return 404 on those mirrors unless
# [arch=amd64] is explicitly set on all deb/deb-src lines that do not already have an [arch=...] restriction.

restrict_to_amd64() {
    local target="$1"
    [ -f "$target" ] || return 0
    # 1. If options already exist "[...]" but don't specify arch=, insert arch=amd64
    sed -i -E 's/^[[:space:]]*(deb(-src)?)[[:space:]]+\[([^]]*)]/\1 [arch=amd64 \3]/' "$target"
    sed -i -E 's/arch=amd64[[:space:]]+(.*arch=)/\1/' "$target"
    # 2. If no options exist (next non-whitespace char is not "["), insert [arch=amd64]
    sed -i -E 's/^[[:space:]]*(deb(-src)?)[[:space:]]+([^\[[:space:]])/\1 [arch=amd64] \3/' "$target"
}

if [ -f /etc/apt/sources.list ]; then
    restrict_to_amd64 /etc/apt/sources.list
fi

# Also update any .list files in sources.list.d (except our arm64-ports.list)
if [ -d /etc/apt/sources.list.d ]; then
    for f in /etc/apt/sources.list.d/*.list; do
        [ -f "$f" ] || continue
        [ "$(basename "$f")" = "arm64-ports.list" ] && continue
        restrict_to_amd64 "$f"
    done
fi

# In Ubuntu 24.04+ (deb822 format in *.sources)
if [ -d /etc/apt/sources.list.d ]; then
    for f in /etc/apt/sources.list.d/*.sources; do
        [ -f "$f" ] || continue
        [ "$(basename "$f")" = "arm64-ports.sources" ] && continue
        if ! grep -q "^Architectures:" "$f"; then
            sed -i '/^Suites:/i Architectures: amd64' "$f"
        fi
    done
fi

# Add ports.ubuntu.com for arm64
PORTS_LIST="/etc/apt/sources.list.d/arm64-ports.list"
cat <<EOF > "$PORTS_LIST"
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME} main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME}-updates main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME}-security main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME}-backports main restricted universe multiverse
EOF

apt-get update

echo "Installing aarch64 cross-compiler and libraries..."
apt-get install -y --no-install-recommends \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    libc6-dev-arm64-cross \
    pkg-config \
    libpixman-1-dev:arm64 \
    libwayland-dev:arm64

echo "Multiarch cross-compilation setup completed successfully."

