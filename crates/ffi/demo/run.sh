#!/usr/bin/env bash
# Build the slimcode-ffi cdylib and run the Node acceptance scripts: the
# ticket-01 ABI smoke test and the ticket-07 acceptance demo (`node/demo.mjs`).
# The runner works from any cwd (the repo root comes from this script's own
# path) and honours a shared `CARGO_TARGET_DIR`, so it finds the artifact cargo
# actually built.
#
# Usage: bash crates/ffi/demo/run.sh
# Prints PASS and exits 0, or prints FAIL and exits non-zero.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-$root/target}"

# The Node entry points this runner drives, relative to this directory: the
# smoke is the cheap ABI check, the demo is the acceptance evidence.
scripts=(node/smoke.mjs node/demo.mjs)

if ! command -v node >/dev/null 2>&1; then
  echo "FAIL: node is not on PATH (needs Node >= 26 with the built-in node:ffi)" >&2
  exit 1
fi
node_major="$(node -p 'process.versions.node.split(".")[0]')"
if [ "$node_major" -lt 26 ]; then
  echo "FAIL: Node >= 26 required for the built-in node:ffi; found $(node --version)" >&2
  exit 1
fi

echo "==> cargo build -p slimcode-ffi"
CARGO_TARGET_DIR="$target_dir" cargo build --manifest-path "$root/Cargo.toml" -p slimcode-ffi

# The platform cdylib name cargo produces.
case "$(uname -s)" in
  Darwin) library="$target_dir/debug/libslimcode_ffi.dylib" ;;
  Linux) library="$target_dir/debug/libslimcode_ffi.so" ;;
  *)
    echo "FAIL: unsupported platform $(uname -s)" >&2
    exit 1
    ;;
esac
if [ ! -f "$library" ]; then
  echo "FAIL: cargo did not produce $library" >&2
  exit 1
fi
echo "==> library: $library"

for script in "${scripts[@]}"; do
  echo "==> node $script"
  if ! node "$here/$script" "$library"; then
    echo "FAIL: $script" >&2
    exit 1
  fi
done

echo "PASS: slimcode-ffi demo (${#scripts[@]} script(s))"