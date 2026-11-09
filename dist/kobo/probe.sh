#!/bin/sh
# Read-only inventory of a Kobo, for porting Zarathustra. Run from NickelMenu (cmd_spawn) with
# Nickel up and Wi-Fi on. Writes /mnt/onboard/.adds/zarathustra/probe/ and changes nothing
# else. The screen shows what to do; at the end it asks to reconnect USB.
# Halfway through it records the touch panel and buttons for 20 s while the user taps the
# top-left corner, then the bottom-right corner, then swipes left to right.
OUT=/mnt/onboard/.adds/zarathustra/probe
mkdir -p "$OUT"
rm -f "$OUT"/*
TXT="$OUT/probe.txt"
exec >"$TXT" 2>&1

FBINK=""
for f in /mnt/onboard/.adds/koreader/fbink /mnt/onboard/bin/fbink; do
	[ -x "$f" ] || chmod +x "$f" 2>/dev/null
	[ -x "$f" ] && FBINK="$f" && break
done
say() { [ -n "$FBINK" ] && "$FBINK" -q -c -m -M -S 3 "$@" >/dev/null 2>&1; }

sec() { echo; echo "===== $* ====="; }
run() { sec "$*"; sh -c "$*" 2>&1; }
show() { for f in "$@"; do [ -e "$f" ] && { echo "--- $f"; cat "$f" 2>&1; }; done; }

say "Sonda Zarathustra: non toccare lo schermo..."

sec "date"; date; uptime
run "uname -a"
run "cat /proc/version"
sec "version"; show /mnt/onboard/.kobo/version
run "cat /proc/cpuinfo | grep -v -i serial"
run "cat /proc/meminfo | head -5"
run "df -h"
run "cat /proc/mounts"
run "cat /proc/cmdline | sed 's/serial[^ ]*//g'"

sec "environment from rcS (udevd and nickel)"
for p in $(pidof -s udevd) $(pidof -s nickel); do
	echo "--- pid $p ($(cat /proc/$p/comm))"
	tr '\0' '\n' </proc/$p/environ | grep -E '^(PLATFORM|PRODUCT|MODEL_NUMBER|WIFI_MODULE|INTERFACE|LANG|TZ|QWS|LD_LIBRARY_PATH|PATH|NICKEL|KOBO)'
done
run "cat /bin/kobo_config.sh"
run "/bin/kobo_config.sh"
run "cat /etc/init.d/rcS"

sec "processes"; ps 2>&1 || ps w 2>&1
run "cat /proc/modules"

sec "framebuffer"
for f in /sys/class/graphics/fb0/*; do [ -f "$f" ] && echo "$(basename "$f"): $(cat "$f" 2>/dev/null | head -3 | tr '\n' ' ')"; done
[ -n "$FBINK" ] && run "$FBINK -e"
[ -n "$FBINK" ] && run "$FBINK -v -q -y 0 ' ' ; true"
cat /dev/fb0 >"$OUT/fb0.raw" 2>/dev/null
ls -la "$OUT/fb0.raw"

sec "input devices"; show /proc/bus/input/devices
for e in /sys/class/input/event*; do
	echo "$(basename "$e"): $(cat "$e/device/name" 2>/dev/null) props=$(cat "$e/device/properties" 2>/dev/null) abs=$(cat "$e/device/capabilities/abs" 2>/dev/null) key=$(cat "$e/device/capabilities/key" 2>/dev/null)"
done
run "ls -la /dev/input /dev/input/by-path 2>/dev/null"

sec "frontlight and leds"
for d in /sys/class/backlight/* /sys/class/leds/*; do
	echo "--- $d"
	for f in "$d"/*; do [ -f "$f" ] && echo "$(basename "$f"): $(head -c 200 "$f" 2>/dev/null | tr '\n' ' ')"; done
done

sec "power supplies"
for d in /sys/class/power_supply/*; do echo "--- $d"; cat "$d/uevent" 2>&1; done

sec "suspend and wakeup"
run "ls -la /sys/power"
show /sys/power/state /sys/power/state-extended /sys/power/mem_sleep /sys/power/wakeup_count /sys/power/autosleep
run "ls /sys/class/rtc; cat /sys/class/rtc/rtc0/name /sys/class/rtc/rtc0/wakealarm 2>&1"
run "ls -la /dev/ntx_io /dev/rtc* 2>&1"

sec "usb"
run "ls -la /tmp/nickel-hardware-status"
run "ls -R /sys/class/udc /sys/kernel/config 2>&1 | head -50"
for f in /sys/class/udc/*/state /sys/class/udc/*/current_speed; do [ -f "$f" ] && echo "$f: $(cat "$f")"; done
run "ls -la /drivers 2>&1"
run "ls -laR /drivers/\$(tr '\0' '\n' </proc/\$(pidof -s udevd)/environ | sed -n 's/^PLATFORM=//p') 2>&1"
run "ls -la /usr/local/Kobo /usr/local/Kobo/udev /etc/udev/rules.d /lib/udev/rules.d 2>&1"
# Kobo's own hotplug scripts: how the system exports storage and talks to Nickel.
mkdir -p "$OUT/scripts"
for f in $(grep -rlE 'nickel-hardware-status|g_file_storage|g_mass_storage|usb_f_mass_storage|mass_storage|state-extended' /etc /usr/local/Kobo /lib/udev /usr/bin /bin /sbin 2>/dev/null | grep -v -E '\.so|nickel$|libnickel' | head -40); do
	[ -f "$f" ] && [ "$(wc -c <"$f")" -lt 200000 ] && cp "$f" "$OUT/scripts/$(echo "$f" | tr '/' '_')"
done
cp /etc/udev/rules.d/* "$OUT/scripts/" 2>/dev/null
cp /etc/init.d/* "$OUT/scripts/" 2>/dev/null
ls -la "$OUT/scripts"

sec "wifi (Nickel's, should be on)"
run "ls -la /etc/wpa_supplicant"
sec "saved networks (count only)"; grep -c '^network=' /etc/wpa_supplicant/wpa_supplicant.conf 2>&1
grep -v -E 'ssid|psk|password|identity|wep_key' /etc/wpa_supplicant/wpa_supplicant.conf 2>&1 | grep -v '^\s*$' | sort -u
run "ls /sys/class/net"
for n in /sys/class/net/*; do echo "$(basename "$n"): driver=$(basename "$(readlink "$n/device/driver" 2>/dev/null)") operstate=$(cat "$n/operstate" 2>/dev/null)"; done
run "cat /proc/net/wireless"
run "iwconfig 2>&1 | grep -v -i -E 'essid|access point'"
IFACE=$(tr '\0' '\n' </proc/$(pidof -s udevd)/environ | sed -n 's/^INTERFACE=//p')
sec "wpa_cli status (filtered)"; wpa_cli -i "${IFACE:-eth0}" status 2>&1 | grep -E 'wpa_state|key_mgmt|pairwise|freq|mode'
run "wpa_cli -i ${IFACE:-eth0} scan_results | wc -l"
run "ls /var/run/wpa_supplicant /var/run 2>&1"
run "cat /etc/dhcpcd.conf 2>&1 | grep -v '^#' | grep -v '^$'"
run "ls -la /etc/udhcpc.d 2>&1"

sec "tools"
for t in wpa_supplicant wpa_cli dhcpcd udhcpc dosfsck fsck.fat fsck.vfat mkfifo timeout insmod rmmod modprobe ifconfig ip iw iwconfig iwpriv killall pkill pidof usleep reboot poweroff sync dropbear sshd telnetd tcpsvd hexdump od evtest busybox; do
	printf '%s: %s\n' "$t" "$(command -v $t 2>/dev/null)"
done
run "busybox 2>&1 | tail -30"

sec "time and fonts"
run "ls -la /etc/localtime; cat /etc/TZ 2>&1; date +%Z%z"
run "ls /usr/share/zoneinfo 2>&1 | head"
run "ls /usr/local/Trolltech/QtEmbedded-4.6.2-arm/lib/fonts 2>&1"
run "ls /mnt/onboard/fonts"

sec "nickel settings (non-personal keys)"
grep -E '^\[|FrontLight|Brightness|ColorSetting|Orientation|Language|Locale|Timezone|AutoUsbGadget|ForceWifi|Wifi|Sleep|Power' "/mnt/onboard/.kobo/Kobo/Kobo eReader.conf" 2>&1 | grep -v -i -E 'ssid|password|psk|key=|token|user|email|serial'

sec "storage layout"
run "ls -la /dev/mmcblk* 2>&1"
run "cat /proc/partitions"
run "ls -la /mnt /mnt/onboard /mnt/onboard/.adds 2>&1"

# Touch and buttons: raw input_event records (16 bytes each) from every event device.
say "Tocca: 1) angolo in alto a sinistra" "2) angolo in basso a destra" "3) scorri da sinistra a destra" "(hai 20 secondi)"
PIDS=""
for d in /dev/input/event*; do
	cat "$d" >"$OUT/$(basename "$d").bin" 2>/dev/null &
	PIDS="$PIDS $!"
done
sleep 20
kill $PIDS 2>/dev/null
sec "input capture"; ls -la "$OUT"/event*.bin

sec "end"; date
# No serial numbers or MACs in the report.
sed -i -E 's/([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}/XX:XX:XX:XX:XX:XX/g; s/N[0-9A-F]{12}/NXXXXXXXXXXXX/g' "$TXT"
sync
say "Sonda finita, grazie!" "Ora ricollega il cavo USB" "e tocca Connetti."
