# Zarathustra on a Kindle.
#   make install-ssh                  build, install the app and the dictionaries over SSH
#   make install-ssh KINDLE_HOST=…    a given address (else found on the network)
#   make update                       app only; the running app restarts into it (fast)
#   make install-usb                  the same onto a USB-mounted Kindle
#   make run-ssh BIN=zarathustra-probe   install over SSH, then run a binary and show its log
#   make dicts                        download the dictionaries into dev-dicts/
# SSH through Zarathustra's own SSH plugin (port 22). The address is found on the network when
# first needed (DHCP moves it; scripts/find-kindle.sh caches it), unless given.
KINDLE_PORT ?= 22
KINDLE_HOST ?= $(eval KINDLE_HOST := $$(or $$(shell KINDLE_PORT=$(KINDLE_PORT) scripts/find-kindle.sh),$$(error no Kindle found on the network: wake it, or set KINDLE_HOST=...)))$(KINDLE_HOST)
KINDLE ?= /run/media/$(USER)/Kindle

TARGET := armv7-unknown-linux-musleabihf
BIN_DIR := target/$(TARGET)/release
BINS := zarathustra zarathustra-probe zarathustra-textprobe zarathustra-powerprobe
STAGE := target/kindle-stage
# Recursive (=): KINDLE_HOST is looked up only by the targets that use SSH.
SSH = ssh -p $(KINDLE_PORT) -o BatchMode=yes root@$(KINDLE_HOST)
# VFAT keeps no owners, permissions or mtimes.
UNTAR_USB := tar -xf - --no-same-owner --no-same-permissions -m

.PHONY: install install-ssh install-usb update run-ssh build stage dicts
install: install-ssh

# Each device built for its own core, Rust and C dependencies (SQLite, MuPDF) alike; the musl
# toolchain's gcc would otherwise default to armv5te.
#   Kindle PW5: MediaTek MT8113, Cortex-A53 (AArch32): hardware divide, VFPv4/ARMv8 NEON.
#   Kobo Clara 2E: i.MX6 SoloLite, Cortex-A9 r4p1: NEON + VFPv3-D32, no hardware divide.
CFLAGS_VAR := CFLAGS_armv7_unknown_linux_musleabihf
CXXFLAGS_VAR := CXXFLAGS_armv7_unknown_linux_musleabihf
KINDLE_CPU := RUSTFLAGS="-C target-cpu=cortex-a53" $(CFLAGS_VAR)="-mcpu=cortex-a53 -mfpu=neon-fp-armv8" $(CXXFLAGS_VAR)="-mcpu=cortex-a53 -mfpu=neon-fp-armv8"
KOBO_CPU := RUSTFLAGS="-C target-cpu=cortex-a9" $(CFLAGS_VAR)="-mcpu=cortex-a9 -mfpu=neon" $(CXXFLAGS_VAR)="-mcpu=cortex-a9 -mfpu=neon"

build:
	$(KINDLE_CPU) cargo build --release --target $(TARGET) -p zarathustra

# The device layout: zarathustra/{bin,hyph}, the KUAL extension, and library/ created empty
# (books go there by hand or over Wi-Fi; nothing is ever copied into it).
stage: build
	rm -rf $(STAGE)
	mkdir -p $(STAGE)/zarathustra/bin $(STAGE)/zarathustra/hyph $(STAGE)/extensions $(STAGE)/library
	cp $(addprefix $(BIN_DIR)/,$(BINS)) $(STAGE)/zarathustra/bin/
	cp -r dist/kual/zarathustra $(STAGE)/extensions/
	cp assets/hyphenation/*.bincode $(STAGE)/zarathustra/hyph/
	@# Plugins, with the SSH binaries from scripts/build-ssh.sh when they are built.
	cp -r dist/plugins $(STAGE)/zarathustra/
	@for b in dropbearmulti sftp-server; do \
		if [ -f target/ssh/$$b ]; then cp target/ssh/$$b $(STAGE)/zarathustra/plugins/ssh/; \
		else echo "warning: target/ssh/$$b not built (scripts/build-ssh.sh)" >&2; fi; \
	done

dev-dicts:
	scripts/fetch-dicts.sh

dicts:
	scripts/fetch-dicts.sh

# A running Zarathustra is told to restart into the new binary: run.sh sees the flag and runs
# it again without bringing the Amazon UI back (seconds instead of a full round trip).
RESTART := pidof zarathustra >/dev/null && touch /tmp/zarathustra-update && killall zarathustra; true
COPY_APP = tar -cf - -C $(STAGE) zarathustra extensions library | $(SSH) 'tar -xf - -C /mnt/us'

# The quick loop while developing: app, KUAL extension and plugins (no dictionaries), then the
# running app restarts into it.
update: stage
	$(COPY_APP)
	$(SSH) '$(RESTART)'
	@echo "Updated on $(KINDLE_HOST)."

install-ssh: stage dev-dicts
	$(COPY_APP)
	tar -cf - --exclude=_dl -C dev-dicts . | $(SSH) 'mkdir -p /mnt/us/dictionaries && tar -xf - -C /mnt/us/dictionaries'
	$(SSH) '$(RESTART)'
	@echo "Installed on $(KINDLE_HOST). Restarted if it was running, else KUAL → Zarathustra."

install-usb: stage dev-dicts
	@test -d "$(KINDLE)" || { echo "Kindle not mounted at $(KINDLE) (set KINDLE=…)" >&2; exit 1; }
	tar -cf - -C $(STAGE) zarathustra extensions library | $(UNTAR_USB) -C "$(KINDLE)"
	mkdir -p "$(KINDLE)/dictionaries"
	tar -cf - --exclude=_dl -C dev-dicts . | $(UNTAR_USB) -C "$(KINDLE)/dictionaries"
	sync
	@echo "Installed on $(KINDLE). Eject, then KUAL → Zarathustra."

BIN ?= zarathustra
run-ssh: stage
	$(COPY_APP)
	@# Another binary takes the screen: the app quits for real first (no update flag), or two
	@# would drive the screen and touch at once.
	$(SSH) 'killall zarathustra 2>/dev/null; sleep 3; /mnt/us/extensions/zarathustra/run.sh $(BIN); cat /mnt/us/zarathustra/$(BIN).log'

# Zarathustra on a Kobo (NickelMenu → Zarathustra).
#   make install-kobo-usb             build and copy onto a USB-mounted Kobo, dictionaries too.
#                                     If the app was open (USB handed over from it), it starts
#                                     the new build when the cable comes out; else eject, then
#                                     NickelMenu → Zarathustra.
KOBO ?= /run/media/$(USER)/KOBOeReader
KOBO_STAGE := target/kobo-stage
KOBO_APP := $(KOBO_STAGE)/.adds/zarathustra
KOBO_BIN_DIR := target/kobo/$(TARGET)/release

.PHONY: build-kobo stage-kobo install-kobo-usb
build-kobo:
	$(KOBO_CPU) cargo build --release --target $(TARGET) --target-dir target/kobo -p zarathustra

stage-kobo: build-kobo
	rm -rf $(KOBO_STAGE)
	mkdir -p $(KOBO_APP)/bin $(KOBO_APP)/hyph $(KOBO_APP)/dictionaries $(KOBO_STAGE)/.adds/nm $(KOBO_STAGE)/library
	cp $(KOBO_BIN_DIR)/zarathustra $(KOBO_APP)/bin/
	cp dist/kobo/zarathustra.sh dist/kobo/probe.sh $(KOBO_APP)/
	cp assets/hyphenation/*.bincode $(KOBO_APP)/hyph/
	cp dist/kobo/nm/zarathustra $(KOBO_STAGE)/.adds/nm/
	cp -r dist/plugins $(KOBO_APP)/
	@for b in dropbearmulti sftp-server; do \
		if [ -f target/ssh/$$b ]; then cp target/ssh/$$b $(KOBO_APP)/plugins/ssh/; \
		else echo "warning: target/ssh/$$b not built (scripts/build-ssh.sh)" >&2; fi; \
	done

install-kobo-usb: stage-kobo dev-dicts
	@test -d "$(KOBO)/.kobo" || { echo "Kobo not mounted at $(KOBO) (set KOBO=…)" >&2; exit 1; }
	tar -cf - -C $(KOBO_STAGE) . | $(UNTAR_USB) -C "$(KOBO)"
	tar -cf - --exclude=_dl -C dev-dicts . | $(UNTAR_USB) -C "$(KOBO)/.adds/zarathustra/dictionaries"
	sync
	@echo "Installed on $(KOBO). Unplug (the app restarts into it) or eject, then NickelMenu → Zarathustra."
