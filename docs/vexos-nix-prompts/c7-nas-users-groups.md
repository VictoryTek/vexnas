# Prompt C7 — declarative NAS-only users and groups

Paste everything below the line into Claude in `~/Projects/vexos-nix`.

---

Task: add `vexos.server.nas.users` and `vexos.server.nas.groups` (declarative NAS-only accounts) in
`modules/server/nas-shares.nix` (created by the declarative-shares change; if it does not exist yet,
stop and report). Follow your CLAUDE.md workflow.

## Options
- `vexos.server.nas.groups.<name>`: name `^[a-z_][a-z0-9_-]{0,31}$`; `gid` (nullOr int, >= 1000).
- `vexos.server.nas.users.<name>`: same name rule; `description` (str, <= 64 chars); `groups`
  (listOf str, each must be declared in `nas.groups` or already exist as a host group — document that
  host groups cannot be asserted at eval time); `uid` (nullOr int, >= 1000).

## Rendering
```nix
users.groups = lib.mapAttrs (_: g: lib.optionalAttrs (g.gid != null) { inherit (g) gid; }) cfg.groups;
users.users = lib.mapAttrs (_: u: {
  isNormalUser = true; createHome = false; home = "/var/empty";
  shell = "${pkgs.shadow}/bin/nologin"; description = u.description; extraGroups = u.groups;
} // lib.optionalAttrs (u.uid != null) { inherit (u) uid; }) cfg.users;
```
Passwords are never set here (Linux account is passwordless/locked; SMB passwords are set by the UI via
`smbpasswd`). Keep SSH/login impossible for these accounts.

## Assertions
- `users.mutableUsers` must be true when `nas.users != { }` (the stateless role sets it false).
- No name collision with users/groups declared elsewhere in the config: investigate how to detect this
  cleanly (e.g. `options.users.users.definitionsWithLocations`) — if the module system already errors on
  conflicting definitions, say so and rely on that instead of a custom assertion.
- Names must not collide with the primary user (`config.vexos.user.name`).

## Research items to resolve in your spec (report findings)
1. On the pinned nixpkgs, what happens to a previously declared user when it is removed from config?
   (`update-users-groups.pl` vs userborn) — is the account deleted, kept, or left orphaned with files?
2. Whether `isNormalUser = true` with `home = "/var/empty"` and `createHome = false` behaves correctly
   (no home dir created, no warnings).
vexnas will additionally run `smbpasswd -x` for removed users, so this change only needs to document the
Linux-side behaviour.

## Constraints
Option B pattern. Never run `nix flake check`, `nixos-rebuild switch/boot`, or any git write. Validate with
dry-build of server-amd/headless-server-amd with sample users/groups and with the assertion cases.

## Done when
Sample users/groups render correctly, assertions fire on bad input, and the research items are answered in
the spec/review doc.
