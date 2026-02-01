mod common;
use common::spawn_app;
use reqwest::Client;

#[tokio::test]
async fn health_check_works() {
    let app = spawn_app().await;
    let client = Client::new();

    let response = client
        .get(&format!("{}/health", app.address))
        .send()
        .await
        .expect("Failed to execute request");

    assert!(response.status().is_success());
    assert_eq!(response.text().await.unwrap(), "OK");
}

#[tokio::test]
async fn auth_flow_works() {
    let app = spawn_app().await;
    let client = Client::new();

    // 1. Login
    let token = app.login_as_dev("testuser", "test@example.com").await;
    assert!(!token.is_empty(), "Token should not be empty");

    // 2. Get Profile
    let response = client
        .get(&format!("{}/users/me", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to execute request");

    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["username"], "testuser");
    assert_eq!(body["email"], "test@example.com");

    // 3. Logout
    let response = client
        .post(&format!("{}/auth/logout", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to execute request");
    assert!(response.status().is_success());

    // 4. Verify Token Revoked
    let response = client
        .get(&format!("{}/users/me", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to execute request");
    assert_eq!(response.status().as_u16(), 401);
}
