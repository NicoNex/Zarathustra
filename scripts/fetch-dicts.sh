#!/bin/sh
# Downloads free StarDict dictionaries into dev-dicts/ (gitignored), and optionally copies them
# to the Kindle's /mnt/us/dictionaries.
#   scripts/fetch-dicts.sh                       # download + unpack into dev-dicts/
#   KINDLE_HOST=192.168.1.113 scripts/fetch-dicts.sh push   # also copy to the device (SSH)
#   scripts/fetch-dicts.sh push                  # also copy to a USB-mounted Kindle
#
# Sources and licenses (dictionary content is Wiktionary: CC BY-SA 4.0 / GFDL; attribute
# "Wiktionary contributors"):
#   it-it, it-en, en-it: xxyzz/wiktionary_stardict (KOReader's own Wiktionary dictionaries,
#     built from Wikimedia dumps; packaging GPL-3.0-or-later). Indexed inflected forms (.syn).
#   en-en: Vuizur/Wiktionary-Dictionaries (kaikki.org data; CC BY-SA 3.0 / GFDL). Much smaller
#     than xxyzz's en-en (22 MB vs 143 MB compressed); set EN_EN=xxyzz for the full one.
set -e
cd "$(dirname "$0")/.."
OUT=dev-dicts
REL=${WIKT_RELEASE:-20261029}
XX="https://github.com/xxyzz/wiktionary_stardict/releases/download/$REL"
VU="https://raw.githubusercontent.com/Vuizur/Wiktionary-Dictionaries/master"
mkdir -p "$OUT/_dl"

get() { # url file
	[ -s "$OUT/_dl/$2" ] || curl -fSL --retry 3 -o "$OUT/_dl/$2" "$1"
}

for n in it-it it-en en-it; do
	get "$XX/$n.tar.zst" "$n.tar.zst"
	mkdir -p "$OUT/$n" && tar --zstd -xf "$OUT/_dl/$n.tar.zst" -C "$OUT/$n"
done
if [ "$EN_EN" = xxyzz ]; then
	get "$XX/en-en.tar.zst" en-en.tar.zst
	mkdir -p "$OUT/en-en" && tar --zstd -xf "$OUT/_dl/en-en.tar.zst" -C "$OUT/en-en"
else
	get "$VU/English-English%20Wiktionary%20dictionary%20stardict.tar.gz" en-en.tar.gz
	mkdir -p "$OUT/en-en" && tar -xzf "$OUT/_dl/en-en.tar.gz" -C "$OUT/en-en"
fi
du -sh "$OUT"/*/ | grep -v _dl

[ "$1" = push ] || exit 0
bundle() { tar -cf - --exclude=_dl -C "$OUT" .; }
if [ -n "$KINDLE_HOST" ]; then
	bundle | ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" \
		'mkdir -p /mnt/us/dictionaries && tar -xf - -C /mnt/us/dictionaries'
else
	KINDLE="${KINDLE:-/run/media/$USER/Kindle}"
	[ -d "$KINDLE" ] || { echo "Kindle not mounted at $KINDLE (or set KINDLE_HOST)" >&2; exit 1; }
	mkdir -p "$KINDLE/dictionaries"
	bundle | tar -xf - -C "$KINDLE/dictionaries" --no-same-owner --no-same-permissions -m
	sync
fi
echo "Dictionaries copied to /mnt/us/dictionaries."
