# Plugins — design (2026-11-08)

A plugin adds a background service, a settings page and menu actions to Zarathustra without
touching its code. Plugins are folders of executables in any language, described by a
manifest; Zarathustra runs them as separate processes (a crashing plugin never takes the
reader down, and Rust has no stable ABI for in-process plugins). SSH is the first plugin and
ships with Zarathustra.

## Layout

```
/mnt/us/zarathustra/plugins/<id>/     what the user installs (USB, Wi-Fi transfer)
  plugin.toml
  executables, data files…
/var/local/zarathustra/plugins/<id>/  runtime copy: rootfs, so files are executable (vfat has no
                                      exec bits) and a running service never keeps the
                                      userstore busy when it goes to a computer over USB
/var/local/zarathustra/data/<id>/     the plugin's own data (keys, logs), on a real filesystem
                                      with owners and permissions
```

- The plugin's id is its folder name (`[a-z0-9-]+`; anything else is skipped with a log line).
- Adding a plugin = copying its folder; removing = deleting it. No central file.
- Before running anything of a plugin, Zarathustra refreshes the runtime copy (files whose size
  or modification time changed) and marks every file executable.
- On the desktop the plugins come from `dist/plugins/` and run in place; data goes under the
  state dir.

## Manifest: `plugin.toml`

Written by the plugin's author; describes what the plugin offers. The user's values are never
written here (updating a plugin by overwriting its folder keeps them).

```toml
display_name = "SSH"
description = "Remote access with keys"
plugin_version = "1.0.0"
format_version = 1

[service]
executable = "run"
arguments = ["--port", "{setting.port}"]
start_at_boot = true
start_in_recovery = true

[status]
executable = "status"

[[action]]
menu_label = "Show host key"
executable = "show-host-key"
shown_in = "plugin"

[[setting]]
id = "port"
kind = "number"
label = "Port"
default_value = 22
minimum = 1
maximum = 65535
step = 1

[[setting]]
id = "authorized_keys"
kind = "list"
label = "Authorized keys"
list_executable = "keys"
```

Top level (required): `display_name`, `description`, `plugin_version`, `format_version`.
A `format_version` newer than this build understands: the plugin is listed as needing a newer
Zarathustra and nothing of it runs.

`[service]` (optional, at most one): a long-running process.
- `executable`, `arguments` (see below).
- `start_at_boot`: the plugin may start before the UI, independently of it (the user still
  chooses, see Service control).
- `start_in_recovery`: also started by the recovery boot, whatever the user's choice (SSH: the
  way back in when Zarathustra itself is broken).

`[status]` (optional): `executable` + `arguments`, printing `Label=value` lines shown at the top
of the plugin's page; run each time the page opens and after every change on it. 2 s timeout.

`[[action]]` (any number): `menu_label`, `executable`, `arguments`, `shown_in`
(`plugin` | `home` | `book` | `selection`). The first `stdout` line of the run is shown as a
toast. Phase 1 shows every action as a button on the plugin's page; the home, book-menu and
selection placements come next.

`[[setting]]` (any number, in page order): `id`, `kind`, `label`, plus by kind:

| `kind` | Control | Fields |
|---|---|---|
| `toggle` | on/off | `default_value` (bool) |
| `choice` | one of the options | `options` (strings), `default_value` |
| `number` | − value + | `minimum`, `maximum`, `step`, `default_value` |
| `text` | a text field | `default_value` (phase 1: shown, not editable) |
| `list` | items, each with remove | `list_executable` |

A `list` executable is called as `list_executable list` (prints `id<TAB>text` per item) and
`list_executable remove <id>`. 2 s timeout.

### Running executables

- `executable` is a path relative to the plugin folder; never through a shell.
- `arguments` is a list of strings passed as they are. Placeholders, replaced before the run:
  `{setting.<id>}`, `{plugin_dir}`, `{data_dir}`, `{user_dir}`, `{database}`, and for actions
  `{book}`, `{selection}` (empty when there is none).
- Environment, for every executable: `ZARATHUSTRA_PLUGIN_DIR` (runtime copy),
  `ZARATHUSTRA_DATA_DIR`, `ZARATHUSTRA_USER_DIR` (the folder the user installed, to pick up files
  dropped there), `ZARATHUSTRA_DATABASE`, `ZARATHUSTRA_SETTING_<ID>` (id upper-cased, `-` → `_`),
  and for actions `ZARATHUSTRA_BOOK`, `ZARATHUSTRA_POSITION`, `ZARATHUSTRA_SELECTION`.
- Values are stored in the database (`prefs`, group `plugin.<id>`, `key=value` lines), like
  every other preference.

## Service control

The plugin's page has, for a plugin with a service, two built-in rows the author doesn't
declare:
- **Active** (on/off): on starts the service now and whenever Zarathustra starts; off stops it.
- **Start at power-on** (only with `start_at_boot`): started before the UI by the boot.

Changing any setting of a running service restarts it (its arguments may have changed).

The service is not Zarathustra's child: it is started in its own process group with its output
in `data/<id>/service.log` and its pid in `data/<id>/service.pid`, so it survives the app
exiting or crashing. "Running" = the pid file names a live process whose command line is the
service's executable. Stop = SIGTERM, then SIGKILL after 3 s.

Boot: `zarathustra --start-services` starts every service that has `start_at_boot` and the
user's "start at power-on", then exits; `--recovery` (safety file present, or Zarathustra
crashed at the last start) also starts every `start_in_recovery` service. If the binary itself
cannot run, the boot script falls back to running the recovery services' executables directly,
with no settings (they must work on their defaults). The boot script itself comes with the
de-Amazon phase A; until then the app starts the active services when it opens.

## Settings screen

The settings screen lists the installed plugins under its rows; a tap opens the plugin's page:
the status lines, the service rows, the declared settings, then the action buttons. Back
returns to the settings.

## Trust

Plugins run as root, like everything on the Kindle: installing one means trusting its code.
Zarathustra never downloads plugins by itself.

## SSH plugin (`dist/plugins/ssh/`)

- `dropbearmulti` (Dropbear 2026.94: dropbear, dropbearkey, scp) and `sftp-server` (OpenSSH
  10.6p2, without OpenSSL), built static by `scripts/build-ssh.sh`, copied in by `make stage`.
- `run`: creates the ed25519 host key in `data/ssh/` if missing, merges keys from
  `{user_dir}/authorized_keys` (dropped over USB) into `data/ssh/authorized_keys` (mode 600: the
  userstore can't hold permissions, and Dropbear refuses keys files writable by others), opens
  the port in iptables, runs `dropbear -F -E -s -D data/ssh -r <host key> -p <port>`, and closes
  the port when it ends.
- Public-key login only (passwords and PAM compiled out), ed25519 only, modern ciphers and key
  exchanges (including post-quantum `mlkem768x25519-sha256`).
- `status`: address (Wi-Fi IP:port), host key fingerprint, number of authorized keys.
- `keys`: lists and removes authorized keys (`id` = line number, text = comment + fingerprint).
- Setting: port (default 22). Service: `start_at_boot`, `start_in_recovery`.
- Default off: an SSH server listening on the network is the user's choice.

## Not now

Plugin store or downloads, sandboxing, plugins drawing their own UI, home/book/selection
placement of actions (next), editable text settings (next, with the keyboard).
