//! What the control plane reaches outside DynamoDB, behind traits: the connections of the
//! Thread channel (API Gateway's `PostToConnection`), the queue of jobs (SQS FIFO), the
//! one-time schedules (EventBridge Scheduler), the Workspaces (ECS, a Host, or a local
//! process), the presigned URLs of S3, and GitHub. `crate::local` has the local ones.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sanchoris_core::blob::Blobs;
use sanchoris_core::db::Db;
use sanchoris_core::secret::SecretKey;
use sanchoris_core::token::TokenKey;
use serde_json::Value;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The connection is gone.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the connection is gone")]
pub struct Gone;

pub trait Connections: Send + Sync {
    fn send(&self, connection: &str, frame: String) -> BoxFuture<'_, Result<(), Gone>>;
}

/// A queue of jobs; the jobs of one thread (`thread` of the job) are taken in order.
pub trait Queue: Send + Sync {
    fn send(&self, job: Value) -> BoxFuture<'_, ()>;
}

/// Sends a job to the queue at a time (milliseconds since the Unix epoch).
pub trait Scheduler: Send + Sync {
    fn schedule(&self, job: Value, due: u64) -> BoxFuture<'_, ()>;
}

/// What a Workspace starts with (`architecture.md`「Thread channel」, 起動の指示).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub user: String,
    pub thread: String,
    pub workspace: u64,
    /// `cloud` or the name of a Host.
    pub target: String,
    pub architecture: String,
    pub bootstrap_token: String,
    pub channel_url: String,
}

/// Starts the Workspaces of threads.
pub trait Launcher: Send + Sync {
    fn launch(&self, launch: Launch) -> BoxFuture<'_, Result<(), String>>;
}

/// An upload of thread records: an S3 presigned POST whose policy allows keys under `prefix`.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub url: String,
    pub fields: serde_json::Map<String, Value>,
}

/// Presigned URLs of the objects of sanchoris.
pub trait Urls: Send + Sync {
    fn download(&self, key: &str, expires_at: u64) -> String;
    fn upload(&self, prefix: &str, expires_at: u64) -> Upload;
}

/// GitHub for the threads of a User: the installation token and the thread branches.
pub trait GitHub: Send + Sync {
    /// A token and when it expires (milliseconds since the Unix epoch).
    fn token(&self, user: &str) -> BoxFuture<'_, Result<(String, u64), String>>;
    /// Makes `branch` from the default branch of `repo` (`github.com/owner/name`) when it is
    /// missing.
    fn create_branch<'a>(
        &'a self,
        user: &'a str,
        repo: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// The control plane: the tables, the objects, the keys and the outside.
#[derive(Clone)]
pub struct Ctx {
    pub db: Db,
    pub blobs: Blobs,
    pub tokens: TokenKey,
    pub secrets: SecretKey,
    pub connections: Arc<dyn Connections>,
    pub queue: Arc<dyn Queue>,
    pub scheduler: Arc<dyn Scheduler>,
    pub launcher: Arc<dyn Launcher>,
    pub urls: Arc<dyn Urls>,
    pub github: Arc<dyn GitHub>,
    /// The WebSocket URL of the Thread channel.
    pub channel_url: String,
}

/// Milliseconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_millis() as u64
}

/// A time in milliseconds as RFC 3339 in UTC, to the second.
pub fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_writes_utc_times() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339(1_795_842_000_000), "2026-11-28T05:00:00Z");
        assert_eq!(super::rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
    }
}
