//! The entities of `README.md`「データ」, each in the partition of its User (`U#<user>`), with
//! the sort keys of `docs/architecture.md`. Times are milliseconds since the Unix epoch.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{Cond, Db, DbError, Item, Key, TableKey, UserTx, item_of, user_pk, value_of};

/// An entity: its kind and its key in the partition of its User.
pub trait Entity: Serialize + DeserializeOwned {
    const KIND: &'static str;
    fn key(&self, user: &str) -> Key;
}

/// A number with a fixed width, so that sort keys sort as numbers.
pub fn padded(n: u64) -> String {
    format!("{n:020}")
}

pub mod keys {
    use super::padded;
    use crate::db::{Key, user_pk};

    pub fn user(user: &str) -> Key {
        Key::main(user_pk(user), "USER")
    }
    pub fn credential(user: &str, name: &str) -> Key {
        Key::main(user_pk(user), format!("CRED#{name}"))
    }
    pub fn usage_snapshot(user: &str, credential: &str, taken_at: u64) -> Key {
        Key::main(
            user_pk(user),
            format!("CRED#{credential}#SNAP#{}", padded(taken_at)),
        )
    }
    pub fn integration(user: &str, kind: &str, id: &str) -> Key {
        Key::main(user_pk(user), format!("INTEG#{kind}#{id}"))
    }
    pub fn host(user: &str, name: &str) -> Key {
        Key::main(user_pk(user), format!("HOST#{name}"))
    }
    pub fn audit_event(user: &str, at: u64, id: &str) -> Key {
        Key::audit(user_pk(user), format!("AUDIT#{}#{id}", padded(at)))
    }
    pub fn config(user: &str, repository_id: u64) -> Key {
        Key::main(user_pk(user), format!("CONFIG#{repository_id}"))
    }
    pub fn project(user: &str, repository_id: u64, name: &str) -> Key {
        Key::main(
            user_pk(user),
            format!("CONFIG#{repository_id}#PROJECT#{name}"),
        )
    }
    pub fn repository(user: &str, repository_id: u64, project: &str, name: &str) -> Key {
        Key::main(
            user_pk(user),
            format!("CONFIG#{repository_id}#PROJECT#{project}#REPO#{name}"),
        )
    }
    pub fn skill(user: &str, repository_id: u64, name: &str) -> Key {
        Key::main(
            user_pk(user),
            format!("CONFIG#{repository_id}#SKILL#{name}"),
        )
    }
    pub fn trigger(user: &str, repository_id: u64, name: &str) -> Key {
        Key::main(
            user_pk(user),
            format!("CONFIG#{repository_id}#TRIGGER#{name}"),
        )
    }
    /// The Trigger event of a delivery: one per Trigger and delivery, which makes a second
    /// delivery of the same event a duplicate.
    pub fn trigger_event(user: &str, repository_id: u64, trigger: &str, delivery: &str) -> Key {
        Key::main(
            user_pk(user),
            format!("CONFIG#{repository_id}#TRIGGER#{trigger}#EVENT#{delivery}"),
        )
    }
    pub fn thread(user: &str, name: &str) -> Key {
        Key::main(user_pk(user), format!("THREAD#{name}"))
    }
    pub fn message(user: &str, thread: &str, seq: u64) -> Key {
        Key::main(
            user_pk(user),
            format!("THREAD#{thread}#MSG#{}", padded(seq)),
        )
    }
    pub fn turn(user: &str, thread: &str, seq: u64) -> Key {
        Key::main(
            user_pk(user),
            format!("THREAD#{thread}#TURN#{}", padded(seq)),
        )
    }
    pub fn workspace(user: &str, thread: &str, seq: u64) -> Key {
        Key::main(user_pk(user), format!("THREAD#{thread}#WS#{}", padded(seq)))
    }
    pub fn usage(user: &str, thread: &str) -> Key {
        Key::main(user_pk(user), format!("THREAD#{thread}#USAGE"))
    }
    pub fn pull_request(user: &str, thread: &str, repository_id: u64, number: u64) -> Key {
        Key::main(
            user_pk(user),
            format!("THREAD#{thread}#PR#{repository_id}#{number}"),
        )
    }
    /// A child of a thread, under its parent.
    pub fn child(user: &str, parent: &str, child: &str) -> Key {
        Key::main(user_pk(user), format!("THREAD#{parent}#CHILD#{child}"))
    }
    /// A message of the outbox of the Thread channel of a thread, in order.
    pub fn outbox(user: &str, thread: &str, seq: u64) -> Key {
        Key::main(
            user_pk(user),
            format!("THREAD#{thread}#OUT#{}", padded(seq)),
        )
    }
    /// The last number of the outbox of a thread.
    pub fn outbox_seq(user: &str, thread: &str) -> Key {
        Key::main(user_pk(user), format!("THREAD#{thread}#OUTSEQ"))
    }
    /// The thread of a Trigger and a thread key, while the key is taken.
    pub fn thread_key(user: &str, repository_id: u64, trigger: &str, key: &str) -> Key {
        Key::main(
            user_pk(user),
            format!("TKEY#{repository_id}#{trigger}#{key}"),
        )
    }
    /// The threads a Trigger started in an hour (UTC).
    pub fn trigger_hour(user: &str, repository_id: u64, trigger: &str, hour: u64) -> Key {
        Key::main(
            user_pk(user),
            format!("TRIGHOUR#{repository_id}#{trigger}#{hour}"),
        )
    }
    /// A mark that makes an operation happen once: the delivery of a webhook, a Stripe
    /// event, a job.
    pub fn mark(user: &str, kind: &str, id: &str) -> Key {
        Key::main(user_pk(user), format!("MARK#{kind}#{id}"))
    }
    /// A job of a thread, for the dispatcher.
    pub fn job(user: &str, id: &str) -> Key {
        Key::main(user_pk(user), format!("JOB#{id}"))
    }
}

/// The subscription of a User in Stripe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subscription {
    None,
    Trialing,
    Active,
    PastDue,
    Canceled,
    Unpaid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserStatus {
    Active,
    Suspended,
    Deleting,
}

/// A User, with the counters of its period (`architecture.md`「処理とデータ」).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub cognito_subject: String,
    pub email: String,
    pub github_user_id: Option<u64>,
    pub github_login: Option<String>,
    pub plan: String,
    /// The threads that became `RUNNING` for the first time in the period.
    pub period_threads: u64,
    pub period_workspace_seconds: u64,
    /// The cloud Workspaces that run, with those that wait before they are removed.
    pub running_workspaces: u64,
    pub stripe_customer: Option<String>,
    pub subscription: Subscription,
    pub period_start: u64,
    pub period_end: u64,
    pub status: UserStatus,
    pub suspended_reason: Option<String>,
    pub operator: bool,
    pub created_at: u64,
}

impl Entity for User {
    const KIND: &'static str = "user";
    fn key(&self, user: &str) -> Key {
        keys::user(user)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credential {
    pub name: String,
    pub provider: String,
    /// `subscription` or `api_key`.
    pub credential_kind: String,
    pub account: Option<String>,
    /// `connected` or `reconnect`.
    pub state: String,
    pub last_used_at: Option<u64>,
}

impl Entity for Credential {
    const KIND: &'static str = "credential";
    fn key(&self, user: &str) -> Key {
        keys::credential(user, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub name: String,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub credential: String,
    pub taken_at: u64,
    pub windows: Vec<UsageWindow>,
}

impl Entity for UsageSnapshot {
    const KIND: &'static str = "usage_snapshot";
    fn key(&self, user: &str) -> Key {
        keys::usage_snapshot(user, &self.credential, self.taken_at)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Integration {
    /// `github_installation` or `slack`.
    pub integration_kind: String,
    pub id: String,
    /// `connected` or `not_connected`.
    pub state: String,
    pub detail: Value,
}

impl Entity for Integration {
    const KIND: &'static str = "integration";
    fn key(&self, user: &str) -> Key {
        keys::integration(user, &self.integration_kind, &self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Host {
    pub name: String,
    /// `online` or `offline`.
    pub state: String,
    pub architecture: String,
    pub driver_version: String,
    pub max_workspaces: u64,
    pub workspace_vcpu: u64,
    pub workspace_memory_mib: u64,
    pub running_workspaces: u64,
    pub last_heartbeat_at: u64,
    pub registered_at: u64,
}

impl Entity for Host {
    const KIND: &'static str = "host";
    fn key(&self, user: &str) -> Key {
        keys::host(user, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub at: u64,
    /// `user`, `agent`, `trigger`, `system` or `operator`, with who.
    pub actor: String,
    pub actor_ref: Option<String>,
    pub action: String,
    pub target: String,
    pub summary: String,
    pub source_ip: Option<String>,
}

impl Entity for AuditEvent {
    const KIND: &'static str = "audit_event";
    fn key(&self, user: &str) -> Key {
        keys::audit_event(user, self.at, &self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub repository_id: u64,
    /// `owner/name`, which follows a rename.
    pub repository: String,
    pub root: String,
    /// `active` or `stopped`.
    pub state: String,
    pub revision: Option<String>,
    pub synced_at: Option<u64>,
    pub last_sync_error: Option<String>,
    /// `valid` or `invalid`.
    pub validation: String,
    pub validation_messages: Vec<String>,
}

impl Entity for Config {
    const KIND: &'static str = "config";
    fn key(&self, user: &str) -> Key {
        keys::config(user, self.repository_id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub repository_id: u64,
    pub name: String,
    /// The project of `sanchoris.yaml`, as JSON.
    pub definition: Value,
    pub file: String,
}

impl Entity for Project {
    const KIND: &'static str = "project";
    fn key(&self, user: &str) -> Key {
        keys::project(user, self.repository_id, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Repository {
    pub config_repository_id: u64,
    pub project: String,
    pub name: String,
    /// `github.com/owner/name`.
    pub repo: String,
}

impl Entity for Repository {
    const KIND: &'static str = "repository";
    fn key(&self, user: &str) -> Key {
        keys::repository(user, self.config_repository_id, &self.project, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    pub repository_id: u64,
    pub name: String,
    pub predictable: bool,
    pub builtin: bool,
    pub description: String,
    pub inputs: Value,
    /// The key in S3 of the `skill.yaml` and its `Layout`.
    pub definition_key: Option<String>,
    pub file: String,
}

impl Entity for Skill {
    const KIND: &'static str = "skill";
    fn key(&self, user: &str) -> Key {
        keys::skill(user, self.repository_id, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    pub repository_id: u64,
    pub name: String,
    pub enabled: bool,
    pub event_pattern: Option<Value>,
    pub schedule_expression: Option<String>,
    pub input_paths_map: Value,
    pub start: Value,
}

impl Entity for Trigger {
    const KIND: &'static str = "trigger";
    fn key(&self, user: &str) -> Key {
        keys::trigger(user, self.repository_id, &self.name)
    }
}

/// What a Trigger did with an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TriggerOutcome {
    /// It started the thread.
    Started { thread: String },
    /// A thread of the same Trigger has the thread key.
    SkippedExisting { thread: String },
    /// The dependencies of the issue could not be read.
    SkippedDependencies { reason: String },
    /// The Trigger started its 60 threads of the hour.
    DroppedLimit { reason: String },
    /// The event could not be evaluated.
    DroppedEvaluation { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerEvent {
    pub repository_id: u64,
    pub trigger: String,
    pub received_at: u64,
    pub delivery: String,
    pub event: Value,
    pub thread_key: Option<String>,
    pub result: TriggerOutcome,
}

impl Entity for TriggerEvent {
    const KIND: &'static str = "trigger_event";
    fn key(&self, user: &str) -> Key {
        keys::trigger_event(user, self.repository_id, &self.trigger, &self.delivery)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ThreadStatus {
    Pending,
    Running,
    Waiting,
    NeedsYou,
    Succeeded,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    BlockedBy,
    Concurrency,
    PlanLimit,
    HostFull,
    Subscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    Skill,
    Children,
    UsageLimit,
    Host,
}

/// How a child thread was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lineage {
    Subagent,
    Fork,
}

/// An issue that blocks the thread (GitHub issue dependencies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedBy {
    pub repository_id: u64,
    pub number: u64,
    pub closed: bool,
}

/// The Trigger that started a thread and its thread key, which the thread holds until it is
/// `ABORTED`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerKey {
    pub repository_id: u64,
    pub trigger: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub name: String,
    pub project: String,
    pub status: ThreadStatus,
    pub pending_reason: Option<PendingReason>,
    pub wait_reason: Option<WaitReason>,
    pub resume_at: Option<u64>,
    pub snooze_until: Option<u64>,
    pub error: Option<String>,
    pub cause: Option<String>,
    pub title: String,
    pub agent: String,
    pub model: String,
    pub credential: String,
    pub provider_session: Option<String>,
    /// The key in S3 of the copy of the Project and the callable Skills at the start.
    pub definition_key: Option<String>,
    pub config_revision: Option<String>,
    /// Where it started: `web`, `slack`, `trigger:<name>`, `subagent`, `fork`.
    pub origin: String,
    pub requester: String,
    pub branch: String,
    pub limit_counted_from: u64,
    pub limit_turns: u64,
    pub limit_cost_usd: f64,
    pub total_turns: u64,
    pub total_cost_usd: f64,
    pub log_key: Option<String>,
    pub started_at: u64,
    pub updated_at: u64,
    pub parent: Option<String>,
    pub lineage: Option<Lineage>,
    pub fork_turn: Option<u64>,
    pub blocked_by: Vec<BlockedBy>,
    pub trigger_key: Option<TriggerKey>,
    /// Whether the turn of a `RUNNING` thread runs; `false` while it is asked for.
    pub turn_active: bool,
    /// Where its Workspace runs: `cloud` or the name of a Host (`Workspace.Host`).
    pub workspace_host: String,
    /// The Workspaces it had; the last one is its Workspace.
    pub workspaces: u64,
    /// The Messages it has.
    pub messages: u64,
    /// Whether it became `RUNNING` once and is counted in the period.
    pub counted: bool,
    /// The inputs that wait for the thread to take them (`skill.md`「スレッドへの入力」).
    pub queued: Vec<Value>,
    /// Goes up by one with each write; a write is made only over the version it read.
    pub version: u64,
}

impl Entity for Thread {
    const KIND: &'static str = "thread";
    fn key(&self, user: &str) -> Key {
        keys::thread(user, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub thread: String,
    pub seq: u64,
    /// `user`, `parent`, `agent` or `sanchoris`.
    pub sender: String,
    pub text: String,
    pub at: u64,
}

impl Entity for Message {
    const KIND: &'static str = "message";
    fn key(&self, user: &str) -> Key {
        keys::message(user, &self.thread, self.seq)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnCommit {
    pub repository: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub thread: String,
    pub seq: u64,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    /// `completed`, `failed` or `interrupted`.
    pub result: Option<String>,
    pub commits: Vec<TurnCommit>,
    pub record_version: Option<String>,
}

impl Entity for Turn {
    const KIND: &'static str = "turn";
    fn key(&self, user: &str) -> Key {
        keys::turn(user, &self.thread, self.seq)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClonedRepository {
    pub name: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub thread: String,
    pub seq: u64,
    /// `cloud` or the name of a Host.
    pub target: String,
    /// `fargate_spot`, `on_demand` or `host`.
    pub platform: Option<String>,
    /// `starting`, `running` or `stopped`.
    pub state: String,
    pub started_at: u64,
    pub stopped_at: Option<u64>,
    pub stop_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub repositories: Vec<ClonedRepository>,
}

impl Entity for Workspace {
    const KIND: &'static str = "workspace";
    fn key(&self, user: &str) -> Key {
        keys::workspace(user, &self.thread, self.seq)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub thread: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

impl Entity for Usage {
    const KIND: &'static str = "usage";
    fn key(&self, user: &str) -> Key {
        keys::usage(user, &self.thread)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub thread: String,
    pub repository_id: u64,
    pub number: u64,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub checks: Option<String>,
    pub created_at: u64,
}

impl Entity for PullRequest {
    const KIND: &'static str = "pull_request";
    fn key(&self, user: &str) -> Key {
        keys::pull_request(user, &self.thread, self.repository_id, self.number)
    }
}

/// A child thread under its parent, with how it was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child {
    pub parent: String,
    pub name: String,
    pub lineage: Lineage,
    pub fork_turn: Option<u64>,
    pub created_at: u64,
}

impl Entity for Child {
    const KIND: &'static str = "child";
    fn key(&self, user: &str) -> Key {
        keys::child(user, &self.parent, &self.name)
    }
}

impl Db {
    pub async fn entity<E: Entity>(&self, key: &Key) -> Result<Option<E>, DbError> {
        match self.get(key).await? {
            Some(item) => Ok(Some(value_of(&item)?)),
            None => Ok(None),
        }
    }

    /// The entities of a kind in the partition of a User whose sort key starts with `prefix`.
    pub async fn entities<E: Entity>(&self, user: &str, prefix: &str) -> Result<Vec<E>, DbError> {
        let table = if E::KIND == AuditEvent::KIND {
            TableKey::Audit
        } else {
            TableKey::Main
        };
        self.query(table, &user_pk(user), prefix)
            .await?
            .iter()
            .filter(|item| crate::db::get_s(item, "kind") == Some(E::KIND))
            .map(value_of)
            .collect()
    }

    pub async fn children(&self, user: &str, parent: &str) -> Result<Vec<Child>, DbError> {
        self.entities(user, &format!("THREAD#{parent}#CHILD#"))
            .await
    }
}

pub fn entity_item<E: Entity>(user: &str, entity: &E) -> Result<Item, DbError> {
    item_of(&entity.key(user), E::KIND, entity)
}

impl UserTx {
    /// Writes a new entity; the transaction fails when it exists.
    pub fn insert_entity<E: Entity>(&mut self, entity: &E) -> Result<(), DbError> {
        let item = entity_item(self.user(), entity)?;
        let table = entity.key(self.user()).table;
        self.put(table, item, Some(Cond::not_exists()))
    }

    /// Writes an entity under a condition.
    pub fn put_entity<E: Entity>(
        &mut self,
        entity: &E,
        condition: Option<Cond>,
    ) -> Result<(), DbError> {
        let item = entity_item(self.user(), entity)?;
        let table = entity.key(self.user()).table;
        self.put(table, item, condition)
    }
}
