//! The store of the engine on DynamoDB, for one User: each transition is one
//! `TransactWriteItems` over the version of the SkillRun it read.
//!
//! Values (the `Input`, `Output` and `Result` of SkillRuns and StateEntries, the output of
//! commands, the `skill.yaml`) are slots: `{"$v": value}` in the item, or `{"$b": key}` with the
//! value in S3 under `u/<user>/runs/<run>/` named by its SHA-256. A slot over [`INLINE_LIMIT`]
//! goes to S3; then, while an item is over [`ITEM_BUDGET`] or the transaction over
//! [`TRANSACTION_BUDGET`], the largest slot left in it goes to S3. The objects are written
//! before the transaction and are named by their content, so a transaction that fails leaves
//! only objects that nothing names. StateEntries are kept [`PAGE`] to an item.

use std::collections::{BTreeMap, BTreeSet};

use leanfran::{Int, Nat};
use sanchoris_engine::lean::{
    Await, Definition, Entry, EntryStatus, ExecOutput, Parent, Phase, Run, RunStatus, Timer,
};
use sanchoris_engine::{
    Engine, ExecKey, ExecRequest, Job, JobRecord, Message, Origin, Response, RunRecord, Store,
    StoreError, Transaction,
};
use serde_json::{Value, json};

use crate::blob::{Blobs, INLINE_LIMIT, content_key};
use crate::db::{
    Cond, Db, DbError, Item, Key, MAX_ACTIONS, TableKey, UserTx, get_n, get_s, item_of, n, s,
    user_pk,
};
use crate::entities::padded;
use crate::outbox;

/// The StateEntries of one item.
pub const PAGE: u64 = 16;
/// The largest item the store writes, under the 400 KiB of DynamoDB.
pub const ITEM_BUDGET: usize = 380_000;
/// The largest transaction the store writes, under the 4 MB of DynamoDB.
pub const TRANSACTION_BUDGET: usize = 3_900_000;

fn run_key(user: &str, run: u64) -> Key {
    Key::main(user_pk(user), format!("RUN#{}", padded(run)))
}

fn member_key(user: &str, thread: &str, run: u64) -> Key {
    Key::main(
        user_pk(user),
        format!("THREAD#{thread}#RUN#{}", padded(run)),
    )
}

fn page_key(user: &str, run: u64, page: u64) -> Key {
    Key::main(
        user_pk(user),
        format!("RUN#{}#E#{}", padded(run), padded(page)),
    )
}

fn result_key(user: &str, key: ExecKey) -> Key {
    Key::main(
        user_pk(user),
        format!(
            "RUN#{}#EXEC#{}#{}",
            padded(key.run),
            padded(key.seq),
            padded(key.attempt)
        ),
    )
}

fn request_key(user: &str, key: ExecKey) -> Key {
    Key::main(
        user_pk(user),
        format!(
            "RUN#{}#XREQ#{}#{}",
            padded(key.run),
            padded(key.seq),
            padded(key.attempt)
        ),
    )
}

fn engine_job_key(user: &str, id: u64) -> Key {
    Key::main(user_pk(user), format!("EJOB#{}", padded(id)))
}

fn response_key(user: &str, request: &str) -> Key {
    Key::main(user_pk(user), format!("REQ#{request}"))
}

fn await_key(user: &str, thread: &str) -> Key {
    Key::main(user_pk(user), format!("THREAD#{thread}#AWAIT"))
}

fn sequence_key(user: &str, name: &str) -> Key {
    Key::main(user_pk(user), format!("SEQ#{name}"))
}

fn invalid(what: &str) -> DbError {
    DbError::Invalid(format!("a stored {what} is not valid"))
}

// --- The codec: engine values as JSON, with slots for values.

fn slot(value: Value) -> Value {
    json!({ "$v": value })
}

fn unslot(value: &Value) -> Result<Value, DbError> {
    value.get("$v").cloned().ok_or_else(|| invalid("value"))
}

fn nat_v(v: &Nat) -> Value {
    json!(sanchoris_engine::json::u64_of(v))
}

fn nat_of(v: &Value) -> Result<Nat, DbError> {
    v.as_u64().map(Nat::from).ok_or_else(|| invalid("number"))
}

fn u64_of(v: &Value) -> Result<u64, DbError> {
    v.as_u64().ok_or_else(|| invalid("number"))
}

fn str_of(v: &Value) -> Result<String, DbError> {
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| invalid("text"))
}

fn opt<T>(v: &Value, f: impl Fn(&Value) -> Result<T, DbError>) -> Result<Option<T>, DbError> {
    if v.is_null() {
        Ok(None)
    } else {
        f(v).map(Some)
    }
}

fn opt_str(v: &Value) -> Result<Option<String>, DbError> {
    opt(v, str_of)
}

fn lean_slot(j: &sanchoris_engine::lean::Json) -> Value {
    slot(sanchoris_engine::json::from_lean(j))
}

fn lean_of(v: &Value) -> Result<sanchoris_engine::lean::Json, DbError> {
    Ok(sanchoris_engine::json::to_lean(&unslot(v)?))
}

const RUN_STATUSES: [(RunStatus, &str); 5] = [
    (RunStatus::Running, "RUNNING"),
    (RunStatus::Waiting, "WAITING"),
    (RunStatus::Succeeded, "SUCCEEDED"),
    (RunStatus::Failed, "FAILED"),
    (RunStatus::Aborted, "ABORTED"),
];

const ENTRY_STATUSES: [(EntryStatus, &str); 7] = [
    (EntryStatus::Running, "RUNNING"),
    (EntryStatus::Waiting, "WAITING"),
    (EntryStatus::Succeeded, "SUCCEEDED"),
    (EntryStatus::Failed, "FAILED"),
    (EntryStatus::Caught, "CAUGHT"),
    (EntryStatus::TimedOut, "TIMED_OUT"),
    (EntryStatus::Aborted, "ABORTED"),
];

fn name_of<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> &'static str {
    table
        .iter()
        .find(|(t, _)| *t == v)
        .map(|(_, n)| *n)
        .unwrap()
}

fn of_name<T: Copy>(table: &[(T, &'static str)], v: &Value) -> Result<T, DbError> {
    let name = v.as_str().ok_or_else(|| invalid("status"))?;
    table
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(t, _)| *t)
        .ok_or_else(|| invalid("status"))
}

fn exec_output_v(o: &ExecOutput) -> Value {
    json!({
        "exit_code": o.exit_code.to_string(),
        "stdout": slot(json!(o.stdout)),
        "stderr": slot(json!(o.stderr)),
        "timed_out": o.timed_out,
    })
}

fn exec_output_of(v: &Value) -> Result<ExecOutput, DbError> {
    Ok(ExecOutput {
        exit_code: str_of(&v["exit_code"])?
            .parse::<Int>()
            .map_err(|_| invalid("exit code"))?,
        stdout: str_of(&unslot(&v["stdout"])?)?,
        stderr: str_of(&unslot(&v["stderr"])?)?,
        timed_out: v["timed_out"].as_bool().ok_or_else(|| invalid("result"))?,
    })
}

fn entry_v(e: &Entry) -> Value {
    json!({
        "seq": nat_v(&e.seq),
        "state": e.state,
        "visit": nat_v(&e.visit),
        "status": name_of(&ENTRY_STATUSES, e.status),
        "entered_at": nat_v(&e.entered_at),
        "exited_at": e.exited_at.as_ref().map(nat_v),
        "input": lean_slot(&e.input),
        "output": e.output.as_ref().map(lean_slot),
        "error": e.error,
        "cause": e.cause,
        "attempts": nat_v(&e.attempts),
        "retry_counts": e.retry_counts.iter().map(nat_v).collect::<Vec<_>>(),
        "phase": match &e.phase {
            Phase::Executing { deadline } => json!({"executing": nat_v(deadline)}),
            Phase::Backoff { due } => json!({"backoff": nat_v(due)}),
            Phase::Sleeping { due } => json!({"sleeping": nat_v(due)}),
            Phase::Closed => json!("closed"),
        },
        "await": match &e.r#await {
            Await::Idle => json!("idle"),
            Await::Waiting { order } => json!({"waiting": nat_v(order)}),
            Await::Delivered { output } => json!({"delivered": lean_slot(output)}),
        },
        "result": e.result.as_ref().map(exec_output_v),
        "next": e.next,
    })
}

fn entry_of(v: &Value) -> Result<Entry, DbError> {
    let phase = &v["phase"];
    let phase = if phase == "closed" {
        Phase::Closed
    } else if let Some(d) = phase.get("executing") {
        Phase::Executing {
            deadline: nat_of(d)?,
        }
    } else if let Some(d) = phase.get("backoff") {
        Phase::Backoff { due: nat_of(d)? }
    } else if let Some(d) = phase.get("sleeping") {
        Phase::Sleeping { due: nat_of(d)? }
    } else {
        return Err(invalid("phase"));
    };
    let a = &v["await"];
    let r#await = if a == "idle" {
        Await::Idle
    } else if let Some(o) = a.get("waiting") {
        Await::Waiting { order: nat_of(o)? }
    } else if let Some(o) = a.get("delivered") {
        Await::Delivered {
            output: lean_of(o)?,
        }
    } else {
        return Err(invalid("await"));
    };
    Ok(Entry {
        seq: nat_of(&v["seq"])?,
        state: str_of(&v["state"])?,
        visit: nat_of(&v["visit"])?,
        status: of_name(&ENTRY_STATUSES, &v["status"])?,
        entered_at: nat_of(&v["entered_at"])?,
        exited_at: opt(&v["exited_at"], nat_of)?,
        input: lean_of(&v["input"])?,
        output: opt(&v["output"], lean_of)?,
        error: opt_str(&v["error"])?,
        cause: opt_str(&v["cause"])?,
        attempts: nat_of(&v["attempts"])?,
        retry_counts: v["retry_counts"]
            .as_array()
            .ok_or_else(|| invalid("retries"))?
            .iter()
            .map(nat_of)
            .collect::<Result<_, _>>()?,
        phase,
        r#await,
        result: opt(&v["result"], exec_output_of)?,
        next: opt_str(&v["next"])?,
    })
}

fn run_v(r: &Run) -> Value {
    json!({
        "id": nat_v(&r.id),
        "skill": r.skill,
        "status": name_of(&RUN_STATUSES, r.status),
        "input": lean_slot(&r.input),
        "output": r.output.as_ref().map(lean_slot),
        "error": r.error,
        "cause": r.cause,
        "started_at": nat_v(&r.started_at),
        "finished_at": r.finished_at.as_ref().map(nat_v),
        "seq": nat_v(&r.seq),
        "visits": r.visits.iter().map(|(s, n)| json!([s, nat_v(n)])).collect::<Vec<_>>(),
        "current": r.current.as_ref().map(entry_v),
        "pending_continue": r.pending_continue.as_ref().map(lean_slot),
        "hidden": r.hidden,
    })
}

fn run_of(v: &Value) -> Result<Run, DbError> {
    Ok(Run {
        id: nat_of(&v["id"])?,
        skill: str_of(&v["skill"])?,
        status: of_name(&RUN_STATUSES, &v["status"])?,
        input: lean_of(&v["input"])?,
        output: opt(&v["output"], lean_of)?,
        error: opt_str(&v["error"])?,
        cause: opt_str(&v["cause"])?,
        started_at: nat_of(&v["started_at"])?,
        finished_at: opt(&v["finished_at"], nat_of)?,
        seq: nat_of(&v["seq"])?,
        visits: v["visits"]
            .as_array()
            .ok_or_else(|| invalid("visits"))?
            .iter()
            .map(|p| Ok((str_of(&p[0])?, nat_of(&p[1])?)))
            .collect::<Result<_, DbError>>()?,
        current: opt(&v["current"], entry_of)?,
        pending_continue: opt(&v["pending_continue"], lean_of)?,
        hidden: v["hidden"]
            .as_array()
            .ok_or_else(|| invalid("hidden"))?
            .iter()
            .map(str_of)
            .collect::<Result<_, _>>()?,
    })
}

fn parent_v(p: &Parent) -> Value {
    json!({"run": nat_v(&p.run), "state": p.state, "visit": nat_v(&p.visit)})
}

fn parent_of(v: &Value) -> Result<Parent, DbError> {
    Ok(Parent {
        run: nat_of(&v["run"])?,
        state: str_of(&v["state"])?,
        visit: nat_of(&v["visit"])?,
    })
}

fn record_v(r: &RunRecord) -> Value {
    json!({
        "thread": r.thread,
        "origin": match r.origin { Origin::Thread => "thread", Origin::User => "user" },
        "parent": r.parent.as_ref().map(parent_v),
        "source": slot(r.source.clone()),
        "run": run_v(&r.run),
        "version": r.version,
    })
}

fn record_of(v: &Value) -> Result<RunRecord, DbError> {
    let run = run_of(&v["run"])?;
    let source = unslot(&v["source"])?;
    let definition: Definition = sanchoris_engine::definition(&run.skill, &source)
        .map_err(|e| DbError::Invalid(format!("the skill.yaml of a SkillRun: {e}")))?;
    Ok(RunRecord {
        thread: str_of(&v["thread"])?,
        origin: match v["origin"].as_str() {
            Some("user") => Origin::User,
            Some("thread") => Origin::Thread,
            _ => return Err(invalid("origin")),
        },
        parent: opt(&v["parent"], parent_of)?,
        definition,
        source,
        run,
        version: u64_of(&v["version"])?,
    })
}

fn key_v(k: ExecKey) -> Value {
    json!(k.to_string())
}

fn key_of(v: &Value) -> Result<ExecKey, DbError> {
    str_of(v)?.parse().map_err(|_| invalid("idempotency key"))
}

fn request_v(r: &ExecRequest) -> Value {
    json!({
        "key": key_v(r.key),
        "thread": r.thread,
        "skill": r.skill,
        "command": r.command,
        "stdin": slot(json!(r.stdin)),
        "timeout_seconds": r.timeout_seconds,
    })
}

fn request_of(v: &Value) -> Result<ExecRequest, DbError> {
    Ok(ExecRequest {
        key: key_of(&v["key"])?,
        thread: str_of(&v["thread"])?,
        skill: str_of(&v["skill"])?,
        command: str_of(&v["command"])?,
        stdin: str_of(&unslot(&v["stdin"])?)?,
        timeout_seconds: u64_of(&v["timeout_seconds"])?,
    })
}

fn timer_v(t: &Timer) -> Value {
    match t {
        Timer::Run => json!("run"),
        Timer::State { seq, attempt } => {
            json!({"state": {"seq": nat_v(seq), "attempt": nat_v(attempt)}})
        }
        Timer::Retry { seq, attempt } => {
            json!({"retry": {"seq": nat_v(seq), "attempt": nat_v(attempt)}})
        }
        Timer::Wait { seq } => json!({"wait": {"seq": nat_v(seq)}}),
    }
}

fn timer_of(v: &Value) -> Result<Timer, DbError> {
    if v == "run" {
        return Ok(Timer::Run);
    }
    if let Some(t) = v.get("state") {
        return Ok(Timer::State {
            seq: nat_of(&t["seq"])?,
            attempt: nat_of(&t["attempt"])?,
        });
    }
    if let Some(t) = v.get("retry") {
        return Ok(Timer::Retry {
            seq: nat_of(&t["seq"])?,
            attempt: nat_of(&t["attempt"])?,
        });
    }
    if let Some(t) = v.get("wait") {
        return Ok(Timer::Wait {
            seq: nat_of(&t["seq"])?,
        });
    }
    Err(invalid("timer"))
}

fn job_v(j: &Job) -> Value {
    match j {
        Job::Exec(r) => json!({"exec": request_v(r)}),
        Job::Timer { run, timer, due } => {
            json!({"timer": {"run": run, "timer": timer_v(timer), "due": due}})
        }
    }
}

fn job_of(v: &Value) -> Result<Job, DbError> {
    if let Some(r) = v.get("exec") {
        return Ok(Job::Exec(request_of(r)?));
    }
    let t = v.get("timer").ok_or_else(|| invalid("job"))?;
    Ok(Job::Timer {
        run: u64_of(&t["run"])?,
        timer: timer_of(&t["timer"])?,
        due: u64_of(&t["due"])?,
    })
}

fn response_v(r: &Response) -> Value {
    match r {
        Response::Ok(v) => json!({"ok": slot(v.clone())}),
        Response::Error { code, message } => {
            json!({"error": {"code": code, "message": message}})
        }
    }
}

fn response_of(v: &Value) -> Result<Response, DbError> {
    if let Some(o) = v.get("ok") {
        return Ok(Response::Ok(unslot(o)?));
    }
    let e = v.get("error").ok_or_else(|| invalid("response"))?;
    Ok(Response::Error {
        code: str_of(&e["code"])?,
        message: str_of(&e["message"])?,
    })
}

fn message_v(thread: &str, m: &Message) -> Value {
    match m {
        Message::Notice { run, text } => {
            json!({"type": "skill_notice", "threadId": thread, "runId": run, "text": slot(json!(text))})
        }
        Message::ContinueOutput { run, output } => json!({
            "type": "continue_output",
            "threadId": thread,
            "runId": run,
            "output": slot(output.clone()),
        }),
    }
}

fn message_of(v: &Value) -> Result<Option<Message>, DbError> {
    match v["type"].as_str() {
        Some("skill_notice") => Ok(Some(Message::Notice {
            run: u64_of(&v["runId"])?,
            text: str_of(&unslot(&v["text"])?)?,
        })),
        Some("continue_output") => Ok(Some(Message::ContinueOutput {
            run: u64_of(&v["runId"])?,
            output: unslot(&v["output"])?,
        })),
        _ => Ok(None),
    }
}

// --- Slots in S3.

/// Finds the slots of a JSON value, by their paths.
fn slots(v: &Value, path: &mut Vec<PathStep>, out: &mut Vec<Vec<PathStep>>) {
    match v {
        Value::Object(m) if m.len() == 1 && (m.contains_key("$v") || m.contains_key("$b")) => {
            out.push(path.clone());
        }
        Value::Object(m) => {
            for (k, child) in m {
                path.push(PathStep::Field(k.clone()));
                slots(child, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                path.push(PathStep::Index(i));
                slots(child, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathStep {
    Field(String),
    Index(usize),
}

fn at<'a>(v: &'a mut Value, path: &[PathStep]) -> &'a mut Value {
    path.iter().fold(v, |v, step| match step {
        PathStep::Field(k) => &mut v[k.as_str()],
        PathStep::Index(i) => &mut v[*i],
    })
}

/// A value to write in an item: its key, kind, attributes, and the JSON body with slots.
struct Draft {
    key: Key,
    kind: &'static str,
    attributes: Vec<(String, aws_sdk_dynamodb::types::AttributeValue)>,
    body: Value,
    condition: Option<Cond>,
    /// The bodies of an outbox item, one JSON value each.
    bodies: Option<Vec<Value>>,
}

impl Draft {
    fn new(key: Key, kind: &'static str, body: Value) -> Draft {
        Draft {
            key,
            kind,
            attributes: Vec::new(),
            body,
            condition: None,
            bodies: None,
        }
    }

    fn with(mut self, name: &str, value: aws_sdk_dynamodb::types::AttributeValue) -> Draft {
        self.attributes.push((name.to_string(), value));
        self
    }

    fn when(mut self, condition: Cond) -> Draft {
        self.condition = Some(condition);
        self
    }

    fn values(&mut self) -> Vec<&mut Value> {
        match &mut self.bodies {
            Some(bodies) => bodies.iter_mut().collect(),
            None => vec![&mut self.body],
        }
    }

    fn item(&self) -> Result<Item, DbError> {
        let mut item = item_of(&self.key, self.kind, &json!({}))?;
        for (k, v) in &self.attributes {
            item.insert(k.clone(), v.clone());
        }
        match &self.bodies {
            Some(bodies) => {
                item.insert(
                    "bodies".into(),
                    aws_sdk_dynamodb::types::AttributeValue::L(
                        bodies.iter().map(|b| s(b.to_string())).collect(),
                    ),
                );
            }
            None => {
                item.insert("body".into(), s(self.body.to_string()));
            }
        }
        Ok(item)
    }

    fn size(&self) -> Result<usize, DbError> {
        Ok(crate::db::item_size(&self.item()?))
    }
}

/// The store of the engine for the SkillRuns of one User.
#[derive(Debug, Clone)]
pub struct DynamoStore {
    pub db: Db,
    pub blobs: Blobs,
    pub user: String,
}

fn store_error(e: DbError) -> StoreError {
    match e {
        DbError::Condition { .. } | DbError::Conflict => StoreError::Conflict(0),
        _ => StoreError::Unavailable,
    }
}

impl DynamoStore {
    pub fn new(db: Db, blobs: Blobs, user: &str) -> DynamoStore {
        DynamoStore {
            db,
            blobs,
            user: user.to_string(),
        }
    }

    fn blob_path(&self, run: u64) -> String {
        format!("runs/{run}")
    }

    /// Moves slots to S3: every slot over [`INLINE_LIMIT`], then the largest ones while an item
    /// or the whole is over its budget.
    async fn offload(&self, run: u64, drafts: &mut [Draft]) -> Result<(), DbError> {
        let path = self.blob_path(run);
        let mut moves: Vec<(usize, usize, Vec<PathStep>)> = Vec::new();
        for (d, draft) in drafts.iter_mut().enumerate() {
            for (b, value) in draft.values().into_iter().enumerate() {
                let mut found = Vec::new();
                slots(value, &mut Vec::new(), &mut found);
                for p in found {
                    if at(value, &p)
                        .get("$v")
                        .is_some_and(|v| v.to_string().len() > INLINE_LIMIT)
                    {
                        moves.push((d, b, p));
                    }
                }
            }
        }
        for (d, b, p) in moves {
            self.move_slot(&path, &mut drafts[d], b, &p).await?;
        }
        // The serialized size of an item changes by the difference between the texts of a slot
        // and of its pointer, so the sizes are kept up to date without serializing again.
        let mut sizes: Vec<usize> = drafts.iter().map(Draft::size).collect::<Result<_, _>>()?;
        let mut inline: Vec<(usize, usize, usize, Vec<PathStep>)> = Vec::new();
        for (d, draft) in drafts.iter_mut().enumerate() {
            for (b, value) in draft.values().into_iter().enumerate() {
                let mut found = Vec::new();
                slots(value, &mut Vec::new(), &mut found);
                for p in found {
                    let slot = at(value, &p);
                    if slot.get("$v").is_some() {
                        inline.push((slot.to_string().len(), d, b, p));
                    }
                }
            }
        }
        inline.sort_by_key(|a| std::cmp::Reverse(a.0));
        loop {
            let total: usize = sizes.iter().sum();
            let pick = match sizes.iter().position(|s| *s > ITEM_BUDGET) {
                Some(over) => inline.iter().position(|c| c.1 == over),
                None if total > TRANSACTION_BUDGET => (!inline.is_empty()).then_some(0),
                None => return Ok(()),
            };
            let Some(pick) = pick else {
                return Err(DbError::TooLarge(format!(
                    "the transition of SkillRun {run} does not fit in one transaction"
                )));
            };
            let (old, d, b, p) = inline.remove(pick);
            self.move_slot(&path, &mut drafts[d], b, &p).await?;
            let new = at(drafts[d].values()[b], &p).to_string().len();
            sizes[d] = sizes[d] + new - old;
        }
    }

    async fn move_slot(
        &self,
        path: &str,
        draft: &mut Draft,
        body: usize,
        p: &[PathStep],
    ) -> Result<(), DbError> {
        let mut values = draft.values();
        let slot = at(values[body], p);
        let bytes = slot["$v"].to_string().into_bytes();
        let key = content_key(&self.user, path, &bytes);
        self.blobs.put(&key, bytes).await?;
        *slot = json!({ "$b": key });
        Ok(())
    }

    /// Reads the slots of a body that are in S3.
    async fn fetch(&self, mut v: Value) -> Result<Value, DbError> {
        let mut found = Vec::new();
        slots(&v, &mut Vec::new(), &mut found);
        for p in found {
            let slot = at(&mut v, &p);
            if let Some(key) = slot.get("$b").and_then(Value::as_str).map(str::to_string) {
                let bytes = self
                    .blobs
                    .get(&key)
                    .await?
                    .ok_or_else(|| DbError::Invalid(format!("no object {key}")))?;
                let value: Value =
                    serde_json::from_slice(&bytes).map_err(|e| DbError::Invalid(e.to_string()))?;
                *slot = json!({ "$v": value });
            }
        }
        Ok(v)
    }

    async fn body(&self, item: &Item) -> Result<Value, DbError> {
        let text = get_s(item, "body").ok_or_else(|| invalid("item"))?;
        let v: Value = serde_json::from_str(text).map_err(|e| DbError::Invalid(e.to_string()))?;
        self.fetch(v).await
    }

    async fn read_record(&self, id: u64) -> Result<Option<RunRecord>, DbError> {
        match self.db.get(&run_key(&self.user, id)).await? {
            Some(item) => Ok(Some(record_of(&self.body(&item).await?)?)),
            None => Ok(None),
        }
    }

    async fn pages(&self, run: u64) -> Result<BTreeMap<u64, Entry>, DbError> {
        let items = self
            .db
            .query(
                TableKey::Main,
                &user_pk(&self.user),
                &format!("RUN#{}#E#", padded(run)),
            )
            .await?;
        self.entries_of(items).await
    }

    async fn read_pages(&self, keys: &[Key]) -> Result<BTreeMap<u64, Entry>, DbError> {
        let items = self.db.get_many(keys).await?;
        self.entries_of(items).await
    }

    async fn entries_of(&self, items: Vec<Item>) -> Result<BTreeMap<u64, Entry>, DbError> {
        let mut entries = BTreeMap::new();
        for item in items {
            let body = self.body(&item).await?;
            for e in body.as_array().ok_or_else(|| invalid("page"))? {
                let e = entry_of(e)?;
                entries.insert(sanchoris_engine::json::u64_of(&e.seq), e);
            }
        }
        Ok(entries)
    }

    async fn sequence(&self, name: &str) -> Result<u64, DbError> {
        Ok(self
            .db
            .get(&sequence_key(&self.user, name))
            .await?
            .and_then(|i| get_n(&i, "last"))
            .unwrap_or(0) as u64)
    }

    fn sequence_draft(&self, name: &str, last: u64, next: u64) -> Draft {
        let condition = if last == 0 {
            Cond::not_exists()
        } else {
            Cond::eq("last", n(last))
        };
        Draft::new(sequence_key(&self.user, name), "sequence", json!(null))
            .with("last", n(next))
            .when(condition)
    }

    /// The items of a transition.
    async fn drafts(&self, tx: &Transaction) -> Result<Vec<Draft>, DbError> {
        let user = &self.user;
        let id = tx.run.id();
        let thread = &tx.run.thread;
        let mut drafts = Vec::new();
        let run_draft = Draft::new(run_key(user, id), "skill_run", record_v(&tx.run))
            .with("thread", s(thread.clone()))
            .with("status", s(name_of(&RUN_STATUSES, tx.run.run.status)))
            .with("version", n(tx.run.version));
        match tx.expected_version {
            Some(v) => drafts.push(run_draft.when(Cond::eq("version", n(v)))),
            None => {
                drafts.push(run_draft.when(Cond::not_exists()));
                drafts.push(
                    Draft::new(member_key(user, thread, id), "thread_run", json!(null))
                        .when(Cond::not_exists()),
                );
                drafts.push(self.sequence_draft("RUN", id - 1, id));
            }
        }
        let mut pages: BTreeMap<u64, Vec<&Entry>> = BTreeMap::new();
        for e in &tx.entries {
            pages
                .entry(sanchoris_engine::json::u64_of(&e.seq) / PAGE)
                .or_default()
                .push(e);
        }
        if !pages.is_empty() {
            let mut current = if tx.expected_version.is_some() {
                let keys: Vec<Key> = pages.keys().map(|p| page_key(user, id, *p)).collect();
                self.read_pages(&keys).await?
            } else {
                BTreeMap::new()
            };
            for e in &tx.entries {
                current.insert(sanchoris_engine::json::u64_of(&e.seq), e.clone());
            }
            for page in pages.keys() {
                let entries: Vec<Value> = current
                    .range(page * PAGE..(page + 1) * PAGE)
                    .map(|(_, e)| entry_v(e))
                    .collect();
                drafts.push(Draft::new(
                    page_key(user, id, *page),
                    "state_entries",
                    json!(entries),
                ));
            }
        }
        if !tx.jobs.is_empty() {
            let last = self.sequence("EJOB").await?;
            drafts.push(self.sequence_draft("EJOB", last, last + tx.jobs.len() as u64));
            for (i, job) in tx.jobs.iter().enumerate() {
                let job_id = last + 1 + i as u64;
                if let Job::Exec(r) = job {
                    drafts.push(Draft::new(
                        request_key(user, r.key),
                        "exec_request",
                        request_v(r),
                    ));
                }
                drafts.push(
                    Draft::new(engine_job_key(user, job_id), "engine_job", job_v(job))
                        .with("thread", s(thread.clone()))
                        .with("due", n(job.due())),
                );
            }
        }
        if !tx.messages.is_empty() {
            let last = outbox::last_seq(&self.db, user, thread).await?;
            let count = tx.messages.len() as u64;
            let condition = if last == 0 {
                Cond::not_exists()
            } else {
                Cond::eq("last", n(last))
            };
            drafts.push(
                Draft::new(
                    crate::entities::keys::outbox_seq(user, thread),
                    "outbox_seq",
                    json!(null),
                )
                .with("last", n(last + count))
                .when(condition),
            );
            let mut d = Draft::new(
                crate::entities::keys::outbox(user, thread, last + 1),
                "outbox",
                json!(null),
            )
            .with("thread", s(thread.clone()))
            .with("first_seq", n(last + 1))
            .when(Cond::not_exists());
            d.bodies = Some(tx.messages.iter().map(|m| message_v(thread, m)).collect());
            drafts.push(d);
        }
        if let Some((request, response)) = &tx.request {
            drafts.push(Draft::new(
                response_key(user, request),
                "engine_response",
                response_v(response),
            ));
        }
        if let Some(order) = tx.await_order {
            drafts.push(
                Draft::new(await_key(user, thread), "await_order", json!(null))
                    .with("order", n(order)),
            );
        }
        if drafts.len() > MAX_ACTIONS {
            return Err(DbError::TooLarge(format!(
                "the transition of SkillRun {id} writes {} items",
                drafts.len()
            )));
        }
        Ok(drafts)
    }

    /// Writes a transition as one transaction, and returns how many items it wrote and how
    /// many bytes.
    pub async fn write(&self, tx: &Transaction) -> Result<(usize, usize), DbError> {
        let mut drafts = self.drafts(tx).await?;
        self.offload(tx.run.id(), &mut drafts).await?;
        let mut write: UserTx = self.db.tx(&self.user);
        for d in drafts {
            let item = d.item()?;
            write.put(d.key.table, item, d.condition)?;
        }
        let written = (write.len(), write.bytes());
        write.commit().await?;
        Ok(written)
    }

    async fn job_items(&self) -> Result<Vec<Item>, DbError> {
        self.db
            .query(TableKey::Main, &user_pk(&self.user), "EJOB#")
            .await
    }
}

fn exec_result_draft(user: &str, key: ExecKey, output: &ExecOutput) -> Draft {
    Draft::new(result_key(user, key), "exec_result", exec_output_v(output)).when(Cond::not_exists())
}

impl Store for DynamoStore {
    /// A failed condition of the SkillRun is a conflict. A failed condition of a count shared
    /// with other writes (the jobs of the User, the outbox of the thread) reads the count again
    /// and writes again.
    async fn commit(&mut self, tx: Transaction) -> Result<(), StoreError> {
        let id = tx.run.id();
        for _ in 0..20 {
            match self.write(&tx).await {
                Ok(_) => return Ok(()),
                Err(DbError::Condition { index: 0 }) => return Err(StoreError::Conflict(id)),
                Err(DbError::Condition { .. } | DbError::Conflict) => continue,
                Err(_) => return Err(StoreError::Unavailable),
            }
        }
        Err(StoreError::Conflict(id))
    }

    async fn run(&mut self, id: u64) -> Result<Option<RunRecord>, StoreError> {
        self.read_record(id).await.map_err(store_error)
    }

    async fn runs_of_thread(&mut self, thread: &str) -> Result<Vec<RunRecord>, StoreError> {
        let members = self
            .db
            .query(
                TableKey::Main,
                &user_pk(&self.user),
                &format!("THREAD#{thread}#RUN#"),
            )
            .await
            .map_err(store_error)?;
        let keys: Vec<Key> = members
            .iter()
            .filter_map(|m| get_s(m, "sk"))
            .filter_map(|sk| sk.rsplit('#').next())
            .filter_map(|id| id.parse::<u64>().ok())
            .map(|id| run_key(&self.user, id))
            .collect();
        let mut records = Vec::new();
        for item in self.db.get_many(&keys).await.map_err(store_error)? {
            let body = self.body(&item).await.map_err(store_error)?;
            records.push(record_of(&body).map_err(store_error)?);
        }
        Ok(records)
    }

    async fn entries(&mut self, run: u64) -> Result<Vec<Entry>, StoreError> {
        Ok(self
            .pages(run)
            .await
            .map_err(store_error)?
            .into_values()
            .collect())
    }

    async fn put_exec_result(
        &mut self,
        key: ExecKey,
        output: ExecOutput,
    ) -> Result<ExecOutput, StoreError> {
        let mut drafts = vec![exec_result_draft(&self.user, key, &output)];
        self.offload(key.run, &mut drafts)
            .await
            .map_err(store_error)?;
        let mut write = self.db.tx(&self.user);
        let d = drafts.pop().expect("a draft");
        write
            .put(d.key.table, d.item().map_err(store_error)?, d.condition)
            .map_err(store_error)?;
        match write.commit().await {
            Ok(()) => Ok(output),
            Err(DbError::Condition { .. }) => {
                self.exec_result(key).await?.ok_or(StoreError::Unavailable)
            }
            Err(_) => Err(StoreError::Unavailable),
        }
    }

    async fn exec_result(&mut self, key: ExecKey) -> Result<Option<ExecOutput>, StoreError> {
        match self
            .db
            .get(&result_key(&self.user, key))
            .await
            .map_err(store_error)?
        {
            Some(item) => {
                let body = self.body(&item).await.map_err(store_error)?;
                Ok(Some(exec_output_of(&body).map_err(store_error)?))
            }
            None => Ok(None),
        }
    }

    async fn exec_request(&mut self, key: ExecKey) -> Result<Option<ExecRequest>, StoreError> {
        match self
            .db
            .get(&request_key(&self.user, key))
            .await
            .map_err(store_error)?
        {
            Some(item) => {
                let body = self.body(&item).await.map_err(store_error)?;
                Ok(Some(request_of(&body).map_err(store_error)?))
            }
            None => Ok(None),
        }
    }

    async fn jobs(&mut self) -> Result<Vec<JobRecord>, StoreError> {
        let mut jobs = Vec::new();
        for item in self.job_items().await.map_err(store_error)? {
            let id = get_s(&item, "sk")
                .and_then(|sk| sk.strip_prefix("EJOB#"))
                .and_then(|id| id.parse::<u64>().ok())
                .ok_or(StoreError::Unavailable)?;
            let body = self.body(&item).await.map_err(store_error)?;
            jobs.push(JobRecord {
                id,
                job: job_of(&body).map_err(store_error)?,
            });
        }
        Ok(jobs)
    }

    async fn ack(&mut self, job: u64) -> Result<(), StoreError> {
        let mut write = self.db.tx(&self.user);
        write
            .delete(&engine_job_key(&self.user, job), None)
            .map_err(store_error)?;
        write.commit().await.map_err(store_error)
    }

    async fn response(&mut self, request: &str) -> Result<Option<Response>, StoreError> {
        match self
            .db
            .get(&response_key(&self.user, request))
            .await
            .map_err(store_error)?
        {
            Some(item) => {
                let body = self.body(&item).await.map_err(store_error)?;
                Ok(Some(response_of(&body).map_err(store_error)?))
            }
            None => Ok(None),
        }
    }

    async fn await_order(&mut self, thread: &str) -> Result<u64, StoreError> {
        Ok(self
            .db
            .get(&await_key(&self.user, thread))
            .await
            .map_err(store_error)?
            .and_then(|i| get_n(&i, "order"))
            .unwrap_or(0) as u64)
    }

    async fn last_run_id(&mut self) -> Result<u64, StoreError> {
        self.sequence("RUN").await.map_err(store_error)
    }

    async fn messages(&mut self, thread: &str) -> Result<Vec<Message>, StoreError> {
        let mut out = Vec::new();
        for (_, body) in outbox::messages(&self.db, &self.user, thread)
            .await
            .map_err(store_error)?
        {
            let body = self.fetch(body).await.map_err(store_error)?;
            if let Some(m) = message_of(&body).map_err(store_error)? {
                out.push(m);
            }
        }
        Ok(out)
    }
}

/// The SkillRuns of a thread that run (`RUNNING`, `WAITING`).
pub async fn running_runs(db: &Db, user: &str, thread: &str) -> Result<u64, DbError> {
    let members = db
        .query(
            TableKey::Main,
            &user_pk(user),
            &format!("THREAD#{thread}#RUN#"),
        )
        .await?;
    let keys: Vec<Key> = members
        .iter()
        .filter_map(|m| get_s(m, "sk"))
        .filter_map(|sk| sk.rsplit('#').next())
        .filter_map(|id| id.parse::<u64>().ok())
        .map(|id| run_key(user, id))
        .collect();
    Ok(db
        .get_many(&keys)
        .await?
        .iter()
        .filter(|item| matches!(get_s(item, "status"), Some("RUNNING" | "WAITING")))
        .count() as u64)
}

/// Aborts the SkillRuns of a thread that was stopped.
pub async fn abort_runs(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    thread: &str,
    now: u64,
) -> Result<(), DbError> {
    let mut engine = Engine::new(DynamoStore::new(db.clone(), blobs.clone(), user));
    let runs = engine
        .store
        .runs_of_thread(thread)
        .await
        .map_err(|_| DbError::Unavailable("the SkillRuns of a thread".into()))?;
    let running: BTreeSet<u64> = runs
        .iter()
        .filter(|r| matches!(r.run.status, RunStatus::Running | RunStatus::Waiting))
        .map(RunRecord::id)
        .collect();
    for id in running {
        engine
            .abort(id, now)
            .await
            .map_err(|e| DbError::Unavailable(e.to_string()))?;
    }
    Ok(())
}

/// A message of the outbox as it goes on the Thread channel: each slot replaced by its value,
/// read from S3 when it is there.
pub async fn resolve_slots(blobs: &Blobs, mut v: Value) -> Result<Value, DbError> {
    let mut found = Vec::new();
    slots(&v, &mut Vec::new(), &mut found);
    for p in found {
        let slot = at(&mut v, &p);
        let value = if let Some(key) = slot.get("$b").and_then(Value::as_str) {
            let bytes = blobs
                .get(key)
                .await?
                .ok_or_else(|| DbError::Invalid(format!("no object {key}")))?;
            serde_json::from_slice(&bytes).map_err(|e| DbError::Invalid(e.to_string()))?
        } else {
            slot["$v"].take()
        };
        *slot = value;
    }
    Ok(v)
}
