//! Reading and validating a revision of a configuration repository: `sanchoris.yaml` and the
//! skills in `skills/<name>/`.

mod config;
pub mod event_pattern;
mod generated;
pub mod issue;
mod reader;
pub mod schedule;
mod skill;
mod skill_md;

use std::collections::BTreeMap;

use indexmap::IndexMap;

pub use config::{
    Architecture, Compaction, Idle, Model, Project, Resume, Script, Start, Trigger, TriggerOn,
    Workspace,
};
pub use issue::{Check, Issue};
pub use skill::{InputType, SkillDefinition};
pub use skill_md::{SkillDocument, is_skill_name};

use issue::join;
use reader::Reader;

/// The file of the projects and triggers, at the root path of the configuration.
pub const CONFIG_FILE: &str = "sanchoris.yaml";

/// The built-in skills, which every configuration can call: name, `SKILL.md` and `skill.yaml`.
pub const BUILTIN_SKILLS: [(&str, &str, &str); 1] = [(
    "open-pr-green-ci",
    include_str!("../builtin/open-pr-green-ci/SKILL.md"),
    include_str!("../builtin/open-pr-green-ci/skill.yaml"),
)];

/// A registered host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub name: String,
    pub architecture: Architecture,
}

/// The values outside the configuration that the validation needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outside {
    /// The names of the owner's credentials.
    pub credentials: Vec<String>,
    /// The owner's registered hosts.
    pub hosts: Vec<Host>,
    /// The repositories of the owner's GitHub App installation, as `owner/name`.
    pub repositories: Vec<String>,
}

/// A skill of the configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub document: SkillDocument,
    /// Whether sanchoris provides the skill.
    pub builtin: bool,
    /// The definition of a predictable skill; `None` for a skill with only `SKILL.md`.
    pub definition: Option<SkillDefinition>,
}

/// A configuration that passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub projects: IndexMap<String, Project>,
    pub triggers: IndexMap<String, Trigger>,
    pub skills: IndexMap<String, Skill>,
}

/// Validates the files of a revision, keyed by their paths below the root path of the
/// configuration.
pub fn validate(files: &BTreeMap<String, String>, outside: &Outside) -> Result<Config, Vec<Issue>> {
    let mut issues = Vec::new();
    let mut r = Reader::new(CONFIG_FILE);
    let file = match files.get(CONFIG_FILE) {
        Some(text) => config::read(&mut r, text),
        None => {
            r.push(Check::Required, "", "missing file");
            None
        }
    };
    if let Some(file) = &file {
        references(&mut r, file, outside);
    }
    issues.append(&mut r.issues);

    let mut skills = IndexMap::new();
    for (name, document, definition) in BUILTIN_SKILLS {
        let mut r = Reader::new(format!("builtin:{name}"));
        if let Some(skill) = read_skill(&mut r, name, Some(document), Some(definition), true) {
            skills.insert(name.to_string(), skill);
        }
        issues.append(&mut r.issues);
    }
    for name in skill_directories(files) {
        let directory = format!("skills/{name}");
        let document = files.get(&format!("{directory}/SKILL.md"));
        let definition = files.get(&format!("{directory}/skill.yaml"));
        let mut r = Reader::new(format!("{directory}/SKILL.md"));
        if name == "compact" {
            r.push(
                Check::Required,
                "",
                "compact is the compact instruction of the worker and cannot name a skill",
            );
        } else if !is_skill_name(&name) {
            r.format(
                "",
                format!("{name} must be 1 to 64 lowercase letters, digits and hyphens"),
            );
        }
        let skill = read_skill(
            &mut r,
            &name,
            document.map(String::as_str),
            definition.map(String::as_str),
            false,
        );
        issues.append(&mut r.issues);
        if let Some(skill) = skill {
            skills.insert(name, skill);
        }
    }
    let Some(file) = file else {
        return Err(issues);
    };
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(Config {
        projects: file.projects,
        triggers: file.triggers,
        skills,
    })
}

fn skill_directories(files: &BTreeMap<String, String>) -> Vec<String> {
    let mut names: Vec<String> = files
        .keys()
        .filter_map(|path| {
            let rest = path.strip_prefix("skills/")?;
            let (name, file) = rest.split_once('/')?;
            (file == "SKILL.md" || file == "skill.yaml").then(|| name.to_string())
        })
        .collect();
    names.dedup();
    names
}

fn read_skill(
    r: &mut Reader,
    name: &str,
    document: Option<&str>,
    definition: Option<&str>,
    builtin: bool,
) -> Option<Skill> {
    let document = match document {
        Some(text) => skill_md::read(r, name, text),
        None => {
            r.push(Check::Required, "", "missing file");
            None
        }
    };
    let definition = definition.and_then(|text| {
        let mut definition_reader = Reader::new(if builtin {
            format!("builtin:{name}/skill.yaml")
        } else {
            format!("skills/{name}/skill.yaml")
        });
        let definition = skill::read(&mut definition_reader, text);
        r.issues.append(&mut definition_reader.issues);
        definition
    });
    Some(Skill {
        document: document?,
        builtin,
        definition,
    })
}

fn same_repository(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The checks of the references (projects, credentials, hosts) and of the repositories.
fn references(r: &mut Reader, file: &config::ConfigFile, outside: &Outside) {
    let credential = |r: &mut Reader, path: &str, name: &str| {
        if !outside.credentials.iter().any(|c| c == name) {
            r.push(
                Check::Reference,
                path,
                format!("{name} is not a credential of the owner"),
            );
        }
    };
    let installed = |name: &str| {
        outside
            .repositories
            .iter()
            .any(|repo| same_repository(repo, name))
    };
    for (name, project) in &file.projects {
        let path = join("Projects", name);
        credential(
            r,
            &join(&join(&path, "DefaultModel"), "Credential"),
            &project.default_model.credential,
        );
        let workspace_path = join(&path, "Workspace");
        let host = &project.workspace.host;
        if host != "cloud" {
            match outside.hosts.iter().find(|h| &h.name == host) {
                None => r.push(
                    Check::Reference,
                    &join(&workspace_path, "Host"),
                    format!("{host} is neither cloud nor a registered host"),
                ),
                Some(h) if h.architecture != project.workspace.architecture => r.push(
                    Check::Reference,
                    &join(&workspace_path, "Architecture"),
                    format!("the host {host} is not of this architecture"),
                ),
                Some(_) => {}
            }
        }
        for (repo_name, repo) in &project.repos {
            if !installed(repo) {
                r.push(
                    Check::Repository,
                    &join(&join(&join(&path, "Repos"), repo_name), "Repo"),
                    format!("{repo} is not in the GitHub App installation"),
                );
            }
        }
    }
    for (name, trigger) in &file.triggers {
        let path = join("Triggers", name);
        let start_path = join(&path, "Start");
        if !file.project_names.contains(&trigger.start.project) {
            r.push(
                Check::Reference,
                &join(&start_path, "Project"),
                format!("{} names no project", trigger.start.project),
            );
        }
        if let Some(model) = &trigger.start.model {
            credential(
                r,
                &join(&join(&start_path, "Model"), "Credential"),
                &model.credential,
            );
        }
        if let TriggerOn::Event(pattern) = &trigger.on {
            let repository = pattern
                .get("detail")
                .and_then(|d| d.get("repository"))
                .and_then(event_pattern::repositories)
                .unwrap_or_default();
            for (i, repo) in repository.into_iter().enumerate() {
                if !installed(repo) {
                    r.push(
                        Check::Repository,
                        &issue::index(&join(&path, "EventPattern.detail.repository"), i),
                        format!("{repo} is not in the GitHub App installation"),
                    );
                }
            }
        }
    }
}
