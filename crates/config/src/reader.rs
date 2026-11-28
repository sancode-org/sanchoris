use serde_yaml_ng::{Mapping, Value};

use crate::issue::{Check, Issue, join};

/// Reads the values of one file and records the issues found in it.
pub(crate) struct Reader {
    file: String,
    pub(crate) issues: Vec<Issue>,
}

impl Reader {
    pub(crate) fn new(file: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            issues: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, check: Check, path: &str, message: impl Into<String>) {
        self.issues.push(Issue {
            check,
            file: self.file.clone(),
            path: path.to_string(),
            message: message.into(),
        });
    }

    pub(crate) fn format(&mut self, path: &str, message: impl Into<String>) {
        self.push(Check::Format, path, message);
    }

    pub(crate) fn parse(&mut self, text: &str) -> Option<Value> {
        match serde_yaml_ng::from_str(text) {
            Ok(value) => Some(value),
            Err(error) => {
                self.format("", format!("not valid YAML: {error}"));
                None
            }
        }
    }

    pub(crate) fn mapping<'v>(&mut self, value: &'v Value, path: &str) -> Option<&'v Mapping> {
        match value {
            Value::Mapping(m) => Some(m),
            _ => {
                self.format(path, "must be a mapping");
                None
            }
        }
    }

    /// The entries of a mapping whose keys are strings; other keys are format issues.
    pub(crate) fn entries<'v>(&mut self, m: &'v Mapping, path: &str) -> Vec<(String, &'v Value)> {
        let mut out = Vec::new();
        for (k, v) in m {
            match k {
                Value::String(k) => out.push((k.clone(), v)),
                _ => self.format(path, "keys must be strings"),
            }
        }
        out
    }

    /// The value of `key`; a missing required key is a required-item issue.
    pub(crate) fn get<'v>(
        &mut self,
        m: &'v Mapping,
        path: &str,
        key: &str,
        required: bool,
    ) -> Option<&'v Value> {
        let value = m.get(key);
        if value.is_none() && required {
            self.push(Check::Required, &join(path, key), "missing required item");
        }
        value
    }

    pub(crate) fn string(&mut self, value: Option<&Value>, path: &str) -> Option<String> {
        match value? {
            Value::String(s) => Some(s.clone()),
            _ => {
                self.format(path, "must be a string");
                None
            }
        }
    }

    pub(crate) fn boolean(&mut self, value: Option<&Value>, path: &str) -> Option<bool> {
        match value? {
            Value::Bool(b) => Some(*b),
            _ => {
                self.format(path, "must be true or false");
                None
            }
        }
    }

    pub(crate) fn integer(&mut self, value: Option<&Value>, path: &str, min: u64) -> Option<u64> {
        match value?.as_u64() {
            Some(n) if n >= min => Some(n),
            _ => {
                self.format(path, format!("must be an integer of at least {min}"));
                None
            }
        }
    }

    pub(crate) fn number(&mut self, value: Option<&Value>, path: &str) -> Option<f64> {
        match value?.as_f64() {
            Some(n) => Some(n),
            None => {
                self.format(path, "must be a number");
                None
            }
        }
    }

    /// One of `names`, written as a string.
    pub(crate) fn one_of<T: Copy>(
        &mut self,
        value: Option<&Value>,
        path: &str,
        names: &[(&str, T)],
    ) -> Option<T> {
        let value = value?;
        let found = value
            .as_str()
            .and_then(|s| names.iter().find(|(n, _)| *n == s).map(|(_, t)| *t));
        if found.is_none() {
            let list: Vec<&str> = names.iter().map(|(n, _)| *n).collect();
            self.format(path, format!("must be one of {}", list.join(", ")));
        }
        found
    }
}

pub(crate) fn to_json(value: &Value) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
