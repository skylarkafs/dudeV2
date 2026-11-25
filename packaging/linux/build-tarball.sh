#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.1.0}"
OUT_DIR="dist/nextdude-server-${VERSION}-linux-x86_64"
rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}/bin" "${OUT_DIR}/etc" "${OUT_DIR}/profiles" "${OUT_DIR}/systemd"

cp target/release/nextdude-server "${OUT_DIR}/bin/"
cp packaging/linux/server.toml "${OUT_DIR}/etc/"
cp packaging/linux/nextdude-server.service "${OUT_DIR}/systemd/"
cp profiles/*.toml "${OUT_DIR}/profiles/"
cp packaging/linux/postinst "${OUT_DIR}/"

tar -C dist -czf "dist/nextdude-server-${VERSION}-linux-x86_64.tar.gz" "nextdude-server-${VERSION}-linux-x86_64"
echo "Created dist/nextdude-server-${VERSION}-linux-x86_64.tar.gz"
