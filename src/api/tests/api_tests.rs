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
    let (video_id, _) = app.init_upload(&token_a, "vid.mp4", 100).await;
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
        .post(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to like video");

    assert_eq!(response.status().as_u16(), 201); // CREATED

    // Verify count
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(video.like_count, 1);

    // Duplicate Like
    let response: Response = client
        .post(&format!("{}/videos/{}/like", app.address, video_id))
        .header("Authorization", format!("Bearer {}", token_b))
        .send()
        .await
        .expect("Failed to like video again");

    assert_eq!(response.status().as_u16(), 200); // OK (Idempotent)

    // Verify count remains 1
    let video = videos::Entity::find_by_id(video_uuid)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(video.like_count, 1);
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
        size_bytes: Set(100),
        like_count: Set(0),
        is_anonymous: Set(false),
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
        size_bytes: Set(100),
        like_count: Set(0),
        is_anonymous: Set(false),
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
}

#[tokio::test]
async fn security_access_control_works() {
    let app = spawn_app().await;
    let client = Client::new();

    // User A uploads
    let token_a = app.login_as_dev("user_a_sec", "a_sec@example.com").await;
    let (video_id, _) = app.init_upload(&token_a, "private.mp4", 100).await;
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

    let (video_id, _) = app.init_upload(&token, "dl.mp4", 100).await;
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
            .init_upload(&token, &format!("bulk{}.mp4", i), 100)
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
