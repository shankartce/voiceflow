#!/usr/bin/env bash
# Network fence (CLAUDE.md non-negotiable #1).
#
# Only `vt-models` may bring an HTTP/TLS client into the *normal* (runtime)
# dependency graph, and only behind its `download` feature. Crates that ship
# the downloader on purpose (the dev CLI `vt-bench`, later the `murmur` app)
# reach it through vt-models and are allowed. Build-time downloads by -sys crates
# (build-dependencies) are not part of the shipped binary and are not checked.
set -euo pipefail
cd "$(dirname "$0")/.."

BANNED='^(ureq|ureq-proto|reqwest|hyper|hyper-util|h2|rustls|native-tls|openssl|isahc|curl|attohttpc|surf|tiny_http)$'
ALLOWED=" vt-models vt-bench murmur "

packages=$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print("\n".join(p["name"] for p in json.load(sys.stdin)["packages"]))')

fail=0
check() { # <package> [extra cargo-tree args...]
  local pkg=$1; shift
  local hits
  hits=$(cargo tree -p "$pkg" -e normal --prefix none --format '{p}' "$@" 2>/dev/null \
    | awk '{print $1}' | sort -u | grep -E "$BANNED" || true)
  if [[ -n "$hits" ]]; then
    echo "✗ $pkg $* pulls in a network client: $(echo "$hits" | tr '\n' ' ')"
    fail=1
  else
    echo "✓ $pkg $*"
  fi
}

for pkg in $packages; do
  if [[ "$ALLOWED" == *" $pkg "* ]]; then
    continue
  fi
  check "$pkg"
  check "$pkg" --all-features
done
# vt-models itself must be network-free unless `download` is requested.
check vt-models --no-default-features

if [[ $fail -ne 0 ]]; then
  echo "Network fence failed: only vt-models (feature \"download\") may depend on an HTTP client."
  exit 1
fi
echo "Network fence OK."
