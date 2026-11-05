#!/bin/sh
# Builds the static ARM binaries and installs them plus the KUAL extension on a Kindle,
# over SSH when KINDLE_HOST is set (KOReader's SSH server, port 2222), else over USB.
#   scripts/deploy.sh               # install
#   scripts/deploy.sh omero-probe   # install, then run it on the device (SSH only)
set -e
cd "$(dirname "$0")/.."
cargo build --release --target armv7-unknown-linux-musleabihf -p omero
BIN=target/armv7-unknown-linux-musleabihf/release
BINS="omero-probe omero-textprobe omero-reader"

# Stage the device layout (omero/bin/*, extensions/omero/*) and ship it as one tar stream.
STAGE=target/kindle-stage
rm -rf "$STAGE" && mkdir -p "$STAGE/omero/bin" "$STAGE/extensions"
for b in $BINS; do cp "$BIN/$b" "$STAGE/omero/bin/"; done
cp -r dist/kual/omero "$STAGE/extensions/"
mkdir -p "$STAGE/omero/hyph" && cp assets/hyphenation/*.bincode "$STAGE/omero/hyph/"
bundle() { tar -cf - -C "$STAGE" omero extensions; }

if [ -n "$KINDLE_HOST" ]; then
	k() { ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" "$@"; }
	bundle | k 'tar -xf - -C /mnt/us'
	echo "Deployed to $KINDLE_HOST."
	[ -z "$1" ] || k "/mnt/us/extensions/omero/run.sh $*; cat /mnt/us/omero/$1.log"
else
	KINDLE="${KINDLE:-/run/media/$USER/Kindle}"
	[ -d "$KINDLE" ] || { echo "Kindle not mounted at $KINDLE (or set KINDLE_HOST)" >&2; exit 1; }
	# VFAT: no ownership, permissions or mtimes to restore.
	bundle | tar -xf - -C "$KINDLE" --no-same-owner --no-same-permissions -m
	sync
	echo "Deployed to $KINDLE. Eject, then KUAL → Omero."
fi
