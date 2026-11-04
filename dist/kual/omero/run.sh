#!/bin/sh
# Runs an Omero binary with the Amazon UI stopped, and always brings the UI back.
BIN="/mnt/us/omero/bin/$1"
LOG="/mnt/us/omero/$1.log"

restore() {
	lipc-set-prop com.lab126.powerd preventScreenSaver 0
	cd / && start lab126_gui
}

# The framework job SIGTERMs its children on stop; don't die with it when launched from KUAL.
trap "" TERM
stop lab126_gui
usleep 1250000
trap - TERM
trap restore EXIT
trap "exit 1" INT TERM

lipc-set-prop com.lab126.powerd preventScreenSaver 1
"$BIN" >"$LOG" 2>&1
echo "exit status $?" >>"$LOG"
