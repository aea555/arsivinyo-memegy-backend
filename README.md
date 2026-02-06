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

## License
MIT
