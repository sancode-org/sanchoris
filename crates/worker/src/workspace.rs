//! The Workspace of a thread as the engine sees it: a command goes to the outbox of the thread
//! as `exec`, and a thread without a Workspace gets one restored first (`README.md`「Workspace」,
//! 戻すとき).

use sanchoris_core::db::DbError;
use sanchoris_core::entities::{Thread, keys};
use sanchoris_core::{outbox, thread};
use sanchoris_engine::ExecRequest;
use serde_json::{Value, json};

use crate::ports::Ctx;

pub struct OutboxWorkspace<'a> {
    ctx: &'a Ctx,
    user: &'a str,
}

impl<'a> OutboxWorkspace<'a> {
    pub fn new(ctx: &'a Ctx, user: &'a str) -> Self {
        OutboxWorkspace { ctx, user }
    }
}

pub fn exec_message(r: &ExecRequest) -> Value {
    json!({
        "type": "exec",
        "threadId": r.thread,
        "idempotencyKey": r.key.to_string(),
        "skill": {"name": r.skill, "runId": r.key.run},
        "run": r.command,
        "stdin": r.stdin,
        "timeoutSeconds": r.timeout_seconds,
    })
}

async fn send_exec(ctx: &Ctx, user: &str, r: &ExecRequest) -> Result<(), DbError> {
    for _ in 0..20 {
        let Some(t) = ctx
            .db
            .entity::<Thread>(&keys::thread(user, &r.thread))
            .await?
        else {
            return Ok(());
        };
        let last = outbox::last_seq(&ctx.db, user, &r.thread).await?;
        let mut tx = ctx.db.tx(user);
        outbox::append(&mut tx, &r.thread, last, &[exec_message(r)])?;
        if !thread::has_workspace(&ctx.db, user, &t).await?
            && t.status != sanchoris_core::entities::ThreadStatus::Aborted
        {
            thread::insert_job(
                &mut tx,
                &thread::restore_job(&r.thread),
                json!({"type": "thread.restore", "thread": r.thread}),
            )?;
        }
        match tx.commit().await {
            Ok(()) => return Ok(()),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(DbError::Conflict)
}

impl sanchoris_engine::Workspace for OutboxWorkspace<'_> {
    async fn exec(&mut self, request: &ExecRequest) {
        if let Err(e) = send_exec(self.ctx, self.user, request).await {
            eprintln!(
                "sanchoris: exec {} of thread {}: {e}",
                request.key, request.thread
            );
        }
    }
}
