# Arsivinyo Memegy Backend

A high-performance meme archive backend built with Rust, Axum, SeaORM, and Redis.

## Architecture

- **API Server (`src/api`)**: Handles HTTP requests, Authentication, and Business Logic.
- **Worker (`src/worker`)**: Background processor for video compression and thumbnail generation (FFmpeg).
- **Shared (`src/shared`)**: Common database entities, configuration, and services.
- **Database**: PostgreSQL (Metadata).
- **Cache/Queue**: Valkey (Redis compatible) - Used for Rate Limiting and Job Queue.
- **Storage**: MinIO (S3 compatible) - Stores raw and processed videos.

## Prerequisites

- Docker & Docker Compose
- Rust (1.93+)

## Getting Started

1.  **Start Infrastructure:**
    ```bash
    cp .env.example .env
    # Update Google Client ID/Secret in .env
    docker compose up -d
    ```

2.  **Run Migrations:**
    ```bash
    # Install SeaORM CLI
    cargo install sea-orm-cli
    
    # Run Migrations
    export DATABASE_URL="postgres://app_user:secure_db_password@localhost:5432/memegy_db"
    cd src/shared/migration
    cargo run -- up
    ```

3.  **Run API Server:**
    ```bash
    cd ../../../ # Back to root
    cargo run -p api
    ```

4.  **Run Worker:**
    ```bash
    cargo run -p worker
    ```

## API Documentation

Once the server is running, visit:
- **Interactive Documentation (Scalar)**: http://localhost:3000/docs
- **OpenAPI Spec**: http://localhost:3000/openapi.yaml

The documentation includes:
- All endpoints with detailed descriptions
- Request/Response schemas
- Authentication requirements
- Rate limiting information
- Error responses

Admin integration and security runbook:
- `ADMIN_PANEL_SUPERADMIN_GUIDE.md`

## Endpoints

### Auth
- `GET /auth/google/login`: Start Google OAuth flow.
- `GET /auth/google/callback`: Callback from Google. Returns Access/Refresh tokens.
- `POST /auth/exchange-otc`: Exchanges OTC. If signup is required, returns required onboarding fields (`requires_age_confirmation`, `required_terms_version`, `terms_url`).
- `POST /auth/signup/complete`: Completes signup. Body requires `signup_ticket`, `username`, `age_confirmed: true`, `terms_version` (must match current version).
- `POST /auth/refresh`: Refresh access token.
- `POST /auth/logout`: Invalidate session.
- `POST /auth/extension/session`: Mint short-lived keyboard extension token.

### Videos
- `POST /videos/init`: Request upload URL. Body: `{ "filename": "meme.mp4", "size_bytes": 123456, "is_nsfw": false }`.
- `POST /videos/init/anonymous`: Same contract as `/videos/init`, including required `is_nsfw`.
- `POST /videos/{id}/confirm`: Confirm upload completion.
- `POST /videos/{id}/report`: Submit or idempotently update an abuse report for a video.
- `GET /feed`: Get video feed. Required query includes `include_nsfw=true|false`. Example: `?sort=random&page=0&include_nsfw=false`.
- `GET /videos/search`: Search videos. Required query includes `include_nsfw=true|false`.
- `GET /videos/search/keyboard`: Keyboard-optimized compact search DTO.
- `POST /videos/{id}/send-ticket`: Create short-lived single-use send ticket.
- `GET /videos/send-ticket/{ticket_id}/media`: Redeem send ticket to media redirect.
- `PUT /videos/{id}/like`: Idempotently like a video. Returns current `{ is_liked, like_count }`.
- `DELETE /videos/{id}/like`: Idempotently unlike a video. Returns current `{ is_liked, like_count }`.
- `GET /users/me/videos/ws`: WebSocket realtime stream for upload status changes.

### Users
- `GET /users/me/onboarding/status`: Returns onboarding state for age confirmation and terms acceptance.
- `POST /users/me/onboarding/complete`: Idempotently completes onboarding. Body: `{ "age_confirmed": true, "terms_version": "v1" }`.
- `GET /users/me/reports`: Returns the authenticated user's abuse reports with cursor pagination.

### System
- `GET /system/terms`: Public endpoint returning active terms bundle for all supported languages.
- `GET /system/terms/en`: Public endpoint returning English terms document.
- `GET /system/terms/tr`: Public endpoint returning Turkish terms document.
- `GET /system/read-only`: Protected status endpoint for read-only mode.
- `GET /system/maintenance`: Protected status endpoint for maintenance mode.

### Realtime Video Status (WebSocket)
- Connect with `Authorization: Bearer <access_token>` to `GET /users/me/videos/ws`.
- Server sends one snapshot first:
  - `type = "video.status.snapshot"`
  - Contains `videos: UserVideoDto[]`
- Then server sends live status events:
  - `video.status.processing`
  - `video.status.published`
  - `video.status.failed`
- Each event contains full `video` payload (same shape as `/users/me/videos` item), plus:
  - `event_id`, `event_at`, `previous_status`, `version`
- Delivery model:
  - At-most-once live delivery (Redis Pub/Sub)
  - Reconnect strategy: reconnect and rely on snapshot to recover missed offline events

## Platform Mode & Terms Config

Key environment flags:
- `TERMS_CURRENT_VERSION` (required, non-empty)
- `TERMS_DEFAULT_LANGUAGE` (`en|tr`, defaults to `en`)
- `TERMS_URL_EN` / `TERMS_URL_TR` (optional)
- `TERMS_CONTENT_EN` / `TERMS_CONTENT_TR` (optional inline document text)
- `TERMS_CONTENT_FILE_PATH_EN` / `TERMS_CONTENT_FILE_PATH_TR` (optional file path for embedded document text)
- `TERMS_CONTENT_TYPE_EN` / `TERMS_CONTENT_TYPE_TR` (optional, defaults to `text/markdown` when embedded content is set)
- Legacy EN fallback keys remain supported for backward compatibility:
  - `TERMS_URL`
  - `TERMS_CONTENT`
  - `TERMS_CONTENT_FILE_PATH`
  - `TERMS_CONTENT_TYPE`
- `TERMS_REQUIRE_VERSION_MATCH` (defaults to `true`; validates frontmatter `version` against `TERMS_CURRENT_VERSION`)
- `TERMS_LEGAL_CONTACT_EMAIL` / `TERMS_ABUSE_CONTACT_EMAIL` (optional metadata)
- `TERMS_JURISDICTIONS` (CSV list, e.g. `US,TR,GLOBAL`)
- `READ_ONLY_MODE_ENABLED` (`true|false`)
- `MAINTENANCE_MODE_ENABLED` (`true|false`)
- `READ_ONLY_STATUS_RPM_PER_USER`
- `MAINTENANCE_STATUS_RPM_PER_USER`
- `ONBOARDING_STATUS_RPM_PER_USER`
- `ONBOARDING_COMPLETE_RPM_PER_USER`
- `REPORT_CREATE_RPM_PER_USER`
- `REPORT_CREATE_RPM_PER_IP`
- `REPORT_DETAILS_MAX_CHARS`
- `REPORT_REASON_MAX_COUNT`
- `AUTO_QUARANTINE_ENABLED`
- `AUTO_QUARANTINE_WINDOW_SECS`
- `AUTO_QUARANTINE_SEVERE_DISTINCT_REPORTERS`
- `ABUSE_REPORT_RETENTION_DAYS`
- `ABUSE_REPORT_PURGE_INTERVAL_SECS`

Terms source rules:
- For each language (`EN` and `TR`), configure either inline content or file path (not both).
- For each language (`EN` and `TR`), at least one of URL or embedded content must be configured.

## Testing

Use Postman or similar to test endpoints.
For Upload:
1. Call `/videos/init` -> Get `upload_url`.
2. PUT file to `upload_url`.
3. Call `/videos/{id}/confirm`.

## Docker Deployment

Build and run the entire stack (API + Worker + Infrastructure):
```bash
docker compose up -d --build
```

This will:
- Build the Rust binaries
- Start PostgreSQL, Valkey, and MinIO
- Launch the API server on port 3000
- Start the background worker

## Production Durability Notes

- Configure production state paths outside the git checkout via:
  - `PROD_POSTGRES_DATA_DIR`
  - `PROD_VALKEY_DATA_DIR`
  - `PROD_MINIO_DATA_DIR`
- Recommended values:
  - Postgres: `/var/lib/memegy/postgres`
  - Valkey: `/var/lib/memegy/valkey`
  - MinIO: `/var/lib/memegy/minio`
- Deployment workflow builds/pushes images in CI, then VPS runs `pull` + `up -d --no-build --remove-orphans`, so production no longer compiles on-host.
- Production requires `BACKEND_IMAGE` in `.env.production` (managed automatically by the deploy workflow).
- Nightly production DB backups are handled by `.github/workflows/backup-production-db.yml` (plus manual `workflow_dispatch` support).
- MinIO snapshot backups to Google Drive are handled by `.github/workflows/backup-production-minio.yml` on a self-hosted `backup-laptop` runner.
- Operational setup/restore instructions live in `docs/ops/minio-backup-runbook.md`.

## License
MIT
