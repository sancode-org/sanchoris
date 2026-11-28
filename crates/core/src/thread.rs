//! The progress of threads (`skill.md`「スレッド」「スレッドへの入力」「スレッドの状態」): the start
//! with the Plan limits and the place of the Workspace, the events of a thread, its inputs and
//! its stop. The decisions are the Lean module `Sanchoris.Thread`; this module reads and writes
//! them, each write over the version of the thread it read.

use leanfran::Nat;
use serde_json::{Value, json};

use crate::db::{Change, Cond, Db, DbError, n, s};
use crate::entities::{
    BlockedBy, Child, Host, Lineage, Message, PendingReason, Thread, ThreadStatus, TriggerKey,
    User, WaitReason, Workspace, keys,
};
use crate::generated::sanchoris_host::{self as host_lean, HostState, Placement};
use crate::generated::sanchoris_thread as lean;
use crate::outbox;
use crate::plans::{self, Plan};

/// How many times a write that another changed meanwhile is read and made again.
const ATTEMPTS: usize = 20;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ThreadError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("no thread {0}")]
    Missing(String),
    #[error("no User {0}")]
    NoUser(String),
    #[error("no Plan {0}")]
    NoPlan(String),
    #[error("thread {0} is ABORTED and takes no input")]
    Aborted(String),
    #[error("thread {0} kept changing while it was written")]
    Busy(String),
}

fn nat(v: u64) -> Nat {
    Nat::from(v)
}

/// The thread error names of `NEEDS_YOU`.
pub fn error_name(e: &lean::Error) -> &'static str {
    match e {
        lean::Error::Limit { limit } => match limit {
            lean::Limit::Turns => "Limit.Turns",
            lean::Limit::CostUsd => "Limit.CostUsd",
            lean::Limit::TimeoutSeconds => "Limit.TimeoutSeconds",
            lean::Limit::HeartbeatSeconds => "Limit.HeartbeatSeconds",
        },
        lean::Error::WorkerFailed => "Worker.Failed",
        lean::Error::UsageLimit => "Worker.UsageLimit",
        lean::Error::WorkspaceStopped => "Workspace.Stopped",
        lean::Error::ScriptFailed => "Script.Failed",
        lean::Error::CredentialMissing => "Credential.Missing",
        lean::Error::HostMissing => "Host.Missing",
    }
}

fn error_of(name: &str) -> lean::Error {
    let limit = |limit| lean::Error::Limit { limit };
    match name {
        "Limit.Turns" => limit(lean::Limit::Turns),
        "Limit.CostUsd" => limit(lean::Limit::CostUsd),
        "Limit.TimeoutSeconds" => limit(lean::Limit::TimeoutSeconds),
        "Limit.HeartbeatSeconds" => limit(lean::Limit::HeartbeatSeconds),
        "Worker.UsageLimit" => lean::Error::UsageLimit,
        "Workspace.Stopped" => lean::Error::WorkspaceStopped,
        "Script.Failed" => lean::Error::ScriptFailed,
        "Credential.Missing" => lean::Error::CredentialMissing,
        "Host.Missing" => lean::Error::HostMissing,
        _ => lean::Error::WorkerFailed,
    }
}

fn pending_to_lean(r: PendingReason) -> lean::PendingReason {
    match r {
        PendingReason::BlockedBy => lean::PendingReason::BlockedBy,
        PendingReason::Concurrency => lean::PendingReason::Concurrency,
        PendingReason::PlanLimit => lean::PendingReason::PlanLimit,
        PendingReason::HostFull => lean::PendingReason::HostFull,
        PendingReason::Subscription => lean::PendingReason::Subscription,
    }
}

fn pending_of(r: &lean::PendingReason) -> PendingReason {
    match r {
        lean::PendingReason::BlockedBy => PendingReason::BlockedBy,
        lean::PendingReason::Concurrency => PendingReason::Concurrency,
        lean::PendingReason::PlanLimit => PendingReason::PlanLimit,
        lean::PendingReason::HostFull => PendingReason::HostFull,
        lean::PendingReason::Subscription => PendingReason::Subscription,
    }
}

fn wait_to_lean(r: WaitReason) -> lean::WaitReason {
    match r {
        WaitReason::Skill => lean::WaitReason::Skill,
        WaitReason::Children => lean::WaitReason::Children,
        WaitReason::UsageLimit => lean::WaitReason::UsageLimit,
        WaitReason::Host => lean::WaitReason::Host,
    }
}

fn wait_of(r: &lean::WaitReason) -> WaitReason {
    match r {
        lean::WaitReason::Skill => WaitReason::Skill,
        lean::WaitReason::Children => WaitReason::Children,
        lean::WaitReason::UsageLimit => WaitReason::UsageLimit,
        lean::WaitReason::Host => WaitReason::Host,
    }
}

/// The Lean state of a thread. A `PENDING` thread that the thread-starter has not decided yet
/// has no reason, and waits as one whose Workspace has no room.
pub fn state_of(t: &Thread) -> lean::State {
    match t.status {
        ThreadStatus::Pending => lean::State::Pending {
            reason: pending_to_lean(t.pending_reason.unwrap_or(PendingReason::Concurrency)),
        },
        ThreadStatus::Running => lean::State::Running {
            turn: if t.turn_active {
                lean::Turn::Active
            } else {
                lean::Turn::Preparing
            },
        },
        ThreadStatus::Waiting => lean::State::Waiting {
            reason: wait_to_lean(t.wait_reason.unwrap_or(WaitReason::Skill)),
        },
        ThreadStatus::NeedsYou => lean::State::NeedsYou {
            error: error_of(t.error.as_deref().unwrap_or("Worker.Failed")),
        },
        ThreadStatus::Succeeded => lean::State::Succeeded,
        ThreadStatus::Aborted => lean::State::Aborted,
    }
}

/// Writes a Lean state into a thread.
pub fn set_state(t: &mut Thread, state: &lean::State) {
    t.pending_reason = None;
    t.wait_reason = None;
    t.turn_active = false;
    if !matches!(state, lean::State::NeedsYou { .. }) {
        t.error = None;
        t.cause = None;
    }
    match state {
        lean::State::Pending { reason } => {
            t.status = ThreadStatus::Pending;
            t.pending_reason = Some(pending_of(reason));
        }
        lean::State::Running { turn } => {
            t.status = ThreadStatus::Running;
            t.turn_active = *turn == lean::Turn::Active;
        }
        lean::State::Waiting { reason } => {
            t.status = ThreadStatus::Waiting;
            t.wait_reason = Some(wait_of(reason));
        }
        lean::State::NeedsYou { error } => {
            t.status = ThreadStatus::NeedsYou;
            t.error = Some(error_name(error).to_string());
        }
        lean::State::Succeeded => t.status = ThreadStatus::Succeeded,
        lean::State::Aborted => t.status = ThreadStatus::Aborted,
    }
}

fn subscription_to_lean(s: crate::entities::Subscription) -> lean::Subscription {
    use crate::entities::Subscription as S;
    match s {
        S::None => lean::Subscription::None,
        S::Trialing => lean::Subscription::Trialing,
        S::Active => lean::Subscription::Active,
        S::PastDue => lean::Subscription::PastDue,
        S::Canceled => lean::Subscription::Canceled,
        S::Unpaid => lean::Subscription::Unpaid,
    }
}

/// What a new thread starts with (`skill.md`「スレッド」).
#[derive(Debug, Clone, PartialEq)]
pub struct NewThread {
    pub name: String,
    pub project: String,
    pub message: String,
    pub title: String,
    pub agent: String,
    pub model: String,
    pub credential: String,
    /// `cloud` or the name of a Host.
    pub workspace_host: String,
    pub origin: String,
    pub requester: String,
    pub parent: Option<(String, Lineage, Option<u64>)>,
    pub blocked_by: Vec<BlockedBy>,
    pub trigger_key: Option<TriggerKey>,
    pub definition_key: Option<String>,
    pub config_revision: Option<String>,
}

/// The thread of a [`NewThread`], `PENDING` until the thread-starter decides.
pub fn new_thread(t: &NewThread, now: u64) -> Thread {
    Thread {
        name: t.name.clone(),
        project: t.project.clone(),
        status: ThreadStatus::Pending,
        pending_reason: None,
        wait_reason: None,
        resume_at: None,
        snooze_until: None,
        error: None,
        cause: None,
        title: t.title.clone(),
        agent: t.agent.clone(),
        model: t.model.clone(),
        credential: t.credential.clone(),
        provider_session: None,
        definition_key: t.definition_key.clone(),
        config_revision: t.config_revision.clone(),
        origin: t.origin.clone(),
        requester: t.requester.clone(),
        branch: format!("sanchoris/{}", t.name),
        limit_counted_from: now,
        limit_turns: 0,
        limit_cost_usd: 0.0,
        total_turns: 0,
        total_cost_usd: 0.0,
        log_key: None,
        started_at: now,
        updated_at: now,
        parent: t.parent.as_ref().map(|p| p.0.clone()),
        lineage: t.parent.as_ref().map(|p| p.1),
        fork_turn: t.parent.as_ref().and_then(|p| p.2),
        blocked_by: t.blocked_by.clone(),
        trigger_key: t.trigger_key.clone(),
        turn_active: false,
        workspace_host: t.workspace_host.clone(),
        workspaces: 0,
        messages: 1,
        counted: false,
        queued: vec![json!({"id": format!("{}-1", t.name), "source": "person", "text": t.message})],
        version: 0,
    }
}

/// The job that asks the thread-starter to start a thread.
pub fn start_job(thread: &str) -> String {
    format!("start#{thread}")
}

/// Adds a new thread to a transaction: the thread, its first Message, its place under its
/// parent, and the job of the thread-starter.
pub fn insert_thread(tx: &mut crate::db::UserTx, t: &NewThread, now: u64) -> Result<(), DbError> {
    let thread = new_thread(t, now);
    tx.insert_entity(&thread)?;
    tx.insert_entity(&Message {
        thread: t.name.clone(),
        seq: 1,
        sender: if matches!(t.parent, Some((_, Lineage::Subagent, _))) {
            "parent".into()
        } else {
            "user".into()
        },
        text: t.message.clone(),
        at: now,
    })?;
    if let Some((parent, lineage, fork_turn)) = &t.parent {
        tx.insert_entity(&Child {
            parent: parent.clone(),
            name: t.name.clone(),
            lineage: *lineage,
            fork_turn: *fork_turn,
            created_at: now,
        })?;
    }
    insert_job(
        tx,
        &start_job(&t.name),
        json!({"type": "thread.start", "thread": t.name}),
    )
}

pub fn insert_job(tx: &mut crate::db::UserTx, id: &str, body: Value) -> Result<(), DbError> {
    let user = tx.user().to_string();
    let item = crate::db::item_of(
        &keys::job(&user, id),
        "job",
        &json!({"id": id, "body": body}),
    )?;
    tx.put(crate::db::TableKey::Main, item, None)
}

async fn read_thread(db: &Db, user: &str, name: &str) -> Result<Thread, ThreadError> {
    db.entity(&keys::thread(user, name))
        .await?
        .ok_or_else(|| ThreadError::Missing(name.to_string()))
}

async fn read_user(db: &Db, user: &str) -> Result<(User, Plan), ThreadError> {
    let u: User = db
        .entity(&keys::user(user))
        .await?
        .ok_or_else(|| ThreadError::NoUser(user.to_string()))?;
    let plan = plans::plan(&u.plan).ok_or_else(|| ThreadError::NoPlan(u.plan.clone()))?;
    Ok((u, plan))
}

/// What the thread-starter did with a thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Started {
    /// The thread is `RUNNING` and its Workspace starts at the placement.
    Running {
        workspace: u64,
        placement: String,
    },
    Pending(PendingReason),
    NeedsYou(String),
    /// The thread was not `PENDING` (a second delivery of the job, or a stopped thread).
    Unchanged(ThreadStatus),
}

fn placement_name(p: &Placement) -> String {
    match p {
        Placement::Cloud => "cloud".into(),
        Placement::Host { name } => name.clone(),
        Placement::Full => "full".into(),
        Placement::Missing => "missing".into(),
    }
}

/// The thread-starter: decides the start of a `PENDING` thread with `decideStart`, and writes
/// in one transaction the thread, the counters of the User (the threads of the period and the
/// cloud Workspaces, under their limits) or of the Host, the new Workspace and the end of the
/// job. A write that another changed meanwhile is read and decided again.
pub async fn start_pending(
    db: &Db,
    user: &str,
    name: &str,
    now: u64,
) -> Result<Started, ThreadError> {
    for _ in 0..ATTEMPTS {
        let mut t = read_thread(db, user, name).await?;
        let job = keys::job(user, &start_job(name));
        if t.status != ThreadStatus::Pending {
            let mut tx = db.tx(user);
            tx.delete(&job, None)?;
            tx.commit().await?;
            return Ok(Started::Unchanged(t.status));
        }
        let (u, plan) = read_user(db, user).await?;
        let hosts: Vec<Host> = if t.workspace_host == "cloud" {
            Vec::new()
        } else {
            db.entities(user, "HOST#").await?
        };
        let target = (t.workspace_host != "cloud").then(|| t.workspace_host.clone());
        let placement = host_lean::place(
            target,
            hosts
                .iter()
                .map(|h| HostState {
                    name: h.name.clone(),
                    online: h.state == "online",
                    running: nat(h.running_workspaces),
                    max_workspaces: nat(h.max_workspaces),
                })
                .collect(),
        );
        let demand = lean::Demand {
            blocked: t.blocked_by.iter().any(|b| !b.closed),
            subscription: subscription_to_lean(u.subscription),
            admin: plan.is_admin(),
            counted: t.counted,
            limits: lean::PlanLimits {
                threads: plan.threads_per_period.map(nat),
                workspace_seconds: plan.workspace_seconds_per_period.map(nat),
                max_concurrency: nat(plan.max_concurrency),
            },
            used: lean::Used {
                threads: nat(u.period_threads),
                workspace_seconds: nat(u.period_workspace_seconds),
                running_workspaces: nat(u.running_workspaces),
            },
        };
        let decision = lean::decide_start(demand, placement);
        let version = t.version;
        t.version += 1;
        t.updated_at = now;
        let mut tx = db.tx(user);
        let outcome = match &decision {
            lean::Start::Start { placement } => {
                let first = !t.counted;
                set_state(
                    &mut t,
                    &lean::State::Running {
                        turn: lean::Turn::Preparing,
                    },
                );
                t.counted = true;
                t.workspaces += 1;
                let mut change = Change::new().when(Cond::exists());
                if first {
                    change = change.add("period_threads", 1);
                    if let Some(limit) = plan.threads_per_period {
                        change = change.when(Cond::lt("period_threads", limit as i64));
                    }
                    if let Some(limit) = plan.workspace_seconds_per_period {
                        change = change.when(Cond::lt("period_workspace_seconds", limit as i64));
                    }
                }
                match placement {
                    Placement::Cloud => {
                        change = change
                            .add("running_workspaces", 1)
                            .when(Cond::lt("running_workspaces", plan.max_concurrency as i64));
                    }
                    Placement::Host { name: host } => {
                        let h = hosts.iter().find(|h| &h.name == host).expect("placed");
                        tx.update(
                            &keys::host(user, host),
                            Change::new()
                                .add("running_workspaces", 1)
                                .when(Cond::eq("state", s("online")))
                                .when(Cond::lt("running_workspaces", h.max_workspaces as i64)),
                        )?;
                    }
                    _ => unreachable!("decideStart starts at the cloud or a Host"),
                }
                if first || matches!(placement, Placement::Cloud) {
                    tx.update(&keys::user(user), change)?;
                }
                tx.insert_entity(&Workspace {
                    thread: name.to_string(),
                    seq: t.workspaces,
                    target: placement_name(placement),
                    platform: None,
                    state: "starting".into(),
                    started_at: now,
                    stopped_at: None,
                    stop_reason: None,
                    exit_code: None,
                    repositories: Vec::new(),
                })?;
                Started::Running {
                    workspace: t.workspaces,
                    placement: placement_name(placement),
                }
            }
            lean::Start::Pending { reason } => {
                set_state(&mut t, &lean::State::Pending { reason: *reason });
                Started::Pending(pending_of(reason))
            }
            lean::Start::NeedsYou { error } => {
                set_state(
                    &mut t,
                    &lean::State::NeedsYou {
                        error: error.clone(),
                    },
                );
                Started::NeedsYou(error_name(error).to_string())
            }
        };
        tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        tx.delete(&job, None)?;
        match tx.commit().await {
            Ok(()) => return Ok(outcome),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ThreadError::Busy(name.to_string()))
}

/// The context of a thread for `step`: its SkillRuns and `subagent` children that run, and the
/// inputs that wait.
pub async fn context(db: &Db, user: &str, t: &Thread) -> Result<lean::Context, ThreadError> {
    let skills = crate::engine_store::running_runs(db, user, &t.name).await?;
    let mut children = 0;
    for child in db.children(user, &t.name).await? {
        if child.lineage != Lineage::Subagent {
            continue;
        }
        if let Some(c) = db
            .entity::<Thread>(&keys::thread(user, &child.name))
            .await?
            && matches!(
                c.status,
                ThreadStatus::Pending | ThreadStatus::Running | ThreadStatus::Waiting
            )
        {
            children += 1;
        }
    }
    Ok(lean::Context {
        running_skills: nat(skills),
        running_children: nat(children),
        queued: nat(t.queued.len() as u64),
    })
}

/// Applies an event to a thread with `step` and writes it over the version it read. A thread
/// that asks for a turn (it becomes `RUNNING` with its turn asked for, or its Workspace is
/// ready) sends the inputs that waited as the input of the turn. The stop goes through
/// [`stop`].
pub async fn apply(
    db: &Db,
    blobs: &crate::blob::Blobs,
    user: &str,
    name: &str,
    event: lean::Event,
    now: u64,
) -> Result<lean::State, ThreadError> {
    if event == lean::Event::Stop {
        stop(db, blobs, user, name, now).await?;
        return Ok(lean::State::Aborted);
    }
    for _ in 0..ATTEMPTS {
        let mut t = read_thread(db, user, name).await?;
        let ctx = context(db, user, &t).await?;
        let before = state_of(&t);
        let after = lean::step(before.clone(), event.clone(), ctx);
        let asks_turn = matches!(
            after,
            lean::State::Running {
                turn: lean::Turn::Preparing
            }
        ) && !t.queued.is_empty()
            && (after != before || event == lean::Event::WorkspaceReady);
        if after == before && !asks_turn {
            return Ok(after);
        }
        let version = t.version;
        set_state(&mut t, &after);
        t.version += 1;
        t.updated_at = now;
        let mut tx = db.tx(user);
        let mut bodies = Vec::new();
        if asks_turn {
            bodies.push(input_message(name, &t.queued, "turn"));
            t.queued.clear();
        }
        let last = if bodies.is_empty() {
            0
        } else {
            outbox::last_seq(db, user, name).await?
        };
        tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        outbox::append(&mut tx, name, last, &bodies)?;
        match tx.commit().await {
            Ok(()) => return Ok(after),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ThreadError::Busy(name.to_string()))
}

/// The `input` of the Thread channel for inputs that go together: their texts, in order, as one
/// input with the ID of the first.
pub fn input_message(thread: &str, inputs: &[Value], delivery: &str) -> Value {
    let text = inputs
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    json!({
        "type": "input",
        "threadId": thread,
        "inputId": inputs.first().and_then(|i| i["id"].as_str()).unwrap_or_default(),
        "text": text,
        "delivery": delivery,
    })
}

/// What happened to an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    Steered,
    Queued,
    StartedTurn,
    /// An input of sanchoris to an `ABORTED` thread, which is not sent.
    Dropped,
}

/// Delivers an input to a thread (`skill.md`「スレッドへの入力」) with `deliver`: into the running
/// turn, into the queue of the thread, or as a new turn with the inputs that waited. An input of
/// a person to an `ABORTED` thread is refused.
pub async fn send_input(
    db: &Db,
    user: &str,
    name: &str,
    source: lean::Source,
    input: Value,
    now: u64,
) -> Result<Delivered, ThreadError> {
    for _ in 0..ATTEMPTS {
        let mut t = read_thread(db, user, name).await?;
        let state = state_of(&t);
        let delivery = lean::deliver(state.clone(), source);
        let version = t.version;
        let mut bodies = Vec::new();
        let outcome = match delivery {
            lean::Delivery::Reject => return Err(ThreadError::Aborted(name.to_string())),
            lean::Delivery::Drop => return Ok(Delivered::Dropped),
            lean::Delivery::Steer => {
                bodies.push(input_message(name, std::slice::from_ref(&input), "steer"));
                Delivered::Steered
            }
            lean::Delivery::Queue => {
                t.queued.push(input.clone());
                Delivered::Queued
            }
            lean::Delivery::StartTurn => {
                let ctx = context(db, user, &t).await?;
                let after = lean::step(state, lean::Event::Input { source }, ctx);
                set_state(&mut t, &after);
                t.queued.push(input.clone());
                bodies.push(input_message(name, &t.queued, "turn"));
                t.queued.clear();
                Delivered::StartedTurn
            }
        };
        t.version += 1;
        t.updated_at = now;
        let last = if bodies.is_empty() {
            0
        } else {
            outbox::last_seq(db, user, name).await?
        };
        let mut tx = db.tx(user);
        tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        outbox::append(&mut tx, name, last, &bodies)?;
        match tx.commit().await {
            Ok(()) => return Ok(outcome),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ThreadError::Busy(name.to_string()))
}

/// Adds to a transaction the stop of the last Workspace of a thread, and gives back its slot of
/// the concurrency of the User or of the Host. The Workspace changes to `stopped` under the
/// condition that it was not, so that a second stop gives back nothing.
fn stop_workspace(
    tx: &mut crate::db::UserTx,
    t: &Thread,
    workspace: &Workspace,
    reason: &str,
    now: u64,
) -> Result<(), DbError> {
    let user = tx.user().to_string();
    tx.update(
        &keys::workspace(&user, &t.name, workspace.seq),
        Change::new()
            .set("state", s("stopped"))
            .set("stopped_at", n(now))
            .set("stop_reason", s(reason))
            .when(Cond::ne("state", s("stopped"))),
    )?;
    match workspace.target.as_str() {
        "cloud" => tx.update(
            &keys::user(&user),
            Change::new()
                .add("running_workspaces", -1)
                .when(Cond::ge("running_workspaces", 1)),
        ),
        host => tx.update(
            &keys::host(&user, host),
            Change::new()
                .add("running_workspaces", -1)
                .when(Cond::ge("running_workspaces", 1)),
        ),
    }
}

/// Stops a Workspace of a thread that stopped on its own or was removed, once.
pub async fn release_workspace(
    db: &Db,
    user: &str,
    name: &str,
    seq: u64,
    reason: &str,
    now: u64,
) -> Result<bool, ThreadError> {
    let t = read_thread(db, user, name).await?;
    let Some(w) = db
        .entity::<Workspace>(&keys::workspace(user, name, seq))
        .await?
    else {
        return Ok(false);
    };
    if w.state == "stopped" {
        return Ok(false);
    }
    let mut tx = db.tx(user);
    stop_workspace(&mut tx, &t, &w, reason, now)?;
    match tx.commit().await {
        Ok(()) => Ok(true),
        Err(DbError::Condition { .. }) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Stops a thread: it becomes `ABORTED`, frees its thread key, stops its Workspace, aborts its
/// SkillRuns, and stops its `subagent` children (`skill.md`「スレッドの状態」).
pub async fn stop(
    db: &Db,
    blobs: &crate::blob::Blobs,
    user: &str,
    name: &str,
    now: u64,
) -> Result<(), ThreadError> {
    let mut pending = vec![name.to_string()];
    while let Some(name) = pending.pop() {
        stop_one(db, blobs, user, &name, now).await?;
        for child in db.children(user, &name).await? {
            if child.lineage == Lineage::Subagent {
                pending.push(child.name);
            }
        }
    }
    Ok(())
}

async fn stop_one(
    db: &Db,
    blobs: &crate::blob::Blobs,
    user: &str,
    name: &str,
    now: u64,
) -> Result<(), ThreadError> {
    let mut stopped = false;
    for _ in 0..ATTEMPTS {
        let mut t = read_thread(db, user, name).await?;
        if t.status == ThreadStatus::Aborted {
            stopped = true;
            break;
        }
        let version = t.version;
        set_state(&mut t, &lean::State::Aborted);
        t.version += 1;
        t.updated_at = now;
        t.queued.clear();
        let mut tx = db.tx(user);
        tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        if let Some(k) = &t.trigger_key {
            tx.delete(
                &keys::thread_key(user, k.repository_id, &k.trigger, &k.key),
                Some(Cond::missing("thread").or(Cond::eq("thread", s(name)))),
            )?;
        }
        if t.workspaces > 0
            && let Some(w) = db
                .entity::<Workspace>(&keys::workspace(user, name, t.workspaces))
                .await?
            && w.state != "stopped"
        {
            stop_workspace(&mut tx, &t, &w, "thread stopped", now)?;
        }
        tx.delete(&keys::job(user, &start_job(name)), None)?;
        match tx.commit().await {
            Ok(()) => {
                stopped = true;
                break;
            }
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    if !stopped {
        return Err(ThreadError::Busy(name.to_string()));
    }
    crate::engine_store::abort_runs(db, blobs, user, name, now).await?;
    Ok(())
}
