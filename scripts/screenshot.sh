#!/bin/sh
# Grabs the Kindle's visible framebuffer over SSH into a PNG: scripts/screenshot.sh out.png
set -e
OUT="${1:-screenshot.png}"
: "${KINDLE_HOST:?set KINDLE_HOST to the Kindle's IP}"
H=1648 # ponytail: PW5 visible size; read FBIOGET_VSCREENINFO if other models need it
ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" \
	"s=\$(cat /sys/class/graphics/fb0/stride); y=\$(cut -d, -f2 /sys/class/graphics/fb0/pan); echo \$s; dd if=/dev/fb0 bs=\$s skip=\$y count=$H 2>/dev/null" |
python3 -c '
import sys
from PIL import Image
f = sys.stdin.buffer
stride, w, h = int(f.readline()), 1236, int(sys.argv[2])
Image.frombuffer("L", (w, h), f.read(stride * h), "raw", "L", stride, 1).save(sys.argv[1])
' "$OUT" "$H"
echo "$OUT"
