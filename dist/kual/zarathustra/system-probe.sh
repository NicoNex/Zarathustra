#!/bin/sh
# Read-only inventory for de-Amazoning: boot jobs, services, modules, mounts, the libraries
# powerd/wifid/cmd need, OTA and USB pieces. Writes /mnt/us/zarathustra/system-probe.txt.
# No secrets: wifid's properties (profiles hold passwords) are never dumped.
OUT=/mnt/us/zarathustra/system-probe.txt
s() { echo; echo "== $*"; sh -c "$*" 2>&1; }
{
	date
	s 'cat /etc/prettyversion.txt /etc/version.txt; uname -a'
	s 'cat /proc/cmdline; cat /proc/cpuinfo | head -20; free; df -h'
	s 'mount'
	s 'initctl list | sort'
	s 'ls -la /etc/upstart'
	for j in framework lab126_gui powerd wifid cmd volumd usbnet wmt otaupd otav3 dbus lipc-daemon; do
		[ -f /etc/upstart/$j.conf ] && s "cat /etc/upstart/$j.conf"
	done
	s 'grep -l -i -E "wmt|wlan|wifi" /etc/upstart/*.conf /etc/init.d/* 2>/dev/null'
	s 'lsmod'
	s 'ls /lib/firmware /lib/modules/*/ 2>/dev/null | head -80'
	s 'ps -o pid,ppid,user,args 2>/dev/null || ps'
	for b in powerd wifid cmd volumd; do
		p=$(which $b 2>/dev/null || ls /usr/bin/$b /usr/sbin/$b /usr/local/bin/$b 2>/dev/null | head -1)
		[ -n "$p" ] && s "ls -la $p; /lib/ld-linux-armhf.so.3 --list $p"
	done
	s 'ls -la /usr/bin/*ota* /usr/sbin/*ota* /usr/bin/*update* /usr/sbin/*update* 2>/dev/null'
	s 'ls -la /mnt/us | grep -i -E "update|ota|partial|hotfix|jb"'
	s 'ls -la /var/local/system 2>/dev/null | head -40'
	s 'lipc-probe -a 2>/dev/null | sort'
	s 'lipc-probe -v com.lab126.powerd'
	s 'lipc-probe -v com.lab126.volumd'
	s 'ls /sys/class/power_supply/*/; cat /sys/class/power_supply/*/uevent'
	s 'ls /sys/class/udc /sys/kernel/config/usb_gadget 2>/dev/null; cat /sys/class/udc/*/state 2>/dev/null'
	s 'which ntpd ntpdate sntp chronyd hwclock busybox; busybox --list 2>/dev/null | tr "\n" " "'
} > "$OUT" 2>&1
