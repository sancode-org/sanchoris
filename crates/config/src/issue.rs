use std::fmt;

/// The check of the configuration validation that an issue fails, one per row of the validation
/// table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Check {
    /// A required item is missing, or a skill is named `compact`.
    Required,
    /// A value is not written in its format.
    Format,
    /// A trigger has neither or both of `EventPattern` and `ScheduleExpression`, or its message
    /// names a value that `InputPathsMap` does not define.
    Trigger,
    /// A project, credential or host that the configuration names does not exist.
    Reference,
    /// A repository is not in the GitHub App installation.
    Repository,
    /// A `skill.yaml` refers to a missing state, has an unreachable state, has no terminal state
    /// or places `States.ALL` before another element.
    SkillDefinition,
}

/// A failed check: the file, the path of the item in it, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub check: Check,
    pub file: String,
    pub path: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.file, self.message)
        } else {
            write!(f, "{}: {}: {}", self.file, self.path, self.message)
        }
    }
}

pub(crate) fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

pub(crate) fn index(path: &str, i: usize) -> String {
    format!("{path}[{i}]")
}
