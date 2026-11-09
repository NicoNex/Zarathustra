#!/bin/sh
# Runs Zarathustra on a Kobo with Nickel stopped, and always brings Nickel back.
#   zarathustra.sh [NAME [ARGS...]]     NAME: a binary in bin/ (default zarathustra)
# Started from NickelMenu. Script and binary run from /tmp and the log is written there, so
# nothing stays open on /mnt/onboard when it is handed to a computer over USB.
# Exit statuses of the app: 0 quit; 42 (USB_EXIT in app.rs) hand the storage to the computer,
# then start again once the cable is out; the update flag file: start the new binary again.
APP=/mnt/onboard/.adds/zarathustra
if [ "$0" != /tmp/zarathustra-run.sh ]; then
	cp "$0" /tmp/zarathustra-run.sh && exec /tmp/zarathustra-run.sh "$@"
fi
cd /
NAME="${1:-zarathustra}"
[ $# -gt 0 ] && shift
BIN="/tmp/$NAME"
LOG="/tmp/$NAME.log"
USB_EXIT=42
UPDATE_FLAG=/tmp/zarathustra-update
FIFO=/tmp/nickel-hardware-status
NICKEL_ENV=/tmp/zarathustra-nickel.env

# Nickel's environment from rcS (platform, Wi-Fi module and interface…), to restart it as
# it was and for the app's Wi-Fi. Not its LD_LIBRARY_PATH: Nickel's Qt libraries would shadow
# the system ones in wpa_supplicant, dhcpcd and the rest; it is set for Nickel alone.
if pidof nickel >/dev/null; then
	tr '\0' '\n' <"/proc/$(pidof -s nickel)/environ" |
		grep -E '^(PLATFORM|PRODUCT|WIFI_MODULE|INTERFACE|LANG|NICKEL_HOME|DBUS_SESSION_BUS_ADDRESS)=' |
		sed 's/^/export /' >"$NICKEL_ENV"
fi
[ -f "$NICKEL_ENV" ] && . "$NICKEL_ENV"

restore() {
	cp "$LOG" "$APP/$NAME.log" 2>/dev/null
	# The screen mode Nickel left (the app switches it to 8-bit gray), the Wi-Fi off.
	"$BIN" --release-device >>"$LOG" 2>&1
	rm -f "$FIFO"
	mkfifo "$FIFO"
	sync
	export LD_LIBRARY_PATH=/usr/local/Kobo
	/usr/local/Kobo/hindenburg &
	LIBC_FATAL_STDERR_=1 /usr/local/Kobo/nickel -platform kobo -skipFontLoad &
	udevadm trigger &
	# If Nickel doesn't come up, don't leave a dead screen: reboot.
	sleep 30
	pidof nickel >/dev/null || reboot
}

# The storage goes to the computer the way the firmware does it, until the cable comes out.
usb_session() {
	VERSION=$(cut -d, -f3 /mnt/onboard/.kobo/version)
	SN=$(cut -d, -f1 /mnt/onboard/.kobo/version)
	# shortcut: the Clara 2E's USB product id (what computers and Calibre see); per model
	# (next to MODELS in kobo/mod.rs) when another Kobo is ported.
	PRODUCT_ID=0x4235
	export VERSION SN PRODUCT_ID
	ACTION=add /usr/local/Kobo/udev/usb >>"$LOG" 2>&1
	# Each udev line is one write; read one line per open.
	while :; do
		read -r line <"$FIFO"
		[ "$line" = "usb plug remove" ] && break
	done
	ACTION=remove /usr/local/Kobo/udev/usb >>"$LOG" 2>&1
}

sync
killall -q -TERM nickel hindenburg sickel fickel strickel fontickel adobehost foxitpdf iink dhcpcd-dbus dhcpcd bluealsa bluetoothd fmon
n=0
while pidof nickel >/dev/null && [ $n -lt 16 ]; do
	usleep 250000
	n=$((n + 1))
done
# A fresh FIFO for the USB notices, read by the app (and by usb_session meanwhile).
rm -f "$FIFO"
mkfifo "$FIFO"

: >"$LOG"
first=1
while :; do
	rm -f "$UPDATE_FLAG"
	if ! cp "$APP/bin/$NAME" "$BIN" || ! chmod +x "$BIN"; then
		echo "no $APP/bin/$NAME" >>"$LOG"
		break
	fi
	# Start from a radio that is off: Nickel's, left up, has lost its dhcpcd.
	[ $first = 1 ] && "$BIN" --release-device >>"$LOG" 2>&1
	first=0
	"$BIN" "$@" >>"$LOG" 2>&1
	status=$?
	echo "exit status $status" >>"$LOG"
	if [ -e "$UPDATE_FLAG" ]; then
		continue
	elif [ $status -eq $USB_EXIT ]; then
		usb_session
		continue
	fi
	break
done
restore
