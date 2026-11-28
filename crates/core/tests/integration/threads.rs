//! The thread-starter, the events and inputs of threads, and their stop.

use sanchoris_core::blob::Blobs;
use sanchoris_core::entities::{
    BlockedBy, Host, Lineage, PendingReason, Subscription, ThreadStatus, WaitReason, Workspace,
    keys,
};
use sanchoris_core::generated::sanchoris_thread::{Event, Source};
use sanchoris_core::outbox;
use sanchoris_core::thread::{self, Delivered, Started, ThreadError, start_pending};
use serde_json::json;

use crate::common::*;

async fn start_all(db: &sanchoris_core::db::Db, u: &str, names: &[String]) -> Vec<Started> {
    let runs: Vec<_> = names
        .iter()
        .map(|name| {
            let db = db.clone();
            let u = u.to_string();
            let name = name.clone();
            tokio::spawn(async move { start_pending(&db, &u, &name, NOW).await.unwrap() })
        })
        .collect();
    let mut out = Vec::new();
    for r in runs {
        out.push(r.await.unwrap());
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn threads_started_at_once_stay_within_the_concurrency_of_the_plan() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let names: Vec<String> = (0..6).map(|i| format!("th_c{i}")).collect();
    for name in &names {
        create(&db, &u, &new_thread(name)).await;
    }
    let started = start_all(&db, &u, &names).await;
    let running = started
        .iter()
        .filter(|s| matches!(s, Started::Running { .. }))
        .count();
    let waiting = started
        .iter()
        .filter(|s| **s == Started::Pending(PendingReason::Concurrency))
        .count();
    assert_eq!((running, waiting), (2, 4), "{started:?}");
    let after = read_user(&db, &u).await;
    assert_eq!(after.running_workspaces, 2);
    assert_eq!(after.period_threads, 2);

    // Stopping a running thread gives its slot to a waiting one, once.
    let first = names
        .iter()
        .zip(&started)
        .find(|(_, s)| matches!(s, Started::Running { .. }))
        .unwrap()
        .0
        .clone();
    thread::stop(&db, &Blobs::memory(), &u, &first, NOW)
        .await
        .unwrap();
    thread::stop(&db, &Blobs::memory(), &u, &first, NOW)
        .await
        .unwrap();
    assert_eq!(read_user(&db, &u).await.running_workspaces, 1);
    let waiting: Vec<String> = names
        .iter()
        .zip(&started)
        .filter(|(_, s)| !matches!(s, Started::Running { .. }))
        .map(|(n, _)| n.clone())
        .collect();
    let again = start_all(&db, &u, &waiting).await;
    assert_eq!(
        again
            .iter()
            .filter(|s| matches!(s, Started::Running { .. }))
            .count(),
        1,
        "{again:?}"
    );
    assert_eq!(read_user(&db, &u).await.running_workspaces, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_job_delivered_twice_starts_the_thread_once() {
    let db = db().await;
    let u = user(&db, "pro").await;
    create(&db, &u, &new_thread("th_twice")).await;
    let both = start_all(&db, &u, &["th_twice".into(), "th_twice".into()]).await;
    assert_eq!(
        both.iter()
            .filter(|s| matches!(s, Started::Running { .. }))
            .count(),
        1,
        "{both:?}"
    );
    assert!(both.contains(&Started::Unchanged(ThreadStatus::Running)));
    // A third delivery after the job was done changes nothing either.
    assert_eq!(
        start_pending(&db, &u, "th_twice", NOW).await.unwrap(),
        Started::Unchanged(ThreadStatus::Running)
    );
    let after = read_user(&db, &u).await;
    assert_eq!((after.running_workspaces, after.period_threads), (1, 1));
    let t = read_thread(&db, &u, "th_twice").await;
    assert_eq!(t.workspaces, 1);
    let workspaces: Vec<Workspace> = db.entities(&u, "THREAD#th_twice#WS#").await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert!(
        db.get(&keys::job(&u, &thread::start_job("th_twice")))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_new_thread_at_the_plan_limit_waits_and_a_counted_one_starts() {
    let db = db().await;
    let u = user_with(&db, "pro", Subscription::Active, |u| u.period_threads = 10).await;
    create(&db, &u, &new_thread("th_new")).await;
    assert_eq!(
        start_pending(&db, &u, "th_new", NOW).await.unwrap(),
        Started::Pending(PendingReason::PlanLimit)
    );
    assert_eq!(read_user(&db, &u).await.period_threads, 10);
    // A thread counted in the period restores its Workspace past the limit.
    create(&db, &u, &new_thread("th_counted")).await;
    set(&db, &u, "th_counted", |t| t.counted = true).await;
    assert!(matches!(
        start_pending(&db, &u, "th_counted", NOW).await.unwrap(),
        Started::Running { .. }
    ));
    assert_eq!(read_user(&db, &u).await.period_threads, 10);
}

#[tokio::test]
async fn a_user_without_a_subscription_waits_unless_it_is_admin() {
    let db = db().await;
    let u = user_with(&db, "pro", Subscription::Canceled, |_| {}).await;
    create(&db, &u, &new_thread("th_s")).await;
    assert_eq!(
        start_pending(&db, &u, "th_s", NOW).await.unwrap(),
        Started::Pending(PendingReason::Subscription)
    );
    let admin = user_with(&db, "admin", Subscription::None, |u| {
        u.period_threads = 1000
    })
    .await;
    create(&db, &admin, &new_thread("th_a")).await;
    assert!(matches!(
        start_pending(&db, &admin, "th_a", NOW).await.unwrap(),
        Started::Running { .. }
    ));
}

#[tokio::test]
async fn an_open_blocking_issue_keeps_the_thread_pending() {
    let db = db().await;
    let u = user(&db, "pro").await;
    let mut t = new_thread("th_b");
    t.blocked_by = vec![BlockedBy {
        repository_id: 7001,
        number: 34,
        closed: false,
    }];
    create(&db, &u, &t).await;
    assert_eq!(
        start_pending(&db, &u, "th_b", NOW).await.unwrap(),
        Started::Pending(PendingReason::BlockedBy)
    );
    set(&db, &u, "th_b", |t| t.blocked_by[0].closed = true).await;
    assert!(matches!(
        start_pending(&db, &u, "th_b", NOW).await.unwrap(),
        Started::Running { .. }
    ));
}

async fn host(db: &sanchoris_core::db::Db, u: &str, name: &str, state: &str, max: u64) {
    let mut tx = db.tx(u);
    tx.insert_entity(&Host {
        name: name.into(),
        state: state.into(),
        architecture: "x86_64".into(),
        driver_version: "0.1.0".into(),
        max_workspaces: max,
        workspace_vcpu: 2,
        workspace_memory_mib: 4096,
        running_workspaces: 0,
        last_heartbeat_at: NOW,
        registered_at: NOW,
    })
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_takes_workspaces_up_to_its_max_and_none_while_offline() {
    let db = db().await;
    let u = user(&db, "pro").await;
    host(&db, &u, "box", "online", 2).await;
    host(&db, &u, "off", "offline", 2).await;
    let names: Vec<String> = (0..4).map(|i| format!("th_h{i}")).collect();
    for name in &names {
        let mut t = new_thread(name);
        t.workspace_host = "box".into();
        create(&db, &u, &t).await;
    }
    let started = start_all(&db, &u, &names).await;
    assert_eq!(
        started
            .iter()
            .filter(|s| **s
                == Started::Running {
                    workspace: 1,
                    placement: "box".into()
                })
            .count(),
        2,
        "{started:?}"
    );
    assert_eq!(
        started
            .iter()
            .filter(|s| **s == Started::Pending(PendingReason::HostFull))
            .count(),
        2
    );
    let h: Host = db.entity(&keys::host(&u, "box")).await.unwrap().unwrap();
    assert_eq!(h.running_workspaces, 2);
    // Host Workspaces are not counted in the concurrency of the Plan.
    assert_eq!(read_user(&db, &u).await.running_workspaces, 0);

    let mut t = new_thread("th_off");
    t.workspace_host = "off".into();
    create(&db, &u, &t).await;
    assert_eq!(
        start_pending(&db, &u, "th_off", NOW).await.unwrap(),
        Started::Pending(PendingReason::HostFull)
    );
    let mut t = new_thread("th_gone");
    t.workspace_host = "gone".into();
    create(&db, &u, &t).await;
    assert_eq!(
        start_pending(&db, &u, "th_gone", NOW).await.unwrap(),
        Started::NeedsYou("Host.Missing".into())
    );
}

async fn last_input(db: &sanchoris_core::db::Db, u: &str, name: &str) -> serde_json::Value {
    outbox::messages(db, u, name)
        .await
        .unwrap()
        .pop()
        .map(|(_, v)| v)
        .unwrap_or_default()
}

fn person(text: &str) -> serde_json::Value {
    json!({"id": text, "source": "person", "text": text})
}

#[tokio::test]
async fn inputs_are_delivered_by_the_state_of_the_thread() {
    let db = db().await;
    let u = user(&db, "pro").await;
    create(&db, &u, &new_thread("th_i")).await;
    let send = |source, text: &'static str| {
        let db = db.clone();
        let u = u.clone();
        async move { thread::send_input(&db, &u, "th_i", source, person(text), NOW).await }
    };
    // PENDING: kept, and sent with the first message when the Workspace runs.
    assert_eq!(send(Source::Person, "second").await, Ok(Delivered::Queued));
    assert!(matches!(
        start_pending(&db, &u, "th_i", NOW).await.unwrap(),
        Started::Running { .. }
    ));
    thread::apply(
        &db,
        &Blobs::memory(),
        &u,
        "th_i",
        Event::WorkspaceReady,
        NOW,
    )
    .await
    .unwrap();
    // RUNNING, the turn asked for: kept for the next turn.
    assert_eq!(send(Source::System, "notice").await, Ok(Delivered::Queued));
    thread::apply(&db, &Blobs::memory(), &u, "th_i", Event::TurnStarted, NOW)
        .await
        .unwrap();
    // RUNNING, the turn runs: steered into it.
    assert_eq!(
        send(Source::Person, "steer me").await,
        Ok(Delivered::Steered)
    );
    let steer = last_input(&db, &u, "th_i").await;
    assert_eq!(steer["delivery"], "steer");
    assert_eq!(steer["text"], "steer me");
    // The turn ends with inputs waiting: the next turn takes them.
    thread::apply(&db, &Blobs::memory(), &u, "th_i", Event::TurnEnded, NOW)
        .await
        .unwrap();
    let t = read_thread(&db, &u, "th_i").await;
    assert_eq!((t.status, t.turn_active), (ThreadStatus::Running, false));
    let next = last_input(&db, &u, "th_i").await;
    assert_eq!(next["delivery"], "turn");
    assert_eq!(next["text"], "notice");
    thread::apply(&db, &Blobs::memory(), &u, "th_i", Event::TurnStarted, NOW)
        .await
        .unwrap();
    thread::apply(&db, &Blobs::memory(), &u, "th_i", Event::TurnEnded, NOW)
        .await
        .unwrap();
    assert_eq!(
        read_thread(&db, &u, "th_i").await.status,
        ThreadStatus::Succeeded
    );
    // SUCCEEDED: any input starts a turn.
    assert_eq!(
        send(Source::System, "later").await,
        Ok(Delivered::StartedTurn)
    );
    thread::apply(&db, &Blobs::memory(), &u, "th_i", Event::TurnStarted, NOW)
        .await
        .unwrap();
    thread::apply(
        &db,
        &Blobs::memory(),
        &u,
        "th_i",
        Event::UsageLimit { auto: false },
        NOW,
    )
    .await
    .unwrap();
    // NEEDS_YOU: notices wait for a person, whose message starts a turn with them.
    assert_eq!(send(Source::System, "wait").await, Ok(Delivered::Queued));
    assert_eq!(
        send(Source::Person, "go on").await,
        Ok(Delivered::StartedTurn)
    );
    assert_eq!(last_input(&db, &u, "th_i").await["text"], "wait\n\ngo on");
    // ABORTED: a person is refused, a notice is not sent.
    thread::stop(&db, &Blobs::memory(), &u, "th_i", NOW)
        .await
        .unwrap();
    let before = outbox::messages(&db, &u, "th_i").await.unwrap().len();
    assert_eq!(
        send(Source::Person, "too late").await,
        Err(ThreadError::Aborted("th_i".into()))
    );
    assert_eq!(
        send(Source::System, "too late").await,
        Ok(Delivered::Dropped)
    );
    assert_eq!(
        outbox::messages(&db, &u, "th_i").await.unwrap().len(),
        before
    );
    // The messages of the outbox are numbered in order.
    let seqs: Vec<u64> = outbox::messages(&db, &u, "th_i")
        .await
        .unwrap()
        .iter()
        .map(|(s, _)| *s)
        .collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    // The first turn had the first message and the input that waited.
    let first = &outbox::messages(&db, &u, "th_i").await.unwrap()[0].1;
    assert_eq!(first["text"], "Work on th_i\n\nsecond");
}

#[tokio::test]
async fn a_thread_with_a_running_subagent_child_waits_instead_of_succeeding() {
    let db = db().await;
    let u = user(&db, "max").await;
    create(&db, &u, &new_thread("th_p")).await;
    start_pending(&db, &u, "th_p", NOW).await.unwrap();
    let mut child = new_thread("th_k");
    child.parent = Some(("th_p".into(), Lineage::Subagent, None));
    create(&db, &u, &child).await;
    thread::apply(
        &db,
        &Blobs::memory(),
        &u,
        "th_p",
        Event::WorkspaceReady,
        NOW,
    )
    .await
    .unwrap();
    thread::apply(&db, &Blobs::memory(), &u, "th_p", Event::TurnStarted, NOW)
        .await
        .unwrap();
    thread::apply(&db, &Blobs::memory(), &u, "th_p", Event::TurnEnded, NOW)
        .await
        .unwrap();
    let t = read_thread(&db, &u, "th_p").await;
    assert_eq!(
        (t.status, t.wait_reason),
        (ThreadStatus::Waiting, Some(WaitReason::Children))
    );
    // Once the child stops, the next turn of the parent can succeed.
    thread::stop(&db, &Blobs::memory(), &u, "th_k", NOW)
        .await
        .unwrap();
    thread::send_input(
        &db,
        &u,
        "th_p",
        Source::System,
        json!({"id": "n1", "text": "child stopped"}),
        NOW,
    )
    .await
    .unwrap();
    thread::apply(&db, &Blobs::memory(), &u, "th_p", Event::TurnStarted, NOW)
        .await
        .unwrap();
    thread::apply(&db, &Blobs::memory(), &u, "th_p", Event::TurnEnded, NOW)
        .await
        .unwrap();
    assert_eq!(
        read_thread(&db, &u, "th_p").await.status,
        ThreadStatus::Succeeded
    );
}

#[tokio::test]
async fn stopping_a_thread_stops_its_subagent_children_and_not_its_forks() {
    let db = db().await;
    let u = user(&db, "max").await;
    create(&db, &u, &new_thread("th_root")).await;
    let mut sub = new_thread("th_sub");
    sub.parent = Some(("th_root".into(), Lineage::Subagent, None));
    create(&db, &u, &sub).await;
    let mut grand = new_thread("th_grand");
    grand.parent = Some(("th_sub".into(), Lineage::Subagent, None));
    create(&db, &u, &grand).await;
    let mut fork = new_thread("th_fork");
    fork.parent = Some(("th_root".into(), Lineage::Fork, Some(3)));
    create(&db, &u, &fork).await;
    for name in ["th_root", "th_sub", "th_grand", "th_fork"] {
        start_pending(&db, &u, name, NOW).await.unwrap();
    }
    assert_eq!(read_user(&db, &u).await.running_workspaces, 4);
    thread::stop(&db, &Blobs::memory(), &u, "th_root", NOW)
        .await
        .unwrap();
    for name in ["th_root", "th_sub", "th_grand"] {
        assert_eq!(
            read_thread(&db, &u, name).await.status,
            ThreadStatus::Aborted,
            "{name}"
        );
    }
    assert_eq!(
        read_thread(&db, &u, "th_fork").await.status,
        ThreadStatus::Running
    );
    assert_eq!(read_user(&db, &u).await.running_workspaces, 1);
}

/// Starts a SkillRun whose command runs until it is stopped, in a thread of a User.
async fn hanging_run(db: &sanchoris_core::db::Db, blobs: &Blobs, u: &str, thread: &str) {
    use sanchoris_engine::{Engine, Origin, StartRequest};
    let source = json!({
        "StartAt": "Run",
        "TimeoutSeconds": 3600,
        "States": {"Run": {"Type": "Command", "Run": "sleep 1000", "TimeoutSeconds": 600, "End": true}}
    });
    let definition = sanchoris_engine::definition("hang", &source).unwrap();
    let mut engine = Engine::new(sanchoris_core::engine_store::DynamoStore::new(
        db.clone(),
        blobs.clone(),
        u,
    ));
    engine
        .start(
            StartRequest {
                request: format!("start-{thread}"),
                thread: thread.into(),
                origin: Origin::Thread,
                definition,
                source,
                input: json!({}),
            },
            NOW,
        )
        .await
        .unwrap();
}

async fn run_status(db: &sanchoris_core::db::Db, blobs: &Blobs, u: &str, thread: &str) -> String {
    use sanchoris_engine::Store;
    let mut store = sanchoris_core::engine_store::DynamoStore::new(db.clone(), blobs.clone(), u);
    let runs = store.runs_of_thread(thread).await.unwrap();
    sanchoris_engine::engine::status_name(&runs[0].run.status).to_string()
}

#[tokio::test]
async fn a_thread_with_a_running_skill_run_waits_and_its_stop_aborts_the_run() {
    let db = db().await;
    let blobs = Blobs::memory();
    let u = user(&db, "max").await;
    create(&db, &u, &new_thread("th_run")).await;
    start_pending(&db, &u, "th_run", NOW).await.unwrap();
    for e in [Event::WorkspaceReady, Event::TurnStarted] {
        thread::apply(&db, &blobs, &u, "th_run", e, NOW)
            .await
            .unwrap();
    }
    hanging_run(&db, &blobs, &u, "th_run").await;
    thread::apply(&db, &blobs, &u, "th_run", Event::TurnEnded, NOW)
        .await
        .unwrap();
    let t = read_thread(&db, &u, "th_run").await;
    assert_eq!(
        (t.status, t.wait_reason),
        (ThreadStatus::Waiting, Some(WaitReason::Skill))
    );
    thread::stop(&db, &blobs, &u, "th_run", NOW).await.unwrap();
    assert_eq!(run_status(&db, &blobs, &u, "th_run").await, "ABORTED");
    assert_eq!(read_user(&db, &u).await.running_workspaces, 0);
}

#[tokio::test]
async fn a_stop_that_ended_after_its_first_write_ends_the_same_when_it_runs_again() {
    let db = db().await;
    let blobs = Blobs::memory();
    let u = user(&db, "max").await;
    create(&db, &u, &new_thread("th_half")).await;
    let mut child = new_thread("th_half_kid");
    child.parent = Some(("th_half".into(), Lineage::Subagent, None));
    create(&db, &u, &child).await;
    for name in ["th_half", "th_half_kid"] {
        start_pending(&db, &u, name, NOW).await.unwrap();
    }
    hanging_run(&db, &blobs, &u, "th_half").await;
    // The stop wrote the thread ABORTED with its Workspace stopped, and ended before its
    // SkillRun and its child.
    set(&db, &u, "th_half", |t| t.status = ThreadStatus::Aborted).await;
    assert!(
        thread::release_workspace(&db, &u, "th_half", 1, "thread stopped", NOW)
            .await
            .unwrap()
    );
    assert_eq!(read_user(&db, &u).await.running_workspaces, 1);
    assert_eq!(run_status(&db, &blobs, &u, "th_half").await, "RUNNING");
    thread::stop(&db, &blobs, &u, "th_half", NOW).await.unwrap();
    thread::stop(&db, &blobs, &u, "th_half", NOW).await.unwrap();
    assert_eq!(run_status(&db, &blobs, &u, "th_half").await, "ABORTED");
    assert_eq!(
        read_thread(&db, &u, "th_half_kid").await.status,
        ThreadStatus::Aborted
    );
    assert_eq!(read_user(&db, &u).await.running_workspaces, 0);
}
