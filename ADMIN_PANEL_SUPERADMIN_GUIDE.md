# Admin / Superadmin Integration Guide

This document is the authoritative reference for admin support in `arsivinyo-memegy-backend`.

It covers:
- The exact admin authentication model
- All `/admin` routes (including run-as-user routes)
- Rate-limit and authorization behavior
- Security controls and failure modes
- How to integrate a standalone admin panel safely
- Operational runbooks (keys, rotation, trusted servers)

---

## 1. Executive Summary

The backend implements admin access using **machine-to-machine signed admin JWTs** under a dedicated `/admin/*` surface.

Core decisions:
- No static app secret request auth (no `X-App-Secret` bypass mode).
- No DB-stored admin user account is required.
- No admin email/password login endpoint in this backend.
- Admin tokens are verified with configured public keys (`kid -> public key`) and strict claim checks.
- Admin operations are audited to `admin_audit_logs`.
- Admin routes are isolated from public JWT auth flow and public middleware.

Important behavior:
- `/admin` routes are designed as superuser operations (including run-as-user operations).
- Admin routes bypass public rate limiting logic and do not use user/business throttles.

---

## 2. Architecture and Trust Model

### 2.1 Components

1. **Admin Panel Backend (trusted signer)**
- Server-side only (Next.js API routes/BFF or separate backend service).
- Holds admin signing private key.
- Mints short-lived admin JWTs.
- Calls target backend `/admin/*` endpoints.

2. **Target Backend (`arsivinyo-memegy-backend`)**
- Holds only admin public keys.
- Verifies admin JWT.
- Executes admin operation.
- Writes immutable admin audit log rows.

3. **Admin Browser UI**
- Authenticates human operator to admin panel backend.
- Must not hold signing private key.

### 2.2 Why this model

- Scales to multiple APIs by audience (`aud`) and key management.
- Avoids coupling superuser auth to one API-local DB account.
- Supports key rotation with `kid`.
- Keeps browser out of signing trust boundary.

---

## 3. Admin Authentication Contract

### 3.1 Header

All admin requests require:

`Authorization: Bearer <admin_jwt>`

### 3.2 Required JWT header

- `alg = EdDSA`
- `kid` must be present and match configured key map

### 3.3 Required JWT claims

- `iss`
- `aud`
- `sub`
- `jti`
- `iat`
- `nbf`
- `exp`
- `role` (must be `superadmin`)

### 3.4 Validation rules enforced by backend

- Issuer must equal `ADMIN_JWT_ISSUER`
- Audience must contain `ADMIN_JWT_AUDIENCE`
- Role must equal `superadmin`
- `nbf <= exp`
- `exp > iat`
- `(exp - iat) <= ADMIN_JWT_MAX_TTL_SECS`
- Clock skew uses `ADMIN_JWT_CLOCK_SKEW_SECS`
- Algorithm must be EdDSA only
- Unknown `kid` is rejected

### 3.5 Replay protection

If `ADMIN_REPLAY_PROTECTION_ENABLED=true`:
- Backend writes `jti` lock in Redis with TTL until token expiration.
- Replayed `jti` is rejected.

Operationally:
- Best practice is minting one fresh admin JWT per request.

### 3.6 Optional admin IP allowlist

If `ADMIN_ALLOWED_IP_CIDRS` is set:
- Source IP must match configured CIDR(s).

If backend is in production/CF-enforced mode:
- `CF-Connecting-IP` is required for IP extraction/validation.

---

## 4. Answers to Common Superadmin Questions

### 4.1 Are all admin routes exempt from rate limits?

Yes, by design.

Current implementation details:
- `/admin/*` is mounted outside public IP limiter middleware.
- Admin run-as operations use admin passthrough state with limiter bypass mode enabled.
- Admin-specific handlers for operations that had hardcoded user limits (for example video delete/update metadata) avoid those public throttles.
- Admin routes therefore do not enforce user/IP/business rate-limit gates that regular routes enforce.

Note:
- Infrastructure limits still exist (DB pool, upstream proxy limits, object storage/network limits). Those are not application rate-limit rules.

### 4.2 Do I have to manually create a superuser in DB?

No.

Admin identity is token-based (`sub` claim in admin JWT), not a row in `users`.

### 4.3 Does superuser log in with email/username/password to this backend?

No.

This backend does not expose admin username/password login endpoints.
Admin auth is via signed admin JWT issued by your trusted admin signer service.

### 4.4 Are there admin refresh tokens?

No.

Admin flow uses short-lived signed JWTs minted on demand by the trusted admin service.

### 4.5 How are admin tokens invalidated?

- Natural expiration (`exp`)
- Replay lock enforcement (`jti` already seen)
- Key removal/rotation (`kid` no longer trusted)
- Issuer/audience mismatch
- Optional IP allowlist mismatch
- Global kill switch (`ADMIN_API_ENABLED=false`)

---

## 5. Complete Admin Route Surface

All routes are rooted at `/admin`.

## 5.1 Database inspection routes

- `GET /admin/db/tables`
- `GET /admin/db/tables/{table}/rows?limit&cursor`
- `GET /admin/db/tables/{table}/rows/{id}`

Allowlisted tables:
- `users`
- `videos`
- `likes`
- `refresh_tokens`
- `download_jobs`
- `send_tickets`
- `extension_sessions`
- `admin_audit_logs`

Notes:
- No raw SQL endpoint.
- Pagination on list-rows is offset cursor (`cursor` as numeric string).

## 5.2 Hard delete routes

- `DELETE /admin/users/{id}/hard`
- `DELETE /admin/videos/{id}/hard`
- `DELETE /admin/download-jobs/{id}/hard`
- `DELETE /admin/send-tickets/{id}/hard`
- `DELETE /admin/extension-sessions/{id}/hard`
- `DELETE /admin/refresh-tokens/{id}/hard`

Notes:
- Irreversible operations.
- Storage object cleanup is attempted for hard video/user deletion paths.
- All operations are audited.

## 5.3 Run-as-user superadmin routes (regular-user surface + privileged control)

Profile/account/session:
- `GET /admin/users/{user_id}/profile`
- `DELETE /admin/users/{user_id}/account`
- `PUT /admin/users/{user_id}/username`
- `POST /admin/users/{user_id}/session`
- `POST /admin/users/{user_id}/session/refresh`
- `POST /admin/users/{user_id}/session/logout`
- `POST /admin/users/{user_id}/extension-session`

Feed:
- `GET /admin/users/{user_id}/feed`

User videos:
- `GET /admin/users/{user_id}/videos`
- `GET /admin/users/{user_id}/videos/ws`

Upload lifecycle:
- `POST /admin/users/{user_id}/videos/init`
- `POST /admin/users/{user_id}/videos/init/anonymous`
- `POST /admin/users/{user_id}/videos/{id}/confirm`

Search and keyboard:
- `GET /admin/users/{user_id}/videos/search`
- `GET /admin/users/{user_id}/videos/search/keyboard`
- `POST /admin/users/{user_id}/videos/{id}/send-ticket`
- `GET /admin/users/{user_id}/videos/send-ticket/{ticket_id}/media`

Download:
- `GET /admin/users/{user_id}/videos/{id}/download`
- `POST /admin/users/{user_id}/videos/{id}/download/refresh`
- `POST /admin/users/{user_id}/videos/download/bulk`
- `GET /admin/users/{user_id}/videos/download/bulk/{id}`

Video actions:
- `PUT /admin/users/{user_id}/videos/{id}/like`
- `DELETE /admin/users/{user_id}/videos/{id}/like`
- `PATCH /admin/users/{user_id}/videos/{id}`
- `DELETE /admin/users/{user_id}/videos/{id}`
- `POST /admin/users/{user_id}/videos/bulk-delete`

Scope note:
- OAuth browser bootstrapping endpoints (`/auth/google/login`, `/auth/google/callback`, OTC exchange) are intentionally not mirrored under `/admin`.
- Superadmin can directly mint user sessions through `/admin/users/{user_id}/session`.

---

## 6. Route Behavior Details

## 6.1 Admin envelope responses

DB/hard-delete catalog-style endpoints return:

```json
{
  "metadata": {
    "request_id": "...",
    "actor_sub": "...",
    "timestamp": "..."
  },
  "data": { "...": "..." }
}
```

`request_id`:
- Uses incoming `X-Request-Id` when provided.
- Otherwise generated by backend.

Run-as endpoints generally return same payload contract as their underlying public counterpart to preserve client compatibility.

## 6.2 Username updates (run-as)

`PUT /admin/users/{user_id}/username`
- Validates username format/rules.
- Enforces case-insensitive uniqueness.
- Updates normalized username fields.
- Invalidates related profile/video cache keys.

## 6.3 Video metadata updates (run-as)

`PATCH /admin/users/{user_id}/videos/{id}`
- Performs same payload validation as public route.
- Applies changes without public per-user update throttle.
- Preserves cache invalidation behavior (feed + user caches).

## 6.4 Video deletion (run-as)

`DELETE /admin/users/{user_id}/videos/{id}`
- Performs soft delete semantics.
- Returns `204` on success and idempotent already-deleted flow.
- Does not apply public per-user delete throttle.
- Preserves cache invalidation behavior.

## 6.5 Session minting (run-as)

`POST /admin/users/{user_id}/session`
- Returns standard user `AuthResponse` with access and refresh token pair for target user.

Security implication:
- Treat this endpoint as highly privileged credential minting.
- Restrict UI/action access and enforce strong operator controls.

---

## 7. Rate-Limit and Middleware Semantics

### 7.1 Public routes

Public routes still enforce their existing middleware and per-feature limits.

### 7.2 Admin routes

Admin routes:
- Are outside public global IP rate limiter middleware.
- Bypass internal rate limiter checks through admin bypass mode where shared handlers are reused.
- Use dedicated admin implementations where public handlers had hardcoded throttles.

Outcome:
- Admin requests should not receive application-level `429` from standard user/business limiting logic.

---

## 8. Auditing and Traceability

All admin operations log immutable audit rows to `admin_audit_logs` with:
- `actor_sub`
- `action`
- `target_table`
- `target_id`
- `request_id`
- `ip`
- `user_agent`
- `outcome`
- `metadata_json`
- `created_at`

Use this table for:
- Forensics
- Operator action traceability
- High-risk operation review (delete/session minting)

Recommended panel behavior:
- Always send `X-Request-Id`.
- Log mapping of panel action ID -> backend request ID.

---

## 9. Environment Configuration

Required when admin is enabled:
- `ADMIN_API_ENABLED=true`
- `ADMIN_JWT_ISSUER`
- `ADMIN_JWT_AUDIENCE`
- `ADMIN_JWT_PUBLIC_KEYS_JSON`

Optional/recommended:
- `ADMIN_JWT_MAX_TTL_SECS` (default `300`)
- `ADMIN_JWT_CLOCK_SKEW_SECS` (default `60`)
- `ADMIN_REPLAY_PROTECTION_ENABLED` (default `true`)
- `ADMIN_ALLOWED_IP_CIDRS` (recommended for production)

`ADMIN_JWT_PUBLIC_KEYS_JSON` format:

```json
{
  "kid-v1": "-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----",
  "kid-v2": "-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----"
}
```

---

## 10. Secrets and Key Material Placement

### 10.1 Backend API

Stores:
- Public verification keys
- Policy config (issuer/audience/TTL/etc.)

Does not store:
- Admin private signing keys

### 10.2 Admin panel backend (trusted signer)

Stores:
- Private signing key (Ed25519)
- Issuer value
- Active `kid`
- Audience per target API

Storage requirements:
- Secret manager/KMS/Vault preferred
- Never commit to git
- Never expose to browser/client bundle

### 10.3 Browser frontend

Must not sign admin JWTs.

### 10.4 Secret placement matrix (clear ownership)

Use this as the source of truth for where each value belongs:

| Item | `arsivinyo-memegy-backend` API env | Admin panel backend env (trusted signer) | Browser frontend env |
|---|---|---|---|
| Ed25519 private key | Never | Required | Never |
| Ed25519 public key | Required (via `ADMIN_JWT_PUBLIC_KEYS_JSON`) | Optional copy for diagnostics | Never |
| `ADMIN_JWT_ISSUER` value | Required | Required (must match exactly) | Never required |
| `ADMIN_JWT_AUDIENCE` value | Required | Required (must match exactly) | Never required |
| Active `kid` | Required implicitly in key map | Required for JWT header signing | Never required |
| Minting logic (`jti/iat/nbf/exp/role`) | Never | Required | Never |
| Long-lived secret material | Never (only public keys) | Required and must be in KMS/secret manager | Never |

Hard rules:
- Only the admin panel backend can sign admin JWTs.
- The target API must only verify admin JWTs.
- The browser must never have signing keys or direct signing capability.
- Do not copy private keys into docker compose files, `.env` files in git, or frontend bundles.

---

## 11. How to Set Up a Trusted Admin Panel

### 11.0 One-time setup responsibility split

When performing section 11 steps, apply them to different systems:
- `11.1 Generate keys`: done once in a secure operator environment.
- `11.2 Configure backend API`: applied to `arsivinyo-memegy-backend` deployment env.
- `11.3 Configure admin panel signer`: implemented in admin panel server-side backend/BFF.

Do not duplicate all settings in both places:
- Backend API gets public verification keys and verification policy.
- Admin panel backend gets private signing key and token minting logic.

### 11.1 Generate keys

```bash
openssl genpkey -algorithm ed25519 -out admin_ed25519_private.pem
openssl pkey -in admin_ed25519_private.pem -pubout -out admin_ed25519_public.pem
```

### 11.2 Configure backend API

```env
ADMIN_API_ENABLED=true
ADMIN_JWT_ISSUER=https://admin.yourcompany.internal
ADMIN_JWT_AUDIENCE=arsivinyo-memegy-backend
ADMIN_JWT_PUBLIC_KEYS_JSON={"admin-panel-v1":"-----BEGIN PUBLIC KEY-----\n...\n-----END PUBLIC KEY-----"}
ADMIN_JWT_MAX_TTL_SECS=300
ADMIN_JWT_CLOCK_SKEW_SECS=60
ADMIN_REPLAY_PROTECTION_ENABLED=true
ADMIN_ALLOWED_IP_CIDRS=203.0.113.10/32
```

### 11.3 Configure admin panel signer

JWT payload template:

```json
{
  "iss": "https://admin.yourcompany.internal",
  "sub": "admin-panel",
  "aud": "arsivinyo-memegy-backend",
  "jti": "<uuid>",
  "iat": 1700000000,
  "nbf": 1700000000,
  "exp": 1700000060,
  "role": "superadmin"
}
```

JWT header template:

```json
{
  "alg": "EdDSA",
  "kid": "admin-panel-v1",
  "typ": "JWT"
}
```

### 11.4 Call backend from server side

Do not call `/admin` directly from browser with long-lived credentials.

Recommended flow:
1. Operator logs into admin panel.
2. Panel backend authorizes operator action.
3. Panel backend mints short-lived admin JWT.
4. Panel backend calls target `/admin` route.
5. Panel backend returns sanitized result to UI.

---

## 12. Key Rotation Runbook (Zero-Downtime)

1. Generate new keypair and `kid-v2`.
2. Add `kid-v2` public key to backend map while keeping `kid-v1`.
3. Deploy backend.
4. Switch signer to use `kid-v2`.
5. Wait past max TTL + skew.
6. Remove `kid-v1` from backend config.
7. Deploy backend.

---

## 13. Adding Another Trusted Server

To onboard another trusted admin signer:
1. Generate separate keypair.
2. Assign unique `kid`.
3. Add new public key to `ADMIN_JWT_PUBLIC_KEYS_JSON`.
4. Deploy backend.
5. Configure signer with matching private key and policy claims.
6. If using allowlist, add egress CIDR to `ADMIN_ALLOWED_IP_CIDRS`.
7. Validate with `GET /admin/db/tables`.

---

## 14. Token Lifecycle: Refresh, Storage, Invalidation

### 14.1 Admin tokens

- No refresh-token flow.
- Mint short-lived signed JWTs on demand.
- Prefer per-request token minting.

### 14.2 Safe storage practices

- Private key in KMS/Vault/secret manager.
- Rotate credentials periodically.
- Never place signing keys in frontend or mobile app.

### 14.3 Invalidation mechanisms

- Token expiry
- Replay rejection
- Key removal
- Admin API disable switch
- Optional IP policy enforcement

### 14.4 User refresh tokens (separate system)

Regular user refresh tokens are independent from admin model and remain in user-auth subsystem.

---

## 15. Failure Modes and Troubleshooting

`401 Invalid admin token`:
- Bad signature
- Unknown `kid`
- Wrong `iss` / `aud`
- Expired token
- Replayed `jti`
- Role not `superadmin`

`403 Admin IP is not allowed`:
- Source IP outside configured CIDR allowlist

`403 Direct access not allowed. Use Cloudflare endpoint.`:
- Required CF header missing in enforced mode

`404 Admin API is disabled`:
- `ADMIN_API_ENABLED=false`

`503 Admin auth temporarily unavailable`:
- Redis unavailable with replay protection enabled

`400 Table is not allowed`:
- Non-allowlisted table used in DB browse endpoint

---

## 16. cURL Examples

```bash
# List allowlisted tables
curl -H "Authorization: Bearer $ADMIN_JWT" \
  -H "X-Request-Id: admin-op-001" \
  https://api.example.com/admin/db/tables

# Browse rows
curl -H "Authorization: Bearer $ADMIN_JWT" \
  "https://api.example.com/admin/db/tables/users/rows?limit=50&cursor=0"

# Run-as feed
curl -H "Authorization: Bearer $ADMIN_JWT" \
  "https://api.example.com/admin/users/<user_uuid>/feed?page=0&sort=latest"

# Run-as update username
curl -X PUT \
  -H "Authorization: Bearer $ADMIN_JWT" \
  -H "Content-Type: application/json" \
  -d '{"username":"new_name_123"}' \
  "https://api.example.com/admin/users/<user_uuid>/username"

# Mint user session
curl -X POST \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "https://api.example.com/admin/users/<user_uuid>/session"

# Hard delete video
curl -X DELETE \
  -H "Authorization: Bearer $ADMIN_JWT" \
  "https://api.example.com/admin/videos/<video_uuid>/hard"
```

If replay protection is enabled, mint fresh JWT per request.

---

## 17. Security Checklist Before Production Enablement

- `ADMIN_API_ENABLED` enabled only after staging validation
- Issuer/audience values finalized
- Public keys configured correctly
- Signing private key in secret manager/KMS
- Replay protection enabled
- IP allowlist configured
- `X-Request-Id` propagation from admin panel backend
- Audit monitoring and alerting configured
- Session minting and hard-delete actions additionally protected in panel UX
- Rotation runbook validated in staging

---

## 18. Scope Boundaries (Current Version)

Not implemented in current version:
- Raw SQL execution endpoint
- Admin refresh-token endpoint
- Multi-role RBAC matrix beyond `superadmin`
- Granular per-operation token scopes in admin JWT

These can be added later if needed, but current implementation intentionally prefers a simpler, strongly controlled superadmin model.

---

## 19. Quick Reference

- Admin entrypoint: `/admin/*`
- Auth: short-lived EdDSA JWT from trusted signer
- DB superuser account required: **No**
- Admin email/password login in backend: **No**
- Admin refresh tokens: **No**
- Replay defense: `jti` lock (optional, recommended)
- Audit log table: `admin_audit_logs`
