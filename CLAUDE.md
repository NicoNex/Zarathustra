# Zarathustra

Start here: `docs/superpowers/HANDOFF.md` (current state, open bugs, next steps).

Rust e-reader shell replacing the Amazon UI on a jailbroken Kindle Paperwhite 5 (portable to other e-ink devices). Design: `docs/superpowers/specs/2026-11-04-zarathustra-reader-design.md`. Plans: `docs/superpowers/plans/`.

## How we work (always)

- **Language**: talk to the user in Italian. Code, comments and commits in English.
- **Process**: brainstorm → spec → plan → implement for each sub-project. Small, verifiable steps.
- **Rust**: load the `fullstack-dev-skills:rust-engineer` skill before writing Rust. Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features` with zero warnings, `cargo test --workspace`. Document every `unsafe`. No `unwrap()` outside tests.
- **Commits**: checkpoint commits as work progresses (Conventional Commits, body explains why). At milestones (a usable feature, not every commit), run the review as **5 parallel agents** (always, user's rule): one `/code-review` for correctness plus one per `/simplify` angle (reuse, simplification, efficiency, altitude), on the diff since the last review. Apply the real findings, note the skipped ones, and commit the fixes. Speed toward something usable on the device matters more than per-commit polish.
- **Agents**: reviews (code-review fork + simplify reviewers) and, when it speeds things up, independent feature work in parallel (one agent per self-contained module, `isolation: worktree`, merged back by the main session).
- **Plato** (`plato/`, gitignored): a reference to learn from and surpass. Never copy its code.
- **Libraries**: state of the art. Prefer pure Rust; C only when it is clearly best (SQLite, MuPDF). Check licenses are GPL-3.0 compatible.
- **Lazy, not careless**: no speculative abstractions. Mark deliberate shortcuts with `// ponytail:` plus their ceiling and upgrade path.
- **Verify on the device**, not just in tests: deploy, run, screenshot, measure latency.

## Build, deploy, test

- Device build: `make build` (Kindle, Cortex-A53) / `make build-kobo` (Kobo, Cortex-A9, in `target/kobo`): each device gets its own CPU flags for Rust and the C deps (static musl, linked with `rust-lld`). C deps use the AUR `arm-linux-musleabihf-cross-bin` toolchain via `.cargo/config.toml`.
- Emulator: `cargo run -p zarathustra --features emulator --bin zarathustra-probe`.
- Deploy over SSH (Zarathustra's SSH plugin, port 22, key auth; the address is found on the network by `scripts/find-kindle.sh`): `make update` (app only, the running app restarts into it in seconds), `make install-ssh` (with dictionaries), `make run-ssh BIN=zarathustra-probe` runs another binary. Over USB: `make install-usb` (the app restarts into the new binary when the cable comes out).
- Screenshot of the device: `KINDLE_HOST=… scripts/screenshot.sh out.png`.
- Local-only test assets (gitignored): `dev-fonts/` (Bookerly, Baskerville from the device), `dev-books/` (user's EPUBs). Tests skip silently when they are missing.

## Layout

- `crates/zarathustra-device`: `Device` trait; Kindle MTK backend (`kindle/`), emulator, touch decoding, sysfs light/battery.
- `crates/zarathustra-engine`: fonts (harfrust + skrifa + zeno), DOM, EPUB container; style, layout and pagination are next.
- `crates/zarathustra-store`: SQLite (rusqlite, bundled): books, annotations, `events` change feed; schema migrations in `src/v*.sql`. Positions (CFI, text-quote re-anchoring) live in `zarathustra-engine::anchor`.
- `crates/zarathustra`: binaries (probes now, the app later).
- `dist/kual/zarathustra`: KUAL extension; `run.sh` stops/restores the Amazon UI.
