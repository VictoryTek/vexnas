# Prompt C6 — non-interactive pool scripts and packaged storage scripts

Paste everything below the line into Claude in `~/Projects/vexos-nix`.

---

Task: make `scripts/create-zfs-pool.sh` and `scripts/create-mergerfs-pool.sh` drivable by a program
(the vexnas web UI) **without** changing their interactive behaviour, and package them. Follow your
CLAUDE.md workflow. Do not run the scripts against real disks; review and test with `bash -n`,
shellcheck if available, and a stubbed environment (fake `lsblk`/`zpool`/etc. on PATH) at most.

## Why
Both scripts pick disks by **on-screen index** into a list built from `ls /dev/disk/by-id`. Feeding the
prompts from a pipe/PTY is unsafe for a destructive operation (index order is not stable identity).
The UI must instead pass explicit by-id names, which the script re-validates itself.

## Requirements (both scripts)
1. `--list-candidates --json`: print the eligible disks exactly as the script's own enumeration would
   offer them (same OS-disk exclusion; mergerfs script also excludes zfs_member disks), then exit 0
   without prompting or modifying anything. Output: JSON array of
   `{"by_id","dev","size_bytes","model","tran","rotational"}` (size in bytes from `lsblk -b`). Does not
   require root if lsblk works without it; do not wake disks beyond what lsblk does.
2. `--non-interactive` plus flags covering every prompt. Disks are `/dev/disk/by-id/<id>` names or bare
   by-id strings — never indices. In this mode every requested disk MUST appear in the script's own
   freshly computed candidate list, otherwise the script exits non-zero **before** the destructive step.
   Destructive-step confirmation is supplied by flag and must match the same phrase the interactive
   prompt requires (pool name for zfs; the word `storage` for mergerfs); mismatch aborts with no changes.
   - zfs: `--pool NAME --topology single|mirror|raidz1|raidz2|raidz3|raid10 --disk ID [--disk ID …]
     (--vm-dataset|--no-vm-dataset) (--proxmox-id ID|--no-proxmox) --confirm NAME`. Keep all existing
     pool-name validation, topology minimum-disk and raid10-even checks.
   - mergerfs: `--fs ext4|xfs --content ID [--content ID …] [--parity ID …] --confirm storage`.
     In non-interactive mode **refuse (exit non-zero, no changes) if `/etc/nixos/storage-pool.nix`
     already exists** — the script regenerates that file from only the disks selected in the run, so
     "adding a disk" would silently drop existing branches. Interactive behaviour is unchanged.
3. Machine-readable progress: print marker lines `::step <n>/<total> <name>` on stdout at each existing
   `hdr` step, in addition to the human output. Errors keep going to stderr; exit code non-zero on any
   failure.
4. Interactive mode must behave exactly as today (same prompts, same files written). Keep `set -uo
   pipefail` semantics; do not refactor beyond what is needed (CLAUDE.md surgical-changes rule).

## Packaging
Add `pkgs/vexos/storage-scripts` (follow how `pkgs/default.nix` registers other `pkgs.vexos.*`
packages): a `stdenvNoCC` derivation installing both scripts to `$out/bin/` (names
`vexos-create-zfs-pool`, `vexos-create-mergerfs-pool`) wrapped with `makeWrapper --prefix PATH` for the
tools they call (zfs from the configured `boot.zfs.package` is not available in a plain derivation, so
instead leave `zpool`/`zfs` to be found on the system PATH and wrap only: gptfdisk, util-linux,
e2fsprogs, xfsprogs, mergerfs, coreutils, gawk, gnugrep, gnused). Document which tools deliberately come
from the system PATH. vexnas will call these store paths (via `services.vexnas.poolScripts`), so the
justfile recipes must keep working unchanged.

## Constraints
Never run `nix flake check`, `nixos-rebuild switch/boot`, or any git write; never execute the pool
scripts against real devices. Provide a test plan the user can run on a throwaway VM with virtual disks.

## Done when
`--list-candidates --json` output is valid JSON; non-interactive runs reject unknown/ineligible disks and
wrong confirmation before any destructive command; interactive runs are unchanged; the package builds
(`nix build` of the derivation, not a system switch).
