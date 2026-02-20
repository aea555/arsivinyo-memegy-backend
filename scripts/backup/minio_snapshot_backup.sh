#!/usr/bin/env bash
set -euo pipefail

log() {
  printf '[%s] %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*"
}

die() {
  log "ERROR: $*"
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

run_with_priority() {
  if command -v ionice >/dev/null 2>&1; then
    ionice -c "${IONICE_CLASS}" -n "${IONICE_LEVEL}" nice -n "${NICE_LEVEL}" "$@"
  else
    nice -n "${NICE_LEVEL}" "$@"
  fi
}

MODE="backup"
FORCE_RUN=false
DRY_RUN=false
MIRROR_ONLY=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode)
      MODE="${2:-}"
      shift 2
      ;;
    --force)
      FORCE_RUN=true
      shift
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    --mirror-only)
      MIRROR_ONLY=true
      shift
      ;;
    *)
      die "Unknown argument: $1"
      ;;
  esac
done

case "$MODE" in
  backup|check|both)
    ;;
  *)
    die "Invalid --mode value '$MODE'. Expected backup|check|both."
    ;;
esac

need_cmd ssh
need_cmd curl
need_cmd rclone
need_cmd restic
need_cmd jq
need_cmd flock

NICE_LEVEL="${NICE_LEVEL:-10}"
IONICE_CLASS="${IONICE_CLASS:-2}"
IONICE_LEVEL="${IONICE_LEVEL:-7}"

BACKUP_STATE_DIR="${BACKUP_STATE_DIR:-$HOME/.local/state/memegy-backup}"
MINIO_MIRROR_DIR="${MINIO_MIRROR_DIR:-$HOME/memegy-backups/minio-mirror}"
BACKUP_MIN_INTERVAL_SECS="${BACKUP_MIN_INTERVAL_SECS:-259200}"
RETENTION_KEEP_WITHIN="${RETENTION_KEEP_WITHIN:-90d}"
RESTIC_COMPRESSION="${RESTIC_COMPRESSION:-auto}"
RESTIC_CHECK_READ_DATA_SUBSET="${RESTIC_CHECK_READ_DATA_SUBSET:-5%}"

LOCK_FILE="${LOCK_FILE:-$BACKUP_STATE_DIR/minio-backup.lock}"
LAST_SUCCESS_FILE="${LAST_SUCCESS_FILE:-$BACKUP_STATE_DIR/minio-last-success-epoch.txt}"
MANIFEST_DIR="${MANIFEST_DIR:-$BACKUP_STATE_DIR/manifests}"

RCLONE_TRANSFERS="${RCLONE_TRANSFERS:-4}"
RCLONE_CHECKERS="${RCLONE_CHECKERS:-8}"
RCLONE_BWLIMIT="${RCLONE_BWLIMIT:-off}"
RCLONE_RETRIES="${RCLONE_RETRIES:-3}"
RCLONE_LOW_LEVEL_RETRIES="${RCLONE_LOW_LEVEL_RETRIES:-6}"
RCLONE_STATS_ONE_LINE="${RCLONE_STATS_ONE_LINE:-true}"

VPS_SSH_HOST="${VPS_SSH_HOST:-}"
VPS_SSH_USER="${VPS_SSH_USER:-}"
VPS_SSH_PORT="${VPS_SSH_PORT:-22}"
VPS_SSH_KEY_FILE="${VPS_SSH_KEY_FILE:-}"
VPS_SSH_STRICT_HOST_KEY_CHECKING="${VPS_SSH_STRICT_HOST_KEY_CHECKING:-accept-new}"
SSH_TUNNEL_LOCAL_BIND="${SSH_TUNNEL_LOCAL_BIND:-127.0.0.1}"
SSH_TUNNEL_LOCAL_PORT="${SSH_TUNNEL_LOCAL_PORT:-19000}"
SSH_TUNNEL_REMOTE_HOST="${SSH_TUNNEL_REMOTE_HOST:-127.0.0.1}"
SSH_TUNNEL_REMOTE_PORT="${SSH_TUNNEL_REMOTE_PORT:-9000}"

MINIO_ACCESS_KEY="${MINIO_ACCESS_KEY:-${MINIO_ROOT_USER:-}}"
MINIO_SECRET_KEY="${MINIO_SECRET_KEY:-${MINIO_ROOT_PASSWORD:-}}"
MINIO_ENDPOINT="${MINIO_ENDPOINT:-http://127.0.0.1:${SSH_TUNNEL_LOCAL_PORT}}"

RESTIC_REPOSITORY="${RESTIC_REPOSITORY:-}"
RESTIC_PASSWORD="${RESTIC_PASSWORD:-}"

[[ -n "$RESTIC_REPOSITORY" ]] || die "RESTIC_REPOSITORY must be set"
[[ -n "$RESTIC_PASSWORD" ]] || die "RESTIC_PASSWORD must be set"

if [[ "$MODE" != "check" ]]; then
  [[ -n "$VPS_SSH_HOST" ]] || die "VPS_SSH_HOST must be set for backup mode"
  [[ -n "$VPS_SSH_USER" ]] || die "VPS_SSH_USER must be set for backup mode"
  [[ -n "$MINIO_ACCESS_KEY" ]] || die "MINIO_ACCESS_KEY (or MINIO_ROOT_USER) must be set"
  [[ -n "$MINIO_SECRET_KEY" ]] || die "MINIO_SECRET_KEY (or MINIO_ROOT_PASSWORD) must be set"
fi

mkdir -p "$BACKUP_STATE_DIR" "$MANIFEST_DIR" "$MINIO_MIRROR_DIR"

exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  die "Another backup process is already running (lock file: $LOCK_FILE)"
fi

TUNNEL_PID=""
TMP_DIR=""

cleanup() {
  local exit_code=$?
  if [[ -n "$TUNNEL_PID" ]] && kill -0 "$TUNNEL_PID" >/dev/null 2>&1; then
    kill "$TUNNEL_PID" >/dev/null 2>&1 || true
    wait "$TUNNEL_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$TMP_DIR" ]] && [[ -d "$TMP_DIR" ]]; then
    rm -rf "$TMP_DIR"
  fi
  exit "$exit_code"
}
trap cleanup EXIT

should_skip_by_cadence() {
  [[ "$FORCE_RUN" == "true" ]] && return 1
  [[ ! -f "$LAST_SUCCESS_FILE" ]] && return 1

  local now last elapsed
  now="$(date -u +%s)"
  last="$(tr -d '[:space:]' < "$LAST_SUCCESS_FILE" || true)"
  [[ -n "$last" ]] || return 1
  [[ "$last" =~ ^[0-9]+$ ]] || return 1

  elapsed=$((now - last))
  if (( elapsed < BACKUP_MIN_INTERVAL_SECS )); then
    log "Skipping backup by cadence: elapsed=${elapsed}s threshold=${BACKUP_MIN_INTERVAL_SECS}s"
    return 0
  fi
  return 1
}

start_tunnel() {
  local ssh_opts
  ssh_opts=(
    -N
    -p "$VPS_SSH_PORT"
    -o "BatchMode=yes"
    -o "ExitOnForwardFailure=yes"
    -o "ServerAliveInterval=30"
    -o "ServerAliveCountMax=3"
    -o "StrictHostKeyChecking=${VPS_SSH_STRICT_HOST_KEY_CHECKING}"
    -L "${SSH_TUNNEL_LOCAL_BIND}:${SSH_TUNNEL_LOCAL_PORT}:${SSH_TUNNEL_REMOTE_HOST}:${SSH_TUNNEL_REMOTE_PORT}"
  )
  if [[ -n "$VPS_SSH_KEY_FILE" ]]; then
    ssh_opts+=( -i "$VPS_SSH_KEY_FILE" )
  fi

  log "Starting SSH tunnel ${SSH_TUNNEL_LOCAL_BIND}:${SSH_TUNNEL_LOCAL_PORT} -> ${SSH_TUNNEL_REMOTE_HOST}:${SSH_TUNNEL_REMOTE_PORT}"
  ssh "${ssh_opts[@]}" "${VPS_SSH_USER}@${VPS_SSH_HOST}" &
  TUNNEL_PID=$!

  local health_url
  health_url="${MINIO_ENDPOINT%/}/minio/health/live"
  for _ in $(seq 1 20); do
    if curl -fsS "$health_url" >/dev/null 2>&1; then
      log "MinIO health check succeeded via tunnel"
      return 0
    fi
    sleep 1
  done
  die "MinIO tunnel health check failed: $health_url"
}

ensure_restic_repo() {
  local allow_init="${1:-false}"
  if restic --repo "$RESTIC_REPOSITORY" snapshots --json >/dev/null 2>&1; then
    return 0
  fi
  if [[ "$allow_init" == "true" ]]; then
    log "Restic repository not found or unavailable. Attempting init."
    restic --repo "$RESTIC_REPOSITORY" init
    return 0
  fi
  die "Restic repository is unavailable and auto-init is disabled for this mode"
}

list_buckets() {
  rclone lsf ":s3:" \
    --s3-provider "Minio" \
    --s3-access-key-id "$MINIO_ACCESS_KEY" \
    --s3-secret-access-key "$MINIO_SECRET_KEY" \
    --s3-endpoint "$MINIO_ENDPOINT" \
    --dirs-only \
    --format "p" \
    --fast-list | sed 's:/$::' | sed '/^$/d' | sort
}

sync_bucket() {
  local bucket="$1"
  local destination="$MINIO_MIRROR_DIR/$bucket"
  local dry_flags=()
  local stats_flags=()

  if [[ "$DRY_RUN" == "true" ]]; then
    dry_flags+=(--dry-run)
  fi
  if [[ "$RCLONE_STATS_ONE_LINE" == "true" ]]; then
    stats_flags+=(--stats-one-line)
  fi

  mkdir -p "$destination"

  run_with_priority rclone sync \
    ":s3:${bucket}" "$destination" \
    --s3-provider "Minio" \
    --s3-access-key-id "$MINIO_ACCESS_KEY" \
    --s3-secret-access-key "$MINIO_SECRET_KEY" \
    --s3-endpoint "$MINIO_ENDPOINT" \
    --transfers "$RCLONE_TRANSFERS" \
    --checkers "$RCLONE_CHECKERS" \
    --bwlimit "$RCLONE_BWLIMIT" \
    --retries "$RCLONE_RETRIES" \
    --low-level-retries "$RCLONE_LOW_LEVEL_RETRIES" \
    --fast-list \
    --delete-during \
    --stats "30s" \
    "${stats_flags[@]}" \
    "${dry_flags[@]}"
}

write_summary() {
  local summary_file="$1"
  local snapshot_id="$2"
  local bucket_count="$3"
  local mirror_bytes="$4"
  local files_new="$5"
  local files_changed="$6"
  local files_unmodified="$7"
  local data_added="$8"

  jq -n \
    --arg timestamp "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" \
    --arg snapshot_id "$snapshot_id" \
    --argjson bucket_count "$bucket_count" \
    --argjson mirror_bytes "$mirror_bytes" \
    --argjson files_new "$files_new" \
    --argjson files_changed "$files_changed" \
    --argjson files_unmodified "$files_unmodified" \
    --argjson data_added "$data_added" \
    '{
      timestamp: $timestamp,
      source: "minio",
      env: "production",
      snapshot_id: $snapshot_id,
      bucket_count: $bucket_count,
      mirror_bytes: $mirror_bytes,
      files_new: $files_new,
      files_changed: $files_changed,
      files_unmodified: $files_unmodified,
      data_added: $data_added
    }' > "$summary_file"
}

append_step_summary() {
  local snapshot_id="$1"
  local bucket_count="$2"
  local mirror_bytes="$3"
  local files_new="$4"
  local files_changed="$5"
  local files_unmodified="$6"
  local data_added="$7"
  local summary_file="$8"

  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "## MinIO backup snapshot"
      echo
      printf -- '- Snapshot ID: `%s`\n' "$snapshot_id"
      echo "- Buckets mirrored: $bucket_count"
      echo "- Mirror size (bytes): $mirror_bytes"
      echo "- Files new: $files_new"
      echo "- Files changed: $files_changed"
      echo "- Files unmodified: $files_unmodified"
      echo "- Data added (bytes): $data_added"
      printf -- '- Manifest: `%s`\n' "$summary_file"
    } >> "$GITHUB_STEP_SUMMARY"
  fi
}

run_backup() {
  if should_skip_by_cadence; then
    return 0
  fi

  start_tunnel

  mapfile -t buckets < <(list_buckets)
  if [[ ${#buckets[@]} -eq 0 ]]; then
    die "No MinIO buckets found to backup"
  fi

  log "Discovered ${#buckets[@]} MinIO buckets"
  for bucket in "${buckets[@]}"; do
    log "Syncing bucket: $bucket"
    sync_bucket "$bucket"
  done

  if [[ "$MIRROR_ONLY" == "true" ]]; then
    log "Mirror-only mode enabled; skipping snapshot creation and retention"
    return 0
  fi

  ensure_restic_repo true

  TMP_DIR="$(mktemp -d)"
  local restic_json
  restic_json="$TMP_DIR/restic-backup.json"

  local ts
  ts="$(date -u +'%Y%m%dT%H%M%SZ')"

  local -a restic_tags
  restic_tags=(
    --tag "source=minio"
    --tag "env=production"
    --tag "run_ts=${ts}"
  )

  if [[ -n "${GITHUB_RUN_ID:-}" ]]; then
    restic_tags+=(--tag "github_run_id=${GITHUB_RUN_ID}")
  fi

  local -a restic_backup_cmd
  restic_backup_cmd=(
    restic
    --repo "$RESTIC_REPOSITORY"
    backup "$MINIO_MIRROR_DIR"
    --compression "$RESTIC_COMPRESSION"
    --json
    "${restic_tags[@]}"
  )

  if [[ "$DRY_RUN" == "true" ]]; then
    restic_backup_cmd+=(--dry-run)
  fi

  log "Creating restic snapshot"
  run_with_priority "${restic_backup_cmd[@]}" | tee "$restic_json"

  local summary_line snapshot_id files_new files_changed files_unmodified data_added
  summary_line="$(jq -c 'select(.message_type=="summary")' "$restic_json" | tail -n1)"
  [[ -n "$summary_line" ]] || die "Missing restic summary output"

  snapshot_id="$(jq -r '.snapshot_id // ""' <<<"$summary_line")"
  files_new="$(jq -r '.files_new // 0' <<<"$summary_line")"
  files_changed="$(jq -r '.files_changed // 0' <<<"$summary_line")"
  files_unmodified="$(jq -r '.files_unmodified // 0' <<<"$summary_line")"
  data_added="$(jq -r '.data_added // 0' <<<"$summary_line")"

  if [[ "$DRY_RUN" == "false" ]]; then
    log "Applying retention: keep within ${RETENTION_KEEP_WITHIN}"
    run_with_priority restic --repo "$RESTIC_REPOSITORY" forget --keep-within "$RETENTION_KEEP_WITHIN" --prune
  else
    log "Dry-run mode: skipping retention prune"
  fi

  local now_epoch mirror_bytes manifest_file
  now_epoch="$(date -u +%s)"
  mirror_bytes="$(du -sb "$MINIO_MIRROR_DIR" | awk '{print $1}')"
  manifest_file="$MANIFEST_DIR/minio-backup-${ts}.json"

  write_summary "$manifest_file" "$snapshot_id" "${#buckets[@]}" "$mirror_bytes" "$files_new" "$files_changed" "$files_unmodified" "$data_added"
  append_step_summary "$snapshot_id" "${#buckets[@]}" "$mirror_bytes" "$files_new" "$files_changed" "$files_unmodified" "$data_added" "$manifest_file"

  if [[ "$DRY_RUN" == "false" ]]; then
    printf '%s\n' "$now_epoch" > "$LAST_SUCCESS_FILE"
  fi

  log "Backup completed. Snapshot=${snapshot_id} Manifest=${manifest_file}"
}

run_check() {
  ensure_restic_repo false
  log "Running restic repository integrity check"
  run_with_priority restic --repo "$RESTIC_REPOSITORY" check --read-data-subset "$RESTIC_CHECK_READ_DATA_SUBSET"
  log "Restic integrity check completed"
}

case "$MODE" in
  backup)
    run_backup
    ;;
  check)
    run_check
    ;;
  both)
    run_backup
    run_check
    ;;
esac
