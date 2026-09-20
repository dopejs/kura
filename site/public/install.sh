#!/bin/sh
# Kura installer — https://kura.dopejs.com/install.sh
#
#   curl -fsSL https://kura.dopejs.com/install.sh | sh
#
# Detects OS/arch, downloads the latest GitHub release tarball, verifies
# its SHA-256 against the release's SHA256SUMS, and installs `kura` and
# `kura-tui` into an existing PATH directory (~/.local/bin or
# /usr/local/bin). Override the version with KURA_VERSION=v0.3.0 and the
# destination with KURA_INSTALL_DIR=/some/bin.

set -eu

REPO="dopejs/kura"

say()  { printf '\033[1m[kura]\033[0m %s\n' "$1"; }
fail() { printf '\033[1;31m[kura]\033[0m %s\n' "$1" >&2; exit 1; }

# --- detect platform ------------------------------------------------------
OS=$(uname -s)
ARCH=$(uname -m)
case "$OS" in
  Darwin) SYS="apple-darwin" ;;
  Linux)  SYS="unknown-linux-gnu" ;;
  *) fail "unsupported OS: $OS (prebuilt binaries cover macOS and Linux; build from source: https://github.com/$REPO)" ;;
esac
case "$ARCH" in
  arm64|aarch64) CPU="aarch64" ;;
  x86_64|amd64)  CPU="x86_64" ;;
  *) fail "unsupported architecture: $ARCH" ;;
esac
TARGET="${CPU}-${SYS}"

# --- resolve version ------------------------------------------------------
if [ -n "${KURA_VERSION:-}" ]; then
  TAG="$KURA_VERSION"
else
  TAG=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" | sed 's#.*/tag/##')
  [ -n "$TAG" ] || fail "could not resolve the latest release tag"
fi
VERSION="${TAG#v}"
PKG="kura-${VERSION}-${TARGET}"
BASE="https://github.com/$REPO/releases/download/$TAG"

say "installing Kura $TAG for $TARGET"

# --- download + verify ----------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
curl -fsSL -o "$TMP/$PKG.tar.gz" "$BASE/$PKG.tar.gz" \
  || fail "download failed: $BASE/$PKG.tar.gz"
if curl -fsSL -o "$TMP/SHA256SUMS" "$BASE/SHA256SUMS" 2>/dev/null; then
  EXPECTED=$(grep "$PKG.tar.gz" "$TMP/SHA256SUMS" | awk '{print $1}' | head -1)
  if [ -n "$EXPECTED" ]; then
    if command -v shasum >/dev/null 2>&1; then
      ACTUAL=$(shasum -a 256 "$TMP/$PKG.tar.gz" | awk '{print $1}')
    else
      ACTUAL=$(sha256sum "$TMP/$PKG.tar.gz" | awk '{print $1}')
    fi
    [ "$EXPECTED" = "$ACTUAL" ] || fail "SHA-256 mismatch for $PKG.tar.gz"
    say "checksum verified"
  fi
fi
tar -xzf "$TMP/$PKG.tar.gz" -C "$TMP"

# --- install --------------------------------------------------------------
if [ -n "${KURA_INSTALL_DIR:-}" ]; then
  DEST="$KURA_INSTALL_DIR"
elif [ -d "$HOME/.local/bin" ] && case ":$PATH:" in *":$HOME/.local/bin:"*) true ;; *) false ;; esac; then
  DEST="$HOME/.local/bin"
else
  DEST="/usr/local/bin"
fi
mkdir -p "$DEST" 2>/dev/null || true

install_bin() {
  if [ -w "$DEST" ]; then
    install -m 755 "$1" "$DEST/"
  else
    say "sudo needed to write $DEST"
    sudo install -m 755 "$1" "$DEST/"
  fi
}
install_bin "$TMP/$PKG/kura"
[ -f "$TMP/$PKG/kura-tui" ] && install_bin "$TMP/$PKG/kura-tui"
if [ -d "$TMP/$PKG/web" ]; then
  WEB_DIR="$HOME/.local/share/kura/web"
  mkdir -p "$WEB_DIR"
  rm -rf "$WEB_DIR"
  cp -R "$TMP/$PKG/web" "$WEB_DIR"
  say "web shell assets: $WEB_DIR"
fi

say "installed to $DEST: kura$([ -f "$TMP/$PKG/kura-tui" ] && printf ', kura-tui')"
say "start the daemon:   kura daemon start   (data: ~/.kura, http://127.0.0.1:19191)"
say "terminal client:    kura tui"
say "web shell:          kura web"
say "docs:               https://kura.dopejs.com"
