#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
CACHE_DIR="$ROOT_DIR/.btls-prebuilt/$TARGET_TRIPLE-debug"
CACHE_INCLUDE="$CACHE_DIR/include"
CACHE_LIB="$CACHE_DIR/lib"

cache_ready() {
  [[ -f "$CACHE_INCLUDE/openssl/x509v3.h" ]] \
    && [[ -f "$CACHE_LIB/libcrypto.a" ]] \
    && [[ -f "$CACHE_LIB/libssl.a" ]]
}

if ! cache_ready; then
  echo "Preparing the one-time btls-sys native cache for $TARGET_TRIPLE..."
  env -u BORING_BSSL_PATH cargo build --manifest-path "$ROOT_DIR/Cargo.toml"

  native_out=""
  crypto_lib=""
  ssl_lib=""
  while IFS= read -r candidate; do
    candidate_crypto="$(find "$candidate/build" -type f -name 'libcrypto.a' -print -quit 2>/dev/null || true)"
    candidate_ssl="$(find "$candidate/build" -type f -name 'libssl.a' -print -quit 2>/dev/null || true)"
    if [[ -f "$candidate/boringssl/include/openssl/x509v3.h" \
       && -n "$candidate_crypto" \
       && -n "$candidate_ssl" ]]; then
      native_out="$candidate"
      crypto_lib="$candidate_crypto"
      ssl_lib="$candidate_ssl"
      break
    fi
  done < <(find "$ROOT_DIR/target/debug/build" -maxdepth 2 -type d -name 'out' -path '*btls-sys-*' -print 2>/dev/null | sort -r)

  if [[ -z "$native_out" ]]; then
    echo "btls-sys finished without a discoverable native output directory." >&2
    exit 1
  fi

  mkdir -p "$CACHE_INCLUDE" "$CACHE_LIB"
  cp -R "$native_out/boringssl/include/." "$CACHE_INCLUDE/"
  cp "$crypto_lib" "$CACHE_LIB/"
  cp "$ssl_lib" "$CACHE_LIB/"
  echo "btls-sys cache ready: $CACHE_DIR"
fi

export BORING_BSSL_PATH="$CACHE_DIR"
exec cargo run --manifest-path "$ROOT_DIR/Cargo.toml" -- dev
