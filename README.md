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

## Endpoints

### Auth
- `GET /auth/google/login`: Start Google OAuth flow.
- `GET /auth/google/callback`: Callback from Google. Returns Access/Refresh tokens.
- `POST /auth/refresh`: Refresh access token.
- `POST /auth/logout`: Invalidate session.

### Videos
- `POST /videos/init`: Request upload URL. Body: `{ "filename": "meme.mp4", "size_bytes": 123456 }`.
- `POST /videos/{id}/confirm`: Confirm upload completion.
- `GET /feed`: Get video feed. Params: `?sort=random|latest|popular&page=0`.
- `PUT /videos/{id}/like`: Idempotently like a video. Returns current `{ is_liked, like_count }`.
- `DELETE /videos/{id}/like`: Idempotently unlike a video. Returns current `{ is_liked, like_count }`.
- `GET /users/me/videos/ws`: WebSocket realtime stream for upload status changes.

### Realtime Video Status (WebSocket)
- Connect with `Authorization: Bearer <access_token>` to `GET /users/me/videos/ws`.
- Server sends one snapshot first:
 Yes, now recreate the plan  - `type = "video.status.snapshot"`
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
- Deployment workflow uses `up -d --build --remove-orphans` and does not run `down`, so persistent data is less exposed to accidental reset.
- Nightly production DB backups are handled by `.github/workflows/backup-production-db.yml` (plus manual `workflow_dispatch` support).

## License
MIT
