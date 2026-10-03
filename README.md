# vexnas

A small, modern web UI to set up and manage the NAS stack of [VexOS](https://github.com/VictoryTek/vexos-nix)
(NixOS) — shares, users, jobs and pools — with every change applied as a **rollback-able NixOS generation**.

Think OpenMediaVault / TrueNAS, with far less scope, built around how vexos-nix actually configures its NAS.

> **Status: Phase 0 (skeleton).** You can sign in with a system account; there is no NAS functionality yet.
> The design, API, security model and phased plan live in [docs/spec.md](docs/spec.md).

## How it works

- Changes flow through Nix: the UI's source of truth is one generated file, `/etc/nixos/nas.nix`, which only
  sets existing `vexos.server.*` options. Apply = diff → build in staging → preview → activate → verify →
  auto-rollback on failure.
- Imperative only where Nix shouldn't own it: SMB passwords, live status, "run now" actions, one-shot disk setup.
- Security first, because the backend can change root-owned state: PAM login, CSRF protection, self-signed
  HTTPS by default, a source-address allowlist, and a narrow root helper instead of a root web server.

```
browser ─HTTPS:7290─▶ vexnas-web (unprivileged) ─unix socket─▶ vexnasd (root helper, PAM + typed verbs)
```

## Use it (NixOS)

```nix
# flake.nix
inputs.vexnas.url = "github:VictoryTek/vexnas";

# configuration
nixpkgs.overlays = [ inputs.vexnas.overlays.default ];
imports = [ inputs.vexnas.nixosModules.default ];
services.vexnas = {
  enable = true;
  firewall.interfaces = [ "eno1" ];   # recommended
  # adminGroup = "wheel";             # members get the admin role; nobody else gets in
  # viewerGroup = "nas-view";         # optional read-only role
};
```

Then open `https://<host>:7290` (accept the self-signed certificate, or set `services.vexnas.tls.*`).
On vexos-nix this is wired up as `vexos.server.vexnas.enable` (see `docs/vexos-nix-prompts/`).

Until the apply engine lands, a rebuild that changes vexnas itself needs a manual
`sudo systemctl restart vexnasd.socket vexnasd.service vexnas.service` (the units deliberately do not
restart on switch, so a rebuild can never kill an in-flight apply).

## Develop

```
nix develop path:.                       # rust toolchain, trunk, wasm-bindgen, pam
nix develop path:. -c scripts/preflight.sh          # fmt, clippy, tests
nix develop path:. -c scripts/preflight.sh --nix    # + package build and NixOS VM test
```

| Path | What |
|---|---|
| `crates/vexnas-web` | axum backend: sessions, CSRF, auth, audit, TLS, static assets |
| `crates/vexnasd` | root helper: PAM and (later) privileged verbs |
| `crates/vexnas-proto` | wire protocol between the two |
| `crates/vexnas-frontend` | Leptos CSR/WASM UI |
| `nix/` | package, NixOS module, VM test |
| `docs/` | spec and prompts for vexos-nix changes |

## License

MIT
