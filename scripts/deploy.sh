#!/bin/sh
# Builds the static ARM binaries and installs them plus the KUAL extension on a Kindle,
# over SSH when KINDLE_HOST is set (KOReader's SSH server, port 2222), else over USB.
#   scripts/deploy.sh               # install
#   scripts/deploy.sh omero-probe   # install, then run it on the device (SSH only)
set -e
cd "$(dirname "$0")/.."
cargo build --release --target armv7-unknown-linux-musleabihf -p omero
BIN=target/armv7-unknown-linux-musleabihf/release
BINS="omero-probe omero-textprobe"

if [ -n "$KINDLE_HOST" ]; then
	k() { ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" "$@"; }
	k 'mkdir -p /mnt/us/omero/bin /mnt/us/extensions/omero'
	# No scp/sftp assumptions: stream files through ssh.
	for b in $BINS; do
		k "cat > /mnt/us/omero/bin/$b.new && chmod +x /mnt/us/omero/bin/$b.new && mv /mnt/us/omero/bin/$b.new /mnt/us/omero/bin/$b" <"$BIN/$b"
	done
	for f in dist/kual/omero/*; do
		k "cat > /mnt/us/extensions/omero/$(basename "$f")" <"$f"
	done
	echo "Deployed to $KINDLE_HOST."
	if [ -n "$1" ]; then
		k "/mnt/us/extensions/omero/run.sh $1; cat /mnt/us/omero/$1.log"
	fi
else
	KINDLE="${KINDLE:-/run/media/$USER/Kindle}"
	[ -d "$KINDLE" ] || { echo "Kindle not mounted at $KINDLE (or set KINDLE_HOST)" >&2; exit 1; }
	mkdir -p "$KINDLE/omero/bin" "$KINDLE/extensions/omero"
	for b in $BINS; do cp "$BIN/$b" "$KINDLE/omero/bin/"; done
	cp dist/kual/omero/* "$KINDLE/extensions/omero/"
	sync
	echo "Deployed to $KINDLE. Eject, then KUAL → Omero."
fi
