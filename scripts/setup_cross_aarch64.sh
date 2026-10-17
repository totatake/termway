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
if [ -f /etc/apt/sources.list ]; then
    # If sources.list doesn't already have [arch=...], add [arch=amd64] to standard mirrors
    if ! grep -q "\[arch=" /etc/apt/sources.list; then
        sed -i 's/^deb http/deb [arch=amd64] http/' /etc/apt/sources.list
        sed -i 's/^deb-src http/deb-src [arch=amd64] http/' /etc/apt/sources.list
    fi
fi

# In Ubuntu 24.04+ (deb822 format in ubuntu.sources)
if [ -f /etc/apt/sources.list.d/ubuntu.sources ]; then
    if ! grep -q "Architectures:" /etc/apt/sources.list.d/ubuntu.sources; then
        sed -i '/Suites:/i Architectures: amd64' /etc/apt/sources.list.d/ubuntu.sources
    fi
fi

# Add ports.ubuntu.com for arm64 if not already present
PORTS_LIST="/etc/apt/sources.list.d/arm64-ports.list"
if [ ! -f "$PORTS_LIST" ]; then
    cat <<EOF > "$PORTS_LIST"
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME} main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME}-updates main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports/ ${UBUNTU_CODENAME}-security main restricted universe multiverse
EOF
fi

apt-get update

echo "Installing aarch64 cross-compiler and libraries..."
apt-get install -y --no-install-recommends \
    gcc-aarch64-linux-gnu \
    g++-aarch64-linux-gnu \
    libc6-dev-arm64-cross \
    libpixman-1-dev:arm64 \
    libwayland-dev:arm64

echo "Multiarch cross-compilation setup completed successfully."

