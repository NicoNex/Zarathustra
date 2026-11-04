#!/bin/sh
# Grabs the Kindle's visible framebuffer over SSH into a PNG: scripts/screenshot.sh out.png
set -e
OUT="${1:-screenshot.png}"
ssh -p "${KINDLE_PORT:-2222}" -o BatchMode=yes "root@$KINDLE_HOST" \
	'cat /sys/class/graphics/fb0/stride; cat /sys/class/graphics/fb0/virtual_size; head -c $((1248*1648)) /dev/fb0' |
python3 -c '
import sys
from PIL import Image
f = sys.stdin.buffer
stride = int(f.readline()); f.readline()
w, h = 1236, 1648  # ponytail: PW5 visible size; read FBIOGET_VSCREENINFO if other models need it
Image.frombuffer("L", (w, h), f.read(stride * h), "raw", "L", stride, 1).save(sys.argv[1])
' "$OUT"
echo "$OUT"
