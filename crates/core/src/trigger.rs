//! Trigger evaluation (`skill.md`「Trigger」): an event against the Triggers of a Config, with
//! the event pattern, `InputPathsMap`, the thread key, the limit of 60 threads an hour, and the
//! Trigger event that records the result.
//!
//! One delivery makes one Trigger event per Trigger, written with a condition that it is new,
//! so a second delivery of the same event changes nothing. A thread starts in one transaction
//! with its Trigger event, its thread key and the count of the hour.

use std::collections::BTreeMap;

use sanchoris_config::{Config, TriggerOn};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::db::{Change, Cond, Db, DbError, get_n, get_s, item_of, n, s};
use crate::entities::{BlockedBy, TriggerEvent, TriggerKey, TriggerOutcome, keys};
use crate::pattern::EventPattern;
use crate::thread::{NewThread, insert_thread};

/// The threads a Trigger starts in an hour (UTC).
pub const STARTS_PER_HOUR: i64 = 60;
const HOUR_MS: u64 = 3_600_000;
const ATTEMPTS: usize = 20;

/// An event of the `github` source, with the ID of the repository it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// `X-GitHub-Delivery`, or the scheduled time for a schedule.
    pub delivery: String,
    pub repository_id: u64,
    /// `source`, `detail-type` and `detail`.
    pub event: Value,
}

impl Event {
    pub fn detail_type(&self) -> &str {
        self.event["detail-type"].as_str().unwrap_or_default()
    }
}

/// The thread key of an event (`skill.md`「Trigger」のスレッドの鍵).
pub fn thread_key(event: &Event) -> Result<String, String> {
    let detail = &event.event["detail"];
    let repo = event.repository_id;
    match event.detail_type() {
        "Issue Labeled" | "Issue Comment Created" => detail["issue"]["number"]
            .as_u64()
            .map(|number| format!("github:{repo}#{number}"))
            .ok_or_else(|| "the event has no issue number".to_string()),
        "Check Run Completed" => detail["check_run"]["id"]
            .as_u64()
            .map(|id| format!("github:{repo}:check:{id}"))
            .ok_or_else(|| "the event has no check run ID".to_string()),
        other => Err(format!("{other} is not an event of a Trigger")),
    }
}

/// The thread key of a schedule at a time.
pub fn schedule_key(at: &str) -> String {
    format!("schedule:{at}")
}

/// A value of the event at a path of `InputPathsMap` (`$.a.b[0]`).
pub fn select<'a>(event: &'a Value, path: &str) -> Result<&'a Value, String> {
    let mut rest = path
        .strip_prefix('$')
        .ok_or_else(|| format!("{path} does not start with $"))?;
    let mut current = event;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            if end == 0 {
                return Err(format!("{path} has an empty key"));
            }
            current = current
                .get(&after[..end])
                .ok_or_else(|| format!("{path} is not in the event"))?;
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after
                .find(']')
                .ok_or_else(|| format!("{path} has an unclosed ["))?;
            let index: usize = after[..end]
                .parse()
                .map_err(|_| format!("{path} has an index that is not a number"))?;
            current = current
                .get(index)
                .ok_or_else(|| format!("{path} is not in the event"))?;
            rest = &after[end + 1..];
        } else {
            return Err(format!("{path} is not a path"));
        }
    }
    Ok(current)
}

/// `Start.Message` with each `<name>` of `InputPathsMap` replaced by its value: a string as it
/// is, another value as JSON.
pub fn render(
    message: &str,
    paths: &indexmap::IndexMap<String, String>,
    event: &Value,
) -> Result<String, String> {
    let mut values = Map::new();
    for (name, path) in paths {
        values.insert(name.clone(), select(event, path)?.clone());
    }
    let mut out = String::new();
    let mut rest = message;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('>') {
            Some(end) if values.contains_key(&after[..end]) => {
                match &values[&after[..end]] {
                    Value::String(s) => out.push_str(s),
                    other => out.push_str(&other.to_string()),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The issue dependencies of GitHub, read when a thread starts.
pub trait Dependencies {
    /// The issues that block an issue, or why they could not be read.
    fn blocked_by(
        &self,
        repository_id: u64,
        issue: u64,
    ) -> impl Future<Output = Result<Vec<BlockedBy>, String>> + Send;
}

/// No dependencies.
pub struct NoDependencies;

impl Dependencies for NoDependencies {
    async fn blocked_by(&self, _: u64, _: u64) -> Result<Vec<BlockedBy>, String> {
        Ok(Vec::new())
    }
}

/// The Config the Triggers come from.
pub struct Source<'a> {
    pub repository_id: u64,
    pub config: &'a Config,
    /// The IDs of the `owner/name` of `detail.repository` of the patterns, from the sync.
    pub repositories: &'a BTreeMap<String, u64>,
    pub revision: Option<String>,
    /// The `RetentionDays` of the Plan of the User, which a thread key is kept for.
    pub retention_days: u64,
}

/// What a Trigger did with an event, and whether it was the first delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluated {
    pub trigger: String,
    pub outcome: TriggerOutcome,
    /// The delivery was evaluated before; nothing was written.
    pub duplicate: bool,
}

fn matches(pattern: &Value, source: &Source, event: &Event) -> Result<bool, String> {
    let mut pattern = pattern.clone();
    let repositories = pattern
        .get_mut("detail")
        .and_then(|d| d.as_object_mut())
        .and_then(|d| d.remove("repository"));
    if let Some(names) = repositories {
        let ids: Vec<u64> = names
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str())
            .filter_map(|n| source.repositories.get(n).copied())
            .collect();
        if !ids.contains(&event.repository_id) {
            return Ok(false);
        }
    }
    if pattern["detail"].as_object().is_some_and(Map::is_empty) {
        pattern.as_object_mut().map(|p| p.remove("detail"));
    }
    let parsed = EventPattern::parse(&pattern).map_err(|e| e.to_string())?;
    Ok(parsed.matches(&event.event))
}

/// The name of the thread a Trigger starts for a delivery.
pub fn thread_name(source: &Source, trigger: &str, delivery: &str) -> String {
    let digest = Sha256::digest(format!("{}\0{trigger}\0{delivery}", source.repository_id));
    let hex: String = digest.iter().take(10).map(|b| format!("{b:02x}")).collect();
    format!("th_{hex}")
}

/// Evaluates an event against every Trigger of a Config with an event pattern.
pub async fn evaluate<D: Dependencies>(
    db: &Db,
    user: &str,
    source: &Source<'_>,
    event: &Event,
    dependencies: &D,
    now: u64,
) -> Result<Vec<Evaluated>, DbError> {
    let mut results = Vec::new();
    for (name, trigger) in &source.config.triggers {
        let TriggerOn::Event(pattern) = &trigger.on else {
            continue;
        };
        if !trigger.enabled {
            continue;
        }
        let matched = matches(pattern, source, event);
        if matched == Ok(false) {
            continue;
        }
        results.push(
            evaluate_one(
                db,
                user,
                source,
                name,
                matched.err(),
                event,
                dependencies,
                now,
            )
            .await?,
        );
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_one<D: Dependencies>(
    db: &Db,
    user: &str,
    source: &Source<'_>,
    name: &str,
    pattern_error: Option<String>,
    event: &Event,
    dependencies: &D,
    now: u64,
) -> Result<Evaluated, DbError> {
    let trigger = &source.config.triggers[name];
    let repo = source.repository_id;
    let event_key = keys::trigger_event(user, repo, name, &event.delivery);
    let record = |outcome: TriggerOutcome, key: Option<String>| TriggerEvent {
        repository_id: repo,
        trigger: name.to_string(),
        received_at: now,
        delivery: event.delivery.clone(),
        event: event.event.clone(),
        thread_key: key,
        result: outcome,
    };
    for _ in 0..ATTEMPTS {
        if let Some(seen) = db.entity::<TriggerEvent>(&event_key).await? {
            return Ok(Evaluated {
                trigger: name.to_string(),
                outcome: seen.result,
                duplicate: true,
            });
        }
        let prepared = pattern_error.clone().map_or_else(
            || {
                let key = thread_key(event)?;
                let message = render(
                    &trigger.start.message,
                    &trigger.input_paths_map,
                    &event.event,
                )?;
                Ok((key, message))
            },
            Err,
        );
        let (key, message) = match prepared {
            Ok(p) => p,
            Err(reason) => {
                let outcome = TriggerOutcome::DroppedEvaluation { reason };
                match write_event(db, user, &record(outcome.clone(), None)).await {
                    Ok(()) => return Ok(evaluated(name, outcome)),
                    Err(DbError::Condition { .. } | DbError::Conflict) => continue,
                    Err(e) => return Err(e),
                }
            }
        };
        let tkey = keys::thread_key(user, repo, name, &key);
        if let Some(item) = db.get(&tkey).await?
            && get_n(&item, "expires_at").is_none_or(|at| at as u64 * 1000 > now)
        {
            let thread = get_s(&item, "thread").unwrap_or_default().to_string();
            let outcome = TriggerOutcome::SkippedExisting { thread };
            match write_event(db, user, &record(outcome.clone(), Some(key))).await {
                Ok(()) => return Ok(evaluated(name, outcome)),
                Err(DbError::Condition { .. } | DbError::Conflict) => continue,
                Err(e) => return Err(e),
            }
        }
        let hour = now / HOUR_MS;
        let hour_key = keys::trigger_hour(user, repo, name, hour);
        let started = db
            .get(&hour_key)
            .await?
            .and_then(|i| get_n(&i, "started"))
            .unwrap_or(0);
        if started >= STARTS_PER_HOUR {
            let outcome = TriggerOutcome::DroppedLimit {
                reason: format!(
                    "Trigger {name} started {STARTS_PER_HOUR} threads in the hour from {}",
                    hour * HOUR_MS
                ),
            };
            match write_event(db, user, &record(outcome.clone(), Some(key))).await {
                Ok(()) => return Ok(evaluated(name, outcome)),
                Err(DbError::Condition { .. } | DbError::Conflict) => continue,
                Err(e) => return Err(e),
            }
        }
        let detail = &event.event["detail"];
        let blocked_by = match event.detail_type() {
            "Issue Labeled" | "Issue Comment Created" => {
                let issue = detail["issue"]["number"].as_u64().unwrap_or_default();
                match dependencies.blocked_by(repo, issue).await {
                    Ok(b) => b,
                    Err(reason) => {
                        let outcome = TriggerOutcome::SkippedDependencies { reason };
                        match write_event(db, user, &record(outcome.clone(), Some(key))).await {
                            Ok(()) => return Ok(evaluated(name, outcome)),
                            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
                            Err(e) => return Err(e),
                        }
                    }
                }
            }
            _ => Vec::new(),
        };
        let thread = thread_name(source, name, &event.delivery);
        let project = &source.config.projects[&trigger.start.project];
        let model = trigger
            .start
            .model
            .clone()
            .unwrap_or_else(|| project.default_model.clone());
        let title = detail["issue"]["title"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| message.lines().next().unwrap_or_default().to_string());
        let new = NewThread {
            name: thread.clone(),
            project: trigger.start.project.clone(),
            message,
            title,
            agent: model.agent,
            model: model.model,
            credential: model.credential,
            workspace_host: project.workspace.host.clone(),
            origin: format!("trigger:{name}"),
            requester: detail["sender"]["login"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            parent: None,
            blocked_by,
            trigger_key: Some(TriggerKey {
                repository_id: repo,
                trigger: name.to_string(),
                key: key.clone(),
            }),
            definition_key: None,
            config_revision: source.revision.clone(),
        };
        let outcome = TriggerOutcome::Started {
            thread: thread.clone(),
        };
        let mut tx = db.tx(user);
        tx.insert_entity(&record(outcome.clone(), Some(key)))?;
        let mut held = item_of(&tkey, "thread_key", &json!({ "thread": thread }))?;
        held.insert(
            "expires_at".into(),
            n((now / 1000) + source.retention_days * 86_400),
        );
        tx.put(
            crate::db::TableKey::Main,
            held,
            Some(Cond::not_exists().or(Cond::lt("expires_at", (now / 1000) as i64))),
        )?;
        let mut counter = Change::new()
            .add("started", 1)
            .set("expires_at", n((hour + 2) * 3600))
            .when(Cond::missing_or_lt("started", STARTS_PER_HOUR));
        if started == 0 {
            counter = counter.set("kind", s("trigger_hour"));
        }
        tx.update(&hour_key, counter)?;
        insert_thread(&mut tx, &new, now)?;
        match tx.commit().await {
            Ok(()) => return Ok(evaluated(name, outcome)),
            Err(DbError::Condition { .. } | DbError::Conflict) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(DbError::Conflict)
}

fn evaluated(trigger: &str, outcome: TriggerOutcome) -> Evaluated {
    Evaluated {
        trigger: trigger.to_string(),
        outcome,
        duplicate: false,
    }
}

async fn write_event(db: &Db, user: &str, event: &TriggerEvent) -> Result<(), DbError> {
    let mut tx = db.tx(user);
    tx.insert_entity(event)?;
    tx.commit().await
}
