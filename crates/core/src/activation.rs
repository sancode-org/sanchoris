//! Config activation: a revision of a configuration repository that passed validation becomes
//! the active Config of its User, with its Projects, Repositories and Skills. The skill
//! documents and definitions go to S3, and each Skill names them.

use std::collections::BTreeMap;

use sanchoris_config::{
    Architecture, BUILTIN_SKILLS, Idle, InputType, Issue, Outside, Project as ConfigProject, Resume,
};
use serde_json::{Map, Value, json};

use crate::blob::{Blobs, content_key};
use crate::db::{Db, DbError};
use crate::entities::{Config, Project, Repository, Skill};

/// A write of the activation.
type Write = Box<dyn FnOnce(&mut crate::db::UserTx) -> Result<(), DbError>>;

/// The actions of one transaction of the activation, below the 100 of DynamoDB.
const CHUNK: usize = 50;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ActivationError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("the configuration is not valid: {}", .0.iter().map(|i| format!("{}: {} {}", i.file, i.path, i.message)).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<Issue>),
}

pub fn input_type_name(t: InputType) -> &'static str {
    match t {
        InputType::String => "string",
        InputType::Number => "number",
        InputType::Boolean => "boolean",
        InputType::Array => "array",
        InputType::Object => "object",
    }
}

/// A Project of `sanchoris.yaml` as JSON, with the keys of the YAML.
pub fn project_json(p: &ConfigProject) -> Value {
    let repos: Map<String, Value> = p
        .repos
        .iter()
        .map(|(name, repo)| (name.clone(), json!({"Repo": format!("github.com/{repo}")})))
        .collect();
    let scripts: Map<String, Value> = p
        .scripts
        .iter()
        .map(|(name, s)| {
            (
                name.clone(),
                json!({
                    "Command": s.command,
                    "TimeoutSeconds": s.timeout_seconds,
                    "RunOnCreate": s.run_on_create,
                    "Async": s.run_async,
                }),
            )
        })
        .collect();
    json!({
        "Repos": repos,
        "DefaultModel": {
            "Agent": p.default_model.agent,
            "Model": p.default_model.model,
            "Credential": p.default_model.credential,
        },
        "Scripts": scripts,
        "UsageLimit": {"Resume": match p.usage_limit { Resume::Auto => "auto", Resume::Manual => "manual" }},
        "Compaction": {
            "Idle": match p.compaction.idle {
                Idle::Off => "off",
                Idle::OnReturn => "onReturn",
                Idle::BeforeCacheExpires => "beforeCacheExpires",
            },
            "AutoCompactWindow": p.compaction.auto_compact_window,
        },
        "Workspace": {
            "Host": p.workspace.host,
            "Architecture": match p.workspace.architecture {
                Architecture::Arm64 => "arm64",
                Architecture::X86_64 => "x86_64",
            },
            "IdleSeconds": p.workspace.idle_seconds,
        },
    })
}

/// The text of the `SKILL.md` of a skill in the files of a revision, or of a built-in skill.
fn skill_document(files: &BTreeMap<String, String>, name: &str, builtin: bool) -> String {
    if builtin {
        return BUILTIN_SKILLS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, document, _)| document.to_string())
            .unwrap_or_default();
    }
    files
        .get(&format!("skills/{name}/SKILL.md"))
        .cloned()
        .unwrap_or_default()
}

/// Validates the files of a revision and makes them the active Config of the User.
#[allow(clippy::too_many_arguments)]
pub async fn activate(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    repository_id: u64,
    repository: &str,
    revision: &str,
    files: &BTreeMap<String, String>,
    outside: &Outside,
    now: u64,
) -> Result<(), ActivationError> {
    let config = sanchoris_config::validate(files, outside).map_err(ActivationError::Invalid)?;
    let mut entities: Vec<Write> = Vec::new();
    for (name, p) in &config.projects {
        let project = Project {
            repository_id,
            name: name.clone(),
            definition: project_json(p),
            file: sanchoris_config::CONFIG_FILE.into(),
        };
        entities.push(Box::new(move |tx| tx.put_entity(&project, None)));
        for (repo_name, repo) in &p.repos {
            let r = Repository {
                config_repository_id: repository_id,
                project: name.clone(),
                name: repo_name.clone(),
                repo: format!("github.com/{repo}"),
            };
            entities.push(Box::new(move |tx| tx.put_entity(&r, None)));
        }
    }
    for (name, skill) in &config.skills {
        let document = skill_document(files, name, skill.builtin);
        let body = json!({
            "document": document,
            "source": skill.definition.as_ref().map(|d| d.source.clone()),
        });
        let bytes = serde_json::to_vec(&body).map_err(|e| DbError::Invalid(e.to_string()))?;
        let key = content_key(user, "skills", &bytes);
        blobs.put(&key, bytes).await?;
        let inputs: Map<String, Value> = skill
            .definition
            .as_ref()
            .map(|d| {
                d.inputs
                    .iter()
                    .map(|(k, t)| (k.clone(), json!(input_type_name(*t))))
                    .collect()
            })
            .unwrap_or_default();
        let s = Skill {
            repository_id,
            name: name.clone(),
            predictable: skill.definition.is_some(),
            builtin: skill.builtin,
            description: skill.document.description.clone(),
            inputs: Value::Object(inputs),
            definition_key: Some(key),
            file: if skill.builtin {
                format!("builtin:{name}")
            } else {
                format!("skills/{name}/SKILL.md")
            },
        };
        entities.push(Box::new(move |tx| tx.put_entity(&s, None)));
    }
    let c = Config {
        repository_id,
        repository: repository.to_string(),
        root: String::new(),
        state: "active".into(),
        revision: Some(revision.to_string()),
        synced_at: Some(now),
        last_sync_error: None,
        validation: "valid".into(),
        validation_messages: Vec::new(),
    };
    entities.push(Box::new(move |tx| tx.put_entity(&c, None)));
    let mut entities = entities.into_iter().peekable();
    while entities.peek().is_some() {
        let mut tx = db.tx(user);
        for write in entities.by_ref().take(CHUNK) {
            write(&mut tx)?;
        }
        tx.commit().await?;
    }
    Ok(())
}

/// The active Config of a User: the first that is `active`.
pub async fn active_config(db: &Db, user: &str) -> Result<Option<Config>, DbError> {
    Ok(db
        .entities::<Config>(user, "CONFIG#")
        .await?
        .into_iter()
        .find(|c| c.state == "active"))
}

/// The document and definition of a Skill from S3: `document` (the text of `SKILL.md`) and
/// `source` (the `skill.yaml` as JSON, or null).
pub async fn skill_body(blobs: &Blobs, skill: &Skill) -> Result<Value, DbError> {
    let Some(key) = &skill.definition_key else {
        return Ok(json!({"document": "", "source": null}));
    };
    let bytes = blobs
        .get(key)
        .await?
        .ok_or_else(|| DbError::Invalid(format!("no object {key}")))?;
    serde_json::from_slice(&bytes).map_err(|e| DbError::Invalid(e.to_string()))
}
