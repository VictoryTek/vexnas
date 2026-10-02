# Prompt C4 — add the vexnas flake input and `vexos.server.vexnas` module

Paste everything below the line into Claude in `~/Projects/vexos-nix`.

---

Task: integrate **vexnas**, a new NAS management web UI, into vexos-nix exactly the way **vexboard** is
integrated today. Follow your normal CLAUDE.md workflow (Phase 1 spec file first, etc.).

## Background
vexnas lives at `github:VictoryTek/vexnas` (Rust + Leptos/WASM, built with rust-overlay, same toolchain
as vexboard). It exports `packages.<system>.{vexnas,default}`, `overlays.default` (adds `pkgs.vexnas`)
and `nixosModules.{vexnas,default}` (declares `services.vexnas.*`). Do not assume anything else about
its internals.

`services.vexnas` options you will map onto (all exist in the vexnas module):
`enable`, `package`, `port` (default 7290), `openFirewall`, `firewall.interfaces`, `allowedCidrs`,
`adminGroup` (default "wheel"), `flakeDir` (default "/etc/nixos"), `poolScripts` (package or null),
`dataDir`.

## Changes
1. `flake.nix`
   - Add input `vexnas = { url = "github:VictoryTek/vexnas"; inputs.nixpkgs.follows = "nixpkgs-unstable"; };`
     with a comment mirroring the vexboard one (follows unstable for the rust-overlay toolchain; do NOT
     change to stable `nixpkgs`).
   - Add `vexnasBase = [ { nixpkgs.overlays = [ inputs.vexnas.overlays.default ]; } inputs.vexnas.nixosModules.default ];`
     next to `vexboardBase`, and append it to `baseModules` of **both** `roles.server` and
     `roles.headless-server`. Check how `mkBaseModule` and `nixosModules.*Base` consume `baseModules` and
     keep all pathways derived from the same table (do not duplicate wiring).
2. `modules/server/vexnas.nix` (new; import from `modules/server/default.nix`). Model it on
   `modules/server/vexboard.nix`. Options under `vexos.server.vexnas`:
   `enable`, `port` (default 7290), `adminGroup` (str, default "wheel"),
   `firewall.interfaces` (listOf str, default []), `allowedCidrs` (listOf str; default
   `127.0.0.1/32 ::1/128 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 fc00::/7 fe80::/10 100.64.0.0/10 fd7a:115c:a1e0::/48`).
   Config under `lib.mkIf cfg.enable`:
   - `vexos.server.backup.servicePaths.vexnas = [ "/var/lib/vexnas" ];` (backup.nix asserts every enabled
     service registers paths or is in `noBackupNeeded`).
   - `services.vexnas = { enable = true; inherit (cfg) port adminGroup allowedCidrs; firewall.interfaces = cfg.firewall.interfaces; };`
   - Emit a `warnings` entry when `firewall.interfaces == []` (mirror cockpit.nix wording).
   - No secret file is required (vexnas generates its own session secret and TLS cert in its state dir).
3. `justfile`: add `vexnas` to `_server_service_names`, plus a one-line description entry wherever
   `just enable` lists services (check how `vexboard` is listed). Verify `just enable vexnas` would write
   `vexos.server.vexnas.enable = true;` to `/etc/nixos/server-services.nix` without special-casing.
4. `template/server-services.nix`: add a commented entry
   `# vexos.server.vexnas.enable = false;  # Port 7290 (HTTPS) — NAS management UI` in the
   Monitoring & Management section, plus `vexnas` in any service-name list comments.
5. `CLAUDE.md`: add `vexnas` to the existing exception note for inputs whose nixpkgs does not follow
   stable (`nixpkgs-unstable` follow, like vexboard).
6. Do NOT make vexnas enabled by default anywhere. Do NOT put it in `vexos.server.nas.enable`'s
   mkDefaults (the NAS umbrella must keep working if vexnas is absent or broken).

## Constraints
- Option B module pattern; `lib.mkIf` only on the module's own `enable`.
- Never run `nix flake check`, `nixos-rebuild switch/boot`, or any git write.
- `flake.lock` will not contain vexnas until the user runs `nix flake lock --update-input vexnas`; say so in
  your handoff. The vexnas repo's first release may not exist yet, so validate with `nix flake show --impure`
  / dry-build only if the input resolves; otherwise document what could not be validated.

## Done when
Server and headless-server variants evaluate with vexnas disabled (no behaviour change), and with
`vexos.server.vexnas.enable = true` the generated config contains `services.vexnas.enable = true`,
the backup path registration, and no assertion failures.
