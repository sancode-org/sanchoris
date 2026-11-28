//! Threads that a User starts and the messages a User sends (`skill.md`「スレッド」「User の
//! 起動」): the copy of the Project and the callable Skills at the start (`Definition`), the
//! instructions of sanchoris, and the SkillRuns that a User launches with `/<name>`.

use sanchoris_engine::{Engine, Origin, Response, StartRequest};
use serde_json::{Map, Value, json};

use crate::activation::{active_config, skill_body};
use crate::blob::{Blobs, user_prefix};
use crate::db::{Cond, Db, DbError, n};
use crate::engine_store::DynamoStore;
use crate::entities::{Message, Project, Skill, Thread, keys};
use crate::generated::sanchoris_thread as lean;
use crate::thread::{self, Delivered, NewThread, ThreadError};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LaunchError {
    #[error(transparent)]
    Thread(#[from] ThreadError),
    #[error("no active Config")]
    NoConfig,
    #[error("no Project {0}")]
    NoProject(String),
    #[error("{code}: {message}")]
    Skill { code: String, message: String },
}

impl From<DbError> for LaunchError {
    fn from(e: DbError) -> Self {
        LaunchError::Thread(ThreadError::Db(e))
    }
}

/// The key in S3 of the `Definition` of a thread.
pub fn definition_key(user: &str, thread: &str) -> String {
    format!("{}threads/{thread}/definition.json", user_prefix(user))
}

/// The `Definition` of a thread: the Project and the callable Skills of the active Config, with
/// their documents and definitions.
pub async fn definition_copy(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    project: &str,
) -> Result<(Value, Option<String>), LaunchError> {
    let config = active_config(db, user)
        .await?
        .ok_or(LaunchError::NoConfig)?;
    let p: Project = db
        .entity(&keys::project(user, config.repository_id, project))
        .await?
        .ok_or_else(|| LaunchError::NoProject(project.to_string()))?;
    let mut skills = Vec::new();
    for s in db
        .entities::<Skill>(user, &format!("CONFIG#{}#SKILL#", config.repository_id))
        .await?
    {
        let body = skill_body(blobs, &s).await?;
        skills.push(json!({
            "name": s.name,
            "predictable": s.predictable,
            "builtin": s.builtin,
            "description": s.description,
            "inputs": s.inputs,
            "document": body["document"],
            "source": body["source"],
        }));
    }
    let mut project = p.definition.clone();
    project["Name"] = json!(p.name);
    Ok((
        json!({
            "config": {"repositoryId": config.repository_id, "repository": config.repository},
            "project": project,
            "skills": skills,
        }),
        config.revision,
    ))
}

pub async fn read_definition(blobs: &Blobs, t: &Thread) -> Result<Value, DbError> {
    let key = t
        .definition_key
        .as_deref()
        .ok_or_else(|| DbError::Invalid(format!("thread {} has no Definition", t.name)))?;
    let bytes = blobs
        .get(key)
        .await?
        .ok_or_else(|| DbError::Invalid(format!("no object {key}")))?;
    serde_json::from_slice(&bytes).map_err(|e| DbError::Invalid(e.to_string()))
}

/// The predictable Skills of a `Definition`.
pub fn predictable(definition: &Value) -> Vec<&Value> {
    definition["skills"]
        .as_array()
        .map(|skills| {
            skills
                .iter()
                .filter(|s| s["predictable"] == json!(true))
                .collect()
        })
        .unwrap_or_default()
}

/// The values of `Inputs` that a thread decides (`skill.md`「スレッドに送る指示」): `Repos` (array)
/// is the names of the `Repos` of the Project. `Repo` and `Issue` come from the issue the thread
/// started from.
pub fn thread_inputs(definition: &Value, skill: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(inputs) = skill["inputs"].as_object() {
        for (name, t) in inputs {
            if name == "Repos" && t == "array" {
                let repos: Vec<Value> = definition["project"]["Repos"]
                    .as_object()
                    .map(|m| m.keys().map(|k| json!(k)).collect())
                    .unwrap_or_default();
                out.insert(name.clone(), Value::Array(repos));
            }
        }
    }
    out
}

/// The first instruction of a thread (`skill.md`「スレッドに送る指示」, 開始).
pub fn start_instruction(t: &Thread, definition: &Value, message: &str) -> String {
    let starter = match t.origin.strip_prefix("trigger:") {
        Some(trigger) => format!("Trigger {trigger}"),
        None => "The user".to_string(),
    };
    let mut lines = vec![
        format!("[sanchoris] {starter} started this thread: {}.", t.title),
        format!("Message: {message}"),
    ];
    let skills = predictable(definition);
    if !skills.is_empty() {
        let names: Vec<&str> = skills.iter().filter_map(|s| s["name"].as_str()).collect();
        lines.push(format!(
            "Skills: {}. Run them with `sanchoris-agent skill run <name>`.",
            names.join(", ")
        ));
    }
    for skill in skills {
        let Some(inputs) = skill["inputs"].as_object() else {
            continue;
        };
        if inputs.is_empty() {
            continue;
        }
        let name = skill["name"].as_str().unwrap_or_default();
        let decided = thread_inputs(definition, skill);
        let described: Vec<String> = inputs
            .iter()
            .map(|(input, t)| {
                let t = t.as_str().unwrap_or_default();
                match decided.get(input) {
                    Some(v) => format!("{input} ({t}) = {v}"),
                    None => format!("{input} ({t})"),
                }
            })
            .collect();
        let mut line = format!("Inputs of {name}: {}.", described.join(", "));
        if decided.len() == inputs.len() {
            line.push_str(&format!(
                " Run it with `sanchoris-agent skill run {name} --input '{}'`.",
                Value::Object(decided)
            ));
        }
        lines.push(line);
    }
    lines.push(
        "To run part of the work in parallel, start a child thread with `sanchoris-agent thread start --message '<task>'`."
            .into(),
    );
    lines.join("\n")
}

/// A message of a person as an input of the thread.
pub fn user_message(text: &str) -> String {
    format!("[sanchoris] Message from the user:\n{text}")
}

/// The failure of a `RunOnCreate` script as an input of the thread.
pub fn script_failed(name: &str, exit_code: i64, timed_out: bool, stderr_tail: &str) -> String {
    let how = if timed_out {
        "timed out".to_string()
    } else {
        format!("failed. Exit {exit_code}")
    };
    let mut text = format!("[sanchoris] Script {name} (RunOnCreate) {how}.");
    if !stderr_tail.is_empty() {
        text.push_str(&format!(
            "\nstderr: {}",
            sanchoris_engine::notice::tail(stderr_tail)
        ));
    }
    text
}

/// What a message of a User is (`skill.md`「User の起動」).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `/compact`: the compact of the Worker.
    Compact,
    /// `/<name>` of a predictable Skill of the thread, with the JSON object after it.
    Launch {
        name: String,
        input: String,
    },
    Message,
}

/// Reads a message with `command` (`Sanchoris.Thread`): the name after a leading `/` and the
/// text after the name, over the predictable Skills of the thread. The text after the name of a
/// launch is the JSON object of its `Inputs`.
pub fn classify(text: &str, skills: &[&str]) -> Command {
    let text = text.trim();
    let (name, input) = match text.strip_prefix('/') {
        Some(rest) => match rest.find(char::is_whitespace) {
            Some(i) => (Some(&rest[..i]), rest[i..].trim()),
            None => (Some(rest), ""),
        },
        None => (None, ""),
    };
    match lean::command(
        name.map(str::to_string),
        !input.is_empty(),
        skills.iter().map(|s| s.to_string()).collect(),
    ) {
        lean::Command::Compact => Command::Compact,
        lean::Command::Launch { name } => Command::Launch {
            name,
            input: input.to_string(),
        },
        lean::Command::Message => Command::Message,
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, classify};

    #[test]
    fn a_message_is_a_compact_a_launch_or_a_message() {
        let skills = ["where", "open-pr-green-ci"];
        assert_eq!(classify("/compact", &skills), Command::Compact);
        assert_eq!(classify("/compact now", &skills), Command::Message);
        assert_eq!(
            classify(" /where ", &skills),
            Command::Launch {
                name: "where".into(),
                input: String::new()
            }
        );
        assert_eq!(
            classify("/open-pr-green-ci {\"Repo\": \"app\"}", &skills),
            Command::Launch {
                name: "open-pr-green-ci".into(),
                input: "{\"Repo\": \"app\"}".into()
            }
        );
        assert_eq!(classify("/review", &skills), Command::Message);
        assert_eq!(classify("where", &skills), Command::Message);
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Null => "null",
    }
}

/// The input of a SkillRun that a User launches: the JSON object after the name over the values
/// the thread decides, checked against `Inputs` as `sanchoris-agent skill run` checks it.
pub fn launch_input(definition: &Value, skill: &Value, text: &str) -> Result<Value, LaunchError> {
    let name = skill["name"].as_str().unwrap_or_default();
    let invalid = |message: String| LaunchError::Skill {
        code: "invalid_input".into(),
        message,
    };
    let given: Map<String, Value> = if text.is_empty() {
        Map::new()
    } else {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m,
            _ => {
                return Err(invalid(format!(
                    "The input of {name} must be a JSON object"
                )));
            }
        }
    };
    let mut input = thread_inputs(definition, skill);
    input.extend(given);
    let inputs = skill["inputs"].as_object().cloned().unwrap_or_default();
    let mut problems = Vec::new();
    for (field, expected) in &inputs {
        match input.get(field) {
            None => problems.push(format!(
                "{field} is required ({})",
                expected.as_str().unwrap_or_default()
            )),
            Some(v) if Some(type_of(v)) != expected.as_str() => problems.push(format!(
                "{field} must be {}, got {}",
                expected.as_str().unwrap_or_default(),
                type_of(v)
            )),
            Some(_) => {}
        }
    }
    for field in input.keys() {
        if !inputs.contains_key(field) {
            problems.push(format!("{field} is not an input of {name}"));
        }
    }
    if !problems.is_empty() {
        return Err(invalid(format!(
            "Invalid input for {name}: {}",
            problems.join("; ")
        )));
    }
    Ok(Value::Object(input))
}

fn random_name() -> String {
    format!("th_{}", &crate::token::random_id()[..20])
}

/// Starts a thread from the screen (New thread), on the `DefaultModel` of the Project. A first
/// message `/<name>` also launches the Skill in the thread.
pub async fn start_thread(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    project: &str,
    message: &str,
    now: u64,
) -> Result<String, LaunchError> {
    let (definition, revision) = definition_copy(db, blobs, user, project).await?;
    let name = random_name();
    let key = definition_key(user, &name);
    blobs
        .put(
            &key,
            serde_json::to_vec(&definition).map_err(|e| DbError::Invalid(e.to_string()))?,
        )
        .await?;
    let model = &definition["project"]["DefaultModel"];
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let new = NewThread {
        name: name.clone(),
        project: project.to_string(),
        message: message.to_string(),
        title: message.lines().next().unwrap_or_default().to_string(),
        agent: text(&model["Agent"]),
        model: text(&model["Model"]),
        credential: text(&model["Credential"]),
        workspace_host: text(&definition["project"]["Workspace"]["Host"]),
        origin: "web".into(),
        requester: user.to_string(),
        parent: None,
        blocked_by: Vec::new(),
        trigger_key: None,
        definition_key: Some(key),
        config_revision: revision,
    };
    let mut tx = db.tx(user);
    thread::insert_thread(&mut tx, &new, now)?;
    tx.commit().await?;
    let names: Vec<&str> = predictable(&definition)
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    if let Command::Launch { name: skill, input } = classify(message, &names) {
        launch(
            db,
            blobs,
            user,
            &name,
            &definition,
            &skill,
            &input,
            &format!("{name}-1"),
            now,
        )
        .await?;
    }
    Ok(name)
}

/// Launches a predictable Skill in a thread for the User (`skill.md`「User の起動」): it runs in
/// the Workspace and on the branch of the thread, its origin is `user`, and its parent follows
/// `parentOf`.
#[allow(clippy::too_many_arguments)]
pub async fn launch(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    thread: &str,
    definition: &Value,
    skill: &str,
    input: &str,
    request: &str,
    now: u64,
) -> Result<Value, LaunchError> {
    let s = predictable(definition)
        .into_iter()
        .find(|s| s["name"] == json!(skill))
        .ok_or_else(|| LaunchError::Skill {
            code: "unknown_skill".into(),
            message: format!("Skill {skill} is not available in this thread"),
        })?;
    let input = launch_input(definition, s, input)?;
    start_run(
        db,
        blobs,
        user,
        thread,
        s,
        input,
        request,
        Origin::User,
        now,
    )
    .await
}

/// Starts a SkillRun of a Skill of the `Definition` of a thread.
#[allow(clippy::too_many_arguments)]
pub async fn start_run(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    thread: &str,
    skill: &Value,
    input: Value,
    request: &str,
    origin: Origin,
    now: u64,
) -> Result<Value, LaunchError> {
    let name = skill["name"].as_str().unwrap_or_default();
    let source = skill["source"].clone();
    let definition =
        sanchoris_engine::definition(name, &source).map_err(|e| LaunchError::Skill {
            code: "invalid_skill".into(),
            message: e.to_string(),
        })?;
    let mut engine = Engine::new(DynamoStore::new(db.clone(), blobs.clone(), user));
    let response = engine
        .start(
            StartRequest {
                request: request.to_string(),
                thread: thread.to_string(),
                origin,
                definition,
                source,
                input,
            },
            now,
        )
        .await
        .map_err(|e| LaunchError::Skill {
            code: "engine".into(),
            message: e.to_string(),
        })?;
    match response {
        Response::Ok(v) => Ok(v),
        Response::Error { code, message } => Err(LaunchError::Skill { code, message }),
    }
}

/// Adds a Message to a thread over the version it read.
pub async fn add_message(
    db: &Db,
    user: &str,
    thread: &str,
    sender: &str,
    text: &str,
    now: u64,
) -> Result<u64, ThreadError> {
    for _ in 0..20 {
        let mut t: Thread = db
            .entity(&keys::thread(user, thread))
            .await?
            .ok_or_else(|| ThreadError::Missing(thread.to_string()))?;
        let version = t.version;
        let seq = push_message(&mut t, sender, text, now);
        t.version += 1;
        t.updated_at = now;
        let mut tx = db.tx(user);
        tx.insert_entity(&seq.1)?;
        tx.put_entity(&t, Some(Cond::eq("version", n(version))))?;
        match tx.commit().await {
            Ok(()) => return Ok(seq.0),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ThreadError::Busy(thread.to_string()))
}

/// The next Message of a thread, counted on the thread.
pub fn push_message(t: &mut Thread, sender: &str, text: &str, now: u64) -> (u64, Message) {
    t.messages += 1;
    (
        t.messages,
        Message {
            thread: t.name.clone(),
            seq: t.messages,
            sender: sender.to_string(),
            text: text.to_string(),
            at: now,
        },
    )
}

/// What a message of a User did.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    Input(Delivered),
    Launched(Value),
    Compact,
}

/// A message of a User to a thread: `/compact` compacts, `/<name>` launches a Skill, and any
/// other message is an input of the thread. Each is a Message of the thread.
pub async fn send_message(
    db: &Db,
    blobs: &Blobs,
    user: &str,
    thread: &str,
    text: &str,
    now: u64,
) -> Result<Sent, LaunchError> {
    let t: Thread = db
        .entity(&keys::thread(user, thread))
        .await?
        .ok_or_else(|| ThreadError::Missing(thread.to_string()))?;
    if t.status == crate::entities::ThreadStatus::Aborted {
        return Err(ThreadError::Aborted(thread.to_string()).into());
    }
    let definition = read_definition(blobs, &t).await?;
    let names: Vec<&str> = predictable(&definition)
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    match classify(text, &names) {
        Command::Compact => {
            add_message(db, user, thread, "user", text, now).await?;
            if thread::has_workspace(db, user, &t).await? {
                crate::outbox::push(
                    db,
                    user,
                    thread,
                    &[json!({"type": "compact", "threadId": thread})],
                )
                .await?;
            }
            Ok(Sent::Compact)
        }
        Command::Launch { name, input } => {
            let seq = add_message(db, user, thread, "user", text, now).await?;
            let result = launch(
                db,
                blobs,
                user,
                thread,
                &definition,
                &name,
                &input,
                &format!("{thread}-{seq}"),
                now,
            )
            .await?;
            Ok(Sent::Launched(result))
        }
        Command::Message => {
            let owned = text.to_string();
            let extra = move |t: &mut Thread, tx: &mut crate::db::UserTx| {
                let (_, m) = push_message(t, "user", &owned, now);
                tx.insert_entity(&m)
            };
            let input = json!({
                "id": format!("{thread}-m{}", t.messages + 1),
                "source": "person",
                "text": user_message(text),
            });
            let delivered = thread::send_input_with(
                db,
                user,
                thread,
                lean::Source::Person,
                input,
                now,
                Some(&extra),
                None,
            )
            .await?;
            Ok(Sent::Input(delivered))
        }
    }
}
