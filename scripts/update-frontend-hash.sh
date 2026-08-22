#!/usr/bin/env sh
set -eu

# Recomputes the fixed-output hash of the frontend dependency closure in
# flake.nix. Run this whenever frontend/bun.lock changes, otherwise `nix build`
# fails with a hash mismatch on any machine that has not already cached the
# previous closure.

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
flake="$root/flake.nix"
fake="sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="

current=$(sed -n 's/^ *outputHash = "\(sha256-[^"]*\)";$/\1/p' "$flake")
if [ -z "$current" ]; then
  echo "no outputHash found in $flake" >&2
  exit 1
fi

sed -i "s|$current|$fake|" "$flake"
got=$(nix build --no-link "$root#gitadel.nodeModules" 2>&1 |
  sed -n 's/.*got: *\(sha256-[^ ]*\).*/\1/p')
sed -i "s|$fake|${got:-$current}|" "$flake"

if [ -z "$got" ]; then
  echo "frontend dependency hash is already up to date"
else
  echo "frontend dependency hash updated to $got"
fi
