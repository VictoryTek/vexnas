# Later / optional vexos-nix prompts (C1.4, C2, C5, C8)

Each section is a separate prompt: paste only the section you want into Claude in `~/Projects/vexos-nix`.
These are **not** needed before vexnas Phase 6, and C1.4/C8 must wait for real-host proof.

---

## C1.4 — opt-in smbd reload instead of restart (after the declarative shares change has run on a real host)

Task: add `vexos.server.nas.smbReloadOnShareChange` (bool, default false) in
`modules/server/nas-shares.nix`. When true:
```nix
systemd.services.samba-smbd = {
  restartTriggers = lib.mkForce [ ];
  reloadTriggers = [ config.environment.etc."samba/smb.conf".source ];
};
```
Background: in the pinned nixpkgs, `samba-smbd` has `restartTriggers = [ configFile ]` and
`ExecReload = kill -HUP $MAINPID`, so share edits restart smbd and drop SMB sessions. Verify in the pinned
nixpkgs source that `reloadTriggers` is supported for services and that package upgrades still restart
(ExecStart path changes). Check nmbd/winbindd are unaffected (they share the same configFile trigger;
decide whether they need the same treatment and say why). Provide a real-host test plan: add/remove a
share with an active SMB transfer; confirm it continues; `testparm` clean. Default stays false.
Never run forbidden commands or git writes.

---

## C2 — SnapRAID scrub passthroughs

Task: in `modules/server/snapraid.nix` add `scrubPlan` (nullOr int, percent), `scrubOlderThan`
(nullOr int, days) and `touchBeforeSync` (nullOr bool), default null, passed through to
`services.snapraid.scrub.plan`, `.scrub.olderThan`, `.touchBeforeSync` only when non-null (leave the
upstream defaults otherwise). Verify the option names/types in the pinned nixpkgs `services.snapraid`.
No behaviour change when unset. Never run forbidden commands or git writes.

---

## C5 — headless-server SMB/NFS discovery (observation; independent of vexnas)

Observation: headless-server does not import `modules/network-desktop.nix`, so it has no
`services.samba-wsdd` and no `services.avahi.publish`; a headless NAS will not appear in Windows or
Nautilus network views. Task: research and spec (spec only unless asked to implement) a new
`modules/network-nas-discovery.nix` for server roles: responder-only wsdd (`discovery = false`) and an
Avahi `_smb._tcp` service record. HARD CONSTRAINTS: do not modify `network-desktop.nix` or `network.nix`;
SMB discovery on the GUI server currently works and must not change; avoid duplicate/conflicting
definitions on the GUI server role (which already enables wsdd with `discovery = true`). Never run
forbidden commands or git writes.

---

## C8 — retire Samba registry mode (GATED)

Do not start until the user confirms: a real host has run declarative shares only, the registry is
empty, Cockpit file-sharing is disabled, and SMB discovery was verified from Windows, macOS and GNOME.
Task: (1) extract the Samba/NFS server + firewall block from `modules/server/cockpit.nix` into a new
`modules/server/nas-sharing.nix` with `vexos.server.nas.sharing.enable`, with
`cockpit.fileSharing.enable` implying it during transition so existing hosts are unchanged; (2) drop the
assertion that declarative shares require `cockpit.fileSharing.enable` in favour of
`nas.sharing.enable`; (3) put `include = registry` behind a flag (default true for one release, then
flip). Preserve every global setting and firewall rule byte-for-byte. Never run forbidden commands or
git writes.
