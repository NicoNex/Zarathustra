#!/bin/sh
# Runs a Zarathustra binary with the Amazon UI stopped, and always brings the UI back.
# Script and binary run from /tmp, the log is written there: nothing stays open on the
# userstore, so it can be handed to a computer over USB.
if [ "$0" != /tmp/zarathustra-run.sh ]; then
	cp "$0" /tmp/zarathustra-run.sh && exec /tmp/zarathustra-run.sh "$@"
fi
cd /
NAME="$1"
BIN="/tmp/$NAME"
LOG="/tmp/$NAME.log"
KEEP_LOG="/mnt/us/zarathustra/$NAME.log"
shift
# The exit status meaning "a computer wants the storage" (USB_EXIT in app.rs).
USB_EXIT=42
UDC_STATE=$(ls /sys/class/udc/*/state 2>/dev/null | head -1)

# blanket (still running without the framework) draws Amazon's screen saver, ads included,
# when powerd goes to sleep. Zarathustra draws its own, so unload those modules while it runs.
SAVERS=""
for m in ad_screensaver screensaver; do
	case " $(lipc-get-prop com.lab126.blanket load 2>/dev/null) " in
	*" $m "*) SAVERS="$SAVERS $m" ;;
	esac
done

restore() {
	# Reload what we unloaded; if an earlier run died before restoring, the live detection
	# above finds nothing, so fall back to the module this device ships.
	for m in ${SAVERS:-ad_screensaver}; do lipc-set-prop com.lab126.blanket load "$m"; done
	lipc-set-prop com.lab126.powerd preventScreenSaver 0
	cp "$LOG" "$KEEP_LOG" 2>/dev/null
	cd / && start lab126_gui
}

# The framework job SIGTERMs its children on stop; don't die with it when launched from KUAL.
trap "" TERM
stop lab126_gui
usleep 1250000
trap - TERM
trap restore EXIT
trap "exit 1" INT TERM HUP

for m in $SAVERS; do lipc-set-prop com.lab126.blanket unload "$m"; done
# powerd can't see our touches: Zarathustra resets its idle timer itself (touchScreenSaverTimeout).
# Never preventScreenSaver: with it set, powerd ignores the power button too.
# powerd honours the power button only once com.lab126.kaf says frameworkStarted (Zarathustra
# publishes that itself) and blanket shows no boot splash: if the framework was stopped while
# booting (a restart right after an exit), tell blanket the splash is gone.
lipc-send-event com.lab126.kaf bootSplashCleanup
while :; do
	cp "/mnt/us/zarathustra/bin/$NAME" "$BIN" || exit 1
	"$BIN" "$@" >>"$LOG" 2>&1
	status=$?
	echo "exit status $status" >>"$LOG"
	[ "$status" = "$USB_EXIT" ] && [ -n "$UDC_STATE" ] || break
	# USB: the Amazon UI hands the storage to the computer; once the cable is out (and the
	# storage is back), stop it again and start over.
	for m in ${SAVERS:-ad_screensaver}; do lipc-set-prop com.lab126.blanket load "$m"; done
	start lab126_gui
	while [ "$(cat "$UDC_STATE")" = configured ]; do sleep 2; done
	while [ ! -e "/mnt/us/zarathustra/bin/$NAME" ]; do sleep 1; done
	sleep 2
	trap "" TERM
	stop lab126_gui
	usleep 1250000
	trap "exit 1" INT TERM HUP
	for m in $SAVERS; do lipc-set-prop com.lab126.blanket unload "$m"; done
	lipc-send-event com.lab126.kaf bootSplashCleanup
done
