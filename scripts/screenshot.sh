#!/bin/sh
# Grabs the Kindle's visible framebuffer over SSH into a PNG: scripts/screenshot.sh out.png
set -e
OUT="${1:-screenshot.png}"
H=1648 # ponytail: PW5 visible size; read FBIOGET_VSCREENINFO if other models need it
ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" \
	"s=\$(cat /sys/class/graphics/fb0/stride); echo \$s; head -c \$((s * $H)) /dev/fb0" |
python3 -c '
import sys
from PIL import Image
f = sys.stdin.buffer
stride, w, h = int(f.readline()), 1236, int(sys.argv[2])
Image.frombuffer("L", (w, h), f.read(stride * h), "raw", "L", stride, 1).save(sys.argv[1])
' "$OUT" "$H"
echo "$OUT"
