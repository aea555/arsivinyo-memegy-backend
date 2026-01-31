use anyhow::Result;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{
    config::{Credentials, SharedCredentialsProvider},
    presigning::PresigningConfig,
    primitives::ByteStream,
    Client,
};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::config::Config;

#[derive(Clone)]
pub struct StorageService {
    client: Client,
}

impl StorageService {
    pub async fn new(config: &Config) -> Self {
        let credentials = Credentials::new(
            &config.minio_access_key,
            &config.minio_secret_key,
            None,
            None,
            "static",
        );

        let s3_config = aws_sdk_s3::Config::builder()
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .endpoint_url(&config.minio_endpoint)
            .region(Region::new("us-east-1")) // MinIO defaults
            .behavior_version(BehaviorVersion::latest())
            .force_path_style(true) // Required for MinIO
            .build();

        let client = Client::from_conf(s3_config);
        Self { client }
    }

    pub async fn generate_presigned_put(
        &self,
        bucket: &str,
        key: &str,
        expires_in: Duration,
    ) -> Result<String> {
        let presigning_config = PresigningConfig::expires_in(expires_in)?;

        let presigned_request = self
            .client
            .put_object()
            .bucket(bucket)
            .key(key)
            .presigned(presigning_config)
            .await?;

        Ok(presigned_request.uri().to_string())
    }

    pub async fn generate_presigned_get(
        &self,
        bucket: &str,
        key: &str,
        expires_in: Duration,
    ) -> Result<String> {
        let presigning_config = PresigningConfig::expires_in(expires_in)?;

        let presigned_request = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .presigned(presigning_config)
            .await?;

        Ok(presigned_request.uri().to_string())
    }

    pub async fn file_exists(&self, bucket: &str, key: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                let service_error = e.into_service_error();
                if service_error.is_not_found() {
                    Ok(false)
                } else {
                    Err(service_error.into())
                }
            }
        }
    }

    pub async fn download_file(&self, bucket: &str, key: &str, dest_path: &Path) -> Result<()> {
        let mut object = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await?;

        let mut file = tokio::fs::File::create(dest_path).await?;
        while let Some(bytes) = object.body.try_next().await? {
            file.write_all(&bytes).await?;
        }
        Ok(())
    }

    pub async fn upload_file(
        &self,
        bucket: &str,
        key: &str,
        src_path: &Path,
        content_type: &str,
    ) -> Result<()> {
        let body = ByteStream::from_path(src_path).await?;
        self.client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(body)
            .content_type(content_type)
            .send()
            .await?;
        Ok(())
    }
}
