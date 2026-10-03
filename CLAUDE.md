# CLAUDE.md — vexnas

NAS management web UI for VexOS (NixOS). Rust (axum 0.8 + Leptos 0.8 CSR/WASM + Trunk), consumed by
`vexos-nix` as a flake input exactly like `vexboard`. **`docs/spec.md` is the source of truth** for design,
API, auth model, apply state machine and the phase plan; read it before changing behaviour. If code and spec
disagree, fix one of them in the same change.

## Hard rules
- **Never run `git add`, `git commit`, `git push`, `git stash`.** Finish work with a commit message for the user to run.
- **This repo never changes code in other repos.** Anything vexos-nix needs is a paste-ready prompt in
  `docs/vexos-nix-prompts/` (index in its README). Prompts must be self-contained.
- Never run `nixos-rebuild switch/boot` or `nix flake check` anywhere. Validate with `nix build`.
- No implementation outside the approved phase plan (spec §10) without asking.

## Build / test
Files must be tracked by git for plain `.` flake refs; until then use `path:.` and keep cargo's target dir
outside the repo so Nix does not copy it:

```
export CARGO_TARGET_DIR=<somewhere outside the repo>
nix develop path:. -c scripts/preflight.sh          # fmt, clippy (native + wasm32), tests
nix develop path:. -c scripts/preflight.sh --nix    # + nix build and the NixOS VM test
nix build path:.#vexnas -L
nix build path:.#checks.x86_64-linux.vm-login -L    # real TLS + PAM + sandboxing + 2 machines
```
The frontend is wasm-only: lint it with `--target wasm32-unknown-unknown`; native clippy excludes it.

## Architecture invariants (do not weaken without changing the spec)
- Three layers: `vexnas-web` (unprivileged, network-facing) → `vexnasd` (root helper, socket-activated, closed
  verb set in `vexnas-proto`) → transient root units for long jobs. The web process has no capabilities and no
  `shadow` access.
- **The helper never accepts Nix text** — typed JSON only; it validates and renders `nas.nix` itself. No
  free-form escape hatches (raw smb.conf params, rsync flags, raw timer config).
- `nas.nix` is pure data, positive-only (`enable = true`, never `false`), and the single source of truth.
- Auth: PAM via the helper; admin = `adminGroup`, viewer = `viewerGroup`, nobody else (no bootstrap admin).
  `__Host-vexnas` cookie, SameSite=Strict, CSRF token + `Origin` check on every write, re-auth for destructive verbs.
- **CSP forbids inline styles and `unsafe-inline`.** Frontend: classes only, no `style=` attributes. The one inline
  script Trunk injects is allowed by hash, computed at startup from the real `index.html`.
- Over TLS browsers speak HTTP/2: there is **no `Host` header** (use `uri().authority()`), a bug the VM test caught.
- vexnas units use `restartIfChanged = false` so a rebuild cannot kill an in-flight apply.

## Dependencies
`wasm-bindgen-cli` in `flake.nix` is pinned and must equal the `wasm-bindgen` crate in `Cargo.lock` (Trunk
enforces this); consumers build us against their own nixpkgs, so it is not taken from nixpkgs. To bump:
`cargo update -p wasm-bindgen --precise X`, update `version` + both hashes in `flake.nix` (build once; Nix prints
the real hashes). Use Context7 to check current APIs before adding a dependency.
