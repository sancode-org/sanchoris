//! The control side of the Thread channel (`apps/sanchoris-agent/PROTOCOL.md`): the connection
//! with a token, and each message of `sanchoris-agent`. A handler keeps no state between calls:
//! the connection's thread and Workspace come with each call (the authorizer context of API
//! Gateway, or the socket locally), and everything else is in DynamoDB. The messages to the
//! agent go to the outbox, which the dispatcher sends.

use sanchoris_core::db::{
    Change, Cond, DbError, Key, TableKey, UserTx, get_n, item_of, n, s, user_pk,
};
use sanchoris_core::engine_store::DynamoStore;
use sanchoris_core::entities::{
    Credential, Thread, ThreadRecord, ThreadStatus, Turn, TurnCommit, Usage, Workspace, keys,
};
use sanchoris_core::generated::sanchoris_thread as lean;
use sanchoris_core::launch::{self, LaunchError};
use sanchoris_core::outbox;
use sanchoris_core::thread::{self, Once};
use sanchoris_core::token::{TokenError, TokenKind};
use sanchoris_engine::{Engine, ExecKey, Origin, Response, Store, exec_output};
use serde_json::{Value, json};

use crate::ports::Ctx;
use crate::workspace::OutboxWorkspace;

/// The connection's User, thread and Workspace, from its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub user: String,
    pub thread: String,
    pub workspace: u64,
    pub bootstrap: bool,
}

/// Why a connection is refused: HTTP 401 for a bad or expired token, 403 for a spent bootstrap
/// token or a Workspace that is not the thread's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Unauthorized,
    Forbidden,
}

/// The frames to send back on the connection, and whether to close it after them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub frames: Vec<String>,
    pub close: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Thread(#[from] thread::ThreadError),
    #[error("{0}")]
    Engine(String),
}

fn mark_key(user: &str, kind: &str, id: &str) -> Key {
    keys::mark(user, kind, id)
}

fn cursor_key(user: &str, thread: &str, instance: &str) -> Key {
    Key::main(user_pk(user), format!("THREAD#{thread}#INST#{instance}"))
}

/// The key that makes a message of an agent take effect once.
fn inbound_key(user: &str, thread: &str, instance: &str, seq: u64) -> Key {
    mark_key(user, "in", &format!("{thread}#{instance}#{seq}"))
}

pub async fn connect(ctx: &Ctx, authorization: Option<&str>, now: u64) -> Result<Session, Refused> {
    let token = authorization
        .and_then(|a| a.strip_prefix("Bearer "))
        .ok_or(Refused::Unauthorized)?;
    let claims = ctx.tokens.verify(token, now).map_err(|e| match e {
        TokenError::Invalid | TokenError::Expired => Refused::Unauthorized,
    })?;
    let t: Thread = ctx
        .db
        .entity(&keys::thread(&claims.user, &claims.thread))
        .await
        .ok()
        .flatten()
        .ok_or(Refused::Forbidden)?;
    let w: Workspace = ctx
        .db
        .entity(&keys::workspace(
            &claims.user,
            &claims.thread,
            claims.workspace,
        ))
        .await
        .ok()
        .flatten()
        .ok_or(Refused::Forbidden)?;
    if t.workspaces != claims.workspace || w.state == "stopped" || t.status == ThreadStatus::Aborted
    {
        return Err(Refused::Forbidden);
    }
    let bootstrap = claims.kind == TokenKind::Bootstrap;
    if bootstrap {
        let key = mark_key(&claims.user, "bootstrap", &claims.id);
        let mut tx = ctx.db.tx(&claims.user);
        let item = item_of(&key, "mark", &json!({"at": now})).map_err(|_| Refused::Forbidden)?;
        tx.put(TableKey::Main, item, Some(Cond::not_exists()))
            .map_err(|_| Refused::Forbidden)?;
        tx.commit().await.map_err(|_| Refused::Forbidden)?;
    }
    Ok(Session {
        user: claims.user,
        thread: claims.thread,
        workspace: claims.workspace,
        bootstrap,
    })
}

/// The connection closed.
pub async fn disconnect(ctx: &Ctx, session: &Session, connection: &str) -> Result<(), DbError> {
    let mut tx = ctx.db.tx(&session.user);
    tx.update(
        &keys::workspace(&session.user, &session.thread, session.workspace),
        Change::new()
            .remove("connection")
            .when(Cond::eq("connection", s(connection))),
    )?;
    match tx.commit().await {
        Ok(()) | Err(DbError::Condition { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

fn frame(v: Value) -> String {
    v.to_string()
}

fn ack(thread: &str, seq: u64) -> String {
    frame(json!({"type": "ack", "threadId": thread, "seq": seq}))
}

fn close(thread: &str, reason: &str) -> Reply {
    Reply {
        frames: vec![frame(
            json!({"type": "close", "threadId": thread, "code": 4000, "reason": reason}),
        )],
        close: true,
    }
}

async fn stored_seq(ctx: &Ctx, session: &Session, instance: &str) -> Result<u64, DbError> {
    Ok(ctx
        .db
        .get(&cursor_key(&session.user, &session.thread, instance))
        .await?
        .and_then(|i| get_n(&i, "seq"))
        .unwrap_or(0) as u64)
}

async fn store_seq(ctx: &Ctx, session: &Session, instance: &str, seq: u64) -> Result<(), DbError> {
    let key = cursor_key(&session.user, &session.thread, instance);
    let mut item = item_of(&key, "inbound", &json!({}))?;
    item.insert("seq".into(), n(seq));
    let mut tx = ctx.db.tx(&session.user);
    tx.put(
        TableKey::Main,
        item,
        Some(Cond::missing_or_lt("seq", seq as i64)),
    )?;
    match tx.commit().await {
        Ok(()) | Err(DbError::Condition { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn workspace(ctx: &Ctx, session: &Session) -> Result<Option<Workspace>, DbError> {
    ctx.db
        .entity(&keys::workspace(
            &session.user,
            &session.thread,
            session.workspace,
        ))
        .await
}

async fn read_thread(ctx: &Ctx, session: &Session) -> Result<Thread, ChannelError> {
    ctx.db
        .entity(&keys::thread(&session.user, &session.thread))
        .await?
        .ok_or_else(|| thread::ThreadError::Missing(session.thread.clone()).into())
}

/// Takes one frame of `sanchoris-agent`.
pub async fn receive(
    ctx: &Ctx,
    session: &Session,
    connection: &str,
    text: &str,
    now: u64,
) -> Result<Reply, ChannelError> {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return Ok(close(&session.thread, "protocol error"));
    };
    if msg["threadId"].as_str() != Some(session.thread.as_str()) {
        return Ok(close(&session.thread, "protocol error"));
    }
    let Some(w) = workspace(ctx, session).await? else {
        return Ok(close(&session.thread, "no workspace"));
    };
    if w.state == "stopped" {
        return Ok(close(&session.thread, "the workspace is stopped"));
    }
    match msg["type"].as_str() {
        Some("hello") => hello(ctx, session, connection, &w, &msg, now).await,
        Some("heartbeat") => {
            if msg["turnRunning"] == json!(true) {
                turn_started(ctx, session, now).await?;
            }
            Ok(Reply::default())
        }
        Some(kind) => {
            let (Some(instance), Some(seq)) = (w.instance.clone(), msg["seq"].as_u64()) else {
                return Ok(close(&session.thread, "protocol error"));
            };
            let stored = stored_seq(ctx, session, &instance).await?;
            if seq <= stored {
                return Ok(Reply {
                    frames: vec![ack(&session.thread, stored)],
                    close: false,
                });
            }
            let once_key = inbound_key(&session.user, &session.thread, &instance, seq);
            let handled = message(ctx, session, kind, &msg, &instance, &once_key, now).await?;
            if let Some(reply) = handled {
                return Ok(reply);
            }
            store_seq(ctx, session, &instance, seq).await?;
            Ok(Reply {
                frames: vec![ack(&session.thread, seq)],
                close: false,
            })
        }
        None => Ok(close(&session.thread, "protocol error")),
    }
}

/// The first instruction of a Workspace: the start of the thread, or the inputs that waited
/// for a restored Workspace.
fn instruction(t: &Thread, definition: &Value) -> Option<String> {
    let texts: Vec<&str> = t.queued.iter().filter_map(|i| i["text"].as_str()).collect();
    if t.record.is_none() && t.total_turns == 0 && !texts.is_empty() {
        let mut parts = vec![launch::start_instruction(t, definition, texts[0])];
        parts.extend(texts[1..].iter().map(|s| s.to_string()));
        return Some(parts.join("\n\n"));
    }
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

async fn hello(
    ctx: &Ctx,
    session: &Session,
    connection: &str,
    w: &Workspace,
    msg: &Value,
    now: u64,
) -> Result<Reply, ChannelError> {
    let (Some(instance), Some(started), Some(last_seq)) = (
        msg["instanceId"].as_str(),
        msg["started"].as_bool(),
        msg["lastSeq"].as_u64(),
    ) else {
        return Ok(close(&session.thread, "protocol error"));
    };
    if msg["protocolVersion"] != json!(1) {
        return Ok(close(&session.thread, "protocol version"));
    }
    for _ in 0..20 {
        let mut t = read_thread(ctx, session).await?;
        let version = t.version;
        let last = outbox::last_seq(&ctx.db, &session.user, &session.thread).await?;
        let mut stubs = Vec::new();
        let mut floor = w.outbox_from;
        if session.bootstrap {
            floor = last;
            stubs.push(json!({"type": "credentials", "threadId": session.thread, "$stub": true}));
        }
        let takes_queue = !started;
        if takes_queue {
            let definition = launch::read_definition(&ctx.blobs, &t).await?;
            stubs.push(json!({
                "type": "start",
                "threadId": session.thread,
                "$stub": true,
                "instruction": instruction(&t, &definition),
            }));
            t.queued.clear();
            t.version += 1;
            t.updated_at = now;
        }
        let mut tx = ctx.db.tx(&session.user);
        tx.update(
            &keys::workspace(&session.user, &session.thread, session.workspace),
            Change::new()
                .set("connection", s(connection))
                .set("instance", s(instance))
                .set("outbox_from", n(floor))
                .set("sent", n(last_seq.max(floor)))
                .when(Cond::ne("state", s("stopped"))),
        )?;
        if takes_queue {
            tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        }
        outbox::append(&mut tx, &session.thread, last, &stubs)?;
        match tx.commit().await {
            Ok(()) => {
                let stored = stored_seq(ctx, session, instance).await?;
                return Ok(Reply {
                    frames: vec![ack(&session.thread, stored)],
                    close: false,
                });
            }
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(thread::ThreadError::Busy(session.thread.clone()).into())
}

/// A turn that was asked for runs.
async fn turn_started(ctx: &Ctx, session: &Session, now: u64) -> Result<(), ChannelError> {
    let t = read_thread(ctx, session).await?;
    if t.status == ThreadStatus::Running && !t.turn_active {
        thread::apply(
            &ctx.db,
            &ctx.blobs,
            &session.user,
            &session.thread,
            lean::Event::TurnStarted,
            now,
        )
        .await?;
    }
    Ok(())
}

/// The key of a line of the log of a thread.
pub fn log_key(user: &str, thread: &str, at: u64, instance: &str, seq: u64) -> String {
    format!(
        "{}threads/{thread}/log/{at:013}-{instance}-{seq:010}.json",
        sanchoris_core::blob::user_prefix(user)
    )
}

pub fn log_prefix(user: &str, thread: &str) -> String {
    format!(
        "{}threads/{thread}/log/",
        sanchoris_core::blob::user_prefix(user)
    )
}

async fn log(
    ctx: &Ctx,
    session: &Session,
    instance: &str,
    msg: &Value,
    now: u64,
) -> Result<(), DbError> {
    let seq = msg["seq"].as_u64().unwrap_or_default();
    let line = json!({"at": now, "type": msg["type"], "kind": msg["kind"], "data": msg});
    ctx.blobs
        .put(
            &log_key(&session.user, &session.thread, now, instance, seq),
            line.to_string().into_bytes(),
        )
        .await
}

async fn respond(
    ctx: &Ctx,
    session: &Session,
    once_key: &Key,
    body: Value,
    now: u64,
) -> Result<(), ChannelError> {
    for _ in 0..20 {
        if ctx.db.get(once_key).await?.is_some() {
            return Ok(());
        }
        let last = outbox::last_seq(&ctx.db, &session.user, &session.thread).await?;
        let mut tx = ctx.db.tx(&session.user);
        let item = item_of(once_key, "mark", &json!({"at": now}))?;
        tx.put(TableKey::Main, item, Some(Cond::not_exists()))?;
        outbox::append(&mut tx, &session.thread, last, std::slice::from_ref(&body))?;
        match tx.commit().await {
            Ok(()) => return Ok(()),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(thread::ThreadError::Busy(session.thread.clone()).into())
}

fn response_body(
    thread: &str,
    kind: &str,
    request: &Value,
    r: Result<Value, (String, String)>,
) -> Value {
    match r {
        Ok(result) => json!({
            "type": kind, "threadId": thread, "requestId": request, "ok": true, "result": result,
        }),
        Err((code, message)) => json!({
            "type": kind, "threadId": thread, "requestId": request, "ok": false,
            "error": {"code": code, "message": message},
        }),
    }
}

fn engine_result(r: Response) -> Result<Value, (String, String)> {
    match r {
        Response::Ok(v) => Ok(v),
        Response::Error { code, message } => Err((code, message)),
    }
}

async fn skill_request(
    ctx: &Ctx,
    session: &Session,
    msg: &Value,
    instance: &str,
    now: u64,
) -> Result<Result<Value, (String, String)>, ChannelError> {
    let request = &msg["request"];
    let id = format!(
        "{}:{instance}:{}",
        session.thread,
        msg["requestId"].as_str().unwrap_or_default()
    );
    let mut engine = Engine::new(DynamoStore::new(
        ctx.db.clone(),
        ctx.blobs.clone(),
        &session.user,
    ));
    let run_id = request["runId"].as_u64().unwrap_or_default();
    let engine_error = |e: sanchoris_engine::EngineError| ChannelError::Engine(e.to_string());
    match request["action"].as_str() {
        Some("run") => {
            let t = read_thread(ctx, session).await?;
            let definition = launch::read_definition(&ctx.blobs, &t).await?;
            let name = request["name"].as_str().unwrap_or_default();
            let Some(skill) = launch::predictable(&definition)
                .into_iter()
                .find(|s| s["name"] == json!(name))
            else {
                return Ok(Err((
                    "unknown_skill".into(),
                    format!("Skill {name} is not available in this thread"),
                )));
            };
            let input =
                match launch::launch_input(&definition, skill, &request["input"].to_string()) {
                    Ok(v) => v,
                    Err(LaunchError::Skill { code, message }) => return Ok(Err((code, message))),
                    Err(e) => return Err(ChannelError::Engine(e.to_string())),
                };
            match launch::start_run(
                &ctx.db,
                &ctx.blobs,
                &session.user,
                &session.thread,
                skill,
                input,
                &id,
                Origin::Thread,
                now,
            )
            .await
            {
                Ok(v) => Ok(Ok(v)),
                Err(LaunchError::Skill { code, message }) => Ok(Err((code, message))),
                Err(e) => Err(ChannelError::Engine(e.to_string())),
            }
        }
        Some("continue") | Some("await") | Some("status") => {
            let Some(record) = engine
                .store
                .run(run_id)
                .await
                .map_err(|e| ChannelError::Engine(e.to_string()))?
                .filter(|r| r.thread == session.thread)
            else {
                return Ok(Err((
                    "unknown_run".into(),
                    format!("SkillRun {run_id} is not a SkillRun of this thread"),
                )));
            };
            match request["action"].as_str() {
                Some("continue") => Ok(engine_result(
                    engine
                        .continue_(&id, run_id, &request["output"], now)
                        .await
                        .map_err(engine_error)?,
                )),
                Some("await") => Ok(engine_result(
                    engine
                        .await_(&id, run_id, now)
                        .await
                        .map_err(engine_error)?,
                )),
                _ => Ok(Ok(run_status(&record))),
            }
        }
        _ => Ok(Err((
            "invalid_request".into(),
            "unknown skill action".into(),
        ))),
    }
}

/// The state of a SkillRun, for `sanchoris-agent skill status` and the screens.
pub fn run_status(record: &sanchoris_engine::RunRecord) -> Value {
    let run = &record.run;
    json!({
        "RunId": record.id(),
        "Skill": record.definition.skill,
        "Status": sanchoris_engine::engine::status_name(&run.status),
        "Origin": match record.origin { Origin::Thread => "thread", Origin::User => "user" },
        "Output": run.output.as_ref().map(sanchoris_engine::json::from_lean),
        "Error": run.error,
        "Cause": run.cause,
    })
}

/// Handles a message with `seq`; `Some` replies without acknowledging it.
async fn message(
    ctx: &Ctx,
    session: &Session,
    kind: &str,
    msg: &Value,
    instance: &str,
    once_key: &Key,
    now: u64,
) -> Result<Option<Reply>, ChannelError> {
    let once = Some(Once { key: once_key });
    let (user, name) = (session.user.as_str(), session.thread.as_str());
    match kind {
        "ready" => {
            let session_id = msg["sessionId"].as_str().unwrap_or_default().to_string();
            let ws_key = keys::workspace(user, name, session.workspace);
            let extra = move |t: &mut Thread, tx: &mut UserTx| {
                t.provider_session = Some(session_id.clone());
                tx.update(
                    &ws_key,
                    Change::new()
                        .set("state", s("running"))
                        .when(Cond::ne("state", s("stopped"))),
                )
            };
            thread::apply_with(
                &ctx.db,
                &ctx.blobs,
                user,
                name,
                lean::Event::WorkspaceReady,
                now,
                Some(&extra),
                once,
            )
            .await?;
            let mut engine = Engine::new(DynamoStore::new(ctx.db.clone(), ctx.blobs.clone(), user));
            let mut w = OutboxWorkspace::new(ctx, user);
            engine
                .redispatch(name, &mut w, now)
                .await
                .map_err(|e| ChannelError::Engine(e.to_string()))?;
        }
        "event" => {
            log(ctx, session, instance, msg, now).await?;
            turn_started(ctx, session, now).await?;
        }
        "rate_limit" => log(ctx, session, instance, msg, now).await?,
        "turn_end" => turn_end(ctx, session, msg, once, now).await?,
        "thread_record" => {
            let record = ThreadRecord {
                turn: msg["turn"].as_u64().unwrap_or_default(),
                session: msg["sessionId"].as_str().unwrap_or_default().into(),
                key: msg["key"].as_str().unwrap_or_default().into(),
                sha256: msg["sha256"].as_str().unwrap_or_default().into(),
                bytes: msg["bytes"].as_u64().unwrap_or_default(),
            };
            if !record.key.starts_with(&format!(
                "{}threads/{name}/records/",
                sanchoris_core::blob::user_prefix(user)
            )) {
                return Ok(Some(close(name, "a record outside the thread")));
            }
            update_thread(ctx, session, once, now, move |t| {
                t.record = Some(record.clone());
            })
            .await?;
        }
        "skill_request" => {
            let result = skill_request(ctx, session, msg, instance, now).await?;
            let body = response_body(name, "skill_response", &msg["requestId"], result);
            respond(ctx, session, once_key, body, now).await?;
        }
        "thread_request" => {
            let body = response_body(
                name,
                "thread_response",
                &msg["requestId"],
                Err((
                    "unsupported".into(),
                    "Child threads are not available yet".into(),
                )),
            );
            respond(ctx, session, once_key, body, now).await?;
        }
        "exec_result" => {
            let Ok(key) = msg["idempotencyKey"]
                .as_str()
                .unwrap_or_default()
                .parse::<ExecKey>()
            else {
                return Ok(Some(close(name, "protocol error")));
            };
            let output = exec_output(
                msg["exitCode"].as_i64().unwrap_or(-1),
                msg["stdout"].as_str().unwrap_or_default(),
                msg["stderr"].as_str().unwrap_or_default(),
                msg["timedOut"] == json!(true),
            );
            let mut engine = Engine::new(DynamoStore::new(ctx.db.clone(), ctx.blobs.clone(), user));
            let owned = engine
                .store
                .run(key.run)
                .await
                .map_err(|e| ChannelError::Engine(e.to_string()))?
                .is_some_and(|r| r.thread == name);
            if !owned {
                return Ok(None);
            }
            engine
                .exec_result(key, output, now)
                .await
                .map_err(|e| ChannelError::Engine(e.to_string()))?;
        }
        "script_result" => {
            log(ctx, session, instance, msg, now).await?;
            script_result(ctx, session, msg, once_key, now).await?;
        }
        "credential_update" => credential_update(ctx, session, msg, now).await?,
        "stopping" => {
            log(ctx, session, instance, msg, now).await?;
            thread::release_workspace(&ctx.db, user, name, session.workspace, "interrupted", now)
                .await?;
        }
        _ => return Ok(Some(close(name, "protocol error"))),
    }
    Ok(None)
}

async fn update_thread(
    ctx: &Ctx,
    session: &Session,
    once: Option<Once<'_>>,
    now: u64,
    change: impl Fn(&mut Thread),
) -> Result<(), ChannelError> {
    for _ in 0..20 {
        if let Some(o) = once
            && ctx.db.get(o.key).await?.is_some()
        {
            return Ok(());
        }
        let mut t = read_thread(ctx, session).await?;
        let version = t.version;
        change(&mut t);
        t.version += 1;
        t.updated_at = now;
        let mut tx = ctx.db.tx(&session.user);
        if let Some(o) = once {
            let item = item_of(o.key, "mark", &json!({"at": now}))?;
            tx.put(TableKey::Main, item, Some(Cond::not_exists()))?;
        }
        tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        match tx.commit().await {
            Ok(()) => return Ok(()),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(thread::ThreadError::Busy(session.thread.clone()).into())
}

/// The thread-supervisor at the end of a turn: the Turn with its commits and its record, the
/// reply as a Message of the agent, the counts and the Usage of the thread, and the step of
/// the thread, in one transaction.
async fn turn_end(
    ctx: &Ctx,
    session: &Session,
    msg: &Value,
    once: Option<Once<'_>>,
    now: u64,
) -> Result<(), ChannelError> {
    let (user, name) = (session.user.as_str(), session.thread.as_str());
    let t = read_thread(ctx, session).await?;
    let result = msg["result"].as_str().unwrap_or("failed").to_string();
    let error_code = msg["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let event = match result.as_str() {
        "completed" | "interrupted" => lean::Event::TurnEnded,
        _ if error_code == "Worker.UsageLimit" => {
            let definition = launch::read_definition(&ctx.blobs, &t).await?;
            lean::Event::UsageLimit {
                auto: definition["project"]["UsageLimit"]["Resume"] == json!("auto"),
            }
        }
        _ => lean::Event::TurnFailed,
    };
    let usage: Usage = ctx
        .db
        .entity(&keys::usage(user, name))
        .await?
        .unwrap_or(Usage {
            thread: name.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
        });
    let seq = msg["turn"].as_u64().unwrap_or_default();
    let cost = msg["costUsd"].as_f64().unwrap_or(0.0);
    let usage = Usage {
        input_tokens: usage.input_tokens + msg["usage"]["inputTokens"].as_u64().unwrap_or(0),
        output_tokens: usage.output_tokens + msg["usage"]["outputTokens"].as_u64().unwrap_or(0),
        cost_usd: usage.cost_usd + cost,
        ..usage
    };
    let commits: Vec<TurnCommit> = msg["commits"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .filter_map(|c| {
                    Some(TurnCommit {
                        repository: c["repo"].as_str()?.to_string(),
                        commit: c["commit"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let summary = msg["summary"].as_str().unwrap_or_default().to_string();
    let cause = msg["error"]["message"].as_str().map(str::to_string);
    let duration = msg["durationMs"].as_u64().unwrap_or(0);
    let extra = move |t: &mut Thread, tx: &mut UserTx| {
        let turn = Turn {
            thread: t.name.clone(),
            seq,
            started_at: now.saturating_sub(duration),
            ended_at: Some(now),
            result: Some(result.clone()),
            commits: commits.clone(),
            record_version: t
                .record
                .as_ref()
                .filter(|r| r.turn == seq)
                .map(|r| r.key.clone()),
        };
        tx.put_entity(&turn, None)?;
        if !summary.is_empty() {
            let (_, m) = launch::push_message(t, "agent", &summary, now);
            tx.insert_entity(&m)?;
        }
        t.total_turns += 1;
        t.limit_turns += 1;
        t.total_cost_usd += cost;
        t.limit_cost_usd += cost;
        if t.status == ThreadStatus::NeedsYou {
            t.cause = cause.clone();
        }
        tx.put_entity(&usage, None)
    };
    thread::apply_with(
        &ctx.db,
        &ctx.blobs,
        user,
        name,
        event,
        now,
        Some(&extra),
        once,
    )
    .await?;
    Ok(())
}

/// The result of a script: a failed `RunOnCreate` script goes to the thread as an input
/// (`skill.md`「スレッドに送る指示」), and with `Async: false` the thread needs the User
/// (`Script.Failed`). Every result is in the log of the thread.
async fn script_result(
    ctx: &Ctx,
    session: &Session,
    msg: &Value,
    once_key: &Key,
    now: u64,
) -> Result<(), ChannelError> {
    let (user, name) = (session.user.as_str(), session.thread.as_str());
    let Some(script) = msg["scriptRunId"]
        .as_str()
        .and_then(|id| id.strip_prefix("create:"))
    else {
        return Ok(());
    };
    let exit_code = msg["exitCode"].as_i64().unwrap_or(-1);
    let timed_out = msg["timedOut"] == json!(true);
    if exit_code == 0 && !timed_out {
        return Ok(());
    }
    let t = read_thread(ctx, session).await?;
    let definition = launch::read_definition(&ctx.blobs, &t).await?;
    let blocking = definition["project"]["Scripts"][script]["Async"] == json!(false);
    if blocking {
        thread::apply(
            &ctx.db,
            &ctx.blobs,
            user,
            name,
            lean::Event::ScriptFailed,
            now,
        )
        .await?;
    }
    let text = launch::script_failed(
        script,
        exit_code,
        timed_out,
        msg["stderrTail"].as_str().unwrap_or_default(),
    );
    deliver_notice(ctx, user, name, &text, once_key, now).await?;
    Ok(())
}

/// Delivers an instruction of sanchoris to a thread as an input, with its Message, once.
pub async fn deliver_notice(
    ctx: &Ctx,
    user: &str,
    name: &str,
    text: &str,
    once_key: &Key,
    now: u64,
) -> Result<thread::Delivered, ChannelError> {
    let owned = text.to_string();
    let extra = move |t: &mut Thread, tx: &mut UserTx| {
        let (_, m) = launch::push_message(t, "sanchoris", &owned, now);
        tx.insert_entity(&m)
    };
    let input = json!({"id": once_key.sk, "source": "system", "text": text});
    match thread::send_input_with(
        &ctx.db,
        user,
        name,
        lean::Source::System,
        input,
        now,
        Some(&extra),
        Some(Once { key: once_key }),
    )
    .await
    {
        Ok(d) => Ok(d),
        Err(thread::ThreadError::Aborted(_)) => Ok(thread::Delivered::Dropped),
        Err(e) => Err(e.into()),
    }
}

/// The provider refreshed the credential: it is sealed and written over the version read, so
/// that the threads that share it keep the latest.
async fn credential_update(
    ctx: &Ctx,
    session: &Session,
    msg: &Value,
    now: u64,
) -> Result<(), ChannelError> {
    let t = read_thread(ctx, session).await?;
    let kind = msg["credential"]["kind"].as_str().unwrap_or_default();
    let value = msg["credential"]["value"].as_str().unwrap_or_default();
    for _ in 0..20 {
        let Some(mut c) = ctx
            .db
            .entity::<Credential>(&keys::credential(&session.user, &t.credential))
            .await?
        else {
            return Ok(());
        };
        let version = c.version;
        c.secret_kind = Some(kind.to_string());
        c.secret = Some(ctx.secrets.seal(&session.user, value));
        c.version += 1;
        c.last_used_at = Some(now);
        let mut tx = ctx.db.tx(&session.user);
        tx.put_entity(&c, Some(Cond::eq("version", n(version))))?;
        match tx.commit().await {
            Ok(()) => return Ok(()),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
