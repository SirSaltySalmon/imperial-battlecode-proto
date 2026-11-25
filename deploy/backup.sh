#!/bin/sh
# Back up a coordinator data directory and apply the replay retention policy. Safe while it runs.
#
#   deploy/backup.sh                       # from cron or a systemd timer, e.g. hourly
#
# Environment (defaults suit deploy/ibc-coordinator.service):
#   IBC_DATA            data directory                          [/var/lib/ibc-coordinator]
#   BACKUP_DIR          where backups go                        [/var/backups/ibc-coordinator]
#   KEEP_SNAPSHOTS      database snapshots kept                 [48]
#   PRUNE_DAYS          delete replays of these kinds older than this many days; 0 = keep all [14]
#   PRUNE_KINDS         kinds whose replays are pruned          [unranked]
#   IBC_COORDINATOR_BIN the coordinator binary                  [ibc-coordinator on PATH]
#   COMPOSE             set to run inside the compose service instead, e.g.
#                       "docker compose -f deploy/docker-compose.yml exec -T coordinator"
#                       (then BACKUP_DIR is a path inside the container; mount a volume there, and
#                       only the latest database snapshot is kept: the image has no shell for dated
#                       copies, so keep them wherever you copy BACKUP_DIR to)
#
# Layout: BACKUP_DIR/blobs/ is one content-addressed copy shared by every snapshot (only new blobs are
# copied), BACKUP_DIR/coordinator.sqlite is the latest snapshot and BACKUP_DIR/snapshots/ keeps the last
# KEEP_SNAPSHOTS. Ship BACKUP_DIR off the box (restic, rclone, rsync); a backup on the same disk is not one.
# Restore: docs/DEPLOY.md, "Backups".
set -eu

IBC_DATA="${IBC_DATA:-/var/lib/ibc-coordinator}"
BACKUP_DIR="${BACKUP_DIR:-/var/backups/ibc-coordinator}"
KEEP_SNAPSHOTS="${KEEP_SNAPSHOTS:-48}"
PRUNE_DAYS="${PRUNE_DAYS:-14}"
PRUNE_KINDS="${PRUNE_KINDS:-unranked}"
BIN="${IBC_COORDINATOR_BIN:-ibc-coordinator}"

coordinator() {
  if [ -n "${COMPOSE:-}" ]; then
    # shellcheck disable=SC2086 # COMPOSE is a command line on purpose
    $COMPOSE ibc-coordinator "$@"
  else
    IBC_DATA="$IBC_DATA" "$BIN" "$@"
  fi
}

# Retention first, so the backup does not copy replays that are about to go.
if [ "$PRUNE_DAYS" -gt 0 ]; then
  coordinator prune-replays --older-than "$PRUNE_DAYS" --kinds "$PRUNE_KINDS"
fi

coordinator backup "$BACKUP_DIR"

# Keep dated copies of the database; blobs are shared and never deleted here (a pruned replay stays in
# the backup until you clear BACKUP_DIR/blobs and take a fresh backup).
if [ -z "${COMPOSE:-}" ]; then
  stamp=$(date -u +%Y%m%dT%H%M%SZ)
  mkdir -p "$BACKUP_DIR/snapshots"
  cp "$BACKUP_DIR/coordinator.sqlite" "$BACKUP_DIR/snapshots/coordinator-$stamp.sqlite"
  # shellcheck disable=SC2012 # names are ours: coordinator-<UTC stamp>.sqlite, no spaces
  ls -1 "$BACKUP_DIR/snapshots" | sort -r | tail -n +"$((KEEP_SNAPSHOTS + 1))" |
    while read -r old; do rm -f "$BACKUP_DIR/snapshots/$old"; done
fi
