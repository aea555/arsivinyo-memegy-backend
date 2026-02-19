mod common;

use common::spawn_app;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, Set};
use shared::entities::{users, videos};
use uuid::Uuid;

async fn create_extension_token(
    app_address: &str,
    access_token: &str,
    scopes: Option<Vec<&str>>,
) -> serde_json::Value {
    let client = reqwest::Client::new();
    let mut payload = serde_json::json!({
        "platform": "android",
        "device_id_hash": "1234567890abcdef1234567890abcdef",
    });
    if let Some(scopes) = scopes {
        payload["requested_scopes"] =
            serde_json::Value::Array(scopes.into_iter().map(|s| s.into()).collect());
    }

    let res = client
        .post(format!("{}/auth/extension/session", app_address))
        .header("Authorization", format!("Bearer {}", access_token))
        .json(&payload)
        .send()
        .await
        .expect("create extension session failed");
    assert_eq!(res.status(), StatusCode::OK);
    res.json().await.expect("invalid json")
}

#[tokio::test]
async fn extension_session_mint_and_scope_enforcement_works() {
    let app = spawn_app().await;
    let user_token = app
        .login_as_dev("ext_scope_user", "ext_scope@example.com")
        .await;

    let session =
        create_extension_token(&app.address, &user_token, Some(vec!["keyboard.search"])).await;
    let ext_token = session["extension_access_token"].as_str().unwrap();

    let search_res = reqwest::Client::new()
        .get(format!("{}/videos/search/keyboard", app.address))
        .header("Authorization", format!("Bearer {}", ext_token))
        .query(&[("q", "test")])
        .send()
        .await
        .unwrap();
    assert_eq!(search_res.status(), StatusCode::OK);

    let send_res = reqwest::Client::new()
        .post(format!(
            "{}/videos/{}/send-ticket",
            app.address,
            Uuid::new_v4()
        ))
        .header("Authorization", format!("Bearer {}", ext_token))
        .json(&serde_json::json!({
            "nonce": "nonce_scope_check_1",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(send_res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn keyboard_search_and_send_ticket_redeem_flow_works() {
    let app = spawn_app().await;
    let user_token = app
        .login_as_dev("ext_flow_user", "ext_flow@example.com")
        .await;

    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(user_id),
        google_id: Set("ext_flow_google".to_string()),
        username: Set("ext_flow_owner".to_string()),
        email: Set("ext_flow_owner@example.com".to_string()),
        ..Default::default()
    }
    .insert(&app.db)
    .await
    .unwrap();

    let video_id = Uuid::new_v4();
    videos::ActiveModel {
        id: Set(video_id),
        user_id: Set(user_id),
        title: Set(Some("keyboard meme cat".to_string())),
        description: Set(Some("for keyboard tests".to_string())),
        s3_bucket: Set("videos".to_string()),
        s3_key: Set("keyboard-cat.mp4".to_string()),
        status: Set("PUBLISHED".to_string()),
        size_bytes: Set(2048),
        duration_seconds: Set(Some(4)),
        like_count: Set(0),
        is_anonymous: Set(false),
        is_nsfw: Set(Some(false)),
        processing_error_code: Set(None),
        processing_error_message: Set(None),
        failed_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    }
    .insert(&app.db)
    .await
    .unwrap();

    let session = create_extension_token(&app.address, &user_token, None).await;
    let ext_token = session["extension_access_token"].as_str().unwrap();

    let search_res = reqwest::Client::new()
        .get(format!("{}/videos/search/keyboard", app.address))
        .header("Authorization", format!("Bearer {}", ext_token))
        .query(&[("q", "keyboard meme"), ("limit", "10")])
        .send()
        .await
        .unwrap();
    assert_eq!(search_res.status(), StatusCode::OK);
    let items: Vec<serde_json::Value> = search_res.json().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], video_id.to_string());
    assert_eq!(items[0]["safe_size_bytes"], 2048);
    assert_eq!(items[0]["published"], true);
    assert_eq!(items[0]["duration_seconds"], 4);

    let send_res = reqwest::Client::new()
        .post(format!("{}/videos/{}/send-ticket", app.address, video_id))
        .header("Authorization", format!("Bearer {}", ext_token))
        .json(&serde_json::json!({
            "nonce": "nonce_send_ticket_1",
            "host_app_hint": "telegram",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(send_res.status(), StatusCode::OK);
    let send_body: serde_json::Value = send_res.json().await.unwrap();
    let media_url = send_body["media_url"].as_str().unwrap();
    assert!(
        send_body["fallback_share_url"]
            .as_str()
            .unwrap()
            .contains("keyboard-cat.mp4")
    );

    let redirect_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let redeem_1 = redirect_client
        .get(format!("{}{}", app.address, media_url))
        .header("Authorization", format!("Bearer {}", ext_token))
        .send()
        .await
        .unwrap();
    assert!(redeem_1.status().is_redirection());

    let redeem_2 = redirect_client
        .get(format!("{}{}", app.address, media_url))
        .header("Authorization", format!("Bearer {}", ext_token))
        .send()
        .await
        .unwrap();
    assert_eq!(redeem_2.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn send_ticket_nonce_replay_is_rejected() {
    let app = spawn_app().await;
    let user_token = app.login_as_dev("nonce_user", "nonce@example.com").await;
    let ext_session = create_extension_token(&app.address, &user_token, None).await;
    let ext_token = ext_session["extension_access_token"].as_str().unwrap();

    let owner_id = Uuid::new_v4();
    users::ActiveModel {
        id: Set(owner_id),
        google_id: Set("nonce_owner_google".to_string()),
        username: Set("nonce_owner".to_string()),
        email: Set("nonce_owner@example.com".to_string()),
        ..Default::default()
    }
    .insert(&app.db)
    .await
    .unwrap();

    let video_id = Uuid::new_v4();
    videos::ActiveModel {
        id: Set(video_id),
        user_id: Set(owner_id),
        title: Set(Some("nonce video".to_string())),
        description: Set(None),
        s3_bucket: Set("videos".to_string()),
        s3_key: Set("nonce-video.mp4".to_string()),
        status: Set("PUBLISHED".to_string()),
        size_bytes: Set(1500),
        duration_seconds: Set(Some(2)),
        like_count: Set(0),
        is_anonymous: Set(false),
        is_nsfw: Set(Some(false)),
        processing_error_code: Set(None),
        processing_error_message: Set(None),
        failed_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    }
    .insert(&app.db)
    .await
    .unwrap();

    let payload = serde_json::json!({
        "nonce": "same_nonce_for_replay",
    });
    let first = reqwest::Client::new()
        .post(format!("{}/videos/{}/send-ticket", app.address, video_id))
        .header("Authorization", format!("Bearer {}", ext_token))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = reqwest::Client::new()
        .post(format!("{}/videos/{}/send-ticket", app.address, video_id))
        .header("Authorization", format!("Bearer {}", ext_token))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn logout_revokes_extension_session() {
    let app = spawn_app().await;
    let user_token = app.login_as_dev("revoke_user", "revoke@example.com").await;
    let session = create_extension_token(&app.address, &user_token, None).await;
    let ext_token = session["extension_access_token"].as_str().unwrap();

    let logout_res = reqwest::Client::new()
        .post(format!("{}/auth/logout", app.address))
        .header("Authorization", format!("Bearer {}", user_token))
        .send()
        .await
        .unwrap();
    assert_eq!(logout_res.status(), StatusCode::OK);

    let after_revoke = reqwest::Client::new()
        .get(format!("{}/videos/search/keyboard", app.address))
        .header("Authorization", format!("Bearer {}", ext_token))
        .query(&[("q", "test")])
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), StatusCode::UNAUTHORIZED);
}
