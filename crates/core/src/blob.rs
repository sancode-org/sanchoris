//! S3: the values over [`INLINE_LIMIT`] and the records of threads. The objects of a User are
//! under `u/<user>/`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aws_sdk_s3::primitives::ByteStream;
use sha2::{Digest, Sha256};

use crate::db::DbError;

/// The largest value that DynamoDB holds; a larger one goes to S3.
pub const INLINE_LIMIT: usize = 32 * 1024;

/// The prefix of the objects of a User.
pub fn user_prefix(user: &str) -> String {
    format!("u/{user}/")
}

/// The key of a value of a User, named by the SHA-256 of its bytes under `path`: writing the
/// same value again writes the same object.
pub fn content_key(user: &str, path: &str, body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}{path}/{hex}", user_prefix(user))
}

/// The objects of sanchoris: an S3 bucket, or a map in memory for tests and `nix run`.
#[derive(Debug, Clone)]
pub enum Blobs {
    S3 {
        client: aws_sdk_s3::Client,
        bucket: String,
    },
    Memory(Arc<Mutex<BTreeMap<String, Vec<u8>>>>),
}

impl Blobs {
    pub fn memory() -> Blobs {
        Blobs::Memory(Arc::default())
    }

    /// The bucket of `SANCHORIS_BUCKET`, with the AWS configuration of the environment.
    pub async fn from_env() -> Result<Blobs, DbError> {
        let bucket = std::env::var("SANCHORIS_BUCKET")
            .map_err(|_| DbError::MissingEnv("SANCHORIS_BUCKET"))?;
        let config = aws_config::load_from_env().await;
        Ok(Blobs::S3 {
            client: aws_sdk_s3::Client::new(&config),
            bucket,
        })
    }

    pub async fn put(&self, key: &str, body: Vec<u8>) -> Result<(), DbError> {
        match self {
            Blobs::S3 { client, bucket } => {
                client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(ByteStream::from(body))
                    .send()
                    .await
                    .map_err(|e| DbError::Unavailable(format!("S3: {e:?}")))?;
                Ok(())
            }
            Blobs::Memory(map) => {
                map.lock().expect("the blobs").insert(key.to_string(), body);
                Ok(())
            }
        }
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, DbError> {
        match self {
            Blobs::S3 { client, bucket } => {
                match client.get_object().bucket(bucket).key(key).send().await {
                    Ok(out) => {
                        let bytes = out
                            .body
                            .collect()
                            .await
                            .map_err(|e| DbError::Unavailable(format!("S3: {e}")))?;
                        Ok(Some(bytes.into_bytes().to_vec()))
                    }
                    Err(e) if e.as_service_error().is_some_and(|s| s.is_no_such_key()) => Ok(None),
                    Err(e) => Err(DbError::Unavailable(format!("S3: {e:?}"))),
                }
            }
            Blobs::Memory(map) => Ok(map.lock().expect("the blobs").get(key).cloned()),
        }
    }

    /// The keys under a prefix, for tests.
    pub fn keys(&self, prefix: &str) -> Vec<String> {
        match self {
            Blobs::S3 { .. } => Vec::new(),
            Blobs::Memory(map) => map
                .lock()
                .expect("the blobs")
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect(),
        }
    }
}
