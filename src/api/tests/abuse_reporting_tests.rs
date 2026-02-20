mod common;

use common::{
    TEST_ADMIN_AUDIENCE, TEST_ADMIN_ISSUER, TEST_ADMIN_KID, TEST_ADMIN_PRIVATE_KEY_PEM, spawn_app,
    spawn_app_with_admin,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, StatusCode};
use serde::Serialize;
use serde_json::json;
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
async fn report_video_is_idempotent_and_visible_in_my_reports() {
    let app = spawn_app().await;
    let client = Client::new();

    let owner_token = app.login_as_dev("owner_a", "owner_a@example.com").await;
    let reporter_token = app
        .login_as_dev("reporter_a", "reporter_a@example.com")
        .await;

    let owner_me = client
        .get(format!("{}/users/me", app.address))
        .bearer_auth(&owner_token)
        .send()
        .await
        .expect("owner me request should execute");
    assert_eq!(owner_me.status(), StatusCode::OK);
    let owner_me_json: serde_json::Value = owner_me.json().await.expect("json");
    let owner_id: Uuid = owner_me_json["id"]
        .as_str()
        .expect("owner id")
        .parse()
        .expect("uuid");

    let video_id = app.create_dummy_video(owner_id, false).await;

    let first = client
        .post(format!("{}/videos/{}/report", app.address, video_id))
        .bearer_auth(&reporter_token)
        .json(&json!({
            "reason_codes": ["GRAPHIC_OR_DISTURBING"],
            "details": "first report"
        }))
        .send()
        .await
        .expect("report should execute");
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_json: serde_json::Value = first.json().await.expect("json");
    assert_eq!(first_json["created"], true);
    let report_id = first_json["report"]["id"]
        .as_str()
        .expect("report id")
        .to_string();

    let second = client
        .post(format!("{}/videos/{}/report", app.address, video_id))
        .bearer_auth(&reporter_token)
        .json(&json!({
            "reason_codes": ["GRAPHIC_OR_DISTURBING", "MURDER_OR_SERIOUS_INJURY"],
            "details": "updated report"
        }))
        .send()
        .await
        .expect("report should execute");
    assert_eq!(second.status(), StatusCode::OK);
    let second_json: serde_json::Value = second.json().await.expect("json");
    assert_eq!(second_json["created"], false);
    assert_eq!(second_json["report"]["id"], report_id);
    assert_eq!(second_json["report"]["details"], "updated report");

    let my_reports = client
        .get(format!("{}/users/me/reports", app.address))
        .bearer_auth(&reporter_token)
        .send()
        .await
        .expect("my reports should execute");
    assert_eq!(my_reports.status(), StatusCode::OK);
    let my_reports_json: serde_json::Value = my_reports.json().await.expect("json");
    let items = my_reports_json["items"].as_array().expect("items array");
    assert!(!items.is_empty());
    assert_eq!(items[0]["report"]["id"], report_id);
}

#[tokio::test]
async fn csam_report_auto_quarantines_video_and_blocks_download() {
    let app = spawn_app().await;
    let client = Client::new();
    let redirect_client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("redirect client should build");

    let owner_token = app.login_as_dev("owner_b", "owner_b@example.com").await;
    let reporter_token = app
        .login_as_dev("reporter_b", "reporter_b@example.com")
        .await;

    let owner_me = client
        .get(format!("{}/users/me", app.address))
        .bearer_auth(&owner_token)
        .send()
        .await
        .expect("owner me request should execute");
    let owner_me_json: serde_json::Value = owner_me.json().await.expect("json");
    let owner_id: Uuid = owner_me_json["id"]
        .as_str()
        .expect("owner id")
        .parse()
        .expect("uuid");

    let video_id = app.create_dummy_video(owner_id, false).await;

    let before_download = redirect_client
        .get(format!("{}/videos/{}/download", app.address, video_id))
        .bearer_auth(&reporter_token)
        .send()
        .await
        .expect("download request should execute");
    assert!(before_download.status().is_redirection());

    let report = client
        .post(format!("{}/videos/{}/report", app.address, video_id))
        .bearer_auth(&reporter_token)
        .json(&json!({
            "reason_codes": ["CHILD_SEXUAL_ABUSE_MATERIAL"],
            "details": "critical"
        }))
        .send()
        .await
        .expect("report request should execute");
    assert_eq!(report.status(), StatusCode::CREATED);
    let report_json: serde_json::Value = report.json().await.expect("json");
    assert_eq!(report_json["report"]["status"], "auto_quarantined");
    assert_eq!(report_json["report"]["auto_quarantined"], true);

    let after_download = redirect_client
        .get(format!("{}/videos/{}/download", app.address, video_id))
        .bearer_auth(&reporter_token)
        .send()
        .await
        .expect("download request should execute");
    assert_eq!(after_download.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_can_list_resolve_and_moderate_reported_video() {
    let app = spawn_app_with_admin().await;
    let client = Client::new();

    let owner_token = app.login_as_dev("owner_c", "owner_c@example.com").await;
    let reporter_token = app
        .login_as_dev("reporter_c", "reporter_c@example.com")
        .await;

    let owner_me = client
        .get(format!("{}/users/me", app.address))
        .bearer_auth(&owner_token)
        .send()
        .await
        .expect("owner me request should execute");
    let owner_me_json: serde_json::Value = owner_me.json().await.expect("json");
    let owner_id: Uuid = owner_me_json["id"]
        .as_str()
        .expect("owner id")
        .parse()
        .expect("uuid");

    let video_id = app.create_dummy_video(owner_id, false).await;

    let report = client
        .post(format!("{}/videos/{}/report", app.address, video_id))
        .bearer_auth(&reporter_token)
        .json(&json!({
            "reason_codes": ["GRAPHIC_OR_DISTURBING"],
            "details": "manual review"
        }))
        .send()
        .await
        .expect("report request should execute");
    assert_eq!(report.status(), StatusCode::CREATED);
    let report_json: serde_json::Value = report.json().await.expect("json");
    let report_id = report_json["report"]["id"].as_str().expect("report id");

    let list = client
        .get(format!("{}/admin/abuse/reports", app.address))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .send()
        .await
        .expect("list request should execute");
    assert_eq!(list.status(), StatusCode::OK);
    let list_json: serde_json::Value = list.json().await.expect("json");
    assert!(
        list_json["items"]
            .as_array()
            .expect("array")
            .iter()
            .any(|it| it["report"]["id"] == report_id)
    );

    let resolve = client
        .post(format!("{}/admin/abuse/reports/{}", app.address, report_id))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({
            "resolution_code": "no_violation",
            "status": "rejected"
        }))
        .send()
        .await
        .expect("resolve request should execute");
    assert_eq!(resolve.status(), StatusCode::OK);
    let resolve_json: serde_json::Value = resolve.json().await.expect("json");
    assert_eq!(resolve_json["report"]["status"], "rejected");

    let moderation = client
        .post(format!(
            "{}/admin/moderation/videos/{}/action",
            app.address, video_id
        ))
        .bearer_auth(build_admin_token(TEST_ADMIN_AUDIENCE, "superadmin"))
        .json(&json!({
            "action": "remove",
            "reason_code": "ADMIN_POLICY",
            "source_report_id": report_id
        }))
        .send()
        .await
        .expect("moderation request should execute");
    assert_eq!(moderation.status(), StatusCode::OK);
    let moderation_json: serde_json::Value = moderation.json().await.expect("json");
    assert_eq!(moderation_json["moderation_state"], "REMOVED");
}
