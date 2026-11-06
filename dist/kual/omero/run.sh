#!/bin/sh
# Runs an Omero binary with the Amazon UI stopped, and always brings the UI back.
BIN="/mnt/us/omero/bin/$1"
LOG="/mnt/us/omero/$1.log"
shift

# blanket (still running without the framework) draws Amazon's screen saver, ads included,
# when powerd goes to sleep. Omero draws its own, so unload those modules while it runs.
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
# powerd can't see our touches: Omero resets its idle timer itself (touchScreenSaverTimeout).
# Never preventScreenSaver: with it set, powerd ignores the power button too.
# powerd honours the power button only once com.lab126.kaf says frameworkStarted (Omero
# publishes that itself) and blanket shows no boot splash: if the framework was stopped while
# booting (a restart right after an exit), tell blanket the splash is gone.
lipc-send-event com.lab126.kaf bootSplashCleanup
"$BIN" "$@" >"$LOG" 2>&1
echo "exit status $?" >>"$LOG"
