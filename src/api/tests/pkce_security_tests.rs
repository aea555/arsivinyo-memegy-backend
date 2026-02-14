mod common;

use common::spawn_app;
use reqwest::StatusCode;
use shared::security::{compute_code_challenge, generate_otc};

/// Test 1: Mobile with valid S256 PKCE parameters should be accepted
#[tokio::test]
async fn test_mobile_with_valid_pkce_accepted() {
    let app = spawn_app().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Generate valid PKCE parameters
    let verifier = generate_otc(); // 64-char hex string
    let challenge = compute_code_challenge(&verifier);

    // Call login endpoint with mobile source and PKCE
    let response = client
        .get(&format!("{}/auth/google/login", app.address))
        .query(&[
            ("source", "mobile"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("code_verifier", &verifier),
        ])
        .send()
        .await
        .expect("Failed to send request");

    // Should redirect to Google (303 SEE_OTHER is returned by reqwest for redirects)
    assert!(
        response.status() == StatusCode::FOUND || response.status() == StatusCode::SEE_OTHER,
        "Expected redirect (302/303), got: {}",
        response.status()
    );
    assert!(response.headers().get("location").is_some());
}

/// Test 2: Mobile without PKCE should be rejected
#[tokio::test]
async fn test_mobile_without_pkce_rejected() {
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    let response = client
        .get(&format!("{}/auth/google/login", app.address))
        .query(&[("source", "mobile")])
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("PKCE required"));
}

/// Test 3: Invalid challenge length should be rejected
#[tokio::test]
async fn test_invalid_challenge_length_rejected() {
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    // Challenge too short (should be 43 chars for S256)
    let invalid_challenge = "tooshort";
    let verifier = generate_otc();

    let response = client
        .get(&format!("{}/auth/google/login", app.address))
        .query(&[
            ("source", "mobile"),
            ("code_challenge", invalid_challenge),
            ("code_challenge_method", "S256"),
            ("code_verifier", &verifier),
        ])
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Invalid code_challenge")
    );
}

/// Test 4: Plain method should be rejected
#[tokio::test]
async fn test_plain_method_rejected() {
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    let verifier = generate_otc();

    let response = client
        .get(&format!("{}/auth/google/login", app.address))
        .query(&[
            ("source", "mobile"),
            ("code_challenge", &verifier), // Using verifier directly as challenge
            ("code_challenge_method", "plain"),
            ("code_verifier", &verifier),
        ])
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("plain method not allowed")
    );
}

/// Test 5: Invalid challenge format (contains invalid chars) should be rejected
#[tokio::test]
async fn test_invalid_challenge_format_rejected() {
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    // Contains invalid characters (+ and /)
    let invalid_challenge = "ABC+DEF/GHI=JKLMNO" // Padding also invalid
        .repeat(3); // Make it long enough
    let verifier = generate_otc();

    let response = client
        .get(&format!("{}/auth/google/login", app.address))
        .query(&[
            ("source", "mobile"),
            ("code_challenge", &invalid_challenge[..43]), // Truncate to 43
            ("code_challenge_method", "S256"),
            ("code_verifier", &verifier),
        ])
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Test 6: Web without PKCE should be accepted (PKCE optional for web)
#[tokio::test]
async fn test_web_without_pkce_accepted() {
    let app = spawn_app().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let response = client
        .get(&format!("{}/auth/google/login", app.address))
        .query(&[("source", "web")])
        .send()
        .await
        .expect("Failed to send request");

    // Should redirect to Google
    assert!(
        response.status() == StatusCode::FOUND || response.status() == StatusCode::SEE_OTHER,
        "Expected redirect, got: {}",
        response.status()
    );
}

/// Test 7: Rate limiting on OTC exchange
#[tokio::test]
async fn test_otc_exchange_rate_limiting() {
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    // Attempt to exchange invalid OTCs multiple times to trigger rate limit
    let invalid_otc = "invalid_otc_code_12345";

    // First 5 attempts should get 400 (bad request)
    for i in 0..5 {
        let response = client
            .post(&format!("{}/auth/exchange-otc", app.address))
            .json(&serde_json::json!({
                "code": format!("{}_{}", invalid_otc, i)
            }))
            .send()
            .await
            .expect("Failed to send request");

        let status = response.status();
        if status != StatusCode::BAD_REQUEST {
            let body = response.text().await.unwrap_or_default();
            panic!(
                "Attempt {} should be bad request, got {}: {}",
                i + 1,
                status,
                body
            );
        }
    }

    // 6th attempt should be rate limited
    let response = client
        .post(&format!("{}/auth/exchange-otc", app.address))
        .json(&serde_json::json!({
            "code": format!("{}_6", invalid_otc)
        }))
        .send()
        .await
        .expect("Failed to send request");

    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "Should be rate limited after 5 attempts"
    );
}

/// Test 8: Invalid OTC exchange returns 400
#[tokio::test]
async fn test_invalid_otc_rejected() {
    let app = spawn_app().await;
    let client = reqwest::Client::new();

    let response = client
        .post(&format!("{}/auth/exchange-otc", app.address))
        .json(&serde_json::json!({
            "code": "definitely_not_a_valid_otc"
        }))
        .send()
        .await
        .expect("Failed to send request");

    let status = response.status();
    if status != StatusCode::BAD_REQUEST {
        let body = response.text().await.unwrap_or_default();
        panic!("Expected 400 BAD_REQUEST, got {}: {}", status, body);
    }
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("Invalid or expired"),
        "Expected error message 'Invalid or expired', got: {:?}",
        body["error"]
    );
}

/// Test 9: PKCE helpers validation (unit-style but in integration context)
#[tokio::test]
async fn test_pkce_helpers() {
    use shared::security::{validate_code_verifier, verify_pkce_challenge};

    // Valid verifier
    let valid_verifier = generate_otc();
    assert!(validate_code_verifier(&valid_verifier));

    // Invalid verifier (too short)
    assert!(!validate_code_verifier("short"));

    // Verify challenge
    let challenge = compute_code_challenge(&valid_verifier);
    assert!(verify_pkce_challenge(&valid_verifier, &challenge));

    // Wrong verifier
    let wrong_verifier = generate_otc();
    assert!(!verify_pkce_challenge(&wrong_verifier, &challenge));
}
