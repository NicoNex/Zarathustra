#!/bin/sh
# Builds the device's SSH server: Dropbear (dropbear + dropbearkey + scp in one static
# multi-call binary) for armv7 musl, key authentication only. Output: target/ssh/dropbearmulti.
#
# The tarball is pinned by SHA-256. To upgrade: bump VERSION, then check the new hash against
# https://matt.ucc.asn.au/dropbear/releases/SHA256SUM.asc and the release signature (key
# F7347EF2EE2E07A267628CA944931494F29C6773, "Dropbear SSH Release Signing"), and update SHA256.
set -eu

VERSION=2026.94
SHA256=e098034a843699200c8c977a991fff73159735bf795d5f72ef672c41a6b1ae81
CROSS=arm-linux-musleabihf
# Where Zarathustra installs the SSH binaries on the device (persistent rootfs, not the
# userstore: a running server must not keep /mnt/us busy when it goes to a computer over USB).
PREFIX=/var/local/zarathustra

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK="$ROOT/target/ssh-build"
OUT="$ROOT/target/ssh"
TARBALL="$WORK/dl/dropbear-$VERSION.tar.bz2"
mkdir -p "$WORK/dl" "$OUT"

if ! echo "$SHA256  $TARBALL" | sha256sum -c --status 2>/dev/null; then
	curl -sSfL -o "$TARBALL" "https://matt.ucc.asn.au/dropbear/releases/dropbear-$VERSION.tar.bz2"
	echo "$SHA256  $TARBALL" | sha256sum -c --quiet
fi

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
make -j"$(nproc)" PROGRAMS="dropbear dropbearkey scp" MULTI=1 STATIC=1 >/dev/null
"$CROSS-strip" dropbearmulti
cp dropbearmulti "$OUT/"
echo "built $OUT/dropbearmulti ($(stat -c %s "$OUT/dropbearmulti") bytes)"
