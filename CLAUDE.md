# Omero

Rust e-reader shell replacing the Amazon UI on a jailbroken Kindle Paperwhite 5 (portable to other e-ink devices). Design: `docs/superpowers/specs/2026-11-04-omero-reader-design.md`. Plans: `docs/superpowers/plans/`.

## How we work (always)

- **Language**: talk to the user in Italian. Code, comments and commits in English.
- **Process**: brainstorm → spec → plan → implement for each sub-project. Small, verifiable steps.
- **Rust**: load the `fullstack-dev-skills:rust-engineer` skill before writing Rust. Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features` with zero warnings, `cargo test --workspace`. Document every `unsafe`. No `unwrap()` outside tests.
- **Commits**: checkpoint commits as work progresses (Conventional Commits, body explains why). At milestones (a usable feature, not every commit), run `/code-review` and the `/simplify` angles (reuse, simplification, efficiency, altitude) on the diff since the last review. Apply the real findings, note the skipped ones, and commit the fixes. Speed toward something usable on the device matters more than per-commit polish.
- **Agents**: reviews (code-review fork + simplify reviewers) and, when it speeds things up, independent feature work in parallel (one agent per self-contained module, `isolation: worktree`, merged back by the main session).
- **Plato** (`plato/`, gitignored): a reference to learn from and surpass. Never copy its code.
- **Libraries**: state of the art. Prefer pure Rust; C only when it is clearly best (SQLite, MuPDF). Check licenses are GPL-3.0 compatible.
- **Lazy, not careless**: no speculative abstractions. Mark deliberate shortcuts with `// ponytail:` plus their ceiling and upgrade path.
- **Verify on the device**, not just in tests: deploy, run, screenshot, measure latency.

## Build, deploy, test

- Device build: `cargo build --release --target armv7-unknown-linux-musleabihf` (static musl, linked with `rust-lld`). C deps use the AUR `arm-linux-musleabihf-cross-bin` toolchain via `.cargo/config.toml`.
- Emulator: `cargo run -p omero --features emulator --bin omero-probe`.
- Deploy over SSH (KOReader's SSH server, port 2222, key auth): `KINDLE_HOST=192.168.1.116 scripts/deploy.sh [binary-to-run]`. Over USB: same script without `KINDLE_HOST`.
- Screenshot of the device: `KINDLE_HOST=… scripts/screenshot.sh out.png`.
- Local-only test assets (gitignored): `dev-fonts/` (Bookerly, Baskerville from the device), `dev-books/` (user's EPUBs). Tests skip silently when they are missing.

## Layout

- `crates/omero-device`: `Device` trait; Kindle MTK backend (`kindle/`), emulator, touch decoding, sysfs light/battery.
- `crates/omero-engine`: fonts (harfrust + skrifa + zeno), DOM, EPUB container; style, layout and pagination are next.
- `crates/omero`: binaries (probes now, the app later).
- `dist/kual/omero`: KUAL extension; `run.sh` stops/restores the Amazon UI.
