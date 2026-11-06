#!/bin/sh
# Dumps what wifid and cmd expose (names, types, current values) to
# /mnt/us/zarathustra/wifi-probe.txt, to match the Wi-Fi backend to this firmware.
OUT=/mnt/us/zarathustra/wifi-probe.txt
{
	date
	for s in com.lab126.wifid com.lab126.cmd; do
		echo "== lipc-probe -v $s"
		lipc-probe -v "$s" 2>&1
	done
	for p in cmState cmIntfInfo signalStrength profileCount; do
		echo "== wifid $p (string)"; lipc-get-prop -s com.lab126.wifid "$p" 2>&1
		echo "== wifid $p (int)"; lipc-get-prop -i com.lab126.wifid "$p" 2>&1
	done
	echo "== wifid cmIntfInfo (hash)"; lipc-hash-prop -n com.lab126.wifid cmIntfInfo 2>&1
	echo "== wifid currentEssid (hash)"; lipc-hash-prop -n com.lab126.wifid currentEssid 2>&1
	echo "== wifid profileData (hash)"; lipc-hash-prop -n com.lab126.wifid profileData 2>&1
	echo "== wifid scanList (hash)"; lipc-hash-prop -n com.lab126.wifid scanList 2>&1
	echo "== cmd wirelessEnable"; lipc-get-prop -i com.lab126.cmd wirelessEnable 2>&1
	echo "== wpa_cli status"; wpa_cli status 2>&1
	echo "== processes"; ps | grep -i -E "wifid|wpa|cmd|lab126_gui" | grep -v grep
	echo "== which"; which lipc-hash-prop lipc-wait-event wpa_cli 2>&1
} > "$OUT" 2>&1
