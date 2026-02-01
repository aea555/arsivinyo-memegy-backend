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

#[async_trait::async_trait]
pub trait StorageBackend: Send + Sync {
    async fn delete_file(&self, bucket: &str, key: &str) -> Result<()>;
    async fn generate_presigned_put(
        &self,
        bucket: &str,
        key: &str,
        expires_in: Duration,
    ) -> Result<String>;
    async fn generate_presigned_get(
        &self,
        bucket: &str,
        key: &str,
        expires_in: Duration,
    ) -> Result<String>;
    async fn file_exists(&self, bucket: &str, key: &str) -> Result<bool>;
    async fn get_file_size(&self, bucket: &str, key: &str) -> Result<u64>;
    async fn download_file(&self, bucket: &str, key: &str, dest_path: &Path) -> Result<()>;
    async fn upload_file(
        &self,
        bucket: &str,
        key: &str,
        src_path: &Path,
        content_type: &str,
    ) -> Result<()>;
}

#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    public_client: Client,
}

impl S3Storage {
    pub async fn new(config: &Config) -> Self {
        let credentials = Credentials::new(
            &config.minio_access_key,
            &config.minio_secret_key,
            None,
            None,
            "static",
        );

        let s3_config = aws_sdk_s3::Config::builder()
            .credentials_provider(SharedCredentialsProvider::new(credentials.clone()))
            .endpoint_url(&config.minio_endpoint)
            .region(Region::new("us-east-1")) // MinIO defaults
            .behavior_version(BehaviorVersion::latest())
            .force_path_style(true) // Required for MinIO
            .build();

        let client = Client::from_conf(s3_config);

        // Separate client for Presigned URLs (Client sees this endpoint)
        let s3_public_config = aws_sdk_s3::Config::builder()
            .credentials_provider(SharedCredentialsProvider::new(credentials))
            .endpoint_url(&config.minio_public_endpoint)
            .region(Region::new("us-east-1"))
            .behavior_version(BehaviorVersion::latest())
            .force_path_style(true)
            .build();

        let public_client = Client::from_conf(s3_public_config);

        Self {
            client,
            public_client,
        }
    }
}

#[async_trait::async_trait]
impl StorageBackend for S3Storage {
    async fn delete_file(&self, bucket: &str, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await?;
        Ok(())
    }

    async fn generate_presigned_put(
        &self,
        bucket: &str,
        key: &str,
        expires_in: Duration,
    ) -> Result<String> {
        let presigning_config = PresigningConfig::expires_in(expires_in)?;

        let presigned_request = self
            .public_client
            .put_object()
            .bucket(bucket)
            .key(key)
            .presigned(presigning_config)
            .await?;

        Ok(presigned_request.uri().to_string())
    }

    async fn generate_presigned_get(
        &self,
        bucket: &str,
        key: &str,
        expires_in: Duration,
    ) -> Result<String> {
        let presigning_config = PresigningConfig::expires_in(expires_in)?;

        let presigned_request = self
            .public_client
            .get_object()
            .bucket(bucket)
            .key(key)
            .presigned(presigning_config)
            .await?;

        Ok(presigned_request.uri().to_string())
    }

    async fn file_exists(&self, bucket: &str, key: &str) -> Result<bool> {
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

    async fn get_file_size(&self, bucket: &str, key: &str) -> Result<u64> {
        let output = self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await?;

        Ok(output.content_length.unwrap_or(0) as u64)
    }

    async fn download_file(&self, bucket: &str, key: &str, dest_path: &Path) -> Result<()> {
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

    async fn upload_file(
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
