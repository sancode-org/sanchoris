//! The unit of work of a User: the keys it takes, its conditions and its counters.

use sanchoris_core::db::{Change, Cond, DbError, Key, item_of};
use serde_json::json;

use crate::common::*;

#[tokio::test]
async fn a_user_tx_rejects_the_keys_of_other_users() {
    let db = db().await;
    let alice = user(&db, "pro").await;
    let bob = user(&db, "pro").await;
    for pk in [
        format!("U#{bob}"),
        format!("U#{alice}x"),
        "TRIAL#card".to_string(),
        format!("X#U#{alice}"),
    ] {
        let mut tx = db.tx(&alice);
        let item = item_of(&Key::main(pk.clone(), "MARK#a"), "mark", &json!({})).unwrap();
        let r = tx.insert(item);
        assert!(matches!(r, Err(DbError::ForeignKey { .. })), "{pk}: {r:?}");
        let r = tx.update(&Key::main(pk.clone(), "USER"), Change::new().add("n", 1));
        assert!(matches!(r, Err(DbError::ForeignKey { .. })), "{pk}: {r:?}");
    }
    let mut tx = db.tx(&alice);
    let own = item_of(
        &Key::main(format!("U#{alice}#RECEIVE"), "RATE#1"),
        "rate",
        &json!({}),
    )
    .unwrap();
    tx.insert(own).unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_user_tx_writes_an_item_once() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let mut tx = db.tx(&u);
    let key = Key::main(format!("U#{u}"), "MARK#x");
    tx.insert(item_of(&key, "mark", &json!({})).unwrap())
        .unwrap();
    let r = tx.update(&key, Change::new().add("n", 1));
    assert!(matches!(r, Err(DbError::SameItem(_))), "{r:?}");
}

#[tokio::test]
async fn a_mark_is_written_once() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let key = sanchoris_core::entities::keys::mark(&u, "github", "delivery-1");
    let mut first = db.tx(&u);
    first
        .insert(item_of(&key, "mark", &json!({})).unwrap())
        .unwrap();
    first.commit().await.unwrap();
    let mut second = db.tx(&u);
    second
        .insert(item_of(&key, "mark", &json!({})).unwrap())
        .unwrap();
    assert_eq!(second.commit().await, Err(DbError::Condition { index: 0 }));
}

#[tokio::test]
async fn a_counter_under_a_condition_stops_at_its_limit_however_many_write_at_once() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let key = Key::main(format!("U#{u}"), "COUNTER#c");
    let writes = (0..20).map(|_| {
        let db = db.clone();
        let key = key.clone();
        let u = u.clone();
        tokio::spawn(async move {
            loop {
                let mut tx = db.tx(&u);
                tx.update(
                    &key,
                    Change::new().add("n", 1).when(Cond::missing_or_lt("n", 5)),
                )
                .unwrap();
                match tx.commit().await {
                    Ok(()) => return true,
                    Err(DbError::Condition { .. }) => return false,
                    Err(DbError::Conflict) => continue,
                    Err(e) => panic!("{e}"),
                }
            }
        })
    });
    let mut done = 0;
    for w in writes {
        if w.await.unwrap() {
            done += 1;
        }
    }
    assert_eq!(done, 5);
    let item = db.get(&key).await.unwrap().unwrap();
    assert_eq!(sanchoris_core::db::get_n(&item, "n"), Some(5));
}
