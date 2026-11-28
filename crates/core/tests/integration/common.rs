#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use sanchoris_config::{Config, Outside, validate};
use sanchoris_core::db::Db;
use sanchoris_core::entities::{Subscription, Thread, User, UserStatus, keys};
use sanchoris_core::local;
use sanchoris_core::thread::{NewThread, insert_thread};
use serde_json::{Value, json};

static USERS: AtomicU64 = AtomicU64::new(0);

pub const NOW: u64 = 1_795_858_200_000;

pub async fn db() -> Db {
    local::shared_tables().await
}

/// A new User on a Plan with an active subscription.
pub async fn user(db: &Db, plan: &str) -> String {
    user_with(db, plan, Subscription::Active, |_| {}).await
}

pub async fn user_with(
    db: &Db,
    plan: &str,
    subscription: Subscription,
    change: impl FnOnce(&mut User),
) -> String {
    let id = format!(
        "u{}x{}",
        std::process::id(),
        USERS.fetch_add(1, Ordering::SeqCst)
    );
    let mut u = User {
        id: id.clone(),
        cognito_subject: format!("sub-{id}"),
        email: format!("{id}@example.com"),
        github_user_id: None,
        github_login: None,
        plan: plan.to_string(),
        period_threads: 0,
        period_workspace_seconds: 0,
        running_workspaces: 0,
        stripe_customer: None,
        subscription,
        period_start: NOW,
        period_end: NOW + 30 * 86_400_000,
        status: UserStatus::Active,
        suspended_reason: None,
        operator: false,
        created_at: NOW,
    };
    change(&mut u);
    let mut tx = db.tx(&id);
    tx.insert_entity(&u).unwrap();
    tx.commit().await.unwrap();
    id
}

pub async fn read_user(db: &Db, user: &str) -> User {
    db.entity(&keys::user(user)).await.unwrap().unwrap()
}

pub async fn read_thread(db: &Db, user: &str, name: &str) -> Thread {
    db.entity(&keys::thread(user, name)).await.unwrap().unwrap()
}

pub fn new_thread(name: &str) -> NewThread {
    NewThread {
        name: name.to_string(),
        project: "shop".into(),
        message: format!("Work on {name}"),
        title: format!("Work on {name}"),
        agent: "claude".into(),
        model: "claude-sonnet-5-5".into(),
        credential: "my-claude".into(),
        workspace_host: "cloud".into(),
        origin: "web".into(),
        requester: "mika-dev".into(),
        parent: None,
        blocked_by: Vec::new(),
        trigger_key: None,
        definition_key: None,
        config_revision: None,
    }
}

/// Writes new threads, `PENDING` until the thread-starter decides.
pub async fn create(db: &Db, user: &str, thread: &NewThread) {
    let mut tx = db.tx(user);
    insert_thread(&mut tx, thread, NOW).unwrap();
    tx.commit().await.unwrap();
}

/// Changes a thread as a test setup, over its version.
pub async fn set(db: &Db, user: &str, name: &str, change: impl FnOnce(&mut Thread)) {
    let mut t = read_thread(db, user, name).await;
    change(&mut t);
    let mut tx = db.tx(user);
    tx.put_entity(&t, None).unwrap();
    tx.commit().await.unwrap();
}

const CONFIG: &str = include_str!("../../../config/tests/fixtures/sanchoris.yaml");

/// The configuration of the fixture of `config`.
pub fn config() -> Config {
    let files = BTreeMap::from([("sanchoris.yaml".to_string(), CONFIG.to_string())]);
    let outside = Outside {
        credentials: vec!["my-claude".to_string()],
        hosts: Vec::new(),
        repositories: vec!["acme/shop-web".to_string(), "acme/shop-api".to_string()],
    };
    validate(&files, &outside).expect("the fixture passes")
}

pub fn repositories() -> BTreeMap<String, u64> {
    BTreeMap::from([
        ("acme/shop-web".to_string(), 7001),
        ("acme/shop-api".to_string(), 7002),
    ])
}

/// `Issue Labeled` on `acme/shop-web`.
pub fn labeled(number: u64, sender: &str) -> Value {
    json!({
        "source": "github",
        "detail-type": "Issue Labeled",
        "detail": {
            "repository": "acme/shop-web",
            "label": "sanchoris",
            "issue": {"number": number, "title": format!("Issue {number}"), "body": "", "author_association": "MEMBER"},
            "sender": {"login": sender},
        }
    })
}
