# Kindle Wi-Fi (unverified on device)

Written without the device at hand: `crates/zarathustra-device/src/kindle/wifi.rs` drives the
firmware's own daemons through the `lipc-*` CLI tools. With `lab126_gui` stopped, `cmd`
(`com.lab126.cmd`) and `wifid` (`com.lab126.wifid`) should still run: SSH over Wi-Fi keeps
working while Zarathustra runs. The calls mirror what KOReader's Kindle network manager does
(learned from, not copied).

## Commands used

| Operation | Command |
|---|---|
| Radio on? | `lipc-get-prop -i com.lab126.cmd wirelessEnable` → `1`/`0` |
| Radio on/off | `lipc-set-prop -i com.lab126.cmd wirelessEnable 1` (or `0`) |
| Connected? | `lipc-get-prop -s com.lab126.wifid cmState` → `CONNECTED` |
| Network joined | `lipc-get-prop -s com.lab126.wifid currentEssid` |
| Scan | `lipc-wait-event -s 10 com.lab126.wifid scanComplete,scanFailed` started first, then `lipc-set-prop -s com.lab126.wifid scan ""`; results read whether or not the event came |
| Scan results | `lipc-hash-prop -n com.lab126.wifid scanList` → records with `essid`, `signal`, `signal_max`, `key_mgmt` (`none` = open) |
| Saved networks | `lipc-hash-prop -n com.lab126.wifid profileData` → records with `essid` |
| Save a new network | `echo '{ essid = "X", secured = "yes", psk = "P", store_nw_user_pref = "0" }' \| lipc-hash-prop com.lab126.wifid createProfile` |
| Join | `lipc-set-prop -s com.lab126.cmd ensureConnection wifi:X`, then poll `cmState`/`currentEssid` every 0.5 s for up to 30 s |
| Forget | `echo '{ essid = "X" }' \| lipc-hash-prop com.lab126.wifid deleteProfile` |

## Check on the device

1. `lipc-probe com.lab126.wifid` and `lipc-probe com.lab126.cmd` answer with the framework
   stopped. If wifid is missing: `start wifid` (upstart) and note it for `run.sh`.
2. `lipc-hash-prop` exists and its flags: `-n` (no input) for reads, stdin for writes. Run the
   two reads above and paste their output into this note. The parser (`parse_hash_array`) is
   lenient (`{ k = v }` or `"k": v`, quoted or bare values), but the stdin format of
   `createProfile` is a guess: if it is wrong, joining a new network fails with the tool's
   stderr on the Wi-Fi screen.
3. `signal_max` and `key_mgmt` really are the field names (otherwise every network shows full
   bars or open).
4. `lipc-wait-event -s` is the timeout flag; a scan should come back in a few seconds.
5. Wrong password: does wifid report it quickly (a `cmState` value or an event), or do we wait
   the full 30 s? If there is an event, listen for it instead of the timeout.
6. Toggling the radio from the quick tile, then reopening the panel, shows the new state.

## Fallback if wifid's hash properties don't work

`wpa_cli -i wlan0` (`scan`, `scan_results`, `list_networks`, `add_network`, `set_network`,
`select_network`, `save_config`, `remove_network`, `status`) has a documented output format,
but wifid owns wpa_supplicant and may undo our changes: use it only for scanning, keep joins
on wifid (`ensureConnection` for saved profiles).

## Verified on the PW5 (2026-11-06, framework running, via the KUAL "Wi-Fi probe")

- wifid and cmd answer; `cmd wirelessEnable` (Int), `wifid cmState` (Str, `CONNECTED`) as assumed.
- `currentEssid` is a **hash** (`netid`, `essid`, `bssid`, `connected`, `signal`, `signal_max`, `secured`), read with `lipc-hash-prop -n`.
- `lipc-hash-prop -n` prints `Hash Index: N` blocks of `\tkey = value` lines; strings quoted (can span lines, e.g. `cmResolvConf`), integers in parentheses.
- scanList fields: `bssid essid secured known netid signal signal_max supported wps enterprise key_mgmt` (`key_mgmt` like `WPA2 PSK`, `SAE`); hidden networks have `essid = ""`.
- profileData fields: `netid essid secured smethod key_mgmt store_nw_user_pref psk …` (the passwords are readable: never log this property).
- `deleteProfile` is a write-only **Int** (the profile's netid). `createProfile` is a hash; its stdin syntax is still unverified.
- Works with Zarathustra running (framework stopped), confirmed by the user 2026-11-06. Creating a new profile from Zarathustra still unverified.
