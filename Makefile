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

build:
	cargo build --release --target $(TARGET) -p zarathustra

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
