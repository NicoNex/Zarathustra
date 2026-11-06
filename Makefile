# Zarathustra on a Kindle.
#   make install-ssh                  build, install the app and the dictionaries over SSH
#   make install-ssh KINDLE_HOST=…    another address (DHCP moves it)
#   make install-usb                  the same onto a USB-mounted Kindle
#   make run-ssh BIN=zarathustra-probe   install over SSH, then run a binary and show its log
#   make dicts                        download the dictionaries into dev-dicts/
KINDLE_HOST ?= 192.168.1.113
KINDLE_PORT ?= 2222
KINDLE ?= /run/media/$(USER)/Kindle

TARGET := armv7-unknown-linux-musleabihf
BIN_DIR := target/$(TARGET)/release
BINS := zarathustra zarathustra-probe zarathustra-textprobe zarathustra-powerprobe
STAGE := target/kindle-stage
SSH := ssh -p $(KINDLE_PORT) -o BatchMode=yes root@$(KINDLE_HOST)
# VFAT keeps no owners, permissions or mtimes.
UNTAR_USB := tar -xf - --no-same-owner --no-same-permissions -m

.PHONY: install install-ssh install-usb run-ssh build stage dicts
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

dev-dicts:
	scripts/fetch-dicts.sh

dicts:
	scripts/fetch-dicts.sh

install-ssh: stage dev-dicts
	@# A running instance would keep the old binary and fight the new one for the screen.
	$(SSH) 'killall zarathustra 2>/dev/null; true'
	tar -cf - -C $(STAGE) zarathustra extensions library | $(SSH) 'tar -xf - -C /mnt/us'
	tar -cf - --exclude=_dl -C dev-dicts . | $(SSH) 'mkdir -p /mnt/us/dictionaries && tar -xf - -C /mnt/us/dictionaries'
	@echo "Installed on $(KINDLE_HOST). KUAL → Zarathustra."

install-usb: stage dev-dicts
	@test -d "$(KINDLE)" || { echo "Kindle not mounted at $(KINDLE) (set KINDLE=…)" >&2; exit 1; }
	tar -cf - -C $(STAGE) zarathustra extensions library | $(UNTAR_USB) -C "$(KINDLE)"
	mkdir -p "$(KINDLE)/dictionaries"
	tar -cf - --exclude=_dl -C dev-dicts . | $(UNTAR_USB) -C "$(KINDLE)/dictionaries"
	sync
	@echo "Installed on $(KINDLE). Eject, then KUAL → Zarathustra."

BIN ?= zarathustra
run-ssh: install-ssh
	$(SSH) '/mnt/us/extensions/zarathustra/run.sh $(BIN); cat /mnt/us/zarathustra/$(BIN).log'
