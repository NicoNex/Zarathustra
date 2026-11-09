# Writing a Zarathustra plugin

A plugin adds things to Zarathustra without touching its code: a **service** that runs in the
background (the SSH server is one), **actions** that are buttons on the Kindle ("send my
highlights", "sync with my server"), **settings** the user changes on the Kindle, and **status**
lines. A plugin is a folder with a `plugin.toml` and executables. The executables can be in any
language the Kindle can run: shell scripts (`/bin/sh` is BusyBox) or static ARM binaries.

Plugins run as **root**, outside the app: a plugin that crashes never takes the reader down, but
installing one means trusting its code. Zarathustra never downloads plugins by itself.

## Install and find it

Copy the folder to the Kindle as `zarathustra/plugins/<id>/` (over USB, or `scp` once SSH is on).
The folder name is the plugin's **id**: lowercase letters, digits and `-` only. The plugin shows
up in **Settings → Extensions**. To remove it, delete the folder. To update it, copy the new
version over the old one: the user's settings are kept, because they live in Zarathustra's
database, not in the folder.

Before anything of a plugin runs, Zarathustra copies it to `/var/local/zarathustra/plugins/<id>/`
and marks every file executable. The userstore is FAT and has no exec bits, and a running
service must not keep it busy when the Kindle is connected to a computer.

| Folder | What | Environment variable | Placeholder |
|---|---|---|---|
| `/mnt/us/zarathustra/plugins/<id>` | what the user installed; files the user drops in over USB | `ZARATHUSTRA_USER_DIR` | `{user_dir}` |
| `/var/local/zarathustra/plugins/<id>` | the runtime copy, where executables run from (the working directory) | `ZARATHUSTRA_PLUGIN_DIR` | `{plugin_dir}` |
| `/var/local/zarathustra/data/<id>` | the plugin's own data: keys, caches, logs. Kept across updates | `ZARATHUSTRA_DATA_DIR` | `{data_dir}` |
| Zarathustra's SQLite database | books, highlights, bookmarks, reading days, positions | `ZARATHUSTRA_DATABASE` | `{database}` |

## The smallest plugin

`zarathustra/plugins/backup/plugin.toml`:

```toml
display_name = "Backup"
description = "Copies your highlights and bookmarks to the USB drive"
plugin_version = "1.0.0"
format_version = 1

[[action]]
menu_label = "Back up now"
executable = "backup"
```

`zarathustra/plugins/backup/backup`:

```sh
#!/bin/sh
set -e
dest="$ZARATHUSTRA_USER_DIR/backups"
mkdir -p "$dest"
cp "$ZARATHUSTRA_DATABASE" "$dest/zarathustra-$(date +%Y%m%d-%H%M).db"
echo "Saved in zarathustra/plugins/backup/backups"
```

Under Extensions, the plugin has a "Back up now" button. Tap it, and the line under it says
"Back up now: done in 1 s". If `cp` fails, the line shows an **Error** chip with the error
message. Tap the line to see everything the script printed.

## `plugin.toml`

### Top level (all required)

```toml
display_name = "SSH"                   # shown in Extensions and the plugin's page
description = "Remote shell and file transfer, keys only"
plugin_version = "1.0.0"               # yours, free form
format_version = 1                     # the manifest format; a newer one than the app knows is refused
```

### `[[action]]`: buttons

Any number of them, in the order they are shown.

```toml
[[action]]
menu_label = "Send highlights"        # the button
executable = "send"                   # relative to the plugin folder; never run through a shell
arguments = ["--to", "{setting.server}"]   # optional; placeholders are filled in (see below)
id = "send"                           # optional: the id used for translations (default: the executable's name)
output = "show"                       # optional: "quiet" (default) or "show"
shown_in = ["extensions", "book"]     # optional: default ["extensions"]
timeout_seconds = 120                 # optional: no limit when absent
```

How an action runs:

- **In the background.** The tap returns at once. The button reads "Running…" and a **Stop**
  button appears beside the line under it; the line counts the time. Leaving the screen does not
  stop the action. Even Zarathustra restarting does not: the action runs detached in its own
  process group, and Zarathustra finds it again when it comes back.
- **One at a time per plugin.** While one action runs, the plugin's other buttons wait (drawn
  grey). Actions of different plugins can run together.
- **Stop** sends SIGTERM to the action's whole process group, then SIGKILL 3 seconds later to
  whatever is left. `timeout_seconds` does the same when the time is up.
- **The result** is the exit code. 0 is success: with `output = "quiet"` the line says "done in
  N s"; with `output = "show"` it shows the first non-empty line printed on stdout (so print a
  short result first, details after). Any other exit code is a failure: the line shows an
  **Error** chip and the last non-empty line printed on **stderr**, or the exit code if stderr is
  empty. So print the reason for a failure last, on stderr.
- **The output sheet.** Tapping the line opens all of stdout and stderr, the exit code, when the
  run started and how long it took. Use it to debug on the Kindle without a computer. Only the
  last run of each plugin is kept, in `data/<id>/action.out` and `action.err`.
- **In the book menu.** `shown_in = ["book"]` also puts the action in the menu that opens when
  you hold a book on the home screen. The action then gets that book: `{book}` and
  `ZARATHUSTRA_BOOK` are its path, and `ZARATHUSTRA_POSITION` is its saved reading position
  (empty if it was never opened). After the tap, Extensions opens so you can watch the run.

### `[[setting]]`: what the user can change

Shown on the plugin's page (tap its name under Extensions), in order. The values are kept in
Zarathustra's database. Every executable gets them as `ZARATHUSTRA_SETTING_<ID>` (the id in
capitals, `-` and `.` as `_`) and as `{setting.<id>}`. Changing a setting restarts the plugin's
service if it is running, since its arguments may have changed.

| `kind` | Control | Fields |
|---|---|---|
| `toggle` | Off / On | `default_value` (true/false); the value is `true` or `false` |
| `choice` | one of the options | `options` (strings), `default_value` |
| `number` | − value +; a tap on the value types it, kept within the range | `minimum`, `maximum`, `step` (default 1), `default_value` |
| `text` | a button with the value; a tap types it on the on-screen keyboard | `default_value` |
| `list` | items, each with Remove | `list_executable`, `empty_label` (what to show when the list is empty: tell the user how to add an item) |

```toml
[[setting]]
id = "server"
kind = "choice"
label = "Server"
options = ["home", "office"]
default_value = "home"
```

A `list` is managed by its executable, so it can be anything (SSH uses it for the authorized
keys):

- `list_executable list` prints one item per line: `id<TAB>text`.
- `list_executable remove <id>` removes an item.

Both runs must finish within 2 seconds.

### `[service]`: something that keeps running

At most one per plugin. The plugin's card under Extensions gets an **Off / Active** switch.

```toml
[service]
executable = "run"
arguments = ["-p", "{setting.port}"]
start_at_boot = true        # offered as "start at power-on" (see below)
start_in_recovery = true    # also started when Zarathustra itself is broken
```

- **Active** starts it now and every time Zarathustra starts. **Off** stops it: SIGTERM, then
  SIGKILL after 3 s to anything of its group that is left.
- It runs detached in its own process group, so it outlives the app. Its output goes to
  `data/<id>/service.log` and its pid to `data/<id>/service.pid`.
- It must stay in the foreground (do not daemonize: `dropbear -F`, for example). It is
  "running" while the pid in the file is alive and is running your executable.
- Start at power-on, before the UI, comes with the boot hook of the de-Amazon work. Until then
  the app starts active services when it opens.

### `[status]`: lines at the top of the plugin's page

```toml
[status]
executable = "status"
```

The executable prints `Label=value` lines, which are shown in order. It runs every time the page
opens and after each change on it, and must finish within 2 seconds. SSH uses it for the
address to connect to, the host key fingerprint, the number of keys, and the last thing the
server logged.

## What every executable gets

**Environment variables**

| Variable | Value |
|---|---|
| `ZARATHUSTRA_PLUGIN_DIR`, `ZARATHUSTRA_DATA_DIR`, `ZARATHUSTRA_USER_DIR`, `ZARATHUSTRA_DATABASE` | see the folder table above |
| `ZARATHUSTRA_SETTING_<ID>` | each setting's current value |
| `ZARATHUSTRA_LANGUAGE` | the app's language, `it` or `en`, for the messages you print |
| `ZARATHUSTRA_BOOK`, `ZARATHUSTRA_POSITION` | actions run from the book menu: the book and its position; empty otherwise |

**Placeholders in `arguments`**: `{setting.<id>}`, `{plugin_dir}`, `{data_dir}`, `{user_dir}`,
`{database}`, `{book}`. A placeholder Zarathustra does not know is left as written. Each argument
is passed as is, with no shell in between, so spaces and quotes are safe.

Stdin is empty. The working directory is the runtime copy.

## Translations

The strings in `plugin.toml` are the source text, in English, and are also the fallback. To
translate them, add [Fluent](https://projectfluent.org) files: `locales/it.ftl`,
`locales/en.ftl`. A missing file or message falls back to the manifest. A broken line is
skipped, and the rest of the file is still used.

| Message id | Translates |
|---|---|
| `display-name`, `description` | the plugin's name and description |
| `action-<id>` | an action's `menu_label`; `<id>` is its `id`, else its executable's name |
| `setting-<id>` | a setting's `label` |
| `setting-<id>-empty` | a list setting's `empty_label` |
| `setting-<id>-option-<value>` | how a choice option is shown (the stored value does not change) |

```ftl
# locales/it.ftl
description = Copia evidenziazioni e segnalibri sul disco USB
action-backup = Esegui il backup
```

Status lines and messages your executables print are yours to translate: read
`ZARATHUSTRA_LANGUAGE`.

## Tips

- Write the reason for a failure as the last line on stderr, and exit with a non-zero code.
  That line is what the user sees.
- For an action with `output = "show"`, print the short result first.
- Keep long-lived state in `ZARATHUSTRA_DATA_DIR`. The user folder is FAT: no permissions,
  no symlinks, no exec bits.
- Read the database, do not write to it. The schema belongs to Zarathustra and changes with its
  migrations (`crates/zarathustra-store/src/v*.sql`).
- `dist/plugins/ssh/` is a complete example: a service, a status, a list setting, an action and
  translations.

The design behind all this: `docs/superpowers/specs/2026-11-08-plugins-design.md` and
`docs/superpowers/specs/2026-11-09-extensions-stats-design.md`.
