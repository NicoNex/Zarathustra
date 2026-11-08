#!/bin/sh
# Builds the device's SSH server for armv7 musl, static, key authentication only:
# - target/ssh/dropbearmulti: Dropbear (dropbear + dropbearkey + scp, one multi-call binary);
# - target/ssh/sftp-server: OpenSSH's, without OpenSSL (it needs no crypto). Since OpenSSH
#   9.0 scp speaks SFTP too, so file copies need it.
#
# Tarballs are pinned by SHA-256. To upgrade one, bump its version, check the new tarball's
# signature, and update the hash:
# - Dropbear: https://matt.ucc.asn.au/dropbear/releases/ (SHA256SUM.asc, *.asc), key
#   F7347EF2EE2E07A267628CA944931494F29C6773 "Dropbear SSH Release Signing";
# - OpenSSH portable: https://cdn.openbsd.org/pub/OpenBSD/OpenSSH/portable/ (*.asc), key
#   7168B983815A5EEF59A4ADFD2A3F414E736060BA (RELEASE_KEY.asc, Damien Miller).
set -eu

VERSION=2026.94
SHA256=e098034a843699200c8c977a991fff73159735bf795d5f72ef672c41a6b1ae81
OPENSSH=10.6p2
OPENSSH_SHA256=9ae44872090cf2f06b3cf9ff147e7f16f309671340763ed2f99248398ae29d5b
CROSS=arm-linux-musleabihf
# Where Zarathustra installs the SSH binaries on the device (persistent rootfs, not the
# userstore: a running server must not keep /mnt/us busy when it goes to a computer over USB).
PREFIX=/var/local/zarathustra

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="$ROOT/target/ssh-build"
OUT="$ROOT/target/ssh"
TARBALL="$WORK/dl/dropbear-$VERSION.tar.bz2"
mkdir -p "$WORK/dl" "$OUT"

# quiet LOG CMD…: runs CMD with its output in LOG, shown only when it fails (libtommath alone
# prints pages of harmless -Wundef warnings against musl's headers).
quiet() {
	log="$1"
	shift
	"$@" >"$log" 2>&1 || { cat "$log"; exit 1; }
}

# fetch FILE URL SHA256: downloads unless already there with the right hash, then checks it.
fetch() {
	if ! echo "$3  $1" | sha256sum -c --status 2>/dev/null; then
		curl -sSfL -o "$1" "$2"
		echo "$3  $1" | sha256sum -c --quiet
	fi
}
fetch "$TARBALL" "https://matt.ucc.asn.au/dropbear/releases/dropbear-$VERSION.tar.bz2" "$SHA256"

SRC="$WORK/dropbear-$VERSION"
rm -rf "$SRC"
tar -xjf "$TARBALL" -C "$WORK"

# Compile-time policy (overrides default_options.h).
cat >"$SRC/localoptions.h" <<EOF
/* Keys only: no passwords, no PAM. */
#define DROPBEAR_SVR_PASSWORD_AUTH 0
#define DROPBEAR_SVR_PAM_AUTH 0
#define DROPBEAR_SVR_PUBKEY_AUTH 1
/* Modern algorithms only. */
#define DROPBEAR_DSS 0
#define DROPBEAR_RSA 0
#define DROPBEAR_ECDSA 0
#define DROPBEAR_ED25519 1
#define DROPBEAR_3DES 0
#define DROPBEAR_ENABLE_CBC_MODE 0
#define DROPBEAR_SHA1_HMAC 0
#define DROPBEAR_SHA1_96_HMAC 0
#define DROPBEAR_DH_GROUP14_SHA1 0
#define DROPBEAR_DH_GROUP1 0
/* No X11 or agent forwarding on an e-reader. */
#define DROPBEAR_X11FWD 0
#define DROPBEAR_SVR_AGENTFWD 0
#define DSS_PRIV_FILENAME "$PREFIX/ssh/dropbear_dss_host_key"
#define RSA_PRIV_FILENAME "$PREFIX/ssh/dropbear_rsa_host_key"
#define ECDSA_PRIV_FILENAME "$PREFIX/ssh/dropbear_ecdsa_host_key"
#define ED25519_PRIV_FILENAME "$PREFIX/ssh/dropbear_ed25519_host_key"
#define SFTPSERVER_PATH "$PREFIX/bin/sftp-server"
#define DEFAULT_PATH "/usr/bin:/bin:/usr/sbin:/sbin"
#define DEFAULT_ROOT_PATH "/usr/sbin:/usr/bin:/sbin:/bin"
EOF

cd "$SRC"
./configure --host="$CROSS" CC="$CROSS-gcc" --enable-static --disable-zlib \
	--disable-lastlog --disable-utmp --disable-utmpx --disable-wtmp --disable-wtmpx >/dev/null
quiet "$WORK/dropbear.log" make -j"$(nproc)" PROGRAMS="dropbear dropbearkey scp" MULTI=1 STATIC=1
"$CROSS-strip" dropbearmulti
cp dropbearmulti "$OUT/"
echo "built $OUT/dropbearmulti ($(stat -c %s "$OUT/dropbearmulti") bytes)"

OSSH_TARBALL="$WORK/dl/openssh-$OPENSSH.tar.gz"
fetch "$OSSH_TARBALL" "https://cdn.openbsd.org/pub/OpenBSD/OpenSSH/portable/openssh-$OPENSSH.tar.gz" "$OPENSSH_SHA256"
OSSH="$WORK/openssh-$OPENSSH"
rm -rf "$OSSH"
tar -xzf "$OSSH_TARBALL" -C "$WORK"
cd "$OSSH"
./configure --host="$CROSS" CC="$CROSS-gcc" LDFLAGS=-static --without-openssl --without-zlib \
	--disable-lastlog --disable-utmp --disable-utmpx --disable-wtmp --disable-wtmpx >/dev/null
quiet "$WORK/sftp-server.log" make -j"$(nproc)" sftp-server
"$CROSS-strip" sftp-server
cp sftp-server "$OUT/"
echo "built $OUT/sftp-server ($(stat -c %s "$OUT/sftp-server") bytes)"
