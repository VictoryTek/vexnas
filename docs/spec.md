# vexnas — Specification (v0.1, draft for approval)

**Status:** Draft. **No implementation until approved.**
**Date:** 2026-10-01
**Scope:** vexnas (this repo) plus the companion changes it needs in `vexos-nix`.
Companion changes are **specified here only**. They get built in vexos-nix under that repo's
own 7-phase workflow.

---

## 0. Decisions recorded

| # | Question | Decision |
|---|---|---|
| D1 | Standalone app vs. a section in vexboard | **Standalone.** Separate repo, service, port, and privilege boundary. vexboard can link to it as a quick link. |
| D2 | Frontend stack | **Match vexboard:** Rust, axum 0.8, Leptos 0.8 (CSR/WASM), Trunk. Styling is plain CSS with vexboard's design tokens, not Tailwind (see §12). |
| D3 | Migrate Samba from registry mode to declarative shares | **Yes, gated.** Phase 1 runs declarative shares *alongside* `include = registry`. Registry mode is removed only after a real host has run declarative-only with discovery verified. |
| D4 | Transport security | **Self-signed HTTPS by default**, generated on first start (Cockpit's model). A user-supplied cert/key can override it. |

Architecture direction from the kickoff is kept:
- Changes flow through Nix: one generated file, `/etc/nixos/nas.nix`, which only sets `vexos.server.*` options.
- Imperative paths exist only for passwords, live status, "run now" actions, and one-shot disk setup.

Three refinements below change *how* that direction is implemented:
- §4.2: the root helper renders Nix itself and never accepts Nix text.
- §5.2: the build is done in a staged copy of `/etc/nixos` before anything is written.
- §5.3: the exact output that was built and previewed is the one activated.

---

## 1. Current state (what vexnas must drive)

Read from `~/Projects/vexos-nix` @ `ce6e0fe` and the pinned nixpkgs (26.05,
`/nix/store/kmr0nfym…-source`).

### 1.1 NAS stack today

| Piece | Where | Notes for vexnas |
|---|---|---|
| Umbrella `vexos.server.nas.enable` | `modules/server/nas.nix` | Only `mkDefault`-enables Cockpit + navigator + fileSharing + identities. Touches no storage. |
| `vexos.server.nas.backend = "zfs" \| "mergerfs"` | `nas.nix` | `"mergerfs"` turns on `vexos.server.storage.mergerfs`. **`storage-pool.nix` also sets `backend = "mergerfs"`**, so `nas.nix` must never set `backend` (two different values cause an eval conflict). |
| Samba | `modules/server/cockpit.nix`, under `cockpit.fileSharing.enable` | Registry mode (`include = registry`). Globals are `bind interfaces only`, `hosts allow` (RFC1918/ULA), and optionally `interfaces`. Shares are created with `net conf` by the Cockpit plugin. Assertion: fileSharing requires `cockpit.enable`. |
| Samba globals (client side) | `modules/network-desktop.nix` (GUI server only) | `workgroup`, `server role`, `client min protocol = NT1`. Merges into the same `services.samba.settings.global`. |
| NFS server | `cockpit.nix` | `services.nfs.server.enable`. Plugin writes `/etc/exports.d/cockpit-file-sharing.exports`. NixOS owns `/etc/exports` (`services.nfs.server.exports`, a plain string). Profile is `v4-minimal` (TCP 2049) or `v3-compatible`. |
| Discovery ("hard-won") | `network.nix` (Avahi, `denyInterfaces = tailscale0`), `network-desktop.nix` (`avahi.publish`, `samba-wsdd` with `discovery = true`) | Independent of where shares are defined. **Headless-server does not import `network-desktop.nix`**, so it has no wsdd and no Avahi publish. vexnas must not touch any of this. |
| mergerfs | `modules/server/mergerfs.nix` | `branches` written to `/etc/nixos/storage-pool.nix` by the script. Union mounted at `/storage`. |
| SnapRAID | `modules/server/snapraid.nix` | Wraps `services.snapraid`. Data disks derive from mergerfs branches; parity comes from `storage-pool.nix`. `syncInterval` / `scrubInterval` are **not** set by `storage-pool.nix`, so `nas.nix` can own them. Units: `snapraid-sync`, `snapraid-scrub`. |
| nas-sync | `modules/server/nas-sync.nix` | `vexos.server.nasSync.jobs.<n>` → `nas-sync-<n>.service/.timer`. Currently documented as set in `server-services.nix`. `extraRsyncOptions` is free-form (see §4.2: vexnas will not emit it). |
| Backup | `modules/server/backup.nix` | restic `services.restic.backups.main` → `restic-backups-main.service`. Needs `repository`/`repositoryFile` and `passwordFile` (asserted). |
| SMART | `modules/server/scrutiny.nix` | Optional Scrutiny on :8078 plus smartd. |
| ZFS | `modules/zfs-server.nix` | autoScrub monthly, trim weekly. Pools registered by script into `/etc/nixos/zfs-pools.nix` (`boot.zfs.extraPools`). OpenZFS **2.3.9 / 2.4.4** in the pin, both of which support `zpool status -j` / `zpool list -j` (JSON). |
| Remote mounts | `modules/storage-remote.nix` | `vexos.storage.remote`, written by `attach-remote-storage.sh`. Out of MVP scope (read-only display only). |

### 1.2 Host-local files and how they are imported

Two separate import paths. **Both need `nas.nix`.**
1. **`/etc/nixos/flake.nix`**, the thin wrapper from `template/etc-nixos-flake.nix`. This is what
   real hosts use, through `just rebuild` → `sudo nixos-rebuild switch --impure --flake "path:/etc/nixos#$(cat /etc/nixos/vexos-variant)"`.
   It uses relative `./x.nix` + `builtins.pathExists` in `mkServerVariant` / `mkHeadlessServerVariant`.
2. **Repo `flake.nix` `mkHost`**: `roles.<role>.hostLocalModules` uses absolute `/etc/nixos/x.nix` paths.

Existing hosts have a *copy* of the template. A template change only reaches a host when its
`/etc/nixos/flake.nix` is refreshed (see risk R6).

### 1.3 Scripts

`scripts/create-zfs-pool.sh` and `scripts/create-mergerfs-pool.sh` are **interactive only**:
- Disks are chosen by **on-screen index** into a list built from `ls /dev/disk/by-id`.
- Confirmation is typed: the pool name, or `storage`.
- `create-zfs-pool.sh` also asks about the `vm` child dataset and the Proxmox storage ID.
- `create-mergerfs-pool.sh` **regenerates `storage-pool.nix` from only the disks selected in that run**.
  Re-running it to "add a disk" reformats the selected disks and drops unselected branches from the file.

Feeding these prompts from a pipe or PTY is rejected: index order is not a stable identity for
destroying disks. See companion change C6.

### 1.4 Facts verified in the pinned nixpkgs (26.05)

- `services.samba`: `samba-smbd` has `restartTriggers = [ configFile ]`.
  **Any declarative share change restarts smbd and drops open SMB sessions.** Registry-mode
  changes do not. See §5.4 and C1.4.
- `services.nfs.server.exports` is a `lines` string written to `/etc/exports`.
  `nfs-mountd` has `restartTriggers = [ exports ]`.
- `smb.conf(5)`:
  - "Activation of global registry options automatically activates registry shares."
  - "**Shares defined in smb.conf take priority over shares of the same name defined in registry.**"
  - So a name collision silently shadows the Cockpit share. vexnas forbids collisions except
    during an explicit import (§6.3).
- nixos-rebuild is **nixos-rebuild-ng**. It runs `switch-to-configuration` inside
  `systemd-run --collect --pipe --service-type=exec --unit=nixos-rebuild-switch-to-configuration`.
  A fixed unit name means two concurrent switches cannot both start. vexnas reuses that name to
  get the same mutual exclusion against a manual `just rebuild` (§5.3).
- `system.switch.inhibitors` and `preSwitchChecks` exist in 26.05. They are not used in the MVP
  but are noted for later.
- Samba 4.23.10 (`smbstatus --json`), smartmontools 7.5 (`smartctl -j`).
- `users.mutableUsers` is `true` on server roles. Only `impermanence.nix` (stateless) sets it `false`.

### 1.5 vexboard conventions reused

- Flake: `flake-utils` + `rust-overlay`. A custom `rustPlatform` with the `wasm32-unknown-unknown` target.
- `wasm-bindgen-cli` is pinned to the `Cargo.lock` version (consumers build against their own nixpkgs, so it is not taken from nixpkgs).
- `nix/package.nix` runs `trunk build --release` then `cargo build --release`.
- Outputs: `packages.<sys>.{vexboard,default}`, `overlays.default`, `nixosModules.{vexboard,default}`.
- vexos-nix input uses `inputs.nixpkgs.follows = "nixpkgs-unstable"` (documented exception).
- The vexos wrapper module `modules/server/vexboard.nix` maps `vexos.server.vexboard.*` onto
  `services.vexboard.*`, auto-generates the secret in an activation script, and registers backup
  paths. vexos-nix `flake.nix` has `vexboardBase = [ overlay, nixosModule ]`.
- Server stack: axum 0.8, tower-sessions with an SQLite store, `pam-sys` behind a `pam-auth`
  feature, login rate limiter, audit log, SSE (`axum::response::sse`), D-Bus with `zbus` for systemd.
- UI: Tailwind plus CSS custom-property tokens (`--color-bg-*`, `--color-accent`, …) with a
  `.light` override, and the Geist / DM Mono fonts.

What vexnas deliberately does **not** copy from vexboard:
- `auth.mode = "none"`.
- Bootstrap-admin-on-first-PAM-login.
- `allowed_origins = ["*"]`.
- No CSRF token (vexboard relies on SameSite only).
- `openFirewall = true` to everyone.
- Google-Fonts CDN (vexnas self-hosts fonts so a strict CSP is possible).

---

## 2. Goals and non-goals

**MVP goals**
1. **Dashboard:** pool health, capacity, SMART summary, NAS service status, last job results.
2. **Shares:** SMB/NFS CRUD, declarative, plus import from Cockpit's registry.
3. **Users/groups:** NAS-only Linux accounts and groups (declarative), plus SMB password set/clear (imperative).
4. **Jobs:**
   - nas-sync jobs: edit / run now / last result.
   - SnapRAID: sync/scrub schedule and run now.
   - Backup: schedule, retention, extra paths, run now.
5. **Apply flow:** diff → build in staging → preview (units that will restart) → activate → verify →
   auto-rollback on failure. Live log throughout.
6. **Disks/pools:** read-only views, plus a create-pool wizard that wraps the vexos scripts.

**Non-goals (MVP)**
- Pool expansion, disk replacement, `snapraid fix`, ZFS dataset management.
- Remote-storage attach, ACL editors, quotas, Time Machine UI.
- Multi-host management, notifications (vexos already routes failures to ntfy via `notify-failure@`).
- Editing anything outside `nas.nix`. `server-services.nix`, `storage-pool.nix`, and `zfs-pools.nix`
  are shown read-only. The pool wizard's scripts write the latter two themselves.

---

## 3. `/etc/nixos/nas.nix` schema

### 3.1 Invariants

1. **Pure data.** The file is `{ ... }: { … }` containing only attrsets, lists, strings, ints, and
   bools: no `lib`, `pkgs`, `mkForce`, interpolation, or imports. This makes it **round-trippable**:
   ```
   nix-instantiate --eval --strict --json -E 'import /etc/nixos/nas.nix { }'
   ```
   That command returns the exact model. **`nas.nix` is the single source of truth**, with no
   shadow state file. Hand edits are fine as long as they stay literal. If evaluation fails or
   non-literal values appear, vexnas shows the file as "not vexnas-readable" and goes read-only
   until it is fixed.
2. **Positive-only.** vexnas emits `enable = true` where needed and **never emits `false`**.
   This avoids type-conflict errors against `server-services.nix` (equal bools merge; unequal ones fail).
3. **Never self-harming.** The schema has no field that can disable vexnas, change its listener
   or firewall, change Cockpit/Samba/NFS **globals**, firewall scoping, discovery (Avahi/wsdd),
   or pool topology. A bad `nas.nix` cannot lock the operator out of the UI that fixes it.
4. **No free-form escape hatches.** No raw smb.conf parameters (`root preexec` would be root code
   execution), no `extraRsyncOptions` (`-e`/`--rsh`), no raw NFS option strings, no raw
   `timerConfig`. Every field is typed and allowlisted, enforced in three places:
   - the Rust model;
   - the helper's validator;
   - Nix assertions in the companion module, so hand edits get the same protection.
5. **Deterministic rendering.** Keys are sorted and formatting is stable, so diffs show only real changes.

### 3.2 Example (rendered output)

```nix
# /etc/nixos/nas.nix
# GENERATED by vexnas 0.1.0 — https://github.com/VictoryTek/vexnas
# Pure data: hand edits are allowed if they stay literal (no lib/pkgs/functions).
# Host-generated — do NOT commit to the vexos-nix repo.
{ ... }:
{
  vexos.server.nas = {
    # ── Shares (companion option C1) ─────────────────────────────────────
    shares = {
      media = {
        path = "/storage/media";
        comment = "Movies and TV";
        directory = { create = true; owner = "root"; group = "nas"; mode = "2775"; };
        smb = {
          enable = true;
          readOnly = false;
          browseable = true;
          guestOk = false;
          validUsers = [ "@nas" ];
          writeList = [ "alice" ];
        };
        nfs = {
          enable = true;
          fsid = 101;                       # auto-assigned once, then stable (required for FUSE/mergerfs)
          clients = [
            { host = "192.168.1.0/24"; access = "rw"; squash = "root"; sync = true; }
          ];
        };
      };
    };

    # ── NAS accounts (companion option C7) ───────────────────────────────
    groups = { nas = { }; media-rw = { gid = 2001; }; };
    users = {
      alice = { description = "Alice"; groups = [ "nas" "media-rw" ]; uid = 2001; };
    };
  };

  # ── Jobs ──────────────────────────────────────────────────────────────
  vexos.server.nasSync = {
    enable = true;                          # emitted only when jobs ≠ {}
    jobs = {
      tv = {
        source = "/goliath/data/media/tv/";
        destination = "/megatron/data/media/tv/";
        deleteOnDestination = true;
        mediaMounts = [ "/goliath" "/megatron" ];
        timerConfig = { OnCalendar = "daily"; Persistent = true; };
      };
    };
  };

  vexos.server.storage.snapraid = {
    syncInterval = "daily";
    scrubInterval = "weekly";
  };

  vexos.server.backup = {
    extraPaths = [ "/storage/documents" ];
    pruneOpts = [ "--keep-daily 7" "--keep-weekly 4" "--keep-monthly 6" ];
    timerConfig = { OnCalendar = "daily"; Persistent = true; };
  };
}
```

### 3.3 Field reference and validation

Notation: **R** = required; defaults shown. Every rule is enforced in the model, the helper, and a
Nix assertion.

**`vexos.server.nas.shares.<name>`**

| Field | Type / rule |
|---|---|
| `<name>` | `^[a-z][a-z0-9_-]{0,31}$`. Not `global`, `homes`, `printers`, `ipc$`. Must not collide with a registry share unless the share is being imported (§6.3). |
| `path` R | Absolute. No `..`, `//`, or trailing `/`. Must be under one of `vexos.server.nas.shareRoots` (default: mergerfs `mountPoint` if enabled, plus every `/<pool>` from `boot.zfs.extraPools`, plus `/srv`). Never `/`, `/etc`, `/nix`, `/boot`, `/root`, `/home`, `/var`, `/run`, `/proc`, `/sys`, `/dev`, or a prefix of them. |
| `comment` | ≤ 64 chars, printable, no `\n`. |
| `directory.create` | bool, default `false`. When true, renders a `systemd.tmpfiles` `d` rule (creates if missing; **adjusts mode/owner of the top directory only, never recursive**). |
| `directory.{owner,group,mode}` | Existing user/group names. Mode is `^[0-7]{3,4}$`. |
| `smb.enable` | bool, default `false`. |
| `smb.readOnly` / `browseable` / `guestOk` | bool; defaults `false` / `true` / `false`. `guestOk = true` requires `readOnly = true` in the MVP. |
| `smb.validUsers` / `smb.writeList` | List of `user` or `@group`, each existing (declared in `nas.users`/`nas.groups` or already present on the host). |
| `nfs.enable` | bool, default `false`. |
| `nfs.fsid` | int 1–65535, unique across shares. Auto-assigned on first save and never changed (FUSE filesystems need an explicit `fsid`). |
| `nfs.clients[].host` | IPv4/IPv6 CIDR, single IP, or hostname (`^[a-zA-Z0-9.-]+$`). `*` is **rejected**. |
| `nfs.clients[].access` | `"ro" \| "rw"`. |
| `nfs.clients[].squash` | `"root" \| "all" \| "none"`. `none` (= `no_root_squash`) needs an explicit UI acknowledgement and shows a warning badge. |
| `nfs.clients[].sync` | bool, default `true`. |
| (always) | Rendered with `no_subtree_check`. `sec=sys` (MVP). |

**`vexos.server.nas.groups.<name>`:** name `^[a-z_][a-z0-9_-]{0,31}$`; optional `gid` (≥ 1000).
Must not collide with a group defined anywhere else in the config (checked with
`definitionsWithLocations`, §6.2) or with a system group.

**`vexos.server.nas.users.<name>`**
- Same name rule; `description` ≤ 64 chars; `groups` must exist; optional `uid` (≥ 1000).
  Pin UIDs if NFS clients will map by UID.
- Must not collide with any user declared elsewhere (e.g. `nimda` from `modules/users.nix`) or
  with an existing `/etc/passwd` entry.
- These are **NAS-only accounts**: no login shell, no home. Existing human users still appear in
  the UI, where you can set their SMB password but not edit them.

**`vexos.server.nasSync.jobs.<name>`**
- Name follows the share rule.
- `source` / `destination`: absolute paths, not equal, neither a prefix of the other, not under the
  forbidden list above.
- `deleteOnDestination` (bool). `mediaMounts`: absolute paths.
- `timerConfig`: **only** `{ OnCalendar = <str>; Persistent = <bool>; }`. `OnCalendar` is validated
  with `systemd-analyze calendar` in the helper.
- `extraRsyncOptions` is never emitted.

**`vexos.server.storage.snapraid.{syncInterval,scrubInterval}`:** `OnCalendar` strings, validated as above.

**`vexos.server.backup`**
- vexnas emits only `extraPaths` (same path rules), `pruneOpts` (allowlist: `--keep-{last,hourly,daily,weekly,monthly,yearly} <n>`),
  and `timerConfig` (same restriction).
- `enable`, `repository`, `repositoryFile`, and `passwordFile` stay where they are today. They are
  shown read-only, with a "configure in `server-services.nix`" hint, because they need a secret.

### 3.4 What `nas.nix` never contains

`vexos.server.nas.{enable,backend}`, `vexos.server.cockpit.*`, `vexos.server.vexnas.*`,
`vexos.server.storage.mergerfs.*`, `snapraid.{enable,parityDisks,dataDisks}`, `boot.zfs.*`,
`networking.*`, `services.*`, `users.*` (users go through `vexos.server.nas.users`), and any secret.

---

## 4. Auth and privilege model

### 4.1 Processes

```
 browser ──HTTPS:7290──▶ vexnas-web   (User=vexnas, no capabilities, sandboxed)
                           │  • UI assets, REST, SSE, sessions (SQLite)
                           │  • reads: D-Bus systemd (units/timers), journal, statvfs, lsblk
                           │  • NEVER: root, /etc/shadow, writes outside /var/lib/vexnas
                           ▼
                 /run/vexnas/helper.sock  (0660 root:vexnas, SO_PEERCRED-checked)
                           ▼
                        vexnasd       (root, socket-activated, narrow typed verbs)
                           │  • validates + renders nas.nix (owns the renderer)
                           │  • PAM authentication
                           │  • smbpasswd / pdbedit, net conf (registry read + delete)
                           │  • privileged reads: smartctl -j, zpool -j, snapraid status, smbstatus --json
                           ▼  systemd-run (transient units, fixed ExecStart from the vexnas store path)
          vexnas-apply-<id>.service      → build / write / switch / verify / rollback
          vexnas-pool-<id>.service       → vexos create-*-pool.sh --non-interactive …
          (nixos-rebuild-switch-to-configuration.service — the switch step itself, §5.3)
```

**Why three layers:**
- The web process faces the network and parses untrusted input, so it gets no privileges.
- The helper is small, has a closed verb set, and stays sandboxed: it needs no network.
- Anything long-running or broadly privileged (Nix builds, activation, disk formatting) runs as a
  transient unit with a fixed program. That puts it outside both services' cgroups, so restarting
  vexnas never interrupts an apply or a pool creation.

### 4.2 Trust rule: the helper never accepts Nix

The web process sends a **JSON model** (§3). It never sends Nix text, shell fragments, unit names
outside an allowlist, or file paths outside the validated fields. `vexnasd`:
1. deserialises with `deny_unknown_fields`;
2. runs the validator from the shared `vexnas-model` crate;
3. renders Nix itself, escaping `\`, `"`, and `${`.

A fully compromised vexnas-web can therefore do only what the schema allows. That is still
significant (it could rw-share `/storage` to the LAN), but it cannot get root code execution
through Nix. Destructive verbs additionally require **re-authentication** (§4.4), which the helper
checks: the web passes a short-lived re-auth ticket that the helper itself minted.

### 4.3 Helper verb set (closed)

| Verb | Privilege | Notes |
|---|---|---|
| `auth.pam(user, password)` → `{ok, groups}` | root (reads shadow) | PAM service `vexnas`. The web process no longer needs the `shadow` group (vexboard does need it). |
| `auth.reauth(user, password)` → `ticket` | root | Ticket is HMAC-bound to user + session id, TTL 5 min. |
| `model.read()` → `{model, hash, readable}` | root (reads `/etc/nixos`) | Runs `nix-instantiate --eval --strict --json` on `nas.nix`. |
| `model.render(model)` → `{nix, hash, errors[]}` | none | Pure; also used for diff preview. |
| `apply.start(model, base_hash)` → `apply_id` | root | Fails if `nas.nix` hash ≠ `base_hash` (optimistic concurrency). Launches `vexnas-apply-<id>`. |
| `apply.activate(id, ticket)` / `apply.cancel(id)` / `apply.confirm(id)` / `apply.rollback(id, ticket)` | root | Signals the runner through its state dir. |
| `smb.set_password(user, password, ticket)` / `smb.disable(user, ticket)` / `smb.list()` | root | Password goes to `smbpasswd -s -a` on **stdin**, never argv. |
| `unit.start(unit)` | root | Allowlist: `nas-sync-*.service` (matching declared jobs), `snapraid-sync.service`, `snapraid-scrub.service`, `restic-backups-main.service`, `zfs-scrub-*.service`. |
| `read.smart(by_id)` | root | `smartctl -j -a -n standby` so **spun-down disks are not woken**. Cached 10 min. |
| `read.zpool()`, `read.snapraid_status()`, `read.smbstatus()`, `read.registry_shares()` | root | Cached; the snapraid one is slow, so it is refreshed on demand. |
| `pool.candidates(backend)` | root | Calls the script's `--list-candidates --json` (C6), so eligibility logic stays in one place. |
| `pool.create(spec, confirm_phrase, ticket)` → `op_id` | root | Re-checks candidates, then launches `vexnas-pool-<id>`. |
| `registry.delete_share(name, ticket)` | root | Only for a share whose declarative replacement is applied and verified (§6.3). |

### 4.4 Authentication and sessions

- **Login:** PAM (`security.pam.services.vexnas = {}`), called in the helper.
  - **Admin:** member of `services.vexnas.adminGroup` (default `"wheel"`).
  - **Viewer:** member of `viewerGroup` (default `null`, i.e. disabled), read-only.
  - **Anyone else:** rejected (403) and audited. There is **no bootstrap-first-login admin**.
- **Sessions:**
  - SQLite via tower-sessions; cookie `__Host-vexnas` (`Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`).
  - 30 min idle, 8 h absolute; id cycled at login.
  - Group membership is re-checked every 5 min and on every destructive call.
- **CSRF:**
  - Per-session synchronizer token returned by `GET /api/v1/auth/session`, required as `X-CSRF-Token` on every non-GET request.
  - `Origin` must equal the request's own origin.
  - Mutating endpoints accept only `application/json`.
- **Re-auth ("sudo mode"):** password re-entry within 5 min for:
  - activate, rollback;
  - pool create;
  - SMB password set/clear;
  - user delete;
  - registry share delete.
- **Rate limiting:** port vexboard's per-IP login limiter (10 per 60 s). The helper adds a global
  PAM back-off.
- **Audit:** every mutating request (user, IP, verb, model hash, apply id, never a password) goes to
  SQLite `audit` and the journal.

### 4.5 Network exposure

- **Bind:** HTTPS on `0.0.0.0:7290`; `services.vexnas.listenAddresses` can narrow it.
- **Firewall:**
  - `openFirewall = true` but scoped to `firewall.interfaces` when set (same pattern as cockpit.nix).
  - With no interfaces set, a warning is emitted, mirroring cockpit.nix.
- **App-level source allowlist**, defence in depth, checked at accept time before TLS:
  - loopback, RFC1918, `100.64.0.0/10` (Tailscale CGNAT), `fd7a:115c:a1e0::/48` (Tailscale ULA),
    `fc00::/7`, `fe80::/10`;
  - configurable via `allowedCidrs`;
  - other sources are closed without a response.
- **TLS:**
  - **Default:** an ECDSA P-256 self-signed cert generated on first start into `/var/lib/vexnas/tls/`
    (SANs: hostname, `hostname.local`, all local IPs), using `rcgen` + `axum-server` `bind_rustls`.
  - `tls.certFile` / `tls.keyFile` override it.
  - Hot-reloaded via `RustlsConfig::reload_from_pem_file`.
- **Headers:**
  - `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'`
  - plus HSTS (`max-age=31536000`, no preload), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`.
  - Fonts are self-hosted.

### 4.6 systemd hardening

**`vexnas.service` (web):**
- `User=vexnas`, `SupplementaryGroups=systemd-journal`, `NoNewPrivileges`, `CapabilityBoundingSet=`.
- `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `PrivateDevices`.
- `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`, `StateDirectory=vexnas`, `SystemCallFilter=@system-service`.
- `restartIfChanged = false` (§5.6).

**`vexnasd.socket` + `vexnasd.service` (helper):**
- Root. `ProtectHome`, `PrivateTmp`, `PrivateNetwork` (it needs none; the D-Bus system bus and its own socket are filesystem sockets).
- `ProtectSystem=strict` with `ReadWritePaths=/etc/nixos /var/lib/vexnas /var/lib/samba /run/vexnas`.
- `restartIfChanged = false`.

The transient units are root and unsandboxed: they run `nix`, `switch-to-configuration`, and
`mkfs`. They execute only fixed binaries from the vexnas store path with validated arguments.

---

## 5. Apply flow: state machine

### 5.1 States

```
                 ┌────────── edit ──────────┐
                 ▼                          │
  [Clean] ──edit──▶ [Draft] ──plan──▶ [Planned] ──build──▶ [Building] ──fail──▶ [BuildFailed]
                                         │                     │ ok                 (nas.nix untouched)
                                         │ conflict/            ▼
                                         │ invalid      [Previewed]  ← dry-activate output:
                                         ▼                │   │        units to restart/reload/start/stop,
                                   [Invalid]        cancel│   │activate  closure diff
                                   (shown inline)         ▼   ▼
                                                   [Draft] [Activating] ──(stc exit≠0 with NEW failed units)──┐
                                                               │ ok                                          │
                                                               ▼                                             │
                                                          [Verifying] ──health fail──────────────────────────┤
                                                               │ ok                                          ▼
                                                               ├─(confirm window on)─▶ [AwaitingConfirm] ─timeout─▶ [RollingBack]
                                                               │                         │ confirm              │ ok      │ fail
                                                               ▼                         ▼                      ▼         ▼
                                                          [Succeeded] ◀────────────────────            [RolledBack] [RollbackFailed]
                                                                                                                     (manual steps shown)
  On helper/web start: any apply whose state.json is in Building/Activating/Verifying/RollingBack
  with no live transient unit → [Interrupted] (report what step it died in; offer rollback if activated).
```

### 5.2 Steps (run by `vexnas-apply run <id>` inside `vexnas-apply-<id>.service`)

1. **Lock.** `flock /run/vexnas/apply.lock`. Abort with `Blocked` if `nixos-rebuild-switch-to-configuration.service`
   is active, or if the `nas.nix` hash ≠ the plan's `base_hash`.
2. **Snapshot.**
   - Record `prev_system = readlink -f /run/current-system`. This is not "profile N-1", which can
     differ after a `nixos-rebuild boot`.
   - Record `target = $(cat /etc/nixos/vexos-variant)`.
   - Record the set of currently failed units.
   - Copy `nas.nix` to `/var/lib/vexnas/applies/<id>/nas.nix.prev`, or record "absent".
3. **Stage.**
   - `rsync -a --delete /etc/nixos/ /run/vexnas/stage/<id>/` (dir 0700 root).
   - Write the candidate `nas.nix` into the stage.
   - The real `/etc/nixos` is **not touched yet**.
4. **Build.**
   ```
   nix build -L --impure --out-link /var/lib/vexnas/applies/<id>/result \
     "path:/run/vexnas/stage/<id>#nixosConfigurations.${target}.config.system.build.toplevel"
   ```
   The out-link is a GC root while pending. Failure → `BuildFailed`, stop. Nothing changed.
5. **Preview.**
   - `result/bin/switch-to-configuration dry-activate`: parse "would stop/restart/reload/start" lines.
   - `nix store diff-closures /run/current-system result`.
   - Flag in the UI when `samba-smbd.service` will restart: active SMB sessions will drop; the live
     count comes from `smbstatus --json`.
   - Wait for `activate` (re-auth) or `cancel`. Pending previews expire after 30 min.
6. **Write.**
   - Re-check the `nas.nix` hash against `base_hash`.
   - Atomically replace `/etc/nixos/nas.nix` (write `.vexnas-tmp`, `fsync`, `rename`).
7. **Activate.** Mirrors what `nixos-rebuild switch` does, using the **exact path that was built and previewed**:
   ```
   nix-env -p /nix/var/nix/profiles/system --set "$result"
   systemd-run --collect --no-ask-password --pipe --quiet --service-type=exec \
     --unit=nixos-rebuild-switch-to-configuration \
     "$result/bin/switch-to-configuration" switch
   ```
   The fixed unit name gives mutual exclusion with a concurrent `just rebuild`.
8. **Verify.** All of these must pass:
   - (a) `switch-to-configuration` exit 0, or exit ≠ 0 with **no newly failed units** compared with step 2;
   - (b) `samba-smbd`, `nfs-server` (if any NFS share), and every declared `nas-sync-*.timer` are active;
   - (c) `testparm -s` exits 0 and lists every declarative SMB share;
   - (d) `exportfs -s` contains every declarative NFS path with the expected client set;
   - (e) every share `path` exists;
   - (f) declared NAS users and groups exist in `getent`.
9. **Confirm window (optional, off by default, per-apply toggle).** Wait N seconds (default 120)
   for `apply.confirm`. This guards against changes that cut the operator off from their own shares.
10. **Finish.**
    - Write the final `state.json` and remove the stage dir.
    - Keep `result` until the next successful apply (the system profile also roots it).
    - Run post-apply side effects: for each user **removed** from `nas.users`, run `smbpasswd -x`.
    - If `vexnas.service`/`vexnasd.service` changed, `systemctl try-restart` them **last** (§5.6).

**Rollback** (automatic from Activating/Verifying/AwaitingConfirm, or manual via `apply.rollback`):
1. Restore `nas.nix.prev` atomically, or remove `nas.nix` if it was absent.
2. `nix-env -p /nix/var/nix/profiles/system --set "$prev_system"`.
3. The same `systemd-run … --unit=nixos-rebuild-switch-to-configuration "$prev_system/bin/switch-to-configuration" switch`.
4. Re-verify the units that were healthy before.

On failure, `RollbackFailed` shows the exact manual commands:
`sudo nix-env -p … --set <path> && sudo <path>/bin/switch-to-configuration switch`, plus the
`nas.nix.prev` location.

### 5.3 Why stage-build-then-activate instead of write-then-`nixos-rebuild switch`

- On an eval or build error, `/etc/nixos` was never modified, so there is nothing to restore.
  This is the common failure mode.
- What is activated is byte-for-byte what was previewed (dry-activate and closure diff), not a re-evaluation.
- It is the same sequence nixos-rebuild-ng performs internally (build → `nix-env --set` →
  `systemd-run … switch-to-configuration switch`), including the mutex unit name.
- **Equivalence check (Phase 1 acceptance test):**
  - `nix eval --raw path:/run/vexnas/stage/<id>#…toplevel.drvPath` must equal the same eval against
    `path:/etc/nixos` after the write.
  - If it differs on a real host (some module references the flake source path), fall back to
    building from `/etc/nixos` after the write, with file-restore on build failure.

### 5.4 SMB session impact

Because of `restartTriggers = [ configFile ]` (§1.4), any declarative share change restarts smbd.
- **MVP:** the preview step shows it explicitly, with live session count.
- **Opt-in fix (C1.4):** after real-host testing, switch smbd to reload-on-config-change.

### 5.5 Observability

- The live log is the transient unit's journal, streamed by vexnas-web
  (`journalctl -u vexnas-apply-<id> -o json -f`) over SSE. The runner prefixes structured step
  markers (`::step build`, `::step verify ok`).
- `state.json` (mode 0640 root:vexnas) is the authoritative state.
- On reconnect, the UI reloads `state.json` and replays the journal from the cursor.

### 5.6 Not killing itself

- `restartIfChanged = false` on both vexnas units, so `switch-to-configuration` leaves them running
  and the SSE stream survives.
- The apply itself runs in a transient unit, so even a crash or restart of vexnas cannot interrupt it.
- New vexnas versions take effect through the runner's final `try-restart`, after the state is persisted.

### 5.7 Concurrency and drift

- **One apply at a time:** flock, plus the fixed switch unit name.
- **Single shared draft:** `/var/lib/vexnas/draft.json` with `base_hash`, the last editor, and a timestamp.
- **Drift:** if `nas.nix` changes outside vexnas (hand edit, git checkout), the next `model.read`
  shows a "changed on disk" banner. The draft must be rebased (re-read and re-apply edits), or discarded.
- **`/etc/nixos/flake.nix` predates nas.nix support:** detected by grepping it for `nas.nix`.
  Apply is blocked, with instructions to refresh the template (R6).

---

## 6. Companion option designs (vexos-nix): spec only

> **Rule: this repo never changes code in other repos.** Every change below is delivered as a
> self-contained prompt in [`docs/vexos-nix-prompts/`](vexos-nix-prompts/README.md), which the
> operator pastes into a Claude session opened in `~/Projects/vexos-nix`. The sections here are the
> design; the prompts are the work orders. If they disagree, fix the prompt.

Each item becomes its own vexos-nix spec/PR under that repo's workflow. All follow Option B, with
`lib.mkIf` only on the module's own options (the carve-out).

### C1. Declarative shares: `modules/server/nas-shares.nix` (new, imported via `modules/server/default.nix`)

**C1.1 Options:** `vexos.server.nas.shares` (attrsOf submodule, §3.3) and `vexos.server.nas.shareRoots`
(listOf str). The default is derived as in §3.3, with `defaultText`.

**C1.2 Rendering** (active when `shares != {}`):

```nix
services.samba.settings = lib.mapAttrs (name: s: {
  path = s.path;
  comment = s.comment;
  "read only" = yesNo s.smb.readOnly;
  browseable = yesNo s.smb.browseable;
  "guest ok" = yesNo s.smb.guestOk;
} // lib.optionalAttrs (s.smb.validUsers != [ ]) { "valid users" = s.smb.validUsers; }
  // lib.optionalAttrs (s.smb.writeList != [ ]) { "write list" = s.smb.writeList; })
  (lib.filterAttrs (_: s: s.smb.enable) cfg.shares);

services.nfs.server.exports = lib.concatMapStrings (s:
  "${lib.escapeShellArg s.path} ${lib.concatMapStringsSep " " (c:
    "${c.host}(${lib.concatStringsSep "," ([ c.access (if c.sync then "sync" else "async")
       "no_subtree_check" "fsid=${toString s.nfs.fsid}" ] ++ squashOpts c.squash)})") s.nfs.clients}\n")
  (lib.filter (s: s.nfs.enable) (lib.attrValues cfg.shares));

systemd.tmpfiles.settings."10-vexos-nas-shares" = /* d rules for directory.create = true */;
```

- `services.samba.settings.global` is **not touched**, so `include = registry`, `hosts allow`,
  `bind interfaces only`, and the discovery stack stay exactly as they are.
- Registry shares keep working, which is **D3's coexistence requirement**.
- `/etc/exports.d/cockpit-file-sharing.exports` keeps working.
- Note: `exports(5)` quoting uses double quotes. The implementation must use a quote helper, not
  `escapeShellArg`; this is pinned down in the C1 spec.

**C1.3 Assertions:**
- Every §3.3 rule.
- `shares != {} → cockpit.fileSharing.enable`: Phase 1 reuses the existing Samba/NFS server
  enablement and firewall rules. C8 removes this dependency.
- No two NFS shares with the same `path` or `fsid`.
- `nfs.clients != []` when `nfs.enable`.

**C1.4 (later, opt-in, test on a real host first): reload instead of restart.**
`vexos.server.nas.smbReloadOnShareChange` (default `false`):
```nix
systemd.services.samba-smbd = {
  restartTriggers = lib.mkForce [ ];
  reloadTriggers = [ config.environment.etc."samba/smb.conf".source ];
};
```
- Package upgrades still restart smbd, because `ExecStart` changes.
- `ExecReload` is already `kill -HUP $MAINPID` upstream.
- Verify on host: share add/remove takes effect on reload with no session drops; `testparm` stays clean.

### C2. SnapRAID passthroughs (small, optional): `modules/server/snapraid.nix`

Add `scrubPlan` (int %, default upstream 8), `scrubOlderThan` (int days, default upstream 10), and
`touchBeforeSync` (bool), each passed to `services.snapraid.*`. vexnas exposes them in a later phase.

### C3. `nas.nix` import

- **`template/etc-nixos-flake.nix`:** add `nasFile = ./nas.nix; hasNas = builtins.pathExists nasFile;`
  next to `zfsPoolsFile`. Append `++ lib.optional hasNas nasFile` in **`mkServerVariant` and
  `mkHeadlessServerVariant` only**.
- **`flake.nix`:** add
  ```nix
  nasModule = let p = /etc/nixos/nas.nix; in if builtins.pathExists p then [ p ] else [];
  ```
  and append it to `roles.server.hostLocalModules` and `roles.headless-server.hostLocalModules`,
  with a comment matching `storagePoolModule`.
- **`template/server-services.nix`:** add a comment noting `nas.nix` is owned by vexnas.

### C4. vexnas input + wrapper module (modelled on vexboard)

**`flake.nix` input:**
```nix
# vexnas: VexOS NAS management UI (Rust + WASM). Used by modules/server/vexnas.nix.
# Follows nixpkgs-unstable for the same reason as vexboard (rust-overlay toolchain).
vexnas = { url = "github:VictoryTek/vexnas"; inputs.nixpkgs.follows = "nixpkgs-unstable"; };
```

Add `vexnasBase = [ { nixpkgs.overlays = [ inputs.vexnas.overlays.default ]; } inputs.vexnas.nixosModules.default ];`
to the server and headless-server `baseModules`. Add the exception note to vexos-nix `CLAUDE.md`.

**`modules/server/vexnas.nix`** (imported by `modules/server/default.nix`):

```nix
options.vexos.server.vexnas = {
  enable = lib.mkEnableOption "vexnas NAS management UI";
  port = lib.mkOption { type = lib.types.port; default = 7290; };
  adminGroup = lib.mkOption { type = lib.types.str; default = "wheel"; };
  firewall.interfaces = lib.mkOption { type = with lib.types; listOf str; default = [ ]; };
  allowedCidrs = lib.mkOption { type = with lib.types; listOf str; default = /* §4.5 list */; };
};
config = lib.mkIf cfg.enable {
  vexos.server.backup.servicePaths.vexnas = [ "/var/lib/vexnas" ];
  services.vexnas = {
    enable = true;
    inherit (cfg) port adminGroup allowedCidrs;
    firewall.interfaces = cfg.firewall.interfaces;
    flakeDir = "/etc/nixos";
    poolScripts = pkgs.vexos.storage-scripts;   # C6
  };
};
```

Also:
- Add `vexnas` to `_server_service_names` in the justfile, so `just enable vexnas` writes it to
  `server-services.nix`. It is deliberately **not** in `nas.nix` (invariant 3).
- Optionally add a `just vexnas-url` helper.

### C5. Headless discovery gap (observation, optional)

Headless-server has no wsdd or Avahi publish, so a headless NAS will not appear in Windows or
Nautilus network views. This is a separate, optional vexos-nix change: a server-side
`network-nas-discovery.nix`, responder-only wsdd plus an Avahi `_smb._tcp` record. It is
**not** required by vexnas, and per the caution it must not modify `network-desktop.nix`.

### C6. Non-interactive pool scripts + packaged scripts

**Both scripts** gain:
- `--list-candidates --json`: prints the eligible disks the script would offer, then exits.
  Output: `[{by_id, dev, size_bytes, model, tran, rotational, in_use_reason?}]`.
- `--non-interactive` with explicit flags. Every prompt maps to a flag, and **disks are passed as
  `/dev/disk/by-id/<id>` names, never indices**:
  - **zfs:** `--pool NAME --topology single|mirror|raidz1|raidz2|raidz3|raid10 --disk ID… [--vm-dataset|--no-vm-dataset] [--proxmox-id ID|--no-proxmox] --confirm NAME`
  - **mergerfs:** `--fs ext4|xfs --content ID… [--parity ID…] --confirm storage`
- In non-interactive mode, each requested by-id must be in the script's own freshly computed
  candidate list, or the script dies before step 6. Same validation path as interactive mode.
- Machine-readable progress markers on stdout (`::step 7/9`), alongside the existing human output.

**mergerfs script:** in non-interactive mode, **refuse if `/etc/nixos/storage-pool.nix` exists**
(MVP is create-only). This removes the "re-run regenerates the file" hazard (R4) for the UI path.
Interactive behaviour is unchanged.

**Packaging:** `pkgs/vexos/storage-scripts` is a `stdenvNoCC` derivation that installs both scripts
with `makeWrapper` PATH (`zfs`, `gptfdisk`, `util-linux`, `e2fsprogs`, `xfsprogs`, `mergerfs`,
`coreutils`, `gawk`, `gnugrep`, `gnused`). vexnas calls the store path and never walks the
filesystem for scripts.

### C7. NAS accounts: `vexos.server.nas.users` / `vexos.server.nas.groups` (in C1's module)

```nix
users.groups = lib.mapAttrs (_: g: lib.optionalAttrs (g.gid != null) { inherit (g) gid; }) cfg.groups;
users.users = lib.mapAttrs (name: u: {
  isNormalUser = true;            # uid ≥ 1000 range — sane for NFS uid mapping
  createHome = false;
  home = "/var/empty";
  shell = "${pkgs.shadow}/bin/nologin";
  description = u.description;
  extraGroups = u.groups;
} // lib.optionalAttrs (u.uid != null) { inherit (u) uid; }) cfg.users;
```

- **Assertion:** `users.mutableUsers` must be `true` when `users != {}` (SMB passwords live in
  passdb, but Linux passwords for these accounts are deliberately unset/locked).
- **Verify in the C7 spec:** that NixOS deletes a previously declarative user when it is removed
  from config on this nixpkgs (`update-users-groups.pl` vs userborn). vexnas also runs
  `smbpasswd -x` on removal (§5.2 step 10) either way.

### C8. (Gated, D3) Retire registry mode

Only after a real host has run declarative shares only, with registry empty, Cockpit fileSharing
disabled, and SMB discovery verified from Windows, macOS, and GNOME. Then:
1. Extract the Samba/NFS server + firewall block from `cockpit.nix` into `modules/server/nas-sharing.nix`
   (`vexos.server.nas.sharing.enable`), with `cockpit.fileSharing.enable` implying it during transition.
2. Drop the C1.3 dependency on Cockpit.
3. Remove `include = registry` behind a flag, with the default flipped one release later.

### Ownership detection (vexnas side, no vexos change needed)

To show which file defines what, the apply runner (and a cached helper read) evaluates
`options.<path>.definitionsWithLocations` (mapped to `{file, keys}` only) for:
- `vexos.server.nasSync.jobs`, `vexos.server.storage.snapraid.{syncInterval,scrubInterval}`;
- `vexos.server.backup.*`, `users.users`, `users.groups`;
- `services.samba.settings` (share names).

Anything defined outside `nas.nix` is shown read-only with its file, for example
"nas-sync job `tv` is defined in `/etc/nixos/server-services.nix`; move it to manage it here".
Results are cached by hash of `/etc/nixos/*`, because each eval takes about 30–60 s.

---

## 7. Backend API surface (vexnas-web, `/api/v1`, JSON)

Conventions:
- Every non-GET request needs a session, CSRF, `application/json`, and the admin role.
- **(R)** marks endpoints that need a fresh re-auth ticket.
- Long operations return `{op_id}` and stream from `GET /ops/{id}/events` (SSE).

**Auth**

| Method | Path | Notes |
|---|---|---|
| POST | `/auth/login` | `{user,password}` → session cookie |
| POST | `/auth/logout` | |
| GET | `/auth/session` | `{user, role, csrf_token, reauth_valid_until}` |
| POST | `/auth/reauth` | `{password}` → refreshes re-auth window |

**Meta and dashboard**

| Method | Path | Notes |
|---|---|---|
| GET | `/meta` | version, hostname, variant, vexos-nix rev (from flake.lock), capabilities (zfs, mergerfs, snapraid, backup configured, cockpit fileSharing, nas.nix import supported) |
| GET | `/dashboard` | pools summary, capacity, SMART summary, NAS services, last results of jobs, active alerts |
| GET | `/dashboard/events` | SSE: pushes dashboard deltas (unit state changes via D-Bus signals, capacity every 60 s) |

**Storage (read-only plus wizard)**

| Method | Path | Notes |
|---|---|---|
| GET | `/storage/disks` | lsblk -J + by-id + pool membership + mount + cached SMART health |
| GET | `/storage/disks/{by_id}/smart` | full smartctl JSON (cached; `?refresh=1` honours standby) |
| GET | `/storage/pools` | ZFS (`zpool list/status -j`), mergerfs (branches + statvfs), SnapRAID (last sync/scrub, parity disks) |
| GET | `/storage/pools/{name}` | detail |
| POST | `/storage/snapraid/status` | runs `snapraid status` (slow) → op_id |
| GET | `/storage/pool-candidates?backend=zfs\|mergerfs` | from C6 `--list-candidates` |
| POST | `/storage/pools/plan` | validate spec → `{summary, destroyed_disks[], confirm_phrase}` |
| POST | `/storage/pools` **(R)** | `{spec, confirm_phrase}` → op_id; on success the UI offers Apply (storage-pool.nix / zfs-pools.nix changed) |

**Configuration model (draft)**

| Method | Path | Notes |
|---|---|---|
| GET | `/config` | `{model, nas_nix_hash, readable, ownership, external_definitions, drift}` |
| GET / PUT / DELETE | `/config/draft` | whole draft model `{base_hash, model}` |
| PUT / DELETE | `/config/draft/shares/{name}` | per-resource convenience |
| PUT / DELETE | `/config/draft/users/{name}` | |
| PUT / DELETE | `/config/draft/groups/{name}` | |
| PUT / DELETE | `/config/draft/jobs/sync/{name}` | |
| PUT | `/config/draft/jobs/snapraid` | |
| PUT | `/config/draft/jobs/backup` | |
| POST | `/config/draft/rebase` | re-read nas.nix, re-apply draft edits, report conflicts |

**Apply**

| Method | Path | Notes |
|---|---|---|
| POST | `/apply/plan` | render + validate + unified diff + structured summary + warnings (SMB restart, no_root_squash, …) |
| POST | `/apply` | `{base_hash, draft_hash, confirm_window_secs?}` → apply_id (starts build) |
| GET | `/apply/{id}` | state.json |
| GET | `/apply/{id}/events` | SSE: log lines + step transitions |
| POST | `/apply/{id}/activate` **(R)** | |
| POST | `/apply/{id}/cancel` | before activation only |
| POST | `/apply/{id}/confirm` | |
| POST | `/apply/{id}/rollback` **(R)** | |
| GET | `/apply/history` | last 50 applies |

**Shares (live and registry)**

| Method | Path | Notes |
|---|---|---|
| GET | `/shares/live` | `testparm -s` share list + `exportfs -s` + `smbstatus --json` sessions |
| GET | `/shares/registry` | Cockpit registry shares (`net conf list`) |
| POST | `/shares/registry/{name}/import` | copies into draft as a declarative share (marked `import_of_registry`) |
| DELETE | `/shares/registry/{name}` **(R)** | only after the imported declarative share is applied and verified |

**Users (imperative parts)**

| Method | Path | Notes |
|---|---|---|
| GET | `/users` | declarative NAS users + other host users (uid ≥ 1000, read-only) + SMB passdb status (`pdbedit -L`) |
| PUT | `/users/{name}/smb-password` **(R)** | `{password}` |
| DELETE | `/users/{name}/smb-password` **(R)** | |

**Jobs (run now / status)**

| Method | Path | Notes |
|---|---|---|
| GET | `/jobs` | all allowlisted units: timer next/last, `Result`, `ExecMainStatus`, duration, defining file |
| POST | `/jobs/{unit}/run` | allowlisted → helper `unit.start` |
| GET | `/jobs/{unit}/log?lines=200` | |
| GET | `/jobs/{unit}/events` | SSE follow |

**Audit**

| Method | Path |
|---|---|
| GET | `/audit?limit=&offset=` |

OpenAPI is generated with `utoipa`, as in vexboard, but the Swagger UI is **dev-only**
(cargo feature), not shipped in the release build.

---

## 8. Repo skeleton and flake outputs

```
vexnas/
├── flake.nix                 # flake-utils + rust-overlay (mirrors vexboard)
├── flake.lock
├── Cargo.toml                # workspace
├── crates/
│   ├── vexnas-model/         # no_std-friendly serde types, validator, Nix renderer, diff.
│   │                         #   Compiles to native AND wasm32 — one source of truth for UI preview and helper.
│   ├── vexnas-web/           # axum 0.8 + axum-server (rustls) + tower-sessions(SQLite) + zbus + SSE
│   ├── vexnasd/              # root helper: unix-socket JSON protocol, PAM, verbs (§4.3)
│   ├── vexnas-apply/         # apply runner binary (§5.2) + pool-op runner
│   ├── vexnas-proto/         # helper request/response enums (shared by web + helper)
│   └── vexnas-frontend/      # Leptos 0.8 CSR + leptos_router, Trunk, Tailwind (vexboard token set)
│       ├── index.html  Trunk.toml  style/main.css  public/fonts/
│       └── src/{pages/{dashboard,shares,users,jobs,storage,apply,audit},components/}
├── nix/
│   ├── package.nix           # trunk build → cargo build (all bins) → $out/{bin,share/vexnas/assets}
│   ├── module.nix            # services.vexnas (generic NixOS module, not vexos-specific)
│   └── tests/                # NixOS VM tests (Phase 1+): apply success, build failure, activation rollback
├── config/default.toml
├── docs/spec.md              # this file
├── scripts/preflight.sh      # cargo fmt --check, clippy -D warnings, cargo test, nix build .#vexnas
├── .github/workflows/ci.yml
├── CLAUDE.md                 # adapted from vexboard/vexos-nix workflow (Phase 0)
└── README.md
```

### Flake outputs (required by the kickoff)

```nix
outputs = { self, nixpkgs, flake-utils, rust-overlay }:
  flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system: {
    packages.vexnas  = pkgs.callPackage ./nix/package.nix { … };
    packages.default = self.packages.${system}.vexnas;
    devShells.default = …;                        # rust toolchain, trunk, wasm-bindgen-cli (pinned), tailwind
    checks.vm-apply  = import ./nix/tests/apply.nix { … };   # vexnas-local only; vexos-nix never runs it
  }) // {
    overlays.default = final: prev: {
      vexnas = self.packages.${prev.stdenv.hostPlatform.system}.vexnas;
    };
    nixosModules.vexnas  = ./nix/module.nix;
    nixosModules.default = self.nixosModules.vexnas;
  };
```

Linux-only (PAM, systemd), so it uses `eachSystem` with Linux systems, not `eachDefaultSystem`.

### `services.vexnas` module options (generic)

| Option | Default |
|---|---|
| `enable`, `package` | `pkgs.vexnas` |
| `port` | `7290` |
| `listenAddresses` | `[ "0.0.0.0" "::" ]` (one socket each; v6 sockets are v6-only) |
| `openFirewall` | `true` |
| `firewall.interfaces` | `[]` (warning when empty) |
| `allowedCidrs` | §4.5 |
| `adminGroup` | `"wheel"` |
| `viewerGroup` | `null` |
| `tls.certFile` / `tls.keyFile` | `null` → self-signed |
| `flakeDir`, `nasFile`, `poolScripts` | added with the phase that first uses them (2, 2, 5) |
| `settings` | TOML passthrough, as vexboard |

The module declares:
- `users.users.vexnas` / `users.groups.vexnas` and `security.pam.services.vexnas = {}`;
- `vexnas.service`, `vexnasd.socket` + `vexnasd.service`, and tmpfiles for `/run/vexnas`;
- runtime tools on the helper's PATH: `nix`, `samba`, `smartmontools`, `zfs` (when `boot.zfs` is
  enabled), `snapraid` (if enabled), `util-linux`, `rsync`.

---

## 9. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Root-capable web app.** RCE in vexnas-web means NAS reconfiguration. | Three-layer split (§4.1); helper accepts JSON only and renders Nix itself (§4.2); no free-form fields; re-auth for destructive verbs; source-IP allowlist + TLS + strict CSP; no shadow access in web; audit log. |
| R2 | **Share edits restart smbd** and drop transfers. | Preview names the restart with live session count (§5.2.5); opt-in reload mode C1.4 after host testing. |
| R3 | **SMB discovery regression** (hard-won). | C1 never touches `settings.global`, Avahi, wsdd, or `network-desktop.nix`; registry stays (D3 gate); C8 only after real-host proof; verification checklist includes Windows, macOS, and GNOME discovery. |
| R4 | **Pool scripts destroy the wrong disk.** | No PTY driving: C6 by-id flags, validated against the script's own fresh candidate list; plan endpoint shows exact disks; typed phrase + re-auth; mergerfs UI path is create-only. |
| R5 | **Name collision between declarative and registry shares** silently shadows the registry share (smb.conf wins). | Validator rejects collisions; the import flow is the only exception, and it is explicit. |
| R6 | **Existing hosts' `/etc/nixos/flake.nix` predates `nas.nix` support**, so a written `nas.nix` would be silently ignored. | Detect (§5.7) and block Apply with refresh instructions; verify step (c)/(d) would also catch it. |
| R7 | **Option conflicts** with definitions in `server-services.nix` (e.g. the same nas-sync job in two files). | Positive-only emission; ownership detection shows external definitions read-only; conflicts surface at build (step 4) with `/etc/nixos` untouched. |
| R8 | **Pre-existing failed units** cause false "activation failed" and needless rollbacks. | Compare failed-unit sets before and after; only *new* failures count. |
| R9 | **Interrupted apply** (power loss, reboot mid-switch). | `state.json` per step; `Interrupted` reconciliation on start; system profile + `nas.nix.prev` allow manual or automatic rollback; boot loader already points to whichever generation was last `--set`. |
| R10 | **Concurrent manual `just rebuild`.** | Same `nixos-rebuild-switch-to-configuration` unit name gives mutual exclusion at the switch; build-time overlap is possible but harmless (activation is serialized); `base_hash` check before write. |
| R11 | **Staged build ≠ /etc/nixos build.** | drvPath equivalence acceptance test (§5.3); documented fallback. |
| R12 | **Spinning up idle disks** for SMART polling. | `smartctl -n standby`; 10-min cache; SMART only on demand for sleeping disks. |
| R13 | **NFS over mergerfs (FUSE)** needs `fsid`, and mergerfs has NFS-specific guidance (inode calc/`noforget`). | Renderer always sets a stable `fsid`. The C1 spec must check current mergerfs docs for NFS-export options against vexos's current `use_ino`-based option list. |
| R14 | **Secrets in `/etc/nixos` get copied to the store** (observation: `path:/etc/nixos` flakes copy the whole directory). | Pre-existing, not introduced by vexnas. vexnas writes no secrets to `/etc/nixos`, and the stage dir is root-only and deleted after build. Flagged for a separate vexos-nix look. |
| R15 | **Build time and memory** of a full system eval on a small NAS. | Same cost as `just rebuild`; progress streamed; cancel allowed before activation. |
| R16 | **User removal semantics** (does NixOS delete removed declarative users on this pin?). | Verify in the C7 spec; vexnas runs `smbpasswd -x` regardless; deletion needs re-auth. |
| R17 | **vexnas upgrade lands mid-apply** (`restartIfChanged=false`). | Runner restarts both units last, after the state is persisted. |

---

## 10. Phased build plan

Each phase ends with:
- vexnas `scripts/preflight.sh` green (`cargo fmt`, `clippy -D warnings`, `cargo test`, `nix build .#vexnas`, VM tests from Phase 2 on);
- commit messages supplied for you to run.

vexos-nix items are delivered as prompts (see §6) and follow that repo's own 7-phase workflow. That means **no `nix flake check`**, and
the agent never runs `nixos-rebuild switch`/`boot`; you run real-host steps.

| Phase | vexnas deliverables | vexos-nix companion | Exit criteria |
|---|---|---|---|
| **0. Skeleton** | Workspace, flake (packages/overlay/module outputs), Trunk + Tailwind shell with login page, CI, preflight, CLAUDE.md, README. `services.vexnas` module with web + helper units, self-signed TLS, PAM login via helper, sessions, CSRF, audit. | none | `nix build .#vexnas` works; VM test: login as wheel user OK, non-wheel rejected, CSRF enforced, out-of-allowlist source refused. |
| **1. Read-only dashboard** | Dashboard, disks, pools (ZFS JSON, mergerfs, SnapRAID), SMART (standby-aware), service and job status, live SSE. | **C4** (input + wrapper + `just enable vexnas`) | Runs on your server alongside Cockpit; numbers match `zpool status`, `df`, `smartctl`. |
| **2. Model + apply engine** | `vexnas-model` (schema, validator, renderer, round-trip), helper `model.*`/`apply.*`, `vexnas-apply` runner with the full §5 state machine, apply UI with diff, preview, live log, rollback. First managed fields: **SnapRAID schedule + backup schedule/retention/extraPaths** (no new vexos options needed). | **C3** (`nas.nix` import) | VM tests: success, eval error (`/etc/nixos` untouched), activation failure → auto-rollback, interrupted → reconcile. drvPath equivalence verified on the real host. |
| **3. Shares** | Shares CRUD, registry list + import + delete, preview SMB-restart warning, live `testparm`/`exportfs`/`smbstatus`. | **C1** (shares, assertions, coexistence with registry) | Real host: declarative share works over SMB and NFS **and** existing registry shares + discovery still work (Windows, macOS, GNOME). |
| **4. Users and jobs** | NAS users/groups, SMB password set/clear, nas-sync job CRUD + run now, ownership display for externally defined jobs. | **C7** (users/groups) | Real host: new NAS user can authenticate over SMB; nas-sync run now + last result correct. |
| **5. Pool wizard** | Candidates → plan → typed confirm + re-auth → transient op with live log → chain into Apply. | **C6** (non-interactive scripts + `pkgs.vexos.storage-scripts`) | VM test with virtual disks for both backends; wrong-disk attempts rejected by the script itself. |
| **6. Hardening and gated migration** | Confirm-window UX, viewer role, polish, docs. | **C1.4** (smbd reload, opt-in), **C2**, then the **C8** gate (registry retirement) when you choose to | Your sign-off after a real host runs declarative-only. |

---

## 11. Open items to confirm during implementation (not blocking approval)

1. Exact `exports(5)` quoting helper (C1.2).
2. Removed-user semantics on this nixpkgs (C7 / R16).
3. mergerfs NFS-export options (R13).
4. Whether `/etc/nixos` on your hosts is a git repo. This affects nothing if `path:` is always used,
   but the justfile also references `git+file:///etc/nixos`.
5. Phase 0 must confirm `axum::serve` over `tokio::net::UnixListener` for the helper. If the API is
   unsuitable, use a plain length-delimited JSON protocol over `UnixStream` (preferred anyway: no
   HTTP stack in the root process).

---

## 12. Deviations and findings recorded during implementation

Kept here so the spec stays honest about what was built. Update when a later phase changes them.

**Phase 0 (skeleton) — 2026-10-03**

| # | Spec said | Built | Why |
|---|---|---|---|
| 1 | Tailwind styling (D2, §1.5, §8) | Plain CSS using vexboard's `--color-*` tokens, light/dark via `prefers-color-scheme` | Tailwind needs a download/config step inside the Nix sandbox; the CSP (`style-src 'self'`) also forbids the inline `style=` attributes vexboard relies on. Same look, simpler build. |
| 2 | Web unit has `SupplementaryGroups=systemd-journal` (§4.6) | Not granted yet | Nothing reads the journal until Phase 2 (apply log streaming). Grant when first needed. |
| 3 | `services.vexnas` has `dataDir`, `flakeDir`, `nasFile`, `poolScripts` (§8) | Only options with a consumer: `port`, `listenAddresses`, `openFirewall`, `firewall.interfaces`, `allowedCidrs`, `adminGroup`, `viewerGroup`, `tls.*`, `settings`, `package` | `StateDirectory` fixes the data dir at `/var/lib/vexnas`; the others are added by phases 2 and 5. |
| 4 | `POST /auth/reauth` in Phase 0 API (§7) | Deferred to Phase 2 | Re-auth tickets are minted by the helper and first needed by the apply engine. |
| 5 | Helper `CapabilityBoundingSet` unspecified | `CAP_DAC_OVERRIDE, CAP_DAC_READ_SEARCH, CAP_SETUID, CAP_SETGID, CAP_AUDIT_WRITE` | Enough for PAM (`pam_unix` reads shadow); verified by the VM test. Revisit as verbs are added. |
| 6 | Helper `PrivateNetwork` (§4.6) | Kept. **Caveat:** NSS/PAM backends that need the network (LDAP without a local daemon) will not work from the helper. | sssd/nscd use unix sockets and are fine. |
| 7 | `restartIfChanged=false` (§5.6) | Kept, with a documented manual restart until the apply engine restarts the units itself | A rebuild that changes vexnas needs `systemctl restart vexnasd.socket vexnasd vexnas`. |
| 8 | `wasm-bindgen-cli` pinned (flake) | 0.2.129, matching `Cargo.lock` | crates.io's `web-sys` already required ≥0.2.129; bump the pin when `Cargo.lock` moves. |

**Findings**
- **HTTP/2 has no `Host` header.** The origin check initially read `Host`; browsers negotiate h2 over TLS, so login
  would have failed for every browser while all in-process tests passed. Found by the NixOS VM test (real TLS).
  Now uses the URI authority, with a regression test.
- **serde ignores `deny_unknown_fields` on unit variants of internally tagged enums**, so `{"verb":"ping","extra":1}`
  was accepted. `Ping` is now an empty struct variant; covered by a test.
- **Trunk injects an inline `<script type="module">`**, confirming the CSP must allow it by hash (computed at
  startup from the real `index.html`) rather than via `unsafe-inline`.
- **Viewers could not log out** under a blanket "writes need admin" rule. Split into `require_session`
  (any role) and `require_admin_for_writes` (data routes only).
- Flakes in a git repo only see tracked files and this workflow never runs `git add`, so builds use `path:.`
  with cargo's `target/` kept outside the repo.
