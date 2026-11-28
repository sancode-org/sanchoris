//! DynamoDB Local for the integration tests: one server per test process, and new tables
//! for each test.

use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials, Region};

use crate::db::{Db, Tables};
use crate::schema::create_tables;

static SERVER: OnceLock<u16> = OnceLock::new();
static TABLES: AtomicU64 = AtomicU64::new(0);

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("a free local port")
}

/// The port of the DynamoDB Local of this process, started on the first call. It stops when
/// the process ends.
pub fn server_port() -> u16 {
    *SERVER.get_or_init(|| {
        let port = free_port();
        let parent = std::process::id();
        let script = format!(
            "dynamodb-local -inMemory -disableTelemetry -port {port} & child=$!; \
             while kill -0 {parent} 2>/dev/null; do sleep 1; done; kill $child"
        );
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("dynamodb-local is on PATH (run inside `nix develop`)");
        std::thread::spawn(move || child.wait());
        let deadline = Instant::now() + Duration::from_secs(60);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(
                Instant::now() < deadline,
                "DynamoDB Local did not listen on port {port}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        port
    })
}

/// A client of the DynamoDB Local of this process.
pub fn client() -> aws_sdk_dynamodb::Client {
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("ap-northeast-1"))
        .endpoint_url(format!("http://127.0.0.1:{}", server_port()))
        .credentials_provider(Credentials::new("local", "local", None, None, "local"))
        .build();
    aws_sdk_dynamodb::Client::from_conf(config)
}

/// New `main` and `audit` tables in the DynamoDB Local of this process.
pub async fn tables() -> Db {
    let n = TABLES.fetch_add(1, Ordering::SeqCst);
    let id = format!("{}-{n}", std::process::id());
    let db = Db::new(
        client(),
        Tables {
            main: format!("main-{id}"),
            audit: format!("audit-{id}"),
        },
    );
    let mut attempts = 0;
    loop {
        attempts += 1;
        match create_tables(&db).await {
            Ok(()) => return db,
            Err(_) if attempts < 50 => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(e) => panic!("the tables of DynamoDB Local: {e}"),
        }
    }
}

/// A client of new tables in the DynamoDB Local of this process, made once for the process:
/// each call has its own client, for the runtime of its test.
pub async fn shared_tables() -> Db {
    static SHARED: tokio::sync::OnceCell<Tables> = tokio::sync::OnceCell::const_new();
    let tables = SHARED
        .get_or_init(|| async { tables().await.tables().clone() })
        .await
        .clone();
    Db::new(client(), tables)
}
