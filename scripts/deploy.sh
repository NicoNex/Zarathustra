#!/bin/sh
# Builds the static ARM binaries and copies them plus the KUAL extension to a USB-mounted Kindle.
set -e
KINDLE="${KINDLE:-/run/media/$USER/Kindle}"
[ -d "$KINDLE" ] || { echo "Kindle not mounted at $KINDLE" >&2; exit 1; }
cd "$(dirname "$0")/.."
cargo build --release --target armv7-unknown-linux-musleabihf -p omero
mkdir -p "$KINDLE/omero/bin" "$KINDLE/extensions/omero"
cp target/armv7-unknown-linux-musleabihf/release/omero-probe "$KINDLE/omero/bin/"
cp dist/kual/omero/* "$KINDLE/extensions/omero/"
sync
echo "Deployed to $KINDLE. Eject, then KUAL → Omero."
