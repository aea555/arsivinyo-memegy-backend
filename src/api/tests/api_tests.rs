mod common;
use common::spawn_app;
use reqwest::{Client, Response};

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

#[tokio::test]
async fn video_upload_flow_works() {
    let app = spawn_app().await;

    // 1. Login
    let token = app.login_as_dev("uploader", "uploader@example.com").await;

    // 2. Init Upload
    let (video_id, upload_url) = app.init_upload(&token, "funny_cat.mp4", 1024 * 1024).await;
    assert!(!video_id.is_empty());
    assert!(!upload_url.is_empty());

    // 3. Confirm Upload
    // Note: In real world, we would PUT to S3 here.
    // MockStorage always says "exists=true", so we can skip the actual upload.
    app.confirm_upload(&token, &video_id).await;

    // 4. Verify DB State

    use sea_orm::EntityTrait;
    use shared::entities::videos;

    let video = videos::Entity::find_by_id(uuid::Uuid::parse_str(&video_id).unwrap())
        .one(&app.db)
        .await
        .expect("Failed to fetch video")
        .expect("Video not found in DB");

    assert_eq!(video.status, "PROCESSING");
    assert_eq!(video.s3_key, format!("{}/{}.mp4", video.user_id, video.id));
}

#[tokio::test]
async fn feed_pagination_and_sorting_works() {
    let app = spawn_app().await;

    // 1. Setup Data
    let token = app.login_as_dev("feed_user", "feed@example.com").await;

    // Extract user_id from token or DB
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use shared::entities::users;
    let user = users::Entity::find()
        .filter(users::Column::Username.eq("feed_user"))
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();

    // Insert 25 videos (default page size is 20 in TestApp config)
    for _ in 0..25 {
        app.create_dummy_video(user.id, false).await;
    }
    // Insert 1 anonymous video
    app.create_dummy_video(user.id, true).await;

    let client = reqwest::Client::new();

    // 2. Test Pagination (Page 1)
    let response: Response = client
        .get(&format!("{}/feed", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("page", "1"), ("sort", "latest")])
        .send()
        .await
        .expect("Failed to get feed");

    // Debug logging
    let status = response.status();
    println!("Response status: {}", status);
    let items: Vec<serde_json::Value> = response.json().await.unwrap();
    println!("Page 1 items count: {}", items.len());
    if items.len() != 20 {
        println!("Items: {:#?}", items);
    }
    // Should return 20 items (page size limit)
    assert_eq!(items.len(), 20);

    // 3. Test Pagination (Page 2)
    let response: Response = client
        .get(&format!("{}/feed", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("page", "2"), ("sort", "latest")])
        .send()
        .await
        .expect("Failed to get feed");

    let items: Vec<serde_json::Value> = response.json().await.unwrap();
    // Total 26 videos. Page 1 had 20. Page 2 should have 6.
    assert_eq!(items.len(), 6);

    // 4. Test Anonymity
    // Find the anonymous video in the response (it might be in page 1 or 2 depending on sort order/insertion time)
    // Since we sort by 'latest' and inserted anonymous last, it should be the FIRST item of Page 1.

    // Check Page 1 again for the anonymous item check
    let response: Response = client
        .get(&format!("{}/feed", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("page", "1"), ("sort", "latest")])
        .send()
        .await
        .expect("Failed to get feed");

    let items: Vec<serde_json::Value> = response.json().await.unwrap();
    let latest_video = &items[0];

    // Verify it is anonymous
    assert!(
        latest_video["uploader"].is_null(),
        "Anonymous video should have null uploader"
    );

    // Verify regular video has uploader
    let regular_video = &items[1];
    assert!(
        regular_video["uploader"].is_object(),
        "Regular video should have uploader info"
    );
    assert_eq!(regular_video["uploader"]["username"], "feed_user");
}
