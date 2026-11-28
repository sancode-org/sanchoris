//! The control plane on one machine, for `nix run` and the end-to-end tests: the dispatcher
//! reads the stream of DynamoDB Local, one queue in memory runs the jobs in order, a timer per
//! schedule, the Workspaces are `sanchoris-agent` processes, the presigned URLs are signed
//! routes of the local server over the objects in memory, and the repositories are bare
//! repositories in a directory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_sdk_dynamodbstreams::types::{AttributeValue, ShardIteratorType};
use sanchoris_core::token::TokenKey;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::jobs;
use crate::ports::{
    BoxFuture, Connections, Ctx, GitHub, Gone, Launch, Launcher, Queue, Scheduler, Upload, Urls,
    now,
};

/// The open sockets of the Thread channel, by connection ID.
#[derive(Default, Clone)]
pub struct LocalConnections {
    sockets: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<String>>>>,
}

impl LocalConnections {
    pub fn open(&self, id: &str) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.sockets
            .lock()
            .expect("the sockets")
            .insert(id.to_string(), tx);
        rx
    }

    pub fn close(&self, id: &str) {
        self.sockets.lock().expect("the sockets").remove(id);
    }
}

impl Connections for LocalConnections {
    fn send(&self, connection: &str, frame: String) -> BoxFuture<'_, Result<(), Gone>> {
        let sent = self
            .sockets
            .lock()
            .expect("the sockets")
            .get(connection)
            .is_some_and(|tx| tx.send(frame).is_ok());
        Box::pin(async move { if sent { Ok(()) } else { Err(Gone) } })
    }
}

/// The queue of jobs in memory, taken one at a time.
#[derive(Clone)]
pub struct LocalQueue(mpsc::UnboundedSender<Value>);

impl Queue for LocalQueue {
    fn send(&self, job: Value) -> BoxFuture<'_, ()> {
        let _ = self.0.send(job);
        Box::pin(async {})
    }
}

#[derive(Clone)]
pub struct LocalScheduler(mpsc::UnboundedSender<Value>);

impl Scheduler for LocalScheduler {
    fn schedule(&self, job: Value, due: u64) -> BoxFuture<'_, ()> {
        let queue = self.0.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(due.saturating_sub(now()))).await;
            let _ = queue.send(job);
        });
        Box::pin(async {})
    }
}

/// How a local Workspace process ended.
#[derive(Debug, Clone)]
pub struct Exited {
    pub user: String,
    pub thread: String,
    pub workspace: u64,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// Starts each Workspace as a `sanchoris-agent run` process in its own directory under `root`
/// (`<root>/<thread>/<seq>/`, with `agent.pid`).
pub struct LocalLauncher {
    pub root: PathBuf,
    /// The command of `sanchoris-agent`, run with `run` after it.
    pub command: Vec<String>,
    /// Environment variables for the agent, such as `SANCHORIS_GIT_URL_PREFIX` and
    /// `SANCHORIS_WORKER_MODULE`.
    pub env: Vec<(String, String)>,
    pub exited: mpsc::UnboundedSender<Exited>,
}

impl Launcher for LocalLauncher {
    fn launch(&self, launch: Launch) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let dir = self
                .root
                .join(&launch.thread)
                .join(launch.workspace.to_string());
            let io = |e: std::io::Error| e.to_string();
            tokio::fs::create_dir_all(&dir).await.map_err(io)?;
            let (program, args) = self
                .command
                .split_first()
                .ok_or("SANCHORIS_AGENT_COMMAND is empty")?;
            let socket = std::env::temp_dir().join(format!(
                "sanchoris-{}.sock",
                &sanchoris_core::token::random_id()[..12]
            ));
            let mut command = tokio::process::Command::new(program);
            command
                .args(args)
                .arg("run")
                .current_dir(&dir)
                .env_remove("LD_PRELOAD")
                .env_remove("FSPY")
                .env_remove("FSPY_PAYLOAD")
                .env("SANCHORIS_THREAD_ID", &launch.thread)
                .env("SANCHORIS_CHANNEL_URL", &launch.channel_url)
                .env("SANCHORIS_BOOTSTRAP_TOKEN", &launch.bootstrap_token)
                .env("SANCHORIS_STATE_DIR", dir.join("state"))
                .env("SANCHORIS_WORKSPACE_DIR", dir.join("workspace"))
                .env("SANCHORIS_SKILLS_DIR", dir.join("skills"))
                .env("SANCHORIS_SOCKET", &socket)
                .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(
                    std::fs::File::create(dir.join("agent.log"))
                        .map(std::process::Stdio::from)
                        .map_err(io)?,
                );
            let mut child = command.spawn().map_err(io)?;
            if let Some(pid) = child.id() {
                tokio::fs::write(dir.join("agent.pid"), pid.to_string())
                    .await
                    .map_err(io)?;
            }
            let exited = self.exited.clone();
            tokio::spawn(async move {
                let status = child.wait().await;
                let (code, signal) = match status {
                    Ok(s) => {
                        use std::os::unix::process::ExitStatusExt;
                        (s.code(), s.signal())
                    }
                    Err(_) => (None, None),
                };
                let _ = std::fs::remove_file(&socket);
                let _ = exited.send(Exited {
                    user: launch.user,
                    thread: launch.thread,
                    workspace: launch.workspace,
                    code,
                    signal,
                });
            });
            Ok(())
        })
    }
}

/// Presigned URLs as signed routes of the local server: `GET /local/blobs/<key>` and
/// `POST /local/records`.
pub struct LocalUrls {
    pub base: String,
    pub key: TokenKey,
}

impl LocalUrls {
    pub fn download_payload(key: &str, expires_at: u64) -> String {
        format!("GET\n{key}\n{expires_at}")
    }

    pub fn upload_payload(prefix: &str, expires_at: u64) -> String {
        format!("POST\n{prefix}\n{expires_at}")
    }
}

impl Urls for LocalUrls {
    fn download(&self, key: &str, expires_at: u64) -> String {
        let signature = self.key.sign(&Self::download_payload(key, expires_at));
        format!(
            "{}/local/blobs/{key}?expires={expires_at}&signature={signature}",
            self.base
        )
    }

    fn upload(&self, prefix: &str, expires_at: u64) -> Upload {
        let mut fields = Map::new();
        fields.insert("prefix".into(), json!(prefix));
        fields.insert("expires".into(), json!(expires_at.to_string()));
        fields.insert(
            "signature".into(),
            json!(self.key.sign(&Self::upload_payload(prefix, expires_at))),
        );
        Upload {
            url: format!("{}/local/records", self.base),
            fields,
        }
    }
}

/// Bare repositories at `<root>/<repo>` (`<root>/github.com/owner/name`), whose default branch
/// is their `HEAD`; the token is a fixed string.
pub struct LocalGitHub {
    pub root: PathBuf,
}

impl GitHub for LocalGitHub {
    fn token(&self, _user: &str) -> BoxFuture<'_, Result<(String, u64), String>> {
        Box::pin(async { Ok(("ghs_local".to_string(), now() + 60 * 60 * 1000)) })
    }

    fn create_branch<'a>(
        &'a self,
        _user: &'a str,
        repo: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let bare = self.root.join(repo);
            let git = |args: &[&str]| {
                std::process::Command::new("git")
                    .arg("--git-dir")
                    .arg(&bare)
                    .args(args)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .output()
            };
            let exists = git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .map_err(|e| e.to_string())?;
            if exists.status.success() {
                return Ok(());
            }
            let made = git(&["branch", branch, "HEAD"]).map_err(|e| e.to_string())?;
            if made.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&made.stderr).trim().to_string())
            }
        })
    }
}

/// The parts of a local control plane that the server also uses.
pub struct Local {
    pub connections: LocalConnections,
    pub queue: mpsc::UnboundedSender<Value>,
    jobs: mpsc::UnboundedReceiver<Value>,
    pub exited: mpsc::UnboundedSender<Exited>,
    exits: mpsc::UnboundedReceiver<Exited>,
}

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

impl Local {
    pub fn new() -> Local {
        let (queue, jobs) = mpsc::unbounded_channel();
        let (exited, exits) = mpsc::unbounded_channel();
        Local {
            connections: LocalConnections::default(),
            queue,
            jobs,
            exited,
            exits,
        }
    }

    pub fn queue(&self) -> LocalQueue {
        LocalQueue(self.queue.clone())
    }

    pub fn scheduler(&self) -> LocalScheduler {
        LocalScheduler(self.queue.clone())
    }

    /// Runs the dispatcher over the stream of the `main` table, the worker over the queue, and
    /// the stops of the Workspace processes.
    pub fn run(self, ctx: Ctx, streams: aws_sdk_dynamodbstreams::Client) {
        let Local {
            mut jobs,
            mut exits,
            ..
        } = self;
        let worker = ctx.clone();
        tokio::spawn(async move {
            while let Some(job) = jobs.recv().await {
                if std::env::var_os("SANCHORIS_DEBUG").is_some() {
                    eprintln!("sanchoris: job {job}");
                }
                if let Err(e) = jobs::handle(&worker, &job).await {
                    eprintln!("sanchoris: job {job}: {e}");
                }
            }
        });
        let supervisor = ctx.clone();
        tokio::spawn(async move {
            while let Some(e) = exits.recv().await {
                let reason = match (e.code, e.signal) {
                    (_, Some(_)) => "interrupted".to_string(),
                    (Some(0), _) => "interrupted".to_string(),
                    (Some(code), _) => format!("sanchoris-agent exited with code {code}"),
                    (None, None) => "sanchoris-agent stopped".to_string(),
                };
                if let Err(err) = jobs::workspace_stopped(
                    &supervisor,
                    &e.user,
                    &e.thread,
                    e.workspace,
                    e.code.map(i64::from),
                    &reason,
                    now(),
                )
                .await
                {
                    eprintln!("sanchoris: the stop of {} {}: {err}", e.thread, e.workspace);
                }
            }
        });
        tokio::spawn(async move {
            if let Err(e) = dispatch(&ctx, &streams).await {
                eprintln!("sanchoris: the dispatcher stopped: {e}");
            }
        });
    }
}

fn s_of(image: &HashMap<String, AttributeValue>, name: &str) -> Option<String> {
    match image.get(name) {
        Some(AttributeValue::S(v)) => Some(v.clone()),
        _ => None,
    }
}

/// Reads the stream of the `main` table, shard by shard, and routes each new image in order.
async fn dispatch(ctx: &Ctx, streams: &aws_sdk_dynamodbstreams::Client) -> Result<(), String> {
    let table = ctx
        .db
        .client()
        .describe_table()
        .table_name(&ctx.db.tables().main)
        .send()
        .await
        .map_err(|e| format!("{e:?}"))?;
    let arn = table
        .table()
        .and_then(|t| t.latest_stream_arn())
        .ok_or("the main table has no stream")?
        .to_string();
    let mut iterators: HashMap<String, Option<String>> = HashMap::new();
    loop {
        let description = streams
            .describe_stream()
            .stream_arn(&arn)
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        for shard in description
            .stream_description()
            .map(|d| d.shards())
            .unwrap_or_default()
        {
            let Some(id) = shard.shard_id() else { continue };
            if iterators.contains_key(id) {
                continue;
            }
            let it = streams
                .get_shard_iterator()
                .stream_arn(&arn)
                .shard_id(id)
                .shard_iterator_type(ShardIteratorType::TrimHorizon)
                .send()
                .await
                .map_err(|e| format!("{e:?}"))?;
            iterators.insert(id.to_string(), it.shard_iterator().map(str::to_string));
        }
        if std::env::var_os("SANCHORIS_DEBUG").is_some() && iterators.is_empty() {
            eprintln!("sanchoris: the stream {arn} has no shards");
        }
        let mut idle = true;
        for iterator in iterators.values_mut() {
            let Some(current) = iterator.clone() else {
                continue;
            };
            let out = match streams.get_records().shard_iterator(current).send().await {
                Ok(out) => out,
                Err(e) => {
                    eprintln!("sanchoris: the stream: {e:?}");
                    continue;
                }
            };
            for record in out.records() {
                let Some(image) = record.dynamodb().and_then(|d| d.new_image()) else {
                    continue;
                };
                idle = false;
                let (Some(pk), Some(sk), Some(kind)) =
                    (s_of(image, "pk"), s_of(image, "sk"), s_of(image, "kind"))
                else {
                    continue;
                };
                if std::env::var_os("SANCHORIS_DEBUG").is_some() {
                    eprintln!("sanchoris: stream {kind} {sk}");
                }
                if let Err(e) = jobs::route(ctx, &pk, &sk, &kind).await {
                    eprintln!("sanchoris: dispatch {pk} {sk}: {e}");
                }
            }
            *iterator = out.next_shard_iterator().map(str::to_string);
        }
        if idle {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
