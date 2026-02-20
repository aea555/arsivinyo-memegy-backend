# MinIO Backup Runbook (Production)

## Scope
This runbook covers offloaded MinIO backups from VPS -> laptop self-hosted runner -> Google Drive, using:
- `rclone` for MinIO mirror sync
- `restic` for encrypted/deduplicated/compressed snapshots

Backup interval is enforced as `72h` (every 3 days) by script cadence gating.

## Files
- Workflow: `.github/workflows/backup-production-minio.yml`
- Script: `scripts/backup/minio_snapshot_backup.sh`
- Env template: `scripts/backup/.env.minio-backup.example`

## Runner Requirements
Runner labels must include:
- `self-hosted`
- `backup-laptop`

Install on runner host:
- `restic`
- `rclone`
- `jq`
- `ssh`
- `curl`
- `flock`
- `ionice`
- `nice`

## GitHub Secrets
Required:
- `VPS_SSH_KEY` (or existing `SSH_KEY`)
- `VPS_SSH_HOST` (or existing `SSH_HOST`)
- `VPS_SSH_USER` (or existing `SSH_USER`)
- `MINIO_BACKUP_ACCESS_KEY` (fallback: `MINIO_ROOT_USER`)
- `MINIO_BACKUP_SECRET_KEY` (fallback: `MINIO_ROOT_PASSWORD`)
- `RESTIC_PASSWORD`
- `RCLONE_CONFIG` (full rclone config content, optional if using path variable below)

## GitHub Variables
Required:
- `MINIO_RESTIC_REPOSITORY` (example: `rclone:gdrive:memegy-backups/minio-prod`)

Recommended:
- `VPS_SSH_PORT` (default `22`)
- `MINIO_BACKUP_LOCAL_PORT` (default `19000`)
- `MINIO_BACKUP_STATE_DIR` (default `$HOME/.local/state/memegy-backup`)
- `MINIO_BACKUP_MIRROR_DIR` (default `$HOME/memegy-backups/minio-mirror`)
- `MINIO_BACKUP_MIN_INTERVAL_SECS` (default `259200`)
- `MINIO_BACKUP_RETENTION_KEEP_WITHIN` (default `90d`)
- `MINIO_BACKUP_CHECK_READ_DATA_SUBSET` (default `5%`)
- `MINIO_BACKUP_RCLONE_TRANSFERS` (default `4`)
- `MINIO_BACKUP_RCLONE_CHECKERS` (default `8`)
- `MINIO_BACKUP_RCLONE_BWLIMIT` (default `off`)
- `MINIO_BACKUP_RCLONE_RETRIES` (default `3`)
- `MINIO_BACKUP_RCLONE_LOW_LEVEL_RETRIES` (default `6`)
- `MINIO_BACKUP_NICE_LEVEL` (default `10`)
- `MINIO_BACKUP_IONICE_CLASS` (default `2`)
- `MINIO_BACKUP_IONICE_LEVEL` (default `7`)
- `RCLONE_CONFIG_PATH` (use only when `RCLONE_CONFIG` secret is not set)

## Workflow Behavior
- Daily schedule runs `backup` mode.
- Weekly schedule runs `check` mode (`restic check`).
- `workflow_dispatch` supports:
  - `operation`: `backup|check|both`
  - `force_backup`: bypass 72h gate
  - `dry_run`: no remote mutations
  - `mirror_only`: only sync MinIO to local mirror

## First-Time Bootstrap
1. Ensure laptop runner is online and idle enough for backup window.
2. Add all required secrets/variables.
3. Run workflow manually:
   - operation: `backup`
   - dry_run: `true`
4. Validate tunnel and bucket discovery success.
5. Run workflow manually:
   - operation: `backup`
   - mirror_only: `true`
6. Validate mirror size and sync timing.
7. Run workflow manually:
   - operation: `backup`
   - dry_run: `false`
8. Confirm snapshot appears in `restic snapshots` output.

## Restore Procedures

### Full MinIO Restore
1. Choose snapshot ID:
   ```bash
   restic --repo "$RESTIC_REPOSITORY" snapshots
   ```
2. Restore snapshot to recovery directory:
   ```bash
   restic --repo "$RESTIC_REPOSITORY" restore <SNAPSHOT_ID> --target /tmp/minio-restore
   ```
3. Rehydrate all buckets back into MinIO (through tunnel endpoint):
   ```bash
   for bucket_dir in "/tmp/minio-restore${MINIO_MIRROR_DIR}"/*; do
     bucket="$(basename "$bucket_dir")"
     rclone sync "$bucket_dir" ":s3:${bucket}" \
       --s3-provider Minio \
       --s3-access-key-id "$MINIO_ACCESS_KEY" \
       --s3-secret-access-key "$MINIO_SECRET_KEY" \
       --s3-endpoint "$MINIO_ENDPOINT"
   done
   ```
4. Validate object counts and key checksums for critical files.

### Single-Object Restore
1. Restore only required bucket subtree:
   ```bash
   restic --repo "$RESTIC_REPOSITORY" restore <SNAPSHOT_ID> \
     --target /tmp/minio-restore \
     --include "$MINIO_MIRROR_DIR/<bucket>/<prefix-or-file>"
   ```
2. Sync restored object(s) back to target bucket:
   ```bash
   rclone copy "/tmp/minio-restore$MINIO_MIRROR_DIR/<bucket>/<prefix-or-file>" \
     ":s3:<bucket>/<prefix-or-file>" \
     --s3-provider Minio \
     --s3-access-key-id "$MINIO_ACCESS_KEY" \
     --s3-secret-access-key "$MINIO_SECRET_KEY" \
     --s3-endpoint "$MINIO_ENDPOINT"
   ```

## Validation Checklist
After each successful production backup:
- Snapshot ID is present in logs.
- Manifest JSON is uploaded as workflow artifact.
- No retention/prune errors in logs.
- Mirror sync completed for all buckets.

Weekly:
- `restic check` passes.

Monthly:
- Execute restore drill for one known object and verify checksum.

## Failure Handling
- Tunnel failure: verify VPS SSH reachability and MinIO service state.
- Auth failure: rotate and re-check MinIO credentials and rclone config.
- Drive/rclone errors: rerun with `operation=backup` and `force_backup=true` once issue is resolved.
- Lock contention: verify no stale local process holds `LOCK_FILE`.

## Security Notes
- `RESTIC_PASSWORD` must be stored only in GitHub Secrets.
- Use restricted SSH key dedicated to backups.
- Keep `RCLONE_CONFIG` secret private (contains Drive auth material).
- Keep runner disk encrypted if possible; local mirror contains production media objects.
