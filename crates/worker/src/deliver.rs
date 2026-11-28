//! The dispatcher's sending of the outbox: the messages after the last one sent, in order, on
//! the connection of the thread's Workspace. `start` and `credentials` are written as stubs and
//! made when they are sent, so that the tokens, the provider credential and the presigned URLs
//! live only on the connection.

use sanchoris_core::blob::{content_key, user_prefix};
use sanchoris_core::db::{Change, Cond, DbError, s};
use sanchoris_core::engine_store::resolve_slots;
use sanchoris_core::entities::{Credential, Thread, User, Workspace, keys};
use sanchoris_core::launch;
use sanchoris_core::token::{self, TokenKind};
use sanchoris_core::{outbox, plans};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ports::{Ctx, now, rfc3339};

/// The git identity of the commits of the threads.
pub const GIT_AUTHOR: (&str, &str) = ("sanchoris[bot]", "sanchoris[bot]@users.noreply.github.com");

/// How long before a token expires the control plane sends new `credentials`.
const RENEW_BEFORE_MS: u64 = 2 * 60 * 1000;
const GITHUB_RENEW_BEFORE_MS: u64 = 5 * 60 * 1000;

pub fn records_prefix(user: &str, thread: &str) -> String {
    format!("{}threads/{thread}/records/", user_prefix(user))
}

/// The skills of a `Definition` as a gzipped tar of `<name>/SKILL.md`.
pub fn skills_archive(definition: &Value) -> Result<Vec<u8>, DbError> {
    let io = |e: std::io::Error| DbError::Invalid(e.to_string());
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    for skill in definition["skills"].as_array().into_iter().flatten() {
        let name = skill["name"].as_str().unwrap_or_default();
        let document = skill["document"].as_str().unwrap_or_default().as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(document.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        tar.append_data(&mut header, format!("{name}/SKILL.md"), document)
            .map_err(io)?;
    }
    tar.into_inner().map_err(io)?.finish().map_err(io)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn credentials(ctx: &Ctx, user: &str, t: &Thread, w: &Workspace) -> Result<Value, DbError> {
    let at = now();
    let claims = token::claims(TokenKind::Thread, user, &t.name, w.seq, at);
    let expires_at = claims.expires_at;
    let thread_token = ctx.tokens.issue(&claims);
    let (github_token, github_expires) =
        ctx.github.token(user).await.map_err(DbError::Unavailable)?;
    let prefix = records_prefix(user, &t.name);
    let upload = ctx.urls.upload(&prefix, expires_at);
    let renew =
        (expires_at - RENEW_BEFORE_MS).min(github_expires.saturating_sub(GITHUB_RENEW_BEFORE_MS));
    ctx.scheduler
        .schedule(
            json!({"type": "thread.credentials", "user": user, "thread": t.name, "workspace": w.seq}),
            renew,
        )
        .await;
    Ok(json!({
        "type": "credentials",
        "threadId": t.name,
        "threadToken": thread_token,
        "threadTokenExpiresAt": rfc3339(expires_at),
        "github": {"token": github_token, "expiresAt": rfc3339(github_expires)},
        "recordUpload": {
            "url": upload.url,
            "fields": upload.fields,
            "keyPrefix": prefix,
            "expiresAt": rfc3339(expires_at),
        },
    }))
}

async fn start(ctx: &Ctx, user: &str, t: &Thread, stub: &Value) -> Result<Value, DbError> {
    let at = now();
    let expires_at = at + token::LIFETIME_MS;
    let definition = launch::read_definition(&ctx.blobs, t).await?;
    let project = &definition["project"];
    let repos: Vec<Value> = project["Repos"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(name, r)| json!({"name": name, "repo": r["Repo"], "branch": t.branch}))
                .collect()
        })
        .unwrap_or_default();
    let scripts: Vec<Value> = project["Scripts"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(name, sc)| {
                    json!({
                        "name": name,
                        "command": sc["Command"],
                        "timeoutSeconds": sc["TimeoutSeconds"],
                        "runOnCreate": sc["RunOnCreate"],
                        "async": sc["Async"],
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let credential = match ctx
        .db
        .entity::<Credential>(&keys::credential(user, &t.credential))
        .await?
    {
        Some(Credential {
            secret: Some(sealed),
            secret_kind: Some(kind),
            ..
        }) => json!({"kind": kind, "value": ctx.secrets.open(user, &sealed)?}),
        _ => json!({"kind": "", "value": ""}),
    };
    let entries: Vec<Value> = definition["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|sk| json!({"name": sk["name"], "predictable": sk["predictable"], "inputs": sk["inputs"]}))
        .collect();
    let skills = if entries.is_empty() {
        json!({"url": null, "sha256": null, "entries": []})
    } else {
        let archive = skills_archive(&definition)?;
        let sha = hex(&Sha256::digest(&archive));
        let key = content_key(user, "skills", &archive);
        ctx.blobs.put(&key, archive).await?;
        json!({"url": ctx.urls.download(&key, expires_at), "sha256": sha, "entries": entries})
    };
    let record = t.record.as_ref().map(|r| {
        json!({
            "url": ctx.urls.download(&r.key, expires_at),
            "sha256": r.sha256,
            "sessionId": r.session,
            "turn": r.turn,
        })
    });
    let heartbeat = match ctx.db.entity::<User>(&keys::user(user)).await? {
        Some(u) => plans::plan(&u.plan)
            .map(|p| p.thread_limits.heartbeat_seconds)
            .unwrap_or(60),
        None => 60,
    };
    Ok(json!({
        "type": "start",
        "threadId": t.name,
        "thread": {"name": t.name, "title": t.title},
        "project": {"name": project["Name"], "repos": repos, "scripts": scripts},
        "worker": {"agent": t.agent, "model": t.model, "credential": credential},
        "gitAuthor": {"name": GIT_AUTHOR.0, "email": GIT_AUTHOR.1},
        "skills": skills,
        "record": record,
        "instruction": stub["instruction"],
        "heartbeatSeconds": heartbeat,
    }))
}

/// A message of the outbox as it goes to the agent; `None` for what is not for the agent.
pub async fn materialize(
    ctx: &Ctx,
    user: &str,
    t: &Thread,
    w: &Workspace,
    seq: u64,
    body: Value,
) -> Result<Option<Value>, DbError> {
    let mut v = match (body["type"].as_str(), body["$stub"] == json!(true)) {
        (Some("skill_notice"), _) => return Ok(None),
        (Some("credentials"), true) => credentials(ctx, user, t, w).await?,
        (Some("start"), true) => start(ctx, user, t, &body).await?,
        _ => resolve_slots(&ctx.blobs, body).await?,
    };
    v["seq"] = json!(seq);
    Ok(Some(v))
}

/// Sends the outbox of a thread after the last message sent, on the connection of its
/// Workspace. The dispatcher is the one sender, so the messages go in order.
pub async fn flush(ctx: &Ctx, user: &str, name: &str) -> Result<(), DbError> {
    let Some(t) = ctx.db.entity::<Thread>(&keys::thread(user, name)).await? else {
        return Ok(());
    };
    if t.workspaces == 0 {
        return Ok(());
    }
    let ws_key = keys::workspace(user, name, t.workspaces);
    let Some(w) = ctx.db.entity::<Workspace>(&ws_key).await? else {
        return Ok(());
    };
    let Some(connection) = w.connection.clone().filter(|_| w.state != "stopped") else {
        return Ok(());
    };
    let mut sent = w.sent;
    for (seq, body) in outbox::messages(&ctx.db, user, name).await? {
        if seq <= sent {
            continue;
        }
        if let Some(message) = materialize(ctx, user, &t, &w, seq, body).await?
            && ctx
                .connections
                .send(&connection, message.to_string())
                .await
                .is_err()
        {
            let mut tx = ctx.db.tx(user);
            tx.update(
                &ws_key,
                Change::new()
                    .remove("connection")
                    .when(Cond::eq("connection", s(&connection))),
            )?;
            return match tx.commit().await {
                Ok(()) | Err(DbError::Condition { .. }) => Ok(()),
                Err(e) => Err(e),
            };
        }
        sent = seq;
    }
    if sent != w.sent {
        let mut tx = ctx.db.tx(user);
        tx.update(
            &ws_key,
            Change::new()
                .set("sent", sanchoris_core::db::n(sent))
                .when(Cond::eq("connection", s(&connection))),
        )?;
        match tx.commit().await {
            Ok(()) | Err(DbError::Condition { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
