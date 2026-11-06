#!/bin/sh
# Builds the static ARM binaries and installs them plus the KUAL extension on a Kindle,
# over SSH when KINDLE_HOST is set (KOReader's SSH server, port 2222), else over USB.
#   scripts/deploy.sh               # install
#   scripts/deploy.sh zarathustra-probe   # install, then run it on the device (SSH only)
set -e
cd "$(dirname "$0")/.."
cargo build --release --target armv7-unknown-linux-musleabihf -p zarathustra
BIN=target/armv7-unknown-linux-musleabihf/release
BINS="zarathustra-probe zarathustra-textprobe zarathustra-powerprobe zarathustra"

# Stage the device layout (zarathustra/bin/*, extensions/zarathustra/*) and ship it as one tar stream.
STAGE=target/kindle-stage
rm -rf "$STAGE" && mkdir -p "$STAGE/zarathustra/bin" "$STAGE/extensions"
for b in $BINS; do cp "$BIN/$b" "$STAGE/zarathustra/bin/"; done
cp -r dist/kual/zarathustra "$STAGE/extensions/"
mkdir -p "$STAGE/zarathustra/hyph" && cp assets/hyphenation/*.bincode "$STAGE/zarathustra/hyph/"
bundle() { tar -cf - -C "$STAGE" zarathustra extensions; }
# The project was called Omero: carry its data (positions, highlights, settings) over once.
# ponytail: drop this after every device has run a deploy since 2026-11-06.
MIGRATE='if [ -d omero ] && [ ! -d zarathustra ]; then mv omero zarathustra;
for f in zarathustra/state/omero.db*; do [ -e "$f" ] && mv "$f" "zarathustra/state/zarathustra.db${f#zarathustra/state/omero.db}"; done;
rm -f zarathustra/bin/omero* zarathustra/bin/powerprobe; fi; rm -rf extensions/omero'

if [ -n "$KINDLE_HOST" ]; then
	k() { ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" "$@"; }
	# A running instance would keep the old binary and fight the new one for the screen.
	k 'killall zarathustra omero 2>/dev/null; true'
	k "cd /mnt/us && $MIGRATE"
	bundle | k 'tar -xf - -C /mnt/us'
	echo "Deployed to $KINDLE_HOST."
	[ -z "$1" ] || k "/mnt/us/extensions/zarathustra/run.sh $*; cat /mnt/us/zarathustra/$1.log"
else
	KINDLE="${KINDLE:-/run/media/$USER/Kindle}"
	[ -d "$KINDLE" ] || { echo "Kindle not mounted at $KINDLE (or set KINDLE_HOST)" >&2; exit 1; }
	(cd "$KINDLE" && eval "$MIGRATE")
	# VFAT: no ownership, permissions or mtimes to restore.
	bundle | tar -xf - -C "$KINDLE" --no-same-owner --no-same-permissions -m
	sync
	echo "Deployed to $KINDLE. Eject, then KUAL → Zarathustra."
fi
