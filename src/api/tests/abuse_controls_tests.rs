mod common;

use common::{
    TEST_ADMIN_AUDIENCE, TEST_ADMIN_ISSUER, TEST_ADMIN_KID, TEST_ADMIN_PRIVATE_KEY_PEM,
    TestAppOptions, spawn_app_with_admin, spawn_app_with_options,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use serde_json::json;
use shared::entities::users;
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
async fn ip_ban_blocks_public_and_admin_routes() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();
    let ban = client
        .put(format!("{}/admin/ip-bans", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({
            "target": "127.0.0.1",
            "reason": "test"
        }))
        .send()
        .await
        .expect("ip ban request should execute");
    assert_eq!(ban.status(), StatusCode::OK);

    let health = client
        .get(format!("{}/health", app.address))
        .send()
        .await
        .expect("health request should execute");
    assert_eq!(health.status(), StatusCode::FORBIDDEN);
    let health_json: serde_json::Value = health.json().await.expect("json");
    assert_eq!(health_json["error"], "ip_banned");

    let admin_tables = client
        .get(format!("{}/admin/db/tables", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("admin request should execute");
    assert_eq!(admin_tables.status(), StatusCode::FORBIDDEN);
    let admin_json: serde_json::Value = admin_tables.json().await.expect("json");
    assert_eq!(admin_json["error"], "ip_banned");

    // Unban from a different spoofed trusted IP.
    let unban = client
        .delete(format!("{}/admin/ip-bans", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .header("cf-connecting-ip", "10.10.10.10")
        .query(&[("target", "127.0.0.1")])
        .send()
        .await
        .expect("unban request should execute");
    assert_eq!(unban.status(), StatusCode::OK);
}

#[tokio::test]
async fn user_ban_blocks_authenticated_and_auth_family_routes() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();
    let login = client
        .post(format!("{}/auth/dev/login", app.address))
        .json(&json!({
            "username": "ban_target",
            "email": "ban_target@example.com"
        }))
        .send()
        .await
        .expect("dev login should execute");
    assert_eq!(login.status(), StatusCode::OK);
    let login_json: serde_json::Value = login.json().await.expect("json");
    let access_token = login_json["access_token"]
        .as_str()
        .expect("access token")
        .to_string();
    let refresh_token = login_json["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_string();

    let user = users::Entity::find()
        .filter(users::Column::Email.eq("ban_target@example.com"))
        .one(&app.db)
        .await
        .expect("db query should succeed")
        .expect("user should exist");

    let ban = client
        .put(format!("{}/admin/users/{}/ban", app.address, user.id))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({ "reason": "policy" }))
        .send()
        .await
        .expect("ban request should execute");
    assert_eq!(ban.status(), StatusCode::OK);

    for path in ["/users/me", "/users/me/onboarding/status"] {
        let response = client
            .get(format!("{}{}", app.address, path))
            .bearer_auth(&access_token)
            .send()
            .await
            .expect("request should execute");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body: serde_json::Value = response.json().await.expect("json");
        assert_eq!(body["error"], "user_banned");
    }

    let refresh = client
        .post(format!("{}/auth/refresh", app.address))
        .json(&json!({
            "access_token": access_token,
            "refresh_token": refresh_token
        }))
        .send()
        .await
        .expect("refresh should execute");
    assert_eq!(refresh.status(), StatusCode::FORBIDDEN);
    let refresh_json: serde_json::Value = refresh.json().await.expect("json");
    assert_eq!(refresh_json["error"], "user_banned");

    let logout = client
        .post(format!("{}/auth/logout", app.address))
        .bearer_auth(&access_token)
        .send()
        .await
        .expect("logout should execute");
    assert_eq!(logout.status(), StatusCode::FORBIDDEN);
    let logout_json: serde_json::Value = logout.json().await.expect("json");
    assert_eq!(logout_json["error"], "user_banned");

    let unban = client
        .delete(format!("{}/admin/users/{}/ban", app.address, user.id))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("unban request should execute");
    assert_eq!(unban.status(), StatusCode::OK);

    let me = client
        .get(format!("{}/users/me", app.address))
        .bearer_auth(&access_token)
        .header("cf-connecting-ip", "10.10.10.20")
        .send()
        .await
        .expect("me request should execute");
    assert_eq!(me.status(), StatusCode::OK);
}

#[tokio::test]
async fn maintenance_precedence_before_ip_ban_on_blocked_routes() {
    let app = spawn_app_with_options(TestAppOptions {
        admin_enabled: true,
        maintenance_mode_enabled: true,
        ..Default::default()
    })
    .await;
    let client = Client::new();
    let ban = client
        .put(format!("{}/admin/ip-bans", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({
            "target": "127.0.0.1",
            "reason": "maintenance_precedence"
        }))
        .send()
        .await
        .expect("ip ban request should execute");
    assert_eq!(ban.status(), StatusCode::OK);

    let terms = client
        .get(format!("{}/system/terms", app.address))
        .send()
        .await
        .expect("terms request should execute");
    assert_eq!(terms.status(), StatusCode::SERVICE_UNAVAILABLE);
    let terms_json: serde_json::Value = terms.json().await.expect("json");
    assert_eq!(terms_json["error"], "maintenance_mode_enabled");

    let exchange = client
        .post(format!("{}/auth/exchange-otc", app.address))
        .json(&json!({ "code": "invalid" }))
        .send()
        .await
        .expect("exchange request should execute");
    assert_eq!(exchange.status(), StatusCode::FORBIDDEN);
    let exchange_json: serde_json::Value = exchange.json().await.expect("json");
    assert_eq!(exchange_json["error"], "ip_banned");
}

#[tokio::test]
async fn admin_parity_endpoints_for_system_and_onboarding_are_available() {
    let app = spawn_app_with_admin().await;
    let user_token = app
        .login_as_dev("parity_user", "parity_user@example.com")
        .await;
    let _ = user_token;
    let user = users::Entity::find()
        .filter(users::Column::Email.eq("parity_user@example.com"))
        .one(&app.db)
        .await
        .expect("db query should succeed")
        .expect("user should exist");
    let client = Client::new();

    let terms = client
        .get(format!("{}/admin/system/terms", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("terms request should execute");
    assert_eq!(terms.status(), StatusCode::OK);

    let read_only = client
        .get(format!("{}/admin/system/read-only", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("read-only request should execute");
    assert_eq!(read_only.status(), StatusCode::OK);

    let maintenance = client
        .get(format!("{}/admin/system/maintenance", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("maintenance request should execute");
    assert_eq!(maintenance.status(), StatusCode::OK);

    let onboarding_status = client
        .get(format!(
            "{}/admin/users/{}/onboarding/status",
            app.address, user.id
        ))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("onboarding status should execute");
    assert_eq!(onboarding_status.status(), StatusCode::OK);

    let onboarding_complete = client
        .post(format!(
            "{}/admin/users/{}/onboarding/complete",
            app.address, user.id
        ))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({
            "age_confirmed": true,
            "terms_version": "v1"
        }))
        .send()
        .await
        .expect("onboarding complete should execute");
    assert_eq!(onboarding_complete.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_upload_init_bypasses_read_only_while_user_route_is_blocked() {
    let app = spawn_app_with_options(TestAppOptions {
        admin_enabled: true,
        read_only_mode_enabled: true,
        ..Default::default()
    })
    .await;
    let client = Client::new();
    let user_token = app
        .login_as_dev("readonly_parity", "readonly_parity@example.com")
        .await;

    let user = users::Entity::find()
        .filter(users::Column::Email.eq("readonly_parity@example.com"))
        .one(&app.db)
        .await
        .expect("db query should succeed")
        .expect("user should exist");

    let user_init = client
        .post(format!("{}/videos/init", app.address))
        .bearer_auth(&user_token)
        .json(&json!({
            "filename": "blocked.mp4",
            "size_bytes": 1024,
            "is_nsfw": false
        }))
        .send()
        .await
        .expect("user init should execute");
    assert_eq!(user_init.status(), StatusCode::SERVICE_UNAVAILABLE);

    let admin_init = client
        .post(format!(
            "{}/admin/users/{}/videos/init",
            app.address, user.id
        ))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({
            "filename": "allowed.mp4",
            "size_bytes": 1024,
            "is_nsfw": false
        }))
        .send()
        .await
        .expect("admin init should execute");
    assert_eq!(admin_init.status(), StatusCode::OK);
}

#[tokio::test]
async fn security_investigation_endpoints_return_user_ip_associations() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();

    let login = client
        .post(format!("{}/auth/dev/login", app.address))
        .json(&json!({
            "username": "investigate_user",
            "email": "investigate_user@example.com"
        }))
        .send()
        .await
        .expect("dev login should execute");
    assert_eq!(login.status(), StatusCode::OK);

    let user = users::Entity::find()
        .filter(users::Column::Email.eq("investigate_user@example.com"))
        .one(&app.db)
        .await
        .expect("db query should succeed")
        .expect("user should exist");

    let per_user = client
        .get(format!(
            "{}/admin/security/users/{}/ips",
            app.address, user.id
        ))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .query(&[("window_days", "30"), ("limit", "20"), ("cursor", "0")])
        .send()
        .await
        .expect("investigation request should execute");
    assert_eq!(per_user.status(), StatusCode::OK);
    let per_user_json: serde_json::Value = per_user.json().await.expect("json");
    assert!(
        per_user_json["items"]
            .as_array()
            .map(|items| !items.is_empty())
            .unwrap_or(false),
        "expected at least one user-ip association"
    );

    let per_ip = client
        .get(format!(
            "{}/admin/security/ips/127.0.0.1/users",
            app.address
        ))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .query(&[("window_days", "30"), ("limit", "20"), ("cursor", "0")])
        .send()
        .await
        .expect("investigation request should execute");
    assert_eq!(per_ip.status(), StatusCode::OK);
    let per_ip_json: serde_json::Value = per_ip.json().await.expect("json");
    assert!(
        per_ip_json["items"]
            .as_array()
            .map(|items| !items.is_empty())
            .unwrap_or(false),
        "expected at least one ip-user association"
    );
}
