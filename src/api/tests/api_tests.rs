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
async fn refresh_token_works_and_rotates_tokens() {
    let app = spawn_app().await;
    let client = Client::new();

    let login_response = client
        .post(&format!("{}/auth/dev/login", app.address))
        .json(&serde_json::json!({
            "username": "refresh_user",
            "email": "refresh@example.com"
        }))
        .send()
        .await
        .expect("Failed to login");
    assert!(login_response.status().is_success());
    let login_body: serde_json::Value = login_response.json().await.unwrap();
    let old_access = login_body["access_token"].as_str().unwrap().to_string();
    let old_refresh = login_body["refresh_token"].as_str().unwrap().to_string();

    let refresh_response = client
        .post(&format!("{}/auth/refresh", app.address))
        .json(&serde_json::json!({
            "access_token": old_access,
            "refresh_token": old_refresh
        }))
        .send()
        .await
        .expect("Failed to refresh token");

    let status = refresh_response.status().as_u16();
    let body_text = refresh_response.text().await.unwrap();
    assert_eq!(
        status, 200,
        "refresh failed with status={}, body={}",
        status, body_text
    );
    let refresh_body: serde_json::Value = serde_json::from_str(&body_text).unwrap();
    let new_access = refresh_body["access_token"].as_str().unwrap();
    let new_refresh = refresh_body["refresh_token"].as_str().unwrap();
    assert!(!new_access.is_empty());
    assert!(!new_refresh.is_empty());
    assert_ne!(refresh_body["refresh_token"], login_body["refresh_token"]);
}

#[tokio::test]
async fn refresh_token_invalid_input_returns_401_not_500() {
    let app = spawn_app().await;
    let client = Client::new();

    let login_response = client
        .post(&format!("{}/auth/dev/login", app.address))
        .json(&serde_json::json!({
            "username": "refresh_invalid_user",
            "email": "refresh-invalid@example.com"
        }))
        .send()
        .await
        .expect("Failed to login");
    assert!(login_response.status().is_success());
    let login_body: serde_json::Value = login_response.json().await.unwrap();

    let refresh_response = client
        .post(&format!("{}/auth/refresh", app.address))
        .json(&serde_json::json!({
            "access_token": login_body["access_token"],
            "refresh_token": "not-a-valid-refresh-token"
        }))
        .send()
        .await
        .expect("Failed to call refresh endpoint");

    let status = refresh_response.status().as_u16();
    let body = refresh_response.text().await.unwrap();
    assert_eq!(status, 401, "status={}, body={}", status, body);
}

#[tokio::test]
async fn video_upload_flow_works() {
    let app = spawn_app().await;

    // 1. Login
    let token = app.login_as_dev("uploader", "uploader@example.com").await;

    // 2. Init Upload
    let (video_id, upload_url) = app.init_upload(&token, "funny_cat.mp4", 1024).await;
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

#[tokio::test]
async fn video_lifecycle_works() {
    let app = spawn_app().await;
    let client = Client::new();
    let token = app
        .login_as_dev("lifecycle_user", "lifecycle@example.com")
        .await;

    // 1. Upload Video
    let (video_id, _) = app.init_upload(&token, "life.mp4", 1024).await;
    app.confirm_upload(&token, &video_id).await;

    // 2. Update Metadata
    let response: Response = client
        .patch(&format!("{}/videos/{}", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "title": "Updated Title",
            "description": "Updated Description"
        }))
        .send()
        .await
        .expect("Failed to update video");

    assert!(response.status().is_success());

    // Verify update via DB
    use sea_orm::EntityTrait;
    use shared::entities::videos;

    let video = videos::Entity::find_by_id(uuid::Uuid::parse_str(&video_id).unwrap())
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(video.title, Some("Updated Title".to_string()));
    assert_eq!(video.description, Some("Updated Description".to_string()));

    // 3. Soft Delete
    let response: Response = client
        .delete(&format!("{}/videos/{}", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to delete video");

    assert!(response.status().is_success());

    // Verify deletion
    let video = videos::Entity::find_by_id(uuid::Uuid::parse_str(&video_id).unwrap())
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();

    assert!(video.deleted_at.is_some());

    // Verify removed from feed
    let response: Response = client
        .get(&format!("{}/feed", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to get feed");

    let items: Vec<serde_json::Value> = response.json().await.unwrap();
    // Should verify items does NOT contain video_id
    let found = items
        .iter()
        .any(|item| item["id"].as_str().unwrap() == video_id);
    assert!(!found, "Deleted video should not be in feed");
}

#[tokio::test]
async fn like_video_works() {
    let app = spawn_app().await;
    let client = Client::new();

    // User A uploads
    let token_a = app.login_as_dev("user_a", "a@example.com").await;
    let (video_id, _) = app.init_upload(&token_a, "vid.mp4", 1024).await;
    app.confirm_upload(&token_a, &video_id).await;

    // Force update to PUBLISHED manually
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use shared::entities::videos;

    let video_uuid = uuid::Uuid::parse_str(&video_id).unwrap();
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: videos::ActiveModel = video.into();
    active.status = Set("PUBLISHED".to_string());
    active.update(&app.db).await.unwrap();

    // User B likes
    let token_b = app.login_as_dev("user_b", "b@example.com").await;

    let response: Response = client
        .put(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to like video");

    assert_eq!(response.status().as_u16(), 200); // OK
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["is_liked"], true);
    assert_eq!(body["like_count"], 1);

    // Verify count
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(video.like_count, 1);

    // Unlike
    let response: Response = client
        .delete(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to unlike video");

    assert_eq!(response.status().as_u16(), 200); // OK
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["is_liked"], false);
    assert_eq!(body["like_count"], 0);

    // Verify count decremented
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(video.like_count, 0);
}

#[tokio::test]
async fn like_video_idempotent_put_delete_works() {
    let app = spawn_app().await;
    let client = Client::new();

    // User A uploads
    let token_a = app.login_as_dev("user_a2", "a2@example.com").await;
    let (video_id, _) = app.init_upload(&token_a, "vid2.mp4", 1024).await;
    app.confirm_upload(&token_a, &video_id).await;

    // Force update to PUBLISHED manually
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use shared::entities::videos;

    let video_uuid = uuid::Uuid::parse_str(&video_id).unwrap();
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: videos::ActiveModel = video.into();
    active.status = Set("PUBLISHED".to_string());
    active.update(&app.db).await.unwrap();

    let token_b = app.login_as_dev("user_b2", "b2@example.com").await;

    // First PUT likes the video.
    let response: Response = client
        .put(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to like video with PUT");
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["is_liked"], true);
    assert_eq!(body["like_count"], 1);

    // Second PUT stays liked and does not increment again.
    let response: Response = client
        .put(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to re-like video with PUT");
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["is_liked"], true);
    assert_eq!(body["like_count"], 1);

    // First DELETE unlikes the video.
    let response: Response = client
        .delete(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to unlike video with DELETE");
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["is_liked"], false);
    assert_eq!(body["like_count"], 0);

    // Second DELETE stays unliked and does not decrement below zero.
    let response: Response = client
        .delete(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to re-unlike video with DELETE");
    assert_eq!(response.status().as_u16(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["is_liked"], false);
    assert_eq!(body["like_count"], 0);
}

#[tokio::test]
async fn user_profile_management_works() {
    let app = spawn_app().await;
    let client = Client::new();
    let token = app
        .login_as_dev("profile_user", "profile@example.com")
        .await;

    // 1. Get Me
    let response: Response = client
        .get(&format!("{}/users/me", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to get profile");

    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["username"], "profile_user");

    // 2. Delete Account
    let response: Response = client
        .delete(&format!("{}/users/me", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to delete account");

    assert_eq!(response.status().as_u16(), 204); // NO_CONTENT

    // 3. Verify Login Fails (or token revoked) in subsequent request
    let response: Response = client
        .get(&format!("{}/users/me", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to get profile after delete");

    // Should be 401 Unauthorized
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn my_videos_returns_playable_url_for_published_only() {
    let app = spawn_app().await;
    let client = Client::new();
    let email = "myvideos@example.com";
    let token = app.login_as_dev("myvideos_user", email).await;

    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
    use shared::entities::{likes, users, videos};

    let user = users::Entity::find()
        .filter(users::Column::Email.eq(email))
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();

    let published_id = uuid::Uuid::new_v4();
    let processing_id = uuid::Uuid::new_v4();

    let published = videos::ActiveModel {
        id: Set(published_id),
        user_id: Set(user.id),
        title: Set(Some("Published video".to_string())),
        description: Set(None),
        s3_bucket: Set("videos".to_string()),
        s3_key: Set("published.mp4".to_string()),
        status: Set("PUBLISHED".to_string()),
        size_bytes: Set(1024),
        duration_seconds: Set(None),
        like_count: Set(3),
        is_anonymous: Set(false),
        processing_error_code: Set(None),
        processing_error_message: Set(None),
        failed_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    };
    published.insert(&app.db).await.unwrap();

    let processing = videos::ActiveModel {
        id: Set(processing_id),
        user_id: Set(user.id),
        title: Set(Some("Processing video".to_string())),
        description: Set(None),
        s3_bucket: Set("raw".to_string()),
        s3_key: Set("processing.mp4".to_string()),
        status: Set("PROCESSING".to_string()),
        size_bytes: Set(1024),
        duration_seconds: Set(None),
        like_count: Set(0),
        is_anonymous: Set(false),
        processing_error_code: Set(None),
        processing_error_message: Set(None),
        failed_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    };
    processing.insert(&app.db).await.unwrap();

    let like = likes::ActiveModel {
        user_id: Set(user.id),
        video_id: Set(published_id),
        ..Default::default()
    };
    like.insert(&app.db).await.unwrap();

    let response: Response = client
        .get(&format!("{}/users/me/videos", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to get my videos");

    assert!(response.status().is_success());
    let body: Vec<serde_json::Value> = response.json().await.unwrap();

    let published_item = body
        .iter()
        .find(|item| item["id"] == published_id.to_string())
        .expect("Published video should exist");
    assert_eq!(
        published_item["url"].as_str(),
        Some("http://mock/videos/published.mp4")
    );
    assert_eq!(published_item["is_liked"], true);

    let processing_item = body
        .iter()
        .find(|item| item["id"] == processing_id.to_string())
        .expect("Processing video should exist");
    assert!(processing_item["url"].is_null());
    assert_eq!(processing_item["is_liked"], false);
}

#[tokio::test]
async fn search_functionality_works() {
    let app = spawn_app().await;
    let client = Client::new();
    let token = app.login_as_dev("search_user", "search@example.com").await;

    // Insert dummy videos with specific titles
    use sea_orm::{ActiveModelTrait, Set};
    use shared::entities::videos;

    // Video 1
    // We need a valid user first
    let user1_id = uuid::Uuid::new_v4();
    use shared::entities::users;
    let user1 = users::ActiveModel {
        id: Set(user1_id),
        username: Set("search_creator_1".to_string()),
        email: Set("sc1@example.com".to_string()),
        google_id: Set("g_sc1".to_string()),
        ..Default::default()
    };
    user1.insert(&app.db).await.unwrap();

    let vid1 = videos::ActiveModel {
        id: Set(uuid::Uuid::new_v4()),
        user_id: Set(user1_id),
        title: Set(Some("Rust Programming Tutorial".to_string())),
        description: Set(Some("Learn Rust".to_string())),
        s3_bucket: Set("raw".to_string()),
        s3_key: Set("key1".to_string()),
        status: Set("PUBLISHED".to_string()),
        size_bytes: Set(1024),
        duration_seconds: Set(None),
        like_count: Set(0),
        is_anonymous: Set(false),
        processing_error_code: Set(None),
        processing_error_message: Set(None),
        failed_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    };
    vid1.insert(&app.db).await.unwrap();

    // Video 2
    let vid2 = videos::ActiveModel {
        id: Set(uuid::Uuid::new_v4()),
        user_id: Set(user1_id),
        title: Set(Some("Cooking Pasta".to_string())),
        description: Set(Some("Food".to_string())),
        s3_bucket: Set("raw".to_string()),
        s3_key: Set("key2".to_string()),
        status: Set("PUBLISHED".to_string()),
        size_bytes: Set(1024),
        duration_seconds: Set(None),
        like_count: Set(0),
        is_anonymous: Set(false),
        processing_error_code: Set(None),
        processing_error_message: Set(None),
        failed_at: Set(None),
        deleted_at: Set(None),
        created_at: Set(chrono::Utc::now().into()),
        updated_at: Set(chrono::Utc::now().into()),
    };
    vid2.insert(&app.db).await.unwrap();

    // 1. Search for "Rust"
    let response: Response = client
        .get(&format!("{}/videos/search", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("q", "Rust")])
        .send()
        .await
        .expect("Failed to search");

    assert!(response.status().is_success());
    let items: Vec<serde_json::Value> = response.json().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Rust Programming Tutorial");
    assert_eq!(items[0]["url"], "http://mock/videos/key1");

    // 2. Search for "Pasta"
    let response: Response = client
        .get(&format!("{}/videos/search", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .query(&[("q", "Pasta")])
        .send()
        .await
        .expect("Failed to search");

    let items: Vec<serde_json::Value> = response.json().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Cooking Pasta");
    assert_eq!(items[0]["url"], "http://mock/videos/key2");
}

#[tokio::test]
async fn security_access_control_works() {
    let app = spawn_app().await;
    let client = Client::new();

    // User A uploads
    let token_a = app.login_as_dev("user_a_sec", "a_sec@example.com").await;
    let (video_id, _) = app.init_upload(&token_a, "private.mp4", 1024).await;
    app.confirm_upload(&token_a, &video_id).await;

    // User B tries to act
    let token_b = app.login_as_dev("user_b_sec", "b_sec@example.com").await;

    // 1. Try to Delete
    let response: Response = client
        .delete(&format!("{}/videos/{}", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to delete video");

    // Should be Not Found (404) or Forbidden (403) depending on impl.
    // Handlers usually return Not Found if filtered by user_id
    assert_eq!(response.status().as_u16(), 404);

    // 2. Try to Update
    let response: Response = client
        .patch(&format!("{}/videos/{}", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .json(&serde_json::json!({"title": "Hacked Title"}))
        .send()
        .await
        .expect("Failed to patch video");

    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn download_flow_works() {
    let app = spawn_app().await;
    // Create client that DOES NOT follow redirects to avoid DNS error on internal minio hostname
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let token = app.login_as_dev("dl_user", "dl@example.com").await;

    let (video_id, _) = app.init_upload(&token, "dl.mp4", 1024).await;
    app.confirm_upload(&token, &video_id).await;

    // Need to publish it
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use shared::entities::videos;
    let video_uuid = uuid::Uuid::parse_str(&video_id).unwrap();
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: videos::ActiveModel = video.into();
    active.status = Set("PUBLISHED".to_string());
    active.update(&app.db).await.unwrap();

    // 1. Get Download URL
    let response: Response = client
        .get(&format!("{}/videos/{}/download", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to get download url");

    // Should be a Redirect (307 Temporary Redirect usually for Redirect::temporary)
    assert!(response.status().is_redirection());
    assert!(response.headers().contains_key("location"));

    // Verify location looks like S3 URL
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    // buckets usually in path or domain, MockStorage uses "http://mock/get"
    assert!(location.contains("videos") || location.contains("mock"));

    // 2. Refresh Download URL (This returns JSON, so standard client is fine too, but custom client works)
    let response: Response = client
        .post(&format!(
            "{}/videos/{}/download/refresh",
            app.address, video_id
        ))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to refresh download url");

    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["download_url"].as_str().is_some());
}

#[tokio::test]
async fn bulk_operations_works() {
    let app = spawn_app().await;
    let client = Client::new();
    let token = app.login_as_dev("bulk_user", "bulk@example.com").await;

    // Upload 3 videos
    let mut video_ids = Vec::new();
    for i in 0..3 {
        let (vid, _) = app
            .init_upload(&token, &format!("bulk{}.mp4", i), 1024)
            .await;
        app.confirm_upload(&token, &vid).await;
        // Force publish
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        use shared::entities::videos;
        let video_uuid = uuid::Uuid::parse_str(&vid).unwrap();
        let video = videos::Entity::find_by_id(video_uuid)
            .one(&app.db)
            .await
            .unwrap()
            .unwrap();
        let mut active: videos::ActiveModel = video.into();
        active.status = Set("PUBLISHED".to_string());
        active.update(&app.db).await.unwrap();
        video_ids.push(vid);
    }

    // 1. Bulk Delete (first 2)
    let ids_to_delete = vec![video_ids[0].clone(), video_ids[1].clone()];
    let response: Response = client
        .post(&format!("{}/videos/bulk-delete", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "video_ids": ids_to_delete
        }))
        .send()
        .await
        .expect("Failed to bulk delete");

    assert!(response.status().is_success());

    // Verify deletion status
    use sea_orm::EntityTrait;
    use shared::entities::videos;

    let v0 = videos::Entity::find_by_id(uuid::Uuid::parse_str(&video_ids[0]).unwrap())
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert!(v0.deleted_at.is_some());

    let v1 = videos::Entity::find_by_id(uuid::Uuid::parse_str(&video_ids[1]).unwrap())
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert!(v1.deleted_at.is_some());

    let v2 = videos::Entity::find_by_id(uuid::Uuid::parse_str(&video_ids[2]).unwrap())
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert!(v2.deleted_at.is_none());

    // 2. Bulk Download (of the remaining one)
    let response: Response = client
        .post(&format!("{}/videos/download/bulk", app.address))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "video_ids": vec![video_ids[2].clone()]
        }))
        .send()
        .await
        .expect("Failed to init bulk download");

    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.unwrap();
    let job_id = body["job_id"].as_str().unwrap();
    assert_eq!(body["status"], "PENDING");

    // 3. Poll Status
    let response: Response = client
        .get(&format!("{}/videos/download/bulk/{}", app.address, job_id))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .expect("Failed to check job status");

    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["job_id"], job_id);
}
