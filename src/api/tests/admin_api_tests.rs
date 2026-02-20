mod common;

use common::{
    TEST_ADMIN_AUDIENCE, TEST_ADMIN_ISSUER, TEST_ADMIN_KID, TEST_ADMIN_PRIVATE_KEY_PEM, spawn_app,
    spawn_app_with_admin,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::Client;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct AdminTokenClaims {
    iss: String,
    sub: String,
    aud: String,
    jti: String,
    iat: usize,
    nbf: usize,
    exp: usize,
    role: String,
}

fn build_admin_token(audience: &str, role: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time is valid")
        .as_secs() as usize;
    let claims = AdminTokenClaims {
        iss: TEST_ADMIN_ISSUER.to_string(),
        sub: "admin-panel".to_string(),
        aud: audience.to_string(),
        jti: Uuid::new_v4().to_string(),
        iat: now,
        nbf: now,
        exp: now + 60,
        role: role.to_string(),
    };
    let mut header = Header::new(Algorithm::EdDSA);
    header.kid = Some(TEST_ADMIN_KID.to_string());
    encode(
        &header,
        &claims,
        &EncodingKey::from_ed_pem(TEST_ADMIN_PRIVATE_KEY_PEM.as_bytes())
            .expect("private key should be valid"),
    )
    .expect("token should be created")
}

#[tokio::test]
async fn admin_routes_require_admin_jwt() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();

    let missing = client
        .get(format!("{}/admin/db/tables", app.address))
        .send()
        .await
        .expect("request should execute");
    assert_eq!(missing.status().as_u16(), 401);

    let user_token = app
        .login_as_dev("regular_user", "regular@example.com")
        .await;
    let with_user_token = client
        .get(format!("{}/admin/db/tables", app.address))
        .bearer_auth(user_token)
        .send()
        .await
        .expect("request should execute");
    assert_eq!(with_user_token.status().as_u16(), 401);

    let admin_token = build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin");
    let success = client
        .get(format!("{}/admin/db/tables", app.address))
        .bearer_auth(admin_token)
        .send()
        .await
        .expect("request should execute");
    assert_eq!(success.status().as_u16(), 200);
}

#[tokio::test]
async fn admin_wrong_audience_is_rejected() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();

    let wrong_audience_token = build_admin_token("wrong-api-audience", "superadmin");
    let response = client
        .get(format!("{}/admin/db/tables", app.address))
        .bearer_auth(wrong_audience_token)
        .send()
        .await
        .expect("request should execute");
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn admin_hard_delete_video_removes_video_and_writes_audit_log() {
    let app = spawn_app_with_admin().await;
    let user_token = app
        .login_as_dev("delete_owner", "delete_owner@example.com")
        .await;

    let user = shared::entities::users::Entity::find()
        .filter(shared::entities::users::Column::Username.eq("delete_owner"))
        .one(&app.db)
        .await
        .expect("db query should succeed")
        .expect("user should exist");
    let video_id = app.create_dummy_video(user.id, false).await;

    let admin_token = build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin");
    let client = Client::new();
    let response = client
        .delete(format!("{}/admin/videos/{}/hard", app.address, video_id))
        .bearer_auth(admin_token)
        .send()
        .await
        .expect("request should execute");
    assert_eq!(response.status().as_u16(), 200);

    let deleted_video = shared::entities::videos::Entity::find_by_id(video_id)
        .one(&app.db)
        .await
        .expect("db query should succeed");
    assert!(deleted_video.is_none(), "Video must be hard-deleted");

    let audit = shared::entities::admin_audit_logs::Entity::find()
        .filter(shared::entities::admin_audit_logs::Column::Action.eq("hard_delete_video"))
        .filter(shared::entities::admin_audit_logs::Column::TargetId.eq(video_id.to_string()))
        .one(&app.db)
        .await
        .expect("db query should succeed");
    assert!(audit.is_some(), "Expected an admin audit log entry");

    // Ensure user auth still works after admin operation.
    let me_response = client
        .get(format!("{}/users/me", app.address))
        .bearer_auth(user_token)
        .send()
        .await
        .expect("request should execute");
    assert_eq!(me_response.status().as_u16(), 200);
}

#[tokio::test]
async fn admin_routes_bypass_public_ip_rate_limiter() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();

    for _ in 0..8 {
        let token = build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin");
        let response = client
            .get(format!("{}/admin/db/tables", app.address))
            .bearer_auth(token)
            .send()
            .await
            .expect("request should execute");
        assert_eq!(
            response.status().as_u16(),
            200,
            "admin route should bypass global IP limiter"
        );
    }

    // Public endpoints still use the global IP limiter (configured to 5/min in spawn_app_with_admin).
    let mut saw_429 = false;
    for _ in 0..8 {
        let response = client
            .get(format!("{}/health", app.address))
            .send()
            .await
            .expect("request should execute");
        if response.status().as_u16() == 429 {
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "public routes should still be rate-limited");
}

#[tokio::test]
async fn admin_routes_disabled_when_feature_flag_is_off() {
    let app = spawn_app().await;
    let client = Client::new();

    let response = client
        .get(format!("{}/admin/db/tables", app.address))
        .send()
        .await
        .expect("request should execute");
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn admin_as_user_routes_cover_regular_surface() {
    let app = spawn_app_with_admin().await;
    let _user_token = app
        .login_as_dev("surface_user", "surface_user@example.com")
        .await;

    let user = shared::entities::users::Entity::find()
        .filter(shared::entities::users::Column::Username.eq("surface_user"))
        .one(&app.db)
        .await
        .expect("db query should succeed")
        .expect("user should exist");
    let _video_id = app.create_dummy_video(user.id, false).await;

    let client = Client::new();

    let feed_response = client
        .get(format!("{}/admin/users/{}/feed", app.address, user.id))
        .query(&[("page", "0"), ("sort", "latest"), ("include_nsfw", "true")])
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("request should execute");
    assert_eq!(feed_response.status().as_u16(), 200);

    let session_response = client
        .post(format!("{}/admin/users/{}/session", app.address, user.id))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("request should execute");
    assert_eq!(session_response.status().as_u16(), 200);
    let session_json: serde_json::Value = session_response
        .json()
        .await
        .expect("response should parse");
    assert!(session_json["access_token"].is_string());
    assert!(session_json["refresh_token"].is_string());

    let videos_response = client
        .get(format!("{}/admin/users/{}/videos", app.address, user.id))
        .query(&[("page", "1"), ("per_page", "20")])
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("request should execute");
    assert_eq!(videos_response.status().as_u16(), 200);
}
