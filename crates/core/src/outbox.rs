//! The outbox of the Thread channel of a thread: the messages to `sanchoris-agent` in the order
//! they were written, with their `seq`. One write puts its messages in one item; the dispatcher
//! sends them from the stream of the table. A `skill_notice` is a notification of a SkillRun,
//! which the thread-supervisor delivers as an input of the thread (`skill.md`「スレッドへの入力」).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{Cond, Db, DbError, TableKey, UserTx, get_n, item_of, n, s, value_of};
use crate::entities::keys;

/// The messages of one write, numbered from `first_seq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxItem {
    pub thread: String,
    pub first_seq: u64,
    /// The messages, each a JSON text (`apps/sanchoris-agent/src/protocol.ts`, or
    /// `skill_notice`). Values in them may be in S3 (`crate::engine_store`).
    pub bodies: Vec<String>,
}

/// The last `seq` of the outbox of a thread, 0 before the first.
pub async fn last_seq(db: &Db, user: &str, thread: &str) -> Result<u64, DbError> {
    Ok(db
        .get(&keys::outbox_seq(user, thread))
        .await?
        .and_then(|item| get_n(&item, "last"))
        .unwrap_or(0) as u64)
}

/// Adds messages after `last` (read with [`last_seq`]) to the transaction, as one item; the
/// transaction fails when another wrote the outbox since.
pub fn append(tx: &mut UserTx, thread: &str, last: u64, bodies: &[Value]) -> Result<(), DbError> {
    append_texts(
        tx,
        thread,
        last,
        bodies.iter().map(Value::to_string).collect(),
    )
}

pub fn append_texts(
    tx: &mut UserTx,
    thread: &str,
    last: u64,
    bodies: Vec<String>,
) -> Result<(), DbError> {
    if bodies.is_empty() {
        return Ok(());
    }
    let user = tx.user().to_string();
    let counter = keys::outbox_seq(&user, thread);
    let mut item = item_of(&counter, "outbox_seq", &serde_json::json!({}))?;
    item.insert("last".into(), n(last + bodies.len() as u64));
    let condition = if last == 0 {
        Cond::not_exists()
    } else {
        Cond::eq("last", n(last))
    };
    tx.put(TableKey::Main, item, Some(condition))?;
    let first_seq = last + 1;
    let mut item = item_of(
        &keys::outbox(&user, thread, first_seq),
        "outbox",
        &serde_json::json!({"thread": thread, "first_seq": first_seq}),
    )?;
    item.insert(
        "bodies".into(),
        aws_sdk_dynamodb::types::AttributeValue::L(bodies.into_iter().map(s).collect()),
    );
    tx.insert(item)
}

/// The items of the outbox of a thread, in order.
pub async fn items(db: &Db, user: &str, thread: &str) -> Result<Vec<OutboxItem>, DbError> {
    let pk = crate::db::user_pk(user);
    db.query(TableKey::Main, &pk, &format!("THREAD#{thread}#OUT#"))
        .await?
        .iter()
        .map(value_of)
        .collect()
}

/// The messages of the outbox of a thread, in order, with their `seq`.
pub async fn messages(db: &Db, user: &str, thread: &str) -> Result<Vec<(u64, Value)>, DbError> {
    let mut out = Vec::new();
    for item in items(db, user, thread).await? {
        for (i, body) in item.bodies.iter().enumerate() {
            let value: Value =
                serde_json::from_str(body).map_err(|e| DbError::Invalid(e.to_string()))?;
            out.push((item.first_seq + i as u64, value));
        }
    }
    Ok(out)
}
