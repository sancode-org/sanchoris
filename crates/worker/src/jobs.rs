//! The jobs of the worker (`architecture.md`「処理とデータ」): the thread-starter, the restore of
//! Workspaces (workspace-supervisor), the engine-worker, the delivery of notifications to
//! threads (thread-supervisor), and the renewal of `credentials`. Each job can come twice.

use sanchoris_core::db::{DbError, get_n, get_s, value_of};
use sanchoris_core::engine_store::{DynamoStore, resolve_slots};
use sanchoris_core::entities::{Credential, Thread, Workspace, keys};
use sanchoris_core::generated::sanchoris_thread as lean;
use sanchoris_core::thread::{self, Started};
use sanchoris_core::token::{self, TokenKind};
use sanchoris_core::{launch, outbox};
use sanchoris_engine::Engine;
use serde_json::{Value, json};

use crate::channel::{self, ChannelError};
use crate::deliver;
use crate::ports::{Ctx, Launch, now};
use crate::workspace::OutboxWorkspace;

fn str_field<'a>(job: &'a Value, name: &str) -> &'a str {
    job[name].as_str().unwrap_or_default()
}

pub async fn handle(ctx: &Ctx, job: &Value) -> Result<(), ChannelError> {
    let user = str_field(job, "user");
    let name = str_field(job, "thread");
    let at = now();
    match str_field(job, "type") {
        "thread.start" => {
            if !credential_ready(ctx, user, name, at).await? {
                return Ok(());
            }
            let started = thread::start_pending(&ctx.db, user, name, at).await?;
            launch_started(ctx, user, name, started, at).await
        }
        "thread.restore" => {
            if !credential_ready(ctx, user, name, at).await? {
                return Ok(());
            }
            let started = thread::restore(&ctx.db, user, name, at).await?;
            launch_started(ctx, user, name, started, at).await
        }
        "engine.jobs" => {
            let mut engine = Engine::new(DynamoStore::new(ctx.db.clone(), ctx.blobs.clone(), user));
            let jobs = engine
                .due_jobs(at)
                .await
                .map_err(|e| ChannelError::Engine(e.to_string()))?;
            let mut w = OutboxWorkspace::new(ctx, user);
            for j in jobs {
                engine
                    .run_job(&j, &mut w, at)
                    .await
                    .map_err(|e| ChannelError::Engine(e.to_string()))?;
            }
            Ok(())
        }
        "thread.notice" => {
            let seq = job["seq"].as_u64().unwrap_or_default();
            let Some((_, body)) = outbox::messages(&ctx.db, user, name)
                .await?
                .into_iter()
                .find(|(s, _)| *s == seq)
            else {
                return Ok(());
            };
            let body = resolve_slots(&ctx.blobs, body).await?;
            let text = body["text"].as_str().unwrap_or_default();
            let mark = keys::mark(user, "notice", &format!("{name}#{seq}"));
            channel::deliver_notice(ctx, user, name, text, &mark, at).await?;
            Ok(())
        }
        "thread.credentials" => {
            let workspace = job["workspace"].as_u64().unwrap_or_default();
            renew_credentials(ctx, user, name, workspace).await?;
            Ok(())
        }
        other => {
            eprintln!("sanchoris: unknown job {other}");
            Ok(())
        }
    }
}

/// A thread whose Credential has no secret needs the User (`Credential.Missing`).
async fn credential_ready(
    ctx: &Ctx,
    user: &str,
    name: &str,
    at: u64,
) -> Result<bool, ChannelError> {
    let Some(t) = ctx.db.entity::<Thread>(&keys::thread(user, name)).await? else {
        return Ok(false);
    };
    let ready = ctx
        .db
        .entity::<Credential>(&keys::credential(user, &t.credential))
        .await?
        .is_some_and(|c| c.secret.is_some());
    if !ready {
        thread::apply(
            &ctx.db,
            &ctx.blobs,
            user,
            name,
            lean::Event::CredentialMissing,
            at,
        )
        .await?;
    }
    Ok(ready)
}

/// Makes the thread branches and starts the Workspace with a bootstrap token. A Workspace that
/// cannot start is stopped, and the thread needs the User (`Workspace.Stopped`).
async fn launch_started(
    ctx: &Ctx,
    user: &str,
    name: &str,
    started: Started,
    at: u64,
) -> Result<(), ChannelError> {
    let Started::Running {
        workspace,
        placement,
    } = started
    else {
        return Ok(());
    };
    let Some(t) = ctx.db.entity::<Thread>(&keys::thread(user, name)).await? else {
        return Ok(());
    };
    let definition = launch::read_definition(&ctx.blobs, &t).await?;
    let mut failure = None;
    for repo in definition["project"]["Repos"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
    {
        let repo = repo["Repo"].as_str().unwrap_or_default();
        if let Err(e) = ctx.github.create_branch(user, repo, &t.branch).await {
            failure = Some(format!("the branch {} of {repo}: {e}", t.branch));
            break;
        }
    }
    if failure.is_none() {
        let claims = token::claims(TokenKind::Bootstrap, user, name, workspace, at);
        let launched = ctx
            .launcher
            .launch(Launch {
                user: user.to_string(),
                thread: name.to_string(),
                workspace,
                target: placement,
                architecture: definition["project"]["Workspace"]["Architecture"]
                    .as_str()
                    .unwrap_or("arm64")
                    .to_string(),
                bootstrap_token: ctx.tokens.issue(&claims),
                channel_url: ctx.channel_url.clone(),
            })
            .await;
        failure = launched.err();
    }
    if let Some(cause) = failure {
        workspace_stopped(ctx, user, name, workspace, None, &cause, at).await?;
    }
    Ok(())
}

/// A Workspace stopped: it gives back its slot. One that stopped on its own makes the thread
/// need the User (`Workspace.Stopped`); one that was interrupted (`interrupted`) is restored
/// at the next input or command.
pub async fn workspace_stopped(
    ctx: &Ctx,
    user: &str,
    name: &str,
    workspace: u64,
    exit_code: Option<i64>,
    reason: &str,
    at: u64,
) -> Result<(), ChannelError> {
    let Some(w) = ctx
        .db
        .entity::<Workspace>(&keys::workspace(user, name, workspace))
        .await?
    else {
        return Ok(());
    };
    if w.state == "stopped" {
        return Ok(());
    }
    let released = thread::release_workspace(&ctx.db, user, name, workspace, reason, at).await?;
    if let Some(code) = exit_code {
        let mut tx = ctx.db.tx(user);
        tx.update(
            &keys::workspace(user, name, workspace),
            sanchoris_core::db::Change::new().set("exit_code", sanchoris_core::db::n(code)),
        )?;
        tx.commit().await?;
    }
    if released && reason != "interrupted" {
        let cause = reason.to_string();
        let extra = move |t: &mut Thread, _: &mut sanchoris_core::db::UserTx| {
            if t.status == sanchoris_core::entities::ThreadStatus::NeedsYou {
                t.cause = Some(cause.clone());
            }
            Ok(())
        };
        thread::apply_with(
            &ctx.db,
            &ctx.blobs,
            user,
            name,
            lean::Event::WorkspaceStopped,
            at,
            Some(&extra),
            None,
        )
        .await?;
    }
    Ok(())
}

async fn renew_credentials(
    ctx: &Ctx,
    user: &str,
    name: &str,
    workspace: u64,
) -> Result<(), DbError> {
    let Some(t) = ctx.db.entity::<Thread>(&keys::thread(user, name)).await? else {
        return Ok(());
    };
    let Some(w) = ctx
        .db
        .entity::<Workspace>(&keys::workspace(user, name, workspace))
        .await?
    else {
        return Ok(());
    };
    if t.workspaces != workspace || w.state == "stopped" || w.connection.is_none() {
        return Ok(());
    }
    for _ in 0..20 {
        let last = outbox::last_seq(&ctx.db, user, name).await?;
        let mut tx = ctx.db.tx(user);
        outbox::append(
            &mut tx,
            name,
            last,
            &[json!({"type": "credentials", "threadId": name, "$stub": true})],
        )?;
        match tx.commit().await {
            Ok(()) => return Ok(()),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(DbError::Conflict)
}

/// The dispatcher: routes a new image of the stream of the `main` table by its kind. Jobs go
/// to the queue, the jobs of the engine to the queue or the scheduler by their time, the
/// outbox to the connection of the thread with its notifications to the thread-supervisor, and
/// a Workspace with a new connection gets the outbox after the last message it has.
pub async fn route(ctx: &Ctx, pk: &str, sk: &str, kind: &str) -> Result<(), ChannelError> {
    let Some(user) = pk.strip_prefix("U#").and_then(|r| r.split('#').next()) else {
        return Ok(());
    };
    let key = sanchoris_core::db::Key::main(pk, sk);
    match kind {
        "job" => {
            let Some(item) = ctx.db.get(&key).await? else {
                return Ok(());
            };
            let v: Value = value_of(&item)?;
            let mut body = v["body"].clone();
            body["user"] = json!(user);
            ctx.queue.send(body).await;
        }
        "engine_job" => {
            let Some(item) = ctx.db.get(&key).await? else {
                return Ok(());
            };
            let due = get_n(&item, "due").unwrap_or(0) as u64;
            let thread = get_s(&item, "thread").unwrap_or_default();
            let job = json!({"type": "engine.jobs", "user": user, "thread": thread});
            if due <= now() {
                ctx.queue.send(job).await;
            } else {
                ctx.scheduler.schedule(job, due).await;
            }
        }
        "outbox" => {
            let Some(item) = ctx.db.get(&key).await? else {
                return Ok(());
            };
            let o: outbox::OutboxItem = value_of(&item)?;
            for (i, body) in o.bodies.iter().enumerate() {
                if body.contains("\"skill_notice\"")
                    && serde_json::from_str::<Value>(body)
                        .is_ok_and(|b| b["type"] == json!("skill_notice"))
                {
                    ctx.queue
                        .send(json!({
                            "type": "thread.notice",
                            "user": user,
                            "thread": o.thread,
                            "seq": o.first_seq + i as u64,
                        }))
                        .await;
                }
            }
            deliver::flush(ctx, user, &o.thread).await?;
        }
        "workspace" => {
            let Some(item) = ctx.db.get(&key).await? else {
                return Ok(());
            };
            let w: Workspace = value_of(&item)?;
            if w.connection.is_some() {
                deliver::flush(ctx, user, &w.thread).await?;
            }
        }
        _ => {}
    }
    Ok(())
}
