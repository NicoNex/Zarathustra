#!/bin/sh
# Prints the device's address on the local network (DHCP moves it): the last one found if it
# still answers, else the first host whose SSH port lets us in and has the device's storage
# folder. Cached in target/.<device>-host.
#   scripts/find-kindle.sh            a Kindle (/mnt/us)
#   DEVICE=kobo scripts/find-kindle.sh   a Kobo (/mnt/onboard)
DEVICE=${DEVICE:-kindle}
case $DEVICE in kobo) MARK=/mnt/onboard ;; *) MARK=/mnt/us ;; esac
PORT=${KINDLE_PORT:-22}
CACHE="$(dirname "$0")/../target/.$DEVICE-host"
is_kindle() {
	ssh -p "$PORT" -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=accept-new \
		"root@$1" "test -d $MARK" 2>/dev/null
}
last=$(cat "$CACHE" 2>/dev/null)
if [ -n "$last" ] && is_kindle "$last"; then
	echo "$last"
	exit 0
fi
# The /24 of the default route's interface.
net=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9]*\.[0-9]*\.[0-9]*\)\..*/\1/p')
[ -n "$net" ] || { echo "find-kindle: no network" >&2; exit 1; }
for h in $(for i in $(seq 1 254); do (timeout 1 bash -c "echo >/dev/tcp/$net.$i/$PORT" 2>/dev/null && echo "$net.$i") & done; wait); do
	if is_kindle "$h"; then
		mkdir -p "$(dirname "$CACHE")" && echo "$h" >"$CACHE"
		echo "$h"
		exit 0
	fi
done
echo "find-kindle: no $DEVICE answering on port $PORT (SSH plugin active?)" >&2
exit 1
