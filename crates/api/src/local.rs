//! The control plane in one local process (`nix run`, `pnpm e2e`): DynamoDB Local with its
//! stream, the objects in memory, one User on the Admin Plan, its Credentials and Config from
//! local files, and Workspaces as local `sanchoris-agent` processes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sanchoris_config::Outside;
use sanchoris_core::blob::Blobs;
use sanchoris_core::db::Db;
use sanchoris_core::entities::{Credential, Subscription, User, UserStatus, keys};
use sanchoris_core::secret::SecretKey;
use sanchoris_core::token::TokenKey;
use sanchoris_worker::local::{Local, LocalGitHub, LocalLauncher, LocalUrls};
use sanchoris_worker::ports::{BoxFuture, Ctx, Launch, Launcher, now};
use serde_json::Value;

use crate::App;

/// The settings of the local control plane, from the environment (`.env.example`).
pub struct Settings {
    pub user: String,
    pub public_url: String,
    pub config_dir: Option<PathBuf>,
    pub credentials_file: Option<PathBuf>,
    pub git_root: Option<PathBuf>,
    pub workspaces_dir: Option<PathBuf>,
    pub agent_command: Vec<String>,
    pub agent_env: Vec<(String, String)>,
}

fn optional_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl Settings {
    pub fn from_env(bind: &str) -> Result<Settings, String> {
        let user = std::env::var("SANCHORIS_LOCAL_USER")
            .map_err(|_| "SANCHORIS_LOCAL_USER must be set")?;
        let workspaces_dir = optional_path("SANCHORIS_LOCAL_WORKSPACES_DIR");
        let agent_command: Vec<String> = std::env::var("SANCHORIS_AGENT_COMMAND")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if workspaces_dir.is_some() && agent_command.is_empty() {
            return Err(
                "SANCHORIS_AGENT_COMMAND must be set with SANCHORIS_LOCAL_WORKSPACES_DIR".into(),
            );
        }
        let agent_env = [
            "SANCHORIS_GIT_URL_PREFIX",
            "SANCHORIS_WORKER_MODULE",
            "SANCHORIS_CLAUDE_PATH",
            "SANCHORIS_CODEX_PATH",
        ]
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|v| (name.to_string(), v)))
        .collect();
        Ok(Settings {
            user,
            public_url: std::env::var("SANCHORIS_PUBLIC_URL")
                .unwrap_or_else(|_| format!("http://{bind}")),
            config_dir: optional_path("SANCHORIS_LOCAL_CONFIG_DIR"),
            credentials_file: optional_path("SANCHORIS_LOCAL_CREDENTIALS"),
            git_root: optional_path("SANCHORIS_LOCAL_GIT_ROOT"),
            workspaces_dir,
            agent_command,
            agent_env,
        })
    }
}

/// A launcher for a server without local Workspaces.
struct NoWorkspaces;

impl Launcher for NoWorkspaces {
    fn launch(&self, _launch: Launch) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async {
            Err("this server runs no Workspaces (set SANCHORIS_LOCAL_WORKSPACES_DIR)".into())
        })
    }
}

fn files_of(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, text);
            }
        }
    }
    Ok(files)
}

/// The repositories `owner/name` of the bare repositories at `<root>/github.com/owner/name`.
fn repositories(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let base = root.join("github.com");
    for owner in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        for repo in std::fs::read_dir(owner.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            out.push(format!(
                "{}/{}",
                owner.file_name().to_string_lossy(),
                repo.file_name().to_string_lossy()
            ));
        }
    }
    out
}

async fn seed(
    db: &Db,
    blobs: &Blobs,
    secrets: &SecretKey,
    settings: &Settings,
) -> Result<(), String> {
    let at = now();
    let user = &settings.user;
    if db
        .entity::<User>(&keys::user(user))
        .await
        .map_err(|e| e.to_string())?
        .is_none()
    {
        let mut tx = db.tx(user);
        tx.insert_entity(&User {
            id: user.clone(),
            cognito_subject: format!("local-{user}"),
            email: format!("{user}@localhost"),
            github_user_id: None,
            github_login: None,
            plan: "admin".into(),
            period_threads: 0,
            period_workspace_seconds: 0,
            running_workspaces: 0,
            stripe_customer: None,
            subscription: Subscription::Active,
            period_start: at,
            period_end: at + 30 * 86_400_000,
            status: UserStatus::Active,
            suspended_reason: None,
            operator: true,
            created_at: at,
        })
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
    }
    let mut credentials = Vec::new();
    if let Some(file) = &settings.credentials_file {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let all: BTreeMap<String, Value> =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        let mut tx = db.tx(user);
        for (name, c) in all {
            let value = match c["valueFile"].as_str() {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|e| format!("{path}: {e}"))?
                    .trim()
                    .to_string(),
                None => c["value"].as_str().unwrap_or_default().to_string(),
            };
            tx.put_entity(
                &Credential {
                    name: name.clone(),
                    provider: c["provider"].as_str().unwrap_or_default().into(),
                    credential_kind: "subscription".into(),
                    account: None,
                    state: "connected".into(),
                    last_used_at: None,
                    secret_kind: c["kind"].as_str().map(str::to_string),
                    secret: Some(secrets.seal(user, &value)),
                    version: 0,
                },
                None,
            )
            .map_err(|e| e.to_string())?;
            credentials.push(name);
        }
        tx.commit().await.map_err(|e| e.to_string())?;
    }
    if let Some(dir) = &settings.config_dir {
        let files = files_of(dir)?;
        let outside = Outside {
            credentials,
            hosts: Vec::new(),
            repositories: settings
                .git_root
                .as_deref()
                .map(repositories)
                .unwrap_or_default(),
        };
        sanchoris_core::activation::activate(
            db,
            blobs,
            user,
            1,
            "local/config",
            "local",
            &files,
            &outside,
            at,
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Makes the tables, seeds the User, its Credentials and Config, and runs the dispatcher and
/// the worker.
pub async fn start(settings: Settings) -> Result<Arc<App>, String> {
    let config = aws_config::load_from_env().await;
    let db = Db::new(
        aws_sdk_dynamodb::Client::new(&config),
        sanchoris_core::db::Tables::from_env().map_err(|e| e.to_string())?,
    );
    if let Err(e) = sanchoris_core::schema::create_tables(&db).await {
        eprintln!("sanchoris: the tables: {e}");
    }
    let blobs = Blobs::memory();
    let tokens = TokenKey::from_env().map_err(|e| e.to_string())?;
    let secrets = SecretKey::from_env().map_err(|e| e.to_string())?;
    seed(&db, &blobs, &secrets, &settings).await?;
    let local = Local::new();
    let launcher: Arc<dyn Launcher> = match &settings.workspaces_dir {
        Some(root) => Arc::new(LocalLauncher {
            root: root.clone(),
            command: settings.agent_command.clone(),
            env: settings.agent_env.clone(),
            exited: local.exited.clone(),
        }),
        None => Arc::new(NoWorkspaces),
    };
    let channel_url = format!(
        "{}/threads/channel",
        settings.public_url.replacen("http", "ws", 1)
    );
    let ctx = Ctx {
        db: db.clone(),
        blobs: blobs.clone(),
        tokens: tokens.clone(),
        secrets,
        connections: Arc::new(local.connections.clone()),
        queue: Arc::new(local.queue()),
        scheduler: Arc::new(local.scheduler()),
        launcher,
        urls: Arc::new(LocalUrls {
            base: settings.public_url.clone(),
            key: tokens,
        }),
        github: Arc::new(LocalGitHub {
            root: settings.git_root.clone().unwrap_or_default(),
        }),
        channel_url,
    };
    let app = Arc::new(App {
        ctx: ctx.clone(),
        schema: sanchoris_graphql::schema(db, blobs),
        connections: local.connections.clone(),
        local_user: Some(settings.user.clone()),
    });
    local.run(ctx, aws_sdk_dynamodbstreams::Client::new(&config));
    Ok(app)
}
