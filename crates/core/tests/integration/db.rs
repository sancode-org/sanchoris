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

async fn round_trip<E>(db: &sanchoris_core::db::Db, u: &str, e: E)
where
    E: sanchoris_core::entities::Entity + PartialEq + std::fmt::Debug,
{
    let mut tx = db.tx(u);
    tx.insert_entity(&e).unwrap();
    tx.commit().await.unwrap();
    let read: E = db.entity(&e.key(u)).await.unwrap().expect(E::KIND);
    assert_eq!(read, e, "{}", E::KIND);
}

#[tokio::test]
async fn every_entity_is_read_back_as_it_was_written() {
    use sanchoris_core::entities::*;
    let db = db().await;
    let u = user(&db, "pro").await;
    round_trip(
        &db,
        &u,
        Credential {
            name: "my-claude".into(),
            provider: "claude".into(),
            credential_kind: "subscription".into(),
            account: None,
            state: "connected".into(),
            last_used_at: Some(NOW),
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        UsageSnapshot {
            credential: "my-claude".into(),
            taken_at: NOW,
            windows: vec![UsageWindow {
                name: "5h".into(),
                used_percent: 42.5,
                resets_at: None,
            }],
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Integration {
            integration_kind: "github_installation".into(),
            id: "123".into(),
            state: "connected".into(),
            detail: serde_json::Value::Null,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Host {
            name: "box".into(),
            state: "online".into(),
            architecture: "arm64".into(),
            driver_version: "0.1.0".into(),
            max_workspaces: 2,
            workspace_vcpu: 2,
            workspace_memory_mib: 4096,
            running_workspaces: 0,
            last_heartbeat_at: NOW,
            registered_at: NOW,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        AuditEvent {
            id: "a1".into(),
            at: NOW,
            actor: "user".into(),
            actor_ref: Some(u.clone()),
            action: "thread.stop".into(),
            target: "th_1".into(),
            summary: "stopped".into(),
            source_ip: None,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Config {
            repository_id: 9001,
            repository: "acme/sanchoris-config".into(),
            root: "/".into(),
            state: "active".into(),
            revision: Some("abc".into()),
            synced_at: Some(NOW),
            last_sync_error: None,
            validation: "valid".into(),
            validation_messages: vec![],
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Project {
            repository_id: 9001,
            name: "shop".into(),
            definition: json!({"Repos": {}}),
            file: "sanchoris.yaml".into(),
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Repository {
            config_repository_id: 9001,
            project: "shop".into(),
            name: "frontend".into(),
            repo: "github.com/acme/shop-web".into(),
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Skill {
            repository_id: 9001,
            name: "open-pr-green-ci".into(),
            predictable: true,
            builtin: true,
            description: "Open a PR".into(),
            inputs: serde_json::Value::Null,
            definition_key: None,
            file: "builtin".into(),
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Trigger {
            repository_id: 9001,
            name: "labeled-issue".into(),
            enabled: true,
            event_pattern: Some(json!({"source": ["github"]})),
            schedule_expression: None,
            input_paths_map: json!({}),
            start: json!({"Project": "shop"}),
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        TriggerEvent {
            repository_id: 9001,
            trigger: "labeled-issue".into(),
            received_at: NOW,
            delivery: "d1".into(),
            event: json!({"detail": {"a": [1, 2.5, null]}}),
            thread_key: None,
            result: TriggerOutcome::DroppedLimit {
                reason: "60".into(),
            },
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Message {
            thread: "th_1".into(),
            seq: 1,
            sender: "user".into(),
            text: "hi".into(),
            at: NOW,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Turn {
            thread: "th_1".into(),
            seq: 1,
            started_at: NOW,
            ended_at: None,
            result: None,
            commits: vec![TurnCommit {
                repository: "frontend".into(),
                commit: "deadbeef".into(),
            }],
            record_version: None,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Workspace {
            thread: "th_1".into(),
            seq: 1,
            target: "cloud".into(),
            platform: Some("fargate_spot".into()),
            state: "running".into(),
            started_at: NOW,
            stopped_at: None,
            stop_reason: None,
            exit_code: Some(-1),
            repositories: vec![ClonedRepository {
                name: "frontend".into(),
                commit: "abc".into(),
            }],
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Usage {
            thread: "th_1".into(),
            input_tokens: 10,
            output_tokens: 20,
            cost_usd: 0.25,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        PullRequest {
            thread: "th_1".into(),
            repository_id: 7001,
            number: 52,
            state: "open".into(),
            checks: None,
            created_at: NOW,
        },
    )
    .await;
    round_trip(
        &db,
        &u,
        Child {
            parent: "th_1".into(),
            name: "th_2".into(),
            lineage: Lineage::Fork,
            fork_turn: Some(3),
            created_at: NOW,
        },
    )
    .await;
    let audit: Vec<AuditEvent> = db.entities(&u, "AUDIT#").await.unwrap();
    assert_eq!(audit.len(), 1);
}
