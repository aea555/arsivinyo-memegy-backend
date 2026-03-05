mod common;

use common::{
    TEST_ADMIN_AUDIENCE, TEST_ADMIN_ISSUER, TEST_ADMIN_KID, TEST_ADMIN_PRIVATE_KEY_PEM,
    TestAppOptions, spawn_app_with_options,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
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
async fn read_only_mode_blocks_upload_endpoints_only() {
    let app = spawn_app_with_options(TestAppOptions {
        read_only_mode_enabled: true,
        ..Default::default()
    })
    .await;
    let client = Client::new();
    let user_token = app
        .login_as_dev("readonly_user", "readonly_user@example.com")
        .await;

    let read_only_status = client
        .get(format!("{}/system/read-only", app.address))
        .bearer_auth(&user_token)
        .send()
        .await
        .expect("read-only status request should execute");
    assert_eq!(read_only_status.status(), StatusCode::OK);
    let read_only_json: serde_json::Value = read_only_status.json().await.expect("json");
    assert_eq!(read_only_json["enabled"], true);

    let init_upload = client
        .post(format!("{}/videos/init", app.address))
        .bearer_auth(&user_token)
        .json(&serde_json::json!({
            "filename": "read_only_test.mp4",
            "size_bytes": 2048,
            "is_nsfw": false
        }))
        .send()
        .await
        .expect("init upload request should execute");
    assert_eq!(init_upload.status(), StatusCode::SERVICE_UNAVAILABLE);
    let init_upload_json: serde_json::Value = init_upload.json().await.expect("json");
    assert_eq!(init_upload_json["error"], "read_only_mode_enabled");

    let init_upload_anon = client
        .post(format!("{}/videos/init/anonymous", app.address))
        .bearer_auth(&user_token)
        .json(&serde_json::json!({
            "filename": "read_only_anon_test.mp4",
            "size_bytes": 2048,
            "is_nsfw": false
        }))
        .send()
        .await
        .expect("init anonymous upload request should execute");
    assert_eq!(init_upload_anon.status(), StatusCode::SERVICE_UNAVAILABLE);
    let init_upload_anon_json: serde_json::Value = init_upload_anon.json().await.expect("json");
    assert_eq!(init_upload_anon_json["error"], "read_only_mode_enabled");

    let confirm_upload = client
        .post(format!("{}/videos/{}/confirm", app.address, Uuid::new_v4()))
        .bearer_auth(&user_token)
        .send()
        .await
        .expect("confirm upload request should execute");
    assert_eq!(confirm_upload.status(), StatusCode::SERVICE_UNAVAILABLE);
    let confirm_upload_json: serde_json::Value = confirm_upload.json().await.expect("json");
    assert_eq!(confirm_upload_json["error"], "read_only_mode_enabled");

    let feed = client
        .get(format!("{}/feed", app.address))
        .bearer_auth(&user_token)
        .query(&[("include_nsfw", "true")])
        .send()
        .await
        .expect("feed request should execute");
    assert_eq!(feed.status(), StatusCode::OK);
}

#[tokio::test]
async fn terms_endpoint_returns_configured_embedded_content() {
    let app = spawn_app_with_options(TestAppOptions {
        terms_content: Some("# Terms\n\nBy using this app, you agree to the policy."),
        terms_content_type: Some("text/markdown"),
        terms_current_version: Some("v7"),
        ..Default::default()
    })
    .await;

    let client = Client::new();
    let terms = client
        .get(format!("{}/system/terms", app.address))
        .send()
        .await
        .expect("terms request should execute");
    assert_eq!(terms.status(), StatusCode::OK);

    let terms_json: serde_json::Value = terms.json().await.expect("json");
    assert_eq!(terms_json["version"], "v7");
    assert_eq!(terms_json["default_language"], "en");
    assert_eq!(
        terms_json["available_languages"],
        serde_json::json!(["en", "tr"])
    );
    assert_eq!(
        terms_json["documents"]["en"]["content"],
        "# Terms\n\nBy using this app, you agree to the policy."
    );
    assert_eq!(
        terms_json["documents"]["en"]["content_type"],
        "text/markdown"
    );
    assert_eq!(
        terms_json["documents"]["en"]["url"],
        "https://example.com/terms"
    );
    assert!(
        terms_json["documents"]["en"]["content_sha256"]
            .as_str()
            .is_some(),
        "content sha256 should be present for embedded terms"
    );

    let terms_en = client
        .get(format!("{}/system/terms/en", app.address))
        .send()
        .await
        .expect("terms en request should execute");
    assert_eq!(terms_en.status(), StatusCode::OK);
    let terms_en_json: serde_json::Value = terms_en.json().await.expect("json");
    assert_eq!(terms_en_json["language"], "en");
    assert_eq!(terms_en_json["version"], "v7");

    let terms_tr = client
        .get(format!("{}/system/terms/tr", app.address))
        .send()
        .await
        .expect("terms tr request should execute");
    assert_eq!(terms_tr.status(), StatusCode::OK);
    let terms_tr_json: serde_json::Value = terms_tr.json().await.expect("json");
    assert_eq!(terms_tr_json["language"], "tr");

    let terms_en_cached = client
        .get(format!("{}/system/terms/en", app.address))
        .header(
            "If-None-Match",
            terms_en_json["content_sha256"]
                .as_str()
                .map(|v| format!("\"{}\"", v))
                .expect("etag"),
        )
        .send()
        .await
        .expect("conditional terms en request should execute");
    assert_eq!(terms_en_cached.status(), StatusCode::NOT_MODIFIED);

    let terms_unknown = client
        .get(format!("{}/system/terms/de", app.address))
        .send()
        .await
        .expect("terms unknown locale request should execute");
    assert_eq!(terms_unknown.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn maintenance_mode_allows_only_allowlisted_routes() {
    let app = spawn_app_with_options(TestAppOptions {
        maintenance_mode_enabled: true,
        ..Default::default()
    })
    .await;

    let client_no_redirect = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client should build");
    let client = Client::new();

    let health = client_no_redirect
        .get(format!("{}/health", app.address))
        .send()
        .await
        .expect("health request should execute");
    assert_eq!(health.status(), StatusCode::OK);

    let google_login = client_no_redirect
        .get(format!("{}/auth/google/login", app.address))
        .send()
        .await
        .expect("google login request should execute");
    assert_ne!(google_login.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(google_login.status().is_redirection());

    let exchange = client
        .post(format!("{}/auth/exchange-otc", app.address))
        .json(&serde_json::json!({ "code": "invalid_otc_code" }))
        .send()
        .await
        .expect("exchange otc request should execute");
    assert_eq!(exchange.status(), StatusCode::BAD_REQUEST);

    let dev_login = client
        .post(format!("{}/auth/dev/login", app.address))
        .json(&serde_json::json!({
            "username": "maintenance_user",
            "email": "maintenance_user@example.com"
        }))
        .send()
        .await
        .expect("dev login should execute");
    assert_eq!(dev_login.status(), StatusCode::OK);
    let login_json: serde_json::Value = dev_login.json().await.expect("json");
    let user_token = login_json["access_token"]
        .as_str()
        .expect("access token should exist");

    let maintenance_status = client
        .get(format!("{}/system/maintenance", app.address))
        .bearer_auth(user_token)
        .send()
        .await
        .expect("maintenance status request should execute");
    assert_eq!(maintenance_status.status(), StatusCode::OK);
    let maintenance_json: serde_json::Value = maintenance_status.json().await.expect("json");
    assert_eq!(maintenance_json["enabled"], true);

    let read_only_status = client
        .get(format!("{}/system/read-only", app.address))
        .bearer_auth(user_token)
        .send()
        .await
        .expect("read-only status request should execute");
    assert_eq!(read_only_status.status(), StatusCode::SERVICE_UNAVAILABLE);
    let read_only_status_json: serde_json::Value = read_only_status.json().await.expect("json");
    assert_eq!(read_only_status_json["error"], "maintenance_mode_enabled");

    let terms = client
        .get(format!("{}/system/terms", app.address))
        .send()
        .await
        .expect("terms request should execute");
    assert_eq!(terms.status(), StatusCode::SERVICE_UNAVAILABLE);
    let terms_json: serde_json::Value = terms.json().await.expect("json");
    assert_eq!(terms_json["error"], "maintenance_mode_enabled");

    let feed = client
        .get(format!("{}/feed", app.address))
        .bearer_auth(user_token)
        .query(&[("include_nsfw", "true")])
        .send()
        .await
        .expect("feed request should execute");
    assert_eq!(feed.status(), StatusCode::SERVICE_UNAVAILABLE);
    let feed_json: serde_json::Value = feed.json().await.expect("json");
    assert_eq!(feed_json["error"], "maintenance_mode_enabled");

    let logout = client
        .post(format!("{}/auth/logout", app.address))
        .bearer_auth(user_token)
        .send()
        .await
        .expect("logout request should execute");
    assert_eq!(logout.status(), StatusCode::SERVICE_UNAVAILABLE);
    let logout_json: serde_json::Value = logout.json().await.expect("json");
    assert_eq!(logout_json["error"], "maintenance_mode_enabled");
}

#[tokio::test]
async fn admin_routes_bypass_maintenance_and_read_only_guards() {
    let app = spawn_app_with_options(TestAppOptions {
        admin_enabled: true,
        read_only_mode_enabled: true,
        maintenance_mode_enabled: true,
        ..Default::default()
    })
    .await;
    let client = Client::new();

    let admin_tables = client
        .get(format!("{}/admin/db/tables", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("admin tables request should execute");
    assert_eq!(admin_tables.status(), StatusCode::OK);

    let user_token = app
        .login_as_dev("maintenance_user_2", "maintenance_user_2@example.com")
        .await;
    let feed = client
        .get(format!("{}/feed", app.address))
        .bearer_auth(user_token)
        .query(&[("include_nsfw", "true")])
        .send()
        .await
        .expect("feed request should execute");
    assert_eq!(feed.status(), StatusCode::SERVICE_UNAVAILABLE);
    let feed_json: serde_json::Value = feed.json().await.expect("json");
    assert_eq!(feed_json["error"], "maintenance_mode_enabled");
}
