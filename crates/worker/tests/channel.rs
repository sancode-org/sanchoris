//! The Thread channel against DynamoDB Local, with the message examples of
//! `apps/sanchoris-agent/protocol/`: the tokens, `hello` and the `credentials` and `start` it
//! asks for, every message of the agent, and a message that comes twice.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use sanchoris_config::Outside;
use sanchoris_core::blob::Blobs;
use sanchoris_core::db::Db;
use sanchoris_core::entities::{
    Credential, Message, Subscription, Thread, Turn, User, UserStatus, Workspace, keys,
};
use sanchoris_core::secret::SecretKey;
use sanchoris_core::thread::{self, Started};
use sanchoris_core::token::{self, TokenKey, TokenKind};
use sanchoris_core::{activation, launch, local};
use sanchoris_worker::channel::{self, Refused, Session};
use sanchoris_worker::deliver;
use sanchoris_worker::local::{LocalGitHub, LocalUrls};
use sanchoris_worker::ports::{
    BoxFuture, Connections, Ctx, Gone, Launch, Launcher, Queue, Scheduler, now,
};
use serde_json::{Value, json};

const PROTOCOL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/sanchoris-agent/protocol"
);

#[derive(Default)]
struct Recorded {
    frames: Mutex<Vec<Value>>,
    jobs: Mutex<Vec<Value>>,
}

struct Sockets(Arc<Recorded>);
impl Connections for Sockets {
    fn send(&self, _connection: &str, frame: String) -> BoxFuture<'_, Result<(), Gone>> {
        self.0
            .frames
            .lock()
            .unwrap()
            .push(serde_json::from_str(&frame).unwrap());
        Box::pin(async { Ok(()) })
    }
}

struct Jobs(Arc<Recorded>);
impl Queue for Jobs {
    fn send(&self, job: Value) -> BoxFuture<'_, ()> {
        self.0.jobs.lock().unwrap().push(job);
        Box::pin(async {})
    }
}
impl Scheduler for Jobs {
    fn schedule(&self, _job: Value, _due: u64) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}
impl Launcher for Jobs {
    fn launch(&self, _launch: Launch) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

const CONFIG: &str = "Version: 1
Projects:
  shop:
    Repos:
      frontend: { Repo: github.com/acme/shop-web }
    DefaultModel: { Agent: claude, Model: claude-opus-5-5, Credential: my-claude }
    Scripts:
      install: { Command: 'pnpm install', RunOnCreate: true, Async: false, TimeoutSeconds: 600 }
    UsageLimit: { Resume: auto }
    Workspace: { Host: cloud, Architecture: arm64, IdleSeconds: 900 }
Triggers: {}
";

async fn setup() -> (Ctx, Arc<Recorded>, String, String) {
    let db: Db = local::tables().await;
    let blobs = Blobs::memory();
    let recorded = Arc::new(Recorded::default());
    let tokens = TokenKey::new(vec![1u8; 32]);
    let ctx = Ctx {
        db: db.clone(),
        blobs: blobs.clone(),
        tokens: tokens.clone(),
        secrets: SecretKey::new([2u8; 32]),
        connections: Arc::new(Sockets(recorded.clone())),
        queue: Arc::new(Jobs(recorded.clone())),
        scheduler: Arc::new(Jobs(recorded.clone())),
        launcher: Arc::new(Jobs(recorded.clone())),
        urls: Arc::new(LocalUrls {
            base: "http://127.0.0.1:1".into(),
            key: tokens,
        }),
        github: Arc::new(LocalGitHub {
            root: "/nonexistent".into(),
        }),
        channel_url: "ws://127.0.0.1:1/threads/channel".into(),
    };
    let user = "u_01".to_string();
    let at = now();
    let mut tx = db.tx(&user);
    tx.insert_entity(&User {
        id: user.clone(),
        cognito_subject: "sub".into(),
        email: "u@example.com".into(),
        github_user_id: None,
        github_login: None,
        plan: "admin".into(),
        period_threads: 0,
        period_workspace_seconds: 0,
        running_workspaces: 0,
        stripe_customer: None,
        subscription: Subscription::Active,
        period_start: at,
        period_end: at + 1,
        status: UserStatus::Active,
        suspended_reason: None,
        operator: false,
        created_at: at,
    })
    .unwrap();
    tx.insert_entity(&Credential {
        name: "my-claude".into(),
        provider: "claude".into(),
        credential_kind: "subscription".into(),
        account: None,
        state: "connected".into(),
        last_used_at: None,
        secret_kind: Some("claude_oauth_token".into()),
        secret: Some(ctx.secrets.seal(&user, "the-token")),
        version: 0,
    })
    .unwrap();
    tx.commit().await.unwrap();
    let files = BTreeMap::from([("sanchoris.yaml".to_string(), CONFIG.to_string())]);
    let outside = Outside {
        credentials: vec!["my-claude".into()],
        hosts: Vec::new(),
        repositories: vec!["acme/shop-web".into()],
    };
    activation::activate(
        &db,
        &blobs,
        &user,
        7,
        "acme/config",
        "rev1",
        &files,
        &outside,
        at,
    )
    .await
    .unwrap();
    let name = launch::start_thread(&db, &blobs, &user, "shop", "Fix the flaky test", at)
        .await
        .unwrap();
    let started = thread::start_pending(&db, &user, &name, at).await.unwrap();
    assert!(matches!(started, Started::Running { workspace: 1, .. }));
    (ctx, recorded, user, name)
}

fn example(dir: &str, file: &str, user: &str, thread: &str) -> Value {
    let text = std::fs::read_to_string(format!("{PROTOCOL}/{dir}/{file}")).unwrap();
    serde_json::from_str(
        &text
            .replace("th_01JD8Q2M4K", thread)
            .replace("u/u_01/", &format!("u/{user}/")),
    )
    .unwrap()
}

fn keys_of(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

async fn read<T: sanchoris_core::entities::Entity>(ctx: &Ctx, key: &sanchoris_core::db::Key) -> T {
    ctx.db.entity(key).await.unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_channel_takes_every_message_of_the_agent() {
    let (ctx, recorded, user, name) = setup().await;
    let at = now();
    let bootstrap = ctx
        .tokens
        .issue(&token::claims(TokenKind::Bootstrap, &user, &name, 1, at));
    let header = format!("Bearer {bootstrap}");
    assert_eq!(
        channel::connect(&ctx, Some("Bearer nope.nope"), at).await,
        Err(Refused::Unauthorized)
    );
    let other = ctx
        .tokens
        .issue(&token::claims(TokenKind::Thread, &user, &name, 2, at));
    assert_eq!(
        channel::connect(&ctx, Some(&format!("Bearer {other}")), at).await,
        Err(Refused::Forbidden)
    );
    let session: Session = channel::connect(&ctx, Some(&header), at).await.unwrap();
    assert!(session.bootstrap);
    assert_eq!(
        channel::connect(&ctx, Some(&header), at).await,
        Err(Refused::Forbidden),
        "a bootstrap token is spent"
    );

    let hello = example("agent-to-control", "hello.json", &user, &name);
    let reply = channel::receive(&ctx, &session, "conn-1", &hello.to_string(), at)
        .await
        .unwrap();
    assert_eq!(reply.frames.len(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&reply.frames[0]).unwrap(),
        json!({"type": "ack", "threadId": name, "seq": 0})
    );
    deliver::flush(&ctx, &user, &name).await.unwrap();
    let frames = recorded.frames.lock().unwrap().clone();
    assert_eq!(
        frames.iter().map(|f| f["type"].clone()).collect::<Vec<_>>(),
        vec![json!("credentials"), json!("start")]
    );
    for frame in &frames {
        let file = format!("{}.json", frame["type"].as_str().unwrap());
        let ex = example("control-to-agent", &file, &user, &name);
        assert_eq!(keys_of(frame), keys_of(&ex), "{file}");
        for nested in [
            "github",
            "recordUpload",
            "thread",
            "project",
            "worker",
            "gitAuthor",
            "skills",
        ] {
            if ex.get(nested).is_some_and(Value::is_object) {
                assert_eq!(
                    keys_of(&frame[nested]),
                    keys_of(&ex[nested]),
                    "{file} {nested}"
                );
            }
        }
    }
    let start = &frames[1];
    assert_eq!(start["worker"]["credential"]["value"], json!("the-token"));
    assert!(
        start["instruction"]
            .as_str()
            .unwrap()
            .starts_with("[sanchoris] The user started this thread: Fix the flaky test.\nMessage: Fix the flaky test\nSkills: open-pr-green-ci.")
    );
    assert_eq!(
        start["project"]["repos"][0]["branch"],
        json!(format!("sanchoris/{name}"))
    );
    let t: Thread = read(&ctx, &keys::thread(&user, &name)).await;
    assert!(
        t.queued.is_empty(),
        "the instruction took the first message"
    );

    let mut seq = 0;
    let files = [
        "ready.json",
        "event.json",
        "rate_limit.json",
        "skill_request.json",
        "skill_request-status.json",
        "skill_request-continue.json",
        "skill_request-await.json",
        "thread_request.json",
        "thread_request-send.json",
        "thread_request-status.json",
        "thread_request-stop.json",
        "exec_result.json",
        "script_result.json",
        "credential_update.json",
        "thread_record.json",
        "turn_end.json",
        "turn_end-usage-limit.json",
        "heartbeat.json",
        "stopping.json",
    ];
    for file in files {
        let mut msg = example("agent-to-control", file, &user, &name);
        if msg.get("seq").is_some() {
            seq += 1;
            msg["seq"] = json!(seq);
        }
        if file == "skill_request.json" {
            msg["request"] = json!({"action": "run", "name": "open-pr-green-ci", "input": {"Repo": "frontend", "Issue": 35}});
        }
        let reply = channel::receive(&ctx, &session, "conn-1", &msg.to_string(), at)
            .await
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(!reply.close, "{file}: {:?}", reply.frames);
        if msg.get("seq").is_some() {
            assert_eq!(
                serde_json::from_str::<Value>(&reply.frames[0]).unwrap()["seq"],
                json!(seq),
                "{file}"
            );
        }
        if file == "turn_end.json" {
            let again = channel::receive(&ctx, &session, "conn-1", &msg.to_string(), at)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&again.frames[0]).unwrap()["seq"],
                json!(seq),
                "a message that comes twice is acknowledged again"
            );
        }
    }

    let t: Thread = read(&ctx, &keys::thread(&user, &name)).await;
    assert_eq!(
        t.provider_session.as_deref(),
        Some("5f0c6a1e-2b3d-4e5f-8a9b-0c1d2e3f4a5b")
    );
    assert_eq!(t.total_turns, 2, "each turn_end counts once");
    assert_eq!(t.error.as_deref(), Some("Script.Failed"));
    let turn: Turn = read(&ctx, &keys::turn(&user, &name, 1)).await;
    assert_eq!(turn.result.as_deref(), Some("completed"));
    assert_eq!(
        turn.record_version,
        Some(format!("u/{user}/threads/{name}/records/000001.tar.gz"))
    );
    let messages: Vec<Message> = ctx
        .db
        .entities(&user, &format!("THREAD#{name}#MSG#"))
        .await
        .unwrap();
    assert_eq!(
        messages.iter().filter(|m| m.sender == "agent").count(),
        2,
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.sender == "sanchoris"
            && m.text
                .starts_with("[sanchoris] Script install (RunOnCreate) failed. Exit 1.")),
        "{messages:?}"
    );
    let c: Credential = read(&ctx, &keys::credential(&user, "my-claude")).await;
    assert_eq!(
        ctx.secrets
            .open(&user, c.secret.as_deref().unwrap())
            .unwrap(),
        "{\"tokens\":\"<refreshed tokens>\"}"
    );
    assert_eq!(c.secret_kind.as_deref(), Some("codex_auth_json"));
    let w: Workspace = read(&ctx, &keys::workspace(&user, &name, 1)).await;
    assert_eq!(w.state, "stopped");
    assert_eq!(w.stop_reason.as_deref(), Some("interrupted"));

    let mut runs = sanchoris_engine::Engine::new(sanchoris_core::engine_store::DynamoStore::new(
        ctx.db.clone(),
        ctx.blobs.clone(),
        &user,
    ));
    let run = sanchoris_engine::Store::run(&mut runs.store, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.origin, sanchoris_engine::Origin::Thread);
    let outbox = sanchoris_core::outbox::messages(&ctx.db, &user, &name)
        .await
        .unwrap();
    let responses: Vec<&Value> = outbox
        .iter()
        .map(|(_, b)| b)
        .filter(|b| b["type"] == json!("skill_response") || b["type"] == json!("thread_response"))
        .collect();
    assert_eq!(responses.len(), 8, "one answer for each request");
    assert_eq!(responses[0]["result"]["RunId"], json!(1));
    assert!(
        responses
            .iter()
            .filter(|r| r["type"] == json!("thread_response"))
            .all(|r| r["ok"] == json!(false))
    );
}
