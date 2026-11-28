//! Trigger evaluation: duplicates of a delivery, the thread key and its release, the limit of
//! the hour, `InputPathsMap`, and the results it records.

use sanchoris_core::blob::Blobs;
use sanchoris_core::db::{Db, item_of, n};
use sanchoris_core::entities::{
    BlockedBy, PendingReason, ThreadStatus, TriggerEvent, TriggerOutcome, keys,
};
use sanchoris_core::thread::{self, Started, start_pending};
use sanchoris_core::trigger::{
    Dependencies, Evaluated, Event, NoDependencies, Source, evaluate, thread_name,
};
use serde_json::json;

use crate::common::*;

const CONFIG_REPO: u64 = 9001;
const HOUR: u64 = 3_600_000;

fn event(delivery: &str, number: u64) -> Event {
    Event {
        delivery: delivery.to_string(),
        repository_id: 7001,
        event: labeled(number, "mika-dev"),
    }
}

async fn run(db: &Db, u: &str, e: &Event, now: u64) -> Vec<Evaluated> {
    run_with(db, u, e, &NoDependencies, now).await
}

async fn run_with<D: Dependencies>(
    db: &Db,
    u: &str,
    e: &Event,
    deps: &D,
    now: u64,
) -> Vec<Evaluated> {
    let config = config();
    let repositories = repositories();
    let source = Source {
        repository_id: CONFIG_REPO,
        config: &config,
        repositories: &repositories,
        revision: Some("abc123".into()),
        retention_days: 90,
    };
    evaluate(db, u, &source, e, deps, now).await.unwrap()
}

fn started(thread: &str) -> TriggerOutcome {
    TriggerOutcome::Started {
        thread: thread.to_string(),
    }
}

fn name_of(delivery: &str) -> String {
    let config = config();
    let repositories = repositories();
    let source = Source {
        repository_id: CONFIG_REPO,
        config: &config,
        repositories: &repositories,
        revision: None,
        retention_days: 90,
    };
    thread_name(&source, "labeled-issue", delivery)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_starts_one_thread_however_many_times_it_arrives() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let e = event("d-1", 35);
    let runs: Vec<_> = (0..4)
        .map(|_| {
            let db = db.clone();
            let u = u.clone();
            let e = e.clone();
            tokio::spawn(async move { run(&db, &u, &e, NOW).await })
        })
        .collect();
    let mut results = Vec::new();
    for r in runs {
        results.extend(r.await.unwrap());
    }
    let thread = name_of("d-1");
    assert!(
        results.iter().all(|r| r.outcome == started(&thread)),
        "{results:?}"
    );
    assert_eq!(
        results.iter().filter(|r| !r.duplicate).count(),
        1,
        "{results:?}"
    );
    let threads: Vec<sanchoris_core::entities::Thread> = db.entities(&u, "THREAD#").await.unwrap();
    assert_eq!(threads.len(), 1);
    let t = &threads[0];
    assert_eq!(t.name, thread);
    assert_eq!(t.status, ThreadStatus::Pending);
    assert_eq!(t.title, "Issue 35");
    assert_eq!(t.requester, "mika-dev");
    assert_eq!(t.origin, "trigger:labeled-issue");
    assert_eq!(
        t.queued[0]["text"],
        "Resolve issue 35 in acme/shop-web: Issue 35. Write tests for the change, then run open-pr-green-ci."
    );
    let recorded: TriggerEvent = db
        .entity(&keys::trigger_event(
            &u,
            CONFIG_REPO,
            "labeled-issue",
            "d-1",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recorded.thread_key.as_deref(), Some("github:7001#35"));
    assert!(
        db.get(&keys::job(&u, &thread::start_job(&thread)))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn the_thread_key_holds_until_the_thread_is_aborted() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let first = name_of("k-1");
    assert_eq!(
        run(&db, &u, &event("k-1", 35), NOW).await[0].outcome,
        started(&first)
    );
    // Another delivery for the same issue is skipped while the thread lives.
    assert_eq!(
        run(&db, &u, &event("k-2", 35), NOW).await[0].outcome,
        TriggerOutcome::SkippedExisting {
            thread: first.clone()
        }
    );
    // Another issue is another key.
    assert_eq!(
        run(&db, &u, &event("k-3", 36), NOW).await[0].outcome,
        started(&name_of("k-3"))
    );
    thread::stop(&db, &Blobs::memory(), &u, &first, NOW)
        .await
        .unwrap();
    assert!(
        db.get(&keys::thread_key(
            &u,
            CONFIG_REPO,
            "labeled-issue",
            "github:7001#35"
        ))
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        run(&db, &u, &event("k-4", 35), NOW).await[0].outcome,
        started(&name_of("k-4"))
    );
}

#[tokio::test]
async fn a_thread_key_past_its_retention_is_free() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let key = keys::thread_key(&u, CONFIG_REPO, "labeled-issue", "github:7001#40");
    let mut held = item_of(&key, "thread_key", &json!({"thread": "th_old"})).unwrap();
    held.insert("expires_at".into(), n(NOW / 1000 - 1));
    let mut tx = db.tx(&u);
    tx.put(sanchoris_core::db::TableKey::Main, held, None)
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        run(&db, &u, &event("r-1", 40), NOW).await[0].outcome,
        started(&name_of("r-1"))
    );
}

#[tokio::test]
async fn a_trigger_starts_60_threads_an_hour() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let hour = NOW - NOW % HOUR;
    for i in 0..60 {
        let r = run(&db, &u, &event(&format!("h-{i}"), 100 + i), hour + i * 1000).await;
        assert!(
            matches!(r[0].outcome, TriggerOutcome::Started { .. }),
            "{i}: {r:?}"
        );
    }
    let r = run(&db, &u, &event("h-60", 160), hour + HOUR - 1).await;
    assert!(
        matches!(&r[0].outcome, TriggerOutcome::DroppedLimit { reason } if reason.contains("60 threads")),
        "{r:?}"
    );
    // The next hour starts again.
    let r = run(&db, &u, &event("h-61", 161), hour + HOUR).await;
    assert!(
        matches!(r[0].outcome, TriggerOutcome::Started { .. }),
        "{r:?}"
    );
}

#[tokio::test]
async fn an_event_that_does_not_match_is_not_recorded() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let mut other_sender = event("m-1", 35);
    other_sender.event = labeled(35, "someone-else");
    assert!(run(&db, &u, &other_sender, NOW).await.is_empty());
    let mut other_repo = event("m-2", 35);
    other_repo.repository_id = 9999;
    assert!(run(&db, &u, &other_repo, NOW).await.is_empty());
    let mut other_label = event("m-3", 35);
    other_label.event["detail"]["label"] = json!("bug");
    assert!(run(&db, &u, &other_label, NOW).await.is_empty());
    assert!(
        db.entities::<TriggerEvent>(&u, "CONFIG#")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_event_without_the_values_of_input_paths_map_is_dropped_with_its_reason() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let mut e = event("x-1", 35);
    e.event["detail"]["issue"]
        .as_object_mut()
        .unwrap()
        .remove("title");
    let r = run(&db, &u, &e, NOW).await;
    assert!(
        matches!(&r[0].outcome, TriggerOutcome::DroppedEvaluation { reason } if reason.contains("$.detail.issue.title")),
        "{r:?}"
    );
    assert!(
        db.entities::<sanchoris_core::entities::Thread>(&u, "THREAD#")
            .await
            .unwrap()
            .is_empty()
    );
}

struct Blocked(Result<Vec<BlockedBy>, String>);

impl Dependencies for Blocked {
    async fn blocked_by(&self, _: u64, _: u64) -> Result<Vec<BlockedBy>, String> {
        self.0.clone()
    }
}

#[tokio::test]
async fn dependencies_that_cannot_be_read_skip_the_event_and_open_ones_keep_the_thread_pending() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let r = run_with(
        &db,
        &u,
        &event("b-1", 35),
        &Blocked(Err("GitHub answered 502".into())),
        NOW,
    )
    .await;
    assert_eq!(
        r[0].outcome,
        TriggerOutcome::SkippedDependencies {
            reason: "GitHub answered 502".into()
        }
    );
    let open = Blocked(Ok(vec![BlockedBy {
        repository_id: 7001,
        number: 30,
        closed: false,
    }]));
    let r = run_with(&db, &u, &event("b-2", 35), &open, NOW).await;
    let thread = name_of("b-2");
    assert_eq!(r[0].outcome, started(&thread));
    assert_eq!(
        start_pending(&db, &u, &thread, NOW).await.unwrap(),
        Started::Pending(PendingReason::BlockedBy)
    );
}
