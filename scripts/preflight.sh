#!/usr/bin/env bash
# vexnas preflight — run before every commit/push.
#
#   nix develop path:. -c scripts/preflight.sh          # fast: fmt, clippy, tests
#   nix develop path:. -c scripts/preflight.sh --nix    # + nix build and the NixOS VM test
#
# (Use `nix develop` / `nix build .` instead of `path:.` once the files are tracked by git.)
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RUN_NIX=0
[ "${1:-}" = "--nix" ] && RUN_NIX=1
FLAKE="${PREFLIGHT_FLAKE:-.}"

FAILED=0
step() { echo ""; echo "--- $1 ---"; }
pass() { echo "[PASS] $1"; }
fail() { echo "[FAIL] $1"; FAILED=$((FAILED + 1)); }
run()  { local name="$1"; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }

echo "=== vexnas preflight ==="

step "Formatting"
run "cargo fmt" cargo fmt --all -- --check

# The frontend is wasm-only: lint it for its real target, everything else natively.
step "Lint: native crates"
run "clippy (native)" cargo clippy --workspace --exclude vexnas-frontend --all-targets -- -D warnings

step "Lint: frontend (wasm32)"
run "clippy (wasm32)" cargo clippy -p vexnas-frontend --target wasm32-unknown-unknown -- -D warnings

step "Tests"
run "cargo test" cargo test -p vexnas-proto -p vexnasd -p vexnas-web

step "Security audit"
if cargo audit --version >/dev/null 2>&1; then
  run "cargo audit" cargo audit
else
  echo "[SKIP] cargo-audit not installed"
fi

if [ "$RUN_NIX" -eq 1 ]; then
  step "Nix: package (includes trunk/wasm build and tests)"
  run "nix build vexnas" nix build "$FLAKE#vexnas" --no-link -L

  step "Nix: NixOS VM test (login, TLS, PAM, sandboxing, allowlist)"
  run "nix build vm-login" nix build "$FLAKE#checks.x86_64-linux.vm-login" --no-link -L
else
  echo ""
  echo "[SKIP] nix package + VM test (pass --nix to run them)"
fi

echo ""
echo "==================================="
if [ "$FAILED" -eq 0 ]; then
  echo "All preflight checks passed."
else
  echo "$FAILED check(s) failed."
  exit 1
fi
