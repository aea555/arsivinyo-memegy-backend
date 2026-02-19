mod common;

use api::cache::otc_cache::{OtcSignupRequiredData, OtcTokenData};
use reqwest::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use shared::entities::users;

async fn set_redis_json(
    app: &common::TestApp,
    key: &str,
    value: serde_json::Value,
    ttl_secs: usize,
) {
    let client = redis::Client::open(app._config.valkey_url.clone()).expect("redis client");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let payload = serde_json::to_string(&value).expect("serialize");
    let _: () = redis::cmd("SET")
        .arg(key)
        .arg(payload)
        .arg("EX")
        .arg(ttl_secs)
        .query_async(&mut conn)
        .await
        .expect("set redis");
}

#[tokio::test]
async fn exchange_otc_returns_username_required_for_pending_signup() {
    let app = common::spawn_app().await;

    let otc_code = "otc_pending_signup_1";
    let payload = serde_json::to_value(OtcTokenData::SignupRequired(OtcSignupRequiredData {
        signup_ticket: "signup_ticket_1".to_string(),
        suggested_username: "john_doe".to_string(),
        required_terms_version: "v1".to_string(),
        terms_url: Some("https://example.com/terms".to_string()),
        requires_age_confirmation: true,
    }))
    .expect("serialize otc payload");

    set_redis_json(&app, &format!("otc:{}", otc_code), payload, 60).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/auth/exchange-otc", app.address))
        .json(&json!({ "code": otc_code }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"], "username_required");
    assert_eq!(body["signup_ticket"], "signup_ticket_1");
    assert_eq!(body["required_terms_version"], "v1");
    assert_eq!(body["requires_age_confirmation"], true);
}

#[tokio::test]
async fn signup_complete_is_idempotent_for_same_ticket() {
    let app = common::spawn_app().await;
    let signup_ticket = "signup_ticket_idempotent";

    set_redis_json(
        &app,
        &format!("signup:ticket:{}", signup_ticket),
        json!({
            "google_id": "google_new_user_1",
            "email": "newuser1@example.com",
            "avatar_url": "https://example.com/avatar.png",
            "source": "web",
            "created_at": chrono::Utc::now().to_rfc3339(),
        }),
        600,
    )
    .await;

    let client = reqwest::Client::new();
    let req_body = json!({
        "signup_ticket": signup_ticket,
        "username": "new_user_1",
        "age_confirmed": true,
        "terms_version": "v1"
    });

    let first = client
        .post(format!("{}/auth/signup/complete", app.address))
        .json(&req_body)
        .send()
        .await
        .expect("first signup");
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: serde_json::Value = first.json().await.expect("json");

    let second = client
        .post(format!("{}/auth/signup/complete", app.address))
        .json(&req_body)
        .send()
        .await
        .expect("second signup");
    assert_eq!(second.status(), StatusCode::OK);
    let second_body: serde_json::Value = second.json().await.expect("json");

    assert_eq!(first_body["user"]["id"], second_body["user"]["id"]);
    assert_eq!(first_body["access_token"], second_body["access_token"]);
    assert_eq!(first_body["refresh_token"], second_body["refresh_token"]);
}

#[tokio::test]
async fn signup_complete_returns_conflict_when_username_taken() {
    let app = common::spawn_app().await;
    let _token = app.login_as_dev("zzq_777", "taken@example.com").await;

    let signup_ticket = "signup_ticket_conflict";
    set_redis_json(
        &app,
        &format!("signup:ticket:{}", signup_ticket),
        json!({
            "google_id": "google_new_user_2",
            "email": "newuser2@example.com",
            "avatar_url": "https://example.com/avatar.png",
            "source": "web",
            "created_at": chrono::Utc::now().to_rfc3339(),
        }),
        600,
    )
    .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/auth/signup/complete", app.address))
        .json(&json!({
            "signup_ticket": signup_ticket,
            "username": "zzq_777",
            "age_confirmed": true,
            "terms_version": "v1"
        }))
        .send()
        .await
        .expect("request failed");

    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn update_username_supports_idempotency_key_and_cache_replay() {
    let app = common::spawn_app().await;
    let token = app
        .login_as_dev("profile_user", "profile_user@example.com")
        .await;

    let client = reqwest::Client::new();
    let first = client
        .put(format!("{}/users/me/username", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .header("Idempotency-Key", "idem-user-1")
        .json(&json!({ "username": "xqz_7788" }))
        .send()
        .await
        .expect("update failed");
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: serde_json::Value = first.json().await.expect("json");
    assert_eq!(first_body["username"], "xqz_7788");

    let replay = client
        .put(format!("{}/users/me/username", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .header("Idempotency-Key", "idem-user-1")
        .json(&json!({ "username": "xqz_7788" }))
        .send()
        .await
        .expect("replay failed");
    assert_eq!(replay.status(), StatusCode::OK);

    let mismatch = client
        .put(format!("{}/users/me/username", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .header("Idempotency-Key", "idem-user-1")
        .json(&json!({ "username": "xqz_8899" }))
        .send()
        .await
        .expect("mismatch failed");
    assert_eq!(mismatch.status(), StatusCode::CONFLICT);

    let me = client
        .get(format!("{}/users/me", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("me failed");
    assert_eq!(me.status(), StatusCode::OK);
    let me_body: serde_json::Value = me.json().await.expect("json");
    assert_eq!(me_body["username"], "xqz_7788");

    let user = users::Entity::find()
        .filter(users::Column::Email.eq("profile_user@example.com"))
        .one(&app.db)
        .await
        .expect("query user")
        .expect("user exists");
    assert_eq!(user.username, "xqz_7788");
    assert_eq!(user.username_normalized.as_deref(), Some("xqz_7788"));
}

#[tokio::test]
async fn legacy_blank_username_user_can_set_username() {
    let app = common::spawn_app().await;
    let token = app.login_as_dev("", "legacy_blank@example.com").await;

    let client = reqwest::Client::new();
    let resp = client
        .put(format!("{}/users/me/username", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "username": "legacy_fixed" }))
        .send()
        .await
        .expect("update failed");

    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["username"], "legacy_fixed");
}
