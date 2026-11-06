# make install                    build, install Omero and the dictionaries on the Kindle (SSH)
# make install KINDLE_HOST=1.2.3.4 another address (DHCP moves it)
# make install KINDLE_HOST=        over USB instead (Kindle mounted)
KINDLE_HOST ?= 192.168.1.113
export KINDLE_HOST

.PHONY: install
install:
	scripts/deploy.sh
	scripts/fetch-dicts.sh push
