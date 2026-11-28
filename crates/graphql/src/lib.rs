//! The GraphQL schema of the screens. `apps/frontend/src/graphql/schema.graphql` is its
//! contract: the test `sdl_matches_the_contract` fails when the two differ.

use async_graphql::{
    Context, EmptySubscription, Enum, Error, Object, Result, Schema, SimpleObject,
};
use sanchoris_core::blob::Blobs;
use sanchoris_core::db::Db;
use sanchoris_core::engine_store::DynamoStore;
use sanchoris_core::entities::{self, Message, Project, Thread, Turn, Workspace, keys};
use sanchoris_core::launch::{self, Sent};
use sanchoris_core::thread::{self, Delivered};
use sanchoris_core::{activation, outbox};
use sanchoris_engine::{Origin, Store};
use sanchoris_worker::channel::{log_prefix, run_status};
use sanchoris_worker::ports::{now, rfc3339};
use serde_json::{Value, json};

pub type SanchorisSchema = Schema<Query, Mutation, EmptySubscription>;

/// The signed-in User of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer(pub String);

#[derive(Clone)]
pub struct Backend {
    pub db: Db,
    pub blobs: Blobs,
}

pub fn schema(db: Db, blobs: Blobs) -> SanchorisSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        .data(Backend { db, blobs })
        .limit_depth(12)
        .finish()
}

pub fn sdl() -> String {
    Schema::build(Query, Mutation, EmptySubscription)
        .finish()
        .sdl()
}

fn parts<'a>(ctx: &'a Context<'_>) -> Result<(&'a Backend, &'a str)> {
    let store = ctx.data::<Backend>()?;
    let viewer = ctx
        .data_opt::<Viewer>()
        .ok_or_else(|| Error::new("Sign in first"))?;
    Ok((store, viewer.0.as_str()))
}

fn internal(e: impl std::fmt::Display) -> Error {
    Error::new(e.to_string())
}

#[derive(Enum, Copy, Clone, Eq, PartialEq)]
pub enum ThreadStatus {
    Pending,
    Running,
    Waiting,
    NeedsYou,
    Succeeded,
    Aborted,
}

impl From<entities::ThreadStatus> for ThreadStatus {
    fn from(s: entities::ThreadStatus) -> Self {
        use entities::ThreadStatus as S;
        match s {
            S::Pending => ThreadStatus::Pending,
            S::Running => ThreadStatus::Running,
            S::Waiting => ThreadStatus::Waiting,
            S::NeedsYou => ThreadStatus::NeedsYou,
            S::Succeeded => ThreadStatus::Succeeded,
            S::Aborted => ThreadStatus::Aborted,
        }
    }
}

fn snake<T: serde::Serialize>(v: &T) -> Option<String> {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

/// A Message of a thread.
#[derive(SimpleObject)]
#[graphql(name = "Message")]
pub struct MessageView {
    pub seq: u64,
    /// `user`, `parent`, `agent` or `sanchoris`.
    pub sender: String,
    pub text: String,
    pub at: String,
}

impl From<Message> for MessageView {
    fn from(m: Message) -> Self {
        MessageView {
            seq: m.seq,
            sender: m.sender,
            text: m.text,
            at: rfc3339(m.at),
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "TurnCommit")]
pub struct TurnCommitView {
    pub repository: String,
    pub commit: String,
}

#[derive(SimpleObject)]
#[graphql(name = "Turn")]
pub struct TurnView {
    pub seq: u64,
    pub started_at: String,
    pub ended_at: Option<String>,
    /// `completed`, `failed` or `interrupted`.
    pub result: Option<String>,
    pub commits: Vec<TurnCommitView>,
    /// The key of the thread record at the end of the turn.
    pub record_version: Option<String>,
}

impl From<Turn> for TurnView {
    fn from(t: Turn) -> Self {
        TurnView {
            seq: t.seq,
            started_at: rfc3339(t.started_at),
            ended_at: t.ended_at.map(rfc3339),
            result: t.result,
            commits: t
                .commits
                .into_iter()
                .map(|c| TurnCommitView {
                    repository: c.repository,
                    commit: c.commit,
                })
                .collect(),
            record_version: t.record_version,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "Workspace")]
pub struct WorkspaceView {
    pub seq: u64,
    /// `cloud` or the name of a Host.
    pub target: String,
    /// `starting`, `running` or `stopped`.
    pub state: String,
    pub started_at: String,
    pub stopped_at: Option<String>,
    pub stop_reason: Option<String>,
    pub exit_code: Option<i64>,
}

impl From<Workspace> for WorkspaceView {
    fn from(w: Workspace) -> Self {
        WorkspaceView {
            seq: w.seq,
            target: w.target,
            state: w.state,
            started_at: rfc3339(w.started_at),
            stopped_at: w.stopped_at.map(rfc3339),
            stop_reason: w.stop_reason,
            exit_code: w.exit_code,
        }
    }
}

#[derive(SimpleObject)]
#[graphql(name = "SkillRun")]
pub struct SkillRunView {
    pub id: u64,
    pub skill: String,
    /// `thread` or `user`.
    pub origin: String,
    /// `RUNNING`, `WAITING`, `SUCCEEDED`, `FAILED` or `ABORTED`.
    pub status: String,
    /// The `Output`, as JSON.
    pub output: Option<String>,
    pub error: Option<String>,
    pub cause: Option<String>,
    /// The parent SkillRun, its State and the entry of the State.
    pub parent_run: Option<u64>,
    pub parent_state: Option<String>,
    pub parent_entry: Option<u64>,
}

/// A line of the log of a thread: a message of `sanchoris-agent`, as JSON.
#[derive(SimpleObject)]
pub struct LogLine {
    pub at: String,
    /// The type of the message (`event`, `script_result`, `rate_limit`, `stopping`).
    pub r#type: String,
    pub data: String,
}

#[derive(SimpleObject)]
#[graphql(name = "Project")]
pub struct ProjectView {
    pub name: String,
    pub scripts: Vec<String>,
}

pub struct ThreadView(Thread);

#[Object(name = "Thread")]
impl ThreadView {
    async fn name(&self) -> &str {
        &self.0.name
    }
    async fn title(&self) -> &str {
        &self.0.title
    }
    async fn project(&self) -> &str {
        &self.0.project
    }
    async fn status(&self) -> ThreadStatus {
        self.0.status.into()
    }
    async fn pending_reason(&self) -> Option<String> {
        self.0.pending_reason.as_ref().and_then(snake)
    }
    async fn wait_reason(&self) -> Option<String> {
        self.0.wait_reason.as_ref().and_then(snake)
    }
    async fn error(&self) -> Option<&str> {
        self.0.error.as_deref()
    }
    async fn cause(&self) -> Option<&str> {
        self.0.cause.as_deref()
    }
    async fn branch(&self) -> &str {
        &self.0.branch
    }
    async fn started_at(&self) -> String {
        rfc3339(self.0.started_at)
    }
    async fn updated_at(&self) -> String {
        rfc3339(self.0.updated_at)
    }
    async fn messages(&self, ctx: &Context<'_>) -> Result<Vec<MessageView>> {
        let (store, user) = parts(ctx)?;
        Ok(store
            .db
            .entities::<Message>(user, &format!("THREAD#{}#MSG#", self.0.name))
            .await
            .map_err(internal)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn turns(&self, ctx: &Context<'_>) -> Result<Vec<TurnView>> {
        let (store, user) = parts(ctx)?;
        Ok(store
            .db
            .entities::<Turn>(user, &format!("THREAD#{}#TURN#", self.0.name))
            .await
            .map_err(internal)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn workspaces(&self, ctx: &Context<'_>) -> Result<Vec<WorkspaceView>> {
        let (store, user) = parts(ctx)?;
        Ok(store
            .db
            .entities::<Workspace>(user, &format!("THREAD#{}#WS#", self.0.name))
            .await
            .map_err(internal)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn skill_runs(&self, ctx: &Context<'_>) -> Result<Vec<SkillRunView>> {
        let (store, user) = parts(ctx)?;
        let mut s = DynamoStore::new(store.db.clone(), store.blobs.clone(), user);
        let mut runs: Vec<SkillRunView> = s
            .runs_of_thread(&self.0.name)
            .await
            .map_err(internal)?
            .iter()
            .map(|r| {
                let v = run_status(r);
                SkillRunView {
                    id: r.id(),
                    skill: r.definition.skill.clone(),
                    origin: match r.origin {
                        Origin::Thread => "thread".into(),
                        Origin::User => "user".into(),
                    },
                    status: v["Status"].as_str().unwrap_or_default().into(),
                    output: (!v["Output"].is_null()).then(|| v["Output"].to_string()),
                    error: v["Error"].as_str().map(str::to_string),
                    cause: v["Cause"].as_str().map(str::to_string),
                    parent_run: r
                        .parent
                        .as_ref()
                        .map(|p| sanchoris_engine::json::u64_of(&p.run)),
                    parent_state: r.parent.as_ref().map(|p| p.state.clone()),
                    parent_entry: r
                        .parent
                        .as_ref()
                        .map(|p| sanchoris_engine::json::u64_of(&p.visit)),
                }
            })
            .collect();
        runs.sort_by_key(|r| r.id);
        Ok(runs)
    }
    async fn log(&self, ctx: &Context<'_>) -> Result<Vec<LogLine>> {
        let (store, user) = parts(ctx)?;
        let mut lines = Vec::new();
        for key in store
            .blobs
            .list(&log_prefix(user, &self.0.name))
            .await
            .map_err(internal)?
        {
            let Some(bytes) = store.blobs.get(&key).await.map_err(internal)? else {
                continue;
            };
            let v: Value = serde_json::from_slice(&bytes).map_err(internal)?;
            lines.push(LogLine {
                at: rfc3339(v["at"].as_u64().unwrap_or_default()),
                r#type: v["type"].as_str().unwrap_or_default().into(),
                data: v["data"].to_string(),
            });
        }
        Ok(lines)
    }
}

/// What a message did: the input went into the turn (`steered`), waits (`queued`), started a
/// turn (`started_turn`), waits for the Workspace to be restored (`restoring`), launched a
/// Skill (`launched`, with `runId`), or compacts (`compact`).
#[derive(SimpleObject)]
pub struct SendResult {
    pub delivery: String,
    pub run_id: Option<u64>,
}

pub struct Query;

async fn read_thread(store: &Backend, user: &str, name: &str) -> Result<Thread> {
    store
        .db
        .entity::<Thread>(&keys::thread(user, name))
        .await
        .map_err(internal)?
        .ok_or_else(|| Error::new(format!("No thread {name}")))
}

#[Object]
impl Query {
    async fn threads(&self, ctx: &Context<'_>) -> Result<Vec<ThreadView>> {
        let (store, user) = parts(ctx)?;
        let mut threads = store
            .db
            .entities::<Thread>(user, "THREAD#")
            .await
            .map_err(internal)?;
        threads.sort_by_key(|t| std::cmp::Reverse(t.started_at));
        Ok(threads.into_iter().map(ThreadView).collect())
    }

    async fn thread(&self, ctx: &Context<'_>, name: String) -> Result<Option<ThreadView>> {
        let (store, user) = parts(ctx)?;
        Ok(store
            .db
            .entity::<Thread>(&keys::thread(user, &name))
            .await
            .map_err(internal)?
            .map(ThreadView))
    }

    async fn projects(&self, ctx: &Context<'_>) -> Result<Vec<ProjectView>> {
        let (store, user) = parts(ctx)?;
        let Some(config) = activation::active_config(&store.db, user)
            .await
            .map_err(internal)?
        else {
            return Ok(Vec::new());
        };
        Ok(store
            .db
            .entities::<Project>(user, &format!("CONFIG#{}#PROJECT#", config.repository_id))
            .await
            .map_err(internal)?
            .into_iter()
            .map(|p| ProjectView {
                scripts: p.definition["Scripts"]
                    .as_object()
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default(),
                name: p.name,
            })
            .collect())
    }
}

pub struct Mutation;

#[Object]
impl Mutation {
    /// Starts a thread in a Project with its first message (New thread).
    async fn start_thread(
        &self,
        ctx: &Context<'_>,
        project: String,
        message: String,
    ) -> Result<ThreadView> {
        let (store, user) = parts(ctx)?;
        let name = launch::start_thread(&store.db, &store.blobs, user, &project, &message, now())
            .await
            .map_err(internal)?;
        Ok(ThreadView(read_thread(store, user, &name).await?))
    }

    /// Sends a message to a thread: `/compact`, `/<Skill> [input]`, or a message.
    async fn send_message(
        &self,
        ctx: &Context<'_>,
        thread: String,
        text: String,
    ) -> Result<SendResult> {
        let (store, user) = parts(ctx)?;
        let sent = launch::send_message(&store.db, &store.blobs, user, &thread, &text, now())
            .await
            .map_err(internal)?;
        Ok(match sent {
            Sent::Compact => SendResult {
                delivery: "compact".into(),
                run_id: None,
            },
            Sent::Launched(v) => SendResult {
                delivery: "launched".into(),
                run_id: v["RunId"].as_u64(),
            },
            Sent::Input(d) => SendResult {
                delivery: match d {
                    Delivered::Steered => "steered",
                    Delivered::Queued => "queued",
                    Delivered::StartedTurn => "started_turn",
                    Delivered::Restoring => "restoring",
                    Delivered::Dropped | Delivered::Duplicate => "dropped",
                }
                .into(),
                run_id: None,
            },
        })
    }

    /// Runs a script of the Project in the Workspace of the thread; its result goes to the log
    /// of the thread. Returns the ID of the run of the script.
    async fn run_script(&self, ctx: &Context<'_>, thread: String, name: String) -> Result<String> {
        let (store, user) = parts(ctx)?;
        let t = read_thread(store, user, &thread).await?;
        let definition = launch::read_definition(&store.blobs, &t)
            .await
            .map_err(internal)?;
        if definition["project"]["Scripts"].get(&name).is_none() {
            return Err(Error::new(format!(
                "No script {name} in Project {}",
                t.project
            )));
        }
        if !thread::has_workspace(&store.db, user, &t)
            .await
            .map_err(internal)?
        {
            return Err(Error::new(format!("Thread {thread} has no Workspace")));
        }
        let id = format!("sr_{}", &sanchoris_core::token::random_id()[..16]);
        outbox::push(
            &store.db,
            user,
            &thread,
            &[json!({"type": "script", "threadId": thread, "scriptRunId": id, "name": name})],
        )
        .await
        .map_err(internal)?;
        Ok(id)
    }

    /// Stops a thread: it becomes `ABORTED` with its SkillRuns and `subagent` children.
    async fn stop_thread(&self, ctx: &Context<'_>, name: String) -> Result<ThreadView> {
        let (store, user) = parts(ctx)?;
        thread::stop(&store.db, &store.blobs, user, &name, now())
            .await
            .map_err(internal)?;
        Ok(ThreadView(read_thread(store, user, &name).await?))
    }
}

#[cfg(test)]
mod tests {
    const CONTRACT: &str = "../../apps/frontend/src/graphql/schema.graphql";

    /// `SANCHORIS_UPDATE_SDL=1 cargo test -p sanchoris-graphql` writes the contract.
    #[test]
    fn sdl_matches_the_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT);
        let sdl = super::sdl();
        if std::env::var_os("SANCHORIS_UPDATE_SDL").is_some() {
            std::fs::write(&path, &sdl).unwrap();
        }
        let contract = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            contract == sdl,
            "the schema differs from {CONTRACT}; run SANCHORIS_UPDATE_SDL=1 cargo test -p sanchoris-graphql"
        );
    }
}
