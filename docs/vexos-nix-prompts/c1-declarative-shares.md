# Prompt C1 — declarative SMB/NFS shares (alongside registry mode)

Paste everything below the line into Claude in `~/Projects/vexos-nix`.

---

Task: add a declarative share option set `vexos.server.nas.shares.<name>` that renders to
`services.samba.settings` and `services.nfs.server.exports`, **alongside** the existing Cockpit
registry-mode setup. Follow your CLAUDE.md workflow; research must include the pinned nixpkgs'
`services.samba` and `services.nfs.server` modules and current `exports(5)` / mergerfs-NFS guidance.

## CAUTION (non-negotiable)
SMB discovery works today and was hard-won (Avahi `denyInterfaces`, `samba-wsdd discovery = true`,
`network-desktop.nix`). Therefore:
- Do NOT touch `services.samba.settings.global`, `include = registry`, `hosts allow`,
  `bind interfaces only`, Avahi, wsdd, `network.nix`, or `network-desktop.nix`.
- Do NOT remove or alter registry mode or Cockpit file-sharing. Registry shares and
  `/etc/exports.d/cockpit-file-sharing.exports` must keep working unchanged.
- Facts verified against smb.conf(5): *"Shares defined in smb.conf take priority over shares of the same
  name defined in registry."* so a name collision silently shadows a Cockpit share. Add what protection
  you can at eval time (you cannot read the runtime registry; document that the vexnas UI enforces this).

## Where
New `modules/server/nas-shares.nix`, imported from `modules/server/default.nix`. Options live under the
existing `vexos.server.nas` namespace (declared in `modules/server/nas.nix`; do not edit nas.nix except
if you must). Also check `modules/server/backup.nix`'s `noBackupNeeded`/servicePaths assertion is not
tripped (nas already appears in `noBackupNeeded`).

## Options
`vexos.server.nas.shareRoots` (listOf str). Default: mergerfs `mountPoint` when
`vexos.server.storage.mergerfs.enable`, plus `/<pool>` for each name in `boot.zfs.extraPools`, plus
`/srv`. Use `defaultText`.

`vexos.server.nas.shares` = attrsOf submodule:
- `path` (str, required): absolute; no `..`, `//` or trailing `/`; must be under one of `shareRoots`;
  must not be or be a prefix of `/ /etc /nix /boot /root /home /var /run /proc /sys /dev`.
- `comment` (str, default "", max 64 chars, no newline).
- `directory.create` (bool, default false), `directory.owner`/`group` (str), `directory.mode`
  (str matching `^[0-7]{3,4}$`). When `create`, render a `systemd.tmpfiles.settings` `d` rule
  (top directory only, never recursive).
- `smb.enable` (bool, false), `smb.readOnly` (bool, false), `smb.browseable` (bool, true),
  `smb.guestOk` (bool, false; assert `guestOk -> readOnly`), `smb.validUsers` / `smb.writeList`
  (listOf str; each `name` or `@group`).
- `nfs.enable` (bool, false), `nfs.fsid` (int 1..65535, required when nfs.enable; unique across
  shares — FUSE/mergerfs exports need an explicit fsid), `nfs.clients` (listOf submodule:
  `host` str — CIDR/IP/hostname, **reject `*`**; `access` enum ro|rw; `squash` enum root|all|none
  (none = no_root_squash); `sync` bool default true).
- Share name rule: `^[a-z][a-z0-9_-]{0,31}$`, not `global homes printers ipc$`.

## Rendering (only when `shares != {}`)
- `services.samba.settings.<name>` = `{ path; comment; "read only"; browseable; "guest ok"; "valid users"?; "write list"? }`
  for shares with `smb.enable`, using yes/no strings. Merge-safe with existing `settings.global`.
- `services.nfs.server.exports`: one line per `nfs.enable` share:
  `"<path>" host(access,sync|async,no_subtree_check,fsid=N[,root_squash|all_squash|no_root_squash]) …`.
  Use correct `exports(5)` double-quote path quoting (NOT `escapeShellArg`). Default squash `root`
  renders `root_squash`.
- The `directory` tmpfiles rules.
- Samba/NFS server enablement, firewall and `hosts allow` are NOT reimplemented: add an assertion
  `shares != {} -> config.vexos.server.cockpit.fileSharing.enable` with a clear message (a later change
  extracts the sharing block from cockpit.nix).

## Assertions
Every validation rule above; unique NFS paths and fsids; `nfs.enable -> nfs.clients != [ ]`;
`smb.enable || nfs.enable` for every share (a share with neither is a mistake).
Users/groups referenced in `validUsers`/`writeList`/`directory.owner|group` are NOT asserted to exist
here (they may be host users); document this.

## Known behaviour to document in the module header (do NOT change it in this task)
In the pinned nixpkgs `samba-smbd` has `restartTriggers = [ configFile ]`, so any declarative share
change **restarts smbd and drops open SMB sessions** (registry changes do not). A later, opt-in change
will switch to reload; this task only documents it.

## Constraints
Option B pattern (lib.mkIf only on the module's own options). Never run `nix flake check`,
`nixos-rebuild switch/boot`, or any git write. Validate with `nix eval --impure` / dry-build of
server-amd and headless-server-amd with (a) no shares, (b) one SMB-only, one NFS-only, one both, and
(c) a deliberately invalid share to confirm each assertion fires. Also confirm
`config.services.samba.settings.global` and Cockpit-related output are byte-identical before/after
for a host with no shares.

## Done when
No-shares hosts are unchanged; sample shares render the expected `smb.conf` sections and `/etc/exports`
lines; all assertions fire on bad input; registry mode untouched.
