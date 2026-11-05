#!/bin/sh
# `ld` for C build systems that call it directly: MuPDF's Makefile embeds its fonts with
# `$(LD) -r -b binary`, which must emit objects for the target being built. Cargo sets
# TARGET for build scripts and make inherits it.
case "$TARGET" in
armv7-unknown-linux-musleabihf) exec arm-linux-musleabihf-ld "$@" ;;
*) exec ld "$@" ;;
esac
