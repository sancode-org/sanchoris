//! The store of the engine at the limits of `TransactWriteItems`: the largest transition (256
//! StateEntries) with large values, the line of 32 KiB between DynamoDB and S3, and two writers
//! of the same version.

use sanchoris_core::blob::{Blobs, INLINE_LIMIT};
use sanchoris_core::db::MAX_ACTIONS;
use sanchoris_core::engine_store::{DynamoStore, ITEM_BUDGET, TRANSACTION_BUDGET};
use sanchoris_engine::{Engine, MemoryStore, Origin, StartRequest, Store, StoreError, Transaction};
use serde_json::{Map, Value, json};

use crate::common::*;

const THREAD: &str = "th_limits";

/// A skill of `n` Pass States in a row, each setting `$.r` to `result`.
fn chain(n: usize, result: Value) -> Value {
    let mut states = Map::new();
    for i in 0..n {
        let mut s = json!({"Type": "Pass", "Result": result, "ResultPath": "$.r"});
        if i + 1 == n {
            s["End"] = json!(true);
        } else {
            s["Next"] = json!(format!("S{}", i + 1));
        }
        states.insert(format!("S{i}"), s);
    }
    json!({"StartAt": "S0", "TimeoutSeconds": 60, "States": states})
}

/// The transaction that starts a SkillRun of `source`, made by the engine in memory.
async fn first_transaction(source: Value, input: Value) -> (Transaction, MemoryStore) {
    let definition = sanchoris_engine::definition("limits", &source).unwrap();
    let mut engine = Engine::new(MemoryStore::default());
    engine
        .start(
            StartRequest {
                request: "req-1".into(),
                thread: THREAD.into(),
                origin: Origin::Thread,
                definition,
                source,
                input,
            },
            NOW,
        )
        .await
        .unwrap();
    let tx = engine.store.log[0].clone();
    (tx, engine.store)
}

async fn store(blobs: &Blobs) -> DynamoStore {
    let db = db().await;
    let u = user(&db, "pro").await;
    DynamoStore::new(db, blobs.clone(), &u)
}

/// Reads back what a store holds of SkillRun 1 and compares it with the store in memory.
async fn same_as(store: &mut DynamoStore, memory: &mut MemoryStore) {
    let stored = store.run(1).await.unwrap().unwrap();
    let expected = memory.run(1).await.unwrap().unwrap();
    assert_eq!(stored.run, expected.run);
    assert_eq!(stored.definition, expected.definition);
    assert_eq!(stored.source, expected.source);
    assert_eq!(
        store.entries(1).await.unwrap(),
        memory.entries(1).await.unwrap()
    );
    assert_eq!(
        store.messages(THREAD).await.unwrap(),
        memory.messages(THREAD).await.unwrap()
    );
}

#[tokio::test]
async fn the_largest_transition_with_large_values_is_one_transaction() {
    // 256 StateEntries, each with an Input and an Output of 30 KiB: 15 MB of values.
    let (tx, mut memory) =
        first_transaction(chain(256, json!("x".repeat(30 * 1024))), json!({})).await;
    assert_eq!(tx.entries.len(), 256);
    let blobs = Blobs::memory();
    let mut store = store(&blobs).await;
    let (items, bytes) = store.write(&tx).await.unwrap();
    assert!(items <= MAX_ACTIONS, "{items} items");
    assert!(bytes <= TRANSACTION_BUDGET, "{bytes} bytes");
    assert!(!blobs.keys(&format!("u/{}/runs/1/", store.user)).is_empty());
    same_as(&mut store, &mut memory).await;
    // Every item is within its budget.
    let items = store
        .db
        .query(
            sanchoris_core::db::TableKey::Main,
            &format!("U#{}", store.user),
            "RUN#",
        )
        .await
        .unwrap();
    for item in &items {
        assert!(sanchoris_core::db::item_size(item) <= ITEM_BUDGET);
    }
}

#[tokio::test]
async fn the_largest_transition_with_small_values_stays_in_dynamodb() {
    let (tx, mut memory) = first_transaction(chain(256, json!(1)), json!({})).await;
    let blobs = Blobs::memory();
    let mut store = store(&blobs).await;
    let (items, _) = store.write(&tx).await.unwrap();
    // The SkillRun, its place in the thread and the count of SkillRuns; 17 pages for the
    // StateEntries 1 to 256; the timer of `TimeoutSeconds` with the count of jobs; the answer;
    // the outbox with its count.
    assert_eq!(items, 3 + 17 + 2 + 1 + 2, "{items}");
    assert!(blobs.keys("").is_empty());
    same_as(&mut store, &mut memory).await;
}

#[tokio::test]
async fn a_value_of_32_kib_stays_in_dynamodb_and_a_larger_one_goes_to_s3() {
    // `{"x":"…"}` is 8 bytes more than its string.
    for (n, in_s3) in [(INLINE_LIMIT, false), (INLINE_LIMIT + 1, true)] {
        let input = json!({"x": "y".repeat(n - 8)});
        assert_eq!(input.to_string().len(), n);
        let (tx, mut memory) = first_transaction(chain(1, json!(1)), input.clone()).await;
        let blobs = Blobs::memory();
        let mut store = store(&blobs).await;
        store.write(&tx).await.unwrap();
        let objects = blobs.keys(&format!("u/{}/runs/1/", store.user));
        let has_input = objects.iter().any(|k| {
            let Blobs::Memory(map) = &blobs else {
                unreachable!()
            };
            let bytes = map.lock().unwrap()[k].clone();
            serde_json::from_slice::<Value>(&bytes).unwrap() == input
        });
        assert_eq!(
            has_input,
            in_s3,
            "a value of {} bytes",
            input.to_string().len()
        );
        same_as(&mut store, &mut memory).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_writers_of_the_same_version_one_wins_and_only_its_values_are_read() {
    let source = chain(3, json!("z".repeat(40 * 1024)));
    let (start, mut memory) = first_transaction(source.clone(), json!({})).await;
    let blobs = Blobs::memory();
    let mut store = store(&blobs).await;
    store.write(&start).await.unwrap();
    // Two transitions over version 0, each with its own answer and values.
    let mut a = start.clone();
    a.expected_version = Some(0);
    a.run.version = 1;
    a.request = Some((
        "req-a".into(),
        sanchoris_engine::Response::Ok(json!({"w": "a".repeat(50 * 1024)})),
    ));
    let mut b = a.clone();
    b.request = Some((
        "req-b".into(),
        sanchoris_engine::Response::Ok(json!({"w": "b".repeat(50 * 1024)})),
    ));
    let sa = store.clone();
    let sb = store.clone();
    let (ra, rb) = tokio::join!(
        tokio::spawn(async move { sa.clone().commit(a).await }),
        tokio::spawn(async move { sb.clone().commit(b).await })
    );
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert!(
        matches!(
            (&ra, &rb),
            (Ok(()), Err(StoreError::Conflict(1))) | (Err(StoreError::Conflict(1)), Ok(()))
        ),
        "{ra:?} {rb:?}"
    );
    let (won, lost) = if ra.is_ok() {
        ("req-a", "req-b")
    } else {
        ("req-b", "req-a")
    };
    assert!(store.response(won).await.unwrap().is_some());
    assert!(store.response(lost).await.unwrap().is_none());
    assert_eq!(store.run(1).await.unwrap().unwrap().version, 1);
    // The outbox has the messages of the start and of the winner only.
    assert_eq!(
        store.messages(THREAD).await.unwrap().len(),
        2 * memory.messages(THREAD).await.unwrap().len()
    );
}
