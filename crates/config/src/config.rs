use indexmap::IndexMap;
use serde_yaml_ng::{Mapping, Value};

use crate::event_pattern;
use crate::issue::{Check, join};
use crate::reader::Reader;
use crate::schedule::{self, Schedule};

/// The architecture of a workspace or a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
    X86_64,
}

const ARCHITECTURES: [(&str, Architecture); 2] = [
    ("arm64", Architecture::Arm64),
    ("x86_64", Architecture::X86_64),
];

/// What a thread does when the usage limit of its worker is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Auto,
    Manual,
}

/// When an idle thread is compacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    Off,
    OnReturn,
    BeforeCacheExpires,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub agent: String,
    pub model: String,
    pub credential: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub command: String,
    pub timeout_seconds: u64,
    pub run_on_create: bool,
    pub run_async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compaction {
    pub idle: Idle,
    /// `None` uses the default of the provider.
    pub auto_compact_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// `cloud` or the name of a registered host.
    pub host: String,
    pub architecture: Architecture,
    pub idle_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Repository names to `owner/name`.
    pub repos: IndexMap<String, String>,
    pub default_model: Model,
    pub scripts: IndexMap<String, Script>,
    pub usage_limit: Resume,
    pub compaction: Compaction,
    pub workspace: Workspace,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerOn {
    Event(serde_json::Value),
    Schedule(Schedule),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub project: String,
    pub message: String,
    pub model: Option<Model>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub enabled: bool,
    pub on: TriggerOn,
    pub input_paths_map: IndexMap<String, String>,
    pub start: Start,
}

/// The projects and triggers of `sanchoris.yaml`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConfigFile {
    /// The names of every project written, also of those with issues.
    pub(crate) project_names: Vec<String>,
    pub(crate) projects: IndexMap<String, Project>,
    pub(crate) triggers: IndexMap<String, Trigger>,
}

/// Reads `sanchoris.yaml` and checks its required items and formats. The references between
/// items and to the outside are checked by the caller.
pub(crate) fn read(r: &mut Reader, text: &str) -> Option<ConfigFile> {
    let value = r.parse(text)?;
    let root = r.mapping(&value, "")?;
    let version = r.get(root, "", "Version", true);
    if version.is_some() && version.and_then(Value::as_u64) != Some(1) {
        r.format("Version", "must be 1");
    }
    let mut projects = IndexMap::new();
    let mut project_names = Vec::new();
    if let Some(m) = section(r, root, "Projects") {
        for (name, v) in r.entries(m, "Projects") {
            project_names.push(name.clone());
            if let Some(project) = project(r, &join("Projects", &name), v) {
                projects.insert(name, project);
            }
        }
    }
    let mut triggers = IndexMap::new();
    if let Some(m) = section(r, root, "Triggers") {
        for (name, v) in r.entries(m, "Triggers") {
            if let Some(trigger) = trigger(r, &join("Triggers", &name), v) {
                triggers.insert(name, trigger);
            }
        }
    }
    Some(ConfigFile {
        project_names,
        projects,
        triggers,
    })
}

fn section<'v>(r: &mut Reader, root: &'v Mapping, key: &str) -> Option<&'v Mapping> {
    let v = r.get(root, "", key, true)?;
    r.mapping(v, key)
}

/// A required mapping item.
fn required_mapping<'v>(
    r: &mut Reader,
    m: &'v Mapping,
    path: &str,
    key: &str,
) -> Option<&'v Mapping> {
    let v = r.get(m, path, key, true)?;
    r.mapping(v, &join(path, key))
}

fn required_string(r: &mut Reader, m: &Mapping, path: &str, key: &str) -> Option<String> {
    let v = r.get(m, path, key, true);
    r.string(v, &join(path, key))
}

fn model(r: &mut Reader, m: &Mapping, path: &str) -> Option<Model> {
    let agent = required_string(r, m, path, "Agent");
    let model = required_string(r, m, path, "Model");
    let credential = required_string(r, m, path, "Credential");
    Some(Model {
        agent: agent?,
        model: model?,
        credential: credential?,
    })
}

/// `github.com/owner/name` as `owner/name`.
fn repository(r: &mut Reader, value: Option<&Value>, path: &str) -> Option<String> {
    let text = r.string(value, path)?;
    match text.strip_prefix("github.com/") {
        Some(name) if event_pattern::is_owner_name(name) => Some(name.to_string()),
        _ => {
            r.format(path, "must be github.com/owner/name");
            None
        }
    }
}

fn project(r: &mut Reader, path: &str, value: &Value) -> Option<Project> {
    let m = r.mapping(value, path)?;
    let mut repos = IndexMap::new();
    let mut repos_ok = true;
    if let Some(repos_map) = required_mapping(r, m, path, "Repos") {
        let repos_path = join(path, "Repos");
        for (name, v) in r.entries(repos_map, &repos_path) {
            let repo_path = join(&repos_path, &name);
            let repo = r.mapping(v, &repo_path).and_then(|rm| {
                let v = r.get(rm, &repo_path, "Repo", true);
                repository(r, v, &join(&repo_path, "Repo"))
            });
            match repo {
                Some(repo) => {
                    repos.insert(name, repo);
                }
                None => repos_ok = false,
            }
        }
    } else {
        repos_ok = false;
    }
    let default_model = required_mapping(r, m, path, "DefaultModel")
        .and_then(|dm| model(r, dm, &join(path, "DefaultModel")));
    let mut scripts = IndexMap::new();
    let mut scripts_ok = true;
    if let Some(v) = m.get("Scripts") {
        let scripts_path = join(path, "Scripts");
        if let Some(sm) = r.mapping(v, &scripts_path) {
            for (name, v) in r.entries(sm, &scripts_path) {
                match script(r, &join(&scripts_path, &name), v) {
                    Some(s) => {
                        scripts.insert(name, s);
                    }
                    None => scripts_ok = false,
                }
            }
        }
    }
    let usage_limit = required_mapping(r, m, path, "UsageLimit").and_then(|um| {
        let usage_path = join(path, "UsageLimit");
        let v = r.get(um, &usage_path, "Resume", true);
        r.one_of(
            v,
            &join(&usage_path, "Resume"),
            &[("auto", Resume::Auto), ("manual", Resume::Manual)],
        )
    });
    let compaction = compaction(r, m, path);
    let workspace = required_mapping(r, m, path, "Workspace").and_then(|wm| {
        let workspace_path = join(path, "Workspace");
        let host = required_string(r, wm, &workspace_path, "Host");
        let architecture = r.get(wm, &workspace_path, "Architecture", true);
        let architecture = r.one_of(
            architecture,
            &join(&workspace_path, "Architecture"),
            &ARCHITECTURES,
        );
        let idle = r.get(wm, &workspace_path, "IdleSeconds", true);
        let idle_seconds = r.integer(idle, &join(&workspace_path, "IdleSeconds"), 0);
        Some(Workspace {
            host: host?,
            architecture: architecture?,
            idle_seconds: idle_seconds?,
        })
    });
    if !repos_ok || !scripts_ok {
        return None;
    }
    Some(Project {
        repos,
        default_model: default_model?,
        scripts,
        usage_limit: usage_limit?,
        compaction: compaction?,
        workspace: workspace?,
    })
}

fn script(r: &mut Reader, path: &str, value: &Value) -> Option<Script> {
    let m = r.mapping(value, path)?;
    let command = required_string(r, m, path, "Command");
    let timeout = r.get(m, path, "TimeoutSeconds", true);
    let timeout_seconds = r.integer(timeout, &join(path, "TimeoutSeconds"), 1);
    let run_on_create = match m.get("RunOnCreate") {
        None => Some(false),
        v => r.boolean(v, &join(path, "RunOnCreate")),
    };
    let run_async = match m.get("Async") {
        None => Some(true),
        v => r.boolean(v, &join(path, "Async")),
    };
    Some(Script {
        command: command?,
        timeout_seconds: timeout_seconds?,
        run_on_create: run_on_create?,
        run_async: run_async?,
    })
}

fn compaction(r: &mut Reader, m: &Mapping, path: &str) -> Option<Compaction> {
    let Some(v) = m.get("Compaction") else {
        return Some(Compaction {
            idle: Idle::Off,
            auto_compact_window: None,
        });
    };
    let path = join(path, "Compaction");
    let cm = r.mapping(v, &path)?;
    let idle = match cm.get("Idle") {
        None => Some(Idle::Off),
        v => r.one_of(
            v,
            &join(&path, "Idle"),
            &[
                ("off", Idle::Off),
                ("onReturn", Idle::OnReturn),
                ("beforeCacheExpires", Idle::BeforeCacheExpires),
            ],
        ),
    };
    let window_path = join(&path, "AutoCompactWindow");
    let window = match cm.get("AutoCompactWindow") {
        None => Some(None),
        v => match r.integer(v, &window_path, 100_000) {
            Some(n) if n <= 1_000_000 => Some(Some(n)),
            Some(_) => {
                r.format(&window_path, "must be from 100000 to 1000000");
                None
            }
            None => None,
        },
    };
    Some(Compaction {
        idle: idle?,
        auto_compact_window: window?,
    })
}

fn trigger(r: &mut Reader, path: &str, value: &Value) -> Option<Trigger> {
    let m = r.mapping(value, path)?;
    let enabled = r.get(m, path, "Enabled", true);
    let enabled = r.boolean(enabled, &join(path, "Enabled"));
    let pattern_path = join(path, "EventPattern");
    let event = m.get("EventPattern").map(|v| {
        let pattern = r.json(v, &pattern_path)?;
        let issues = event_pattern::check(&pattern, &pattern_path);
        let ok = issues.is_empty();
        for (p, message) in issues {
            r.format(&p, message);
        }
        ok.then_some(pattern)
    });
    let schedule_path = join(path, "ScheduleExpression");
    let schedule = m.get("ScheduleExpression").map(|v| {
        let text = r.string(Some(v), &schedule_path)?;
        match schedule::parse(&text) {
            Ok(s) => Some(s),
            Err(error) => {
                r.format(&schedule_path, error.to_string());
                None
            }
        }
    });
    let on = match (event, schedule) {
        (Some(event), None) => event.map(TriggerOn::Event),
        (None, Some(schedule)) => schedule.map(TriggerOn::Schedule),
        (Some(_), Some(_)) => {
            r.push(
                Check::Trigger,
                path,
                "has both EventPattern and ScheduleExpression",
            );
            None
        }
        (None, None) => {
            r.push(
                Check::Trigger,
                path,
                "needs EventPattern or ScheduleExpression",
            );
            None
        }
    };
    let mut input_paths_map = IndexMap::new();
    let map_path = join(path, "InputPathsMap");
    if let Some(v) = m.get("InputPathsMap")
        && let Some(im) = r.mapping(v, &map_path)
    {
        for (name, v) in r.entries(im, &map_path) {
            let value_path = join(&map_path, &name);
            match r.string(Some(v), &value_path) {
                Some(p) if p.starts_with('$') => {
                    input_paths_map.insert(name, p);
                }
                Some(_) => r.format(&value_path, "must be a path starting with $"),
                None => {}
            }
        }
    }
    let start = required_mapping(r, m, path, "Start").and_then(|sm| {
        let start_path = join(path, "Start");
        let project = required_string(r, sm, &start_path, "Project");
        let message = required_string(r, sm, &start_path, "Message");
        let model = match sm.get("Model") {
            None => Some(None),
            Some(v) => {
                let model_path = join(&start_path, "Model");
                r.mapping(v, &model_path)
                    .and_then(|mm| model(r, mm, &model_path))
                    .map(Some)
            }
        };
        if let Some(message) = &message {
            for name in placeholders(message) {
                if !input_paths_map.contains_key(name) {
                    r.push(
                        Check::Trigger,
                        &join(&start_path, "Message"),
                        format!("<{name}> is not in InputPathsMap"),
                    );
                }
            }
        }
        Some(Start {
            project: project?,
            message: message?,
            model: model?,
        })
    });
    Some(Trigger {
        enabled: enabled?,
        on: on?,
        input_paths_map,
        start: start?,
    })
}

/// The names written as `<name>` in a message.
pub(crate) fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else { break };
        let name = &after[..close];
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            out.push(name);
        }
        rest = &after[close + 1..];
    }
    out
}
