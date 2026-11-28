use crate::issue::Check;
use crate::reader::Reader;

/// The frontmatter and the body of a `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDocument {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// A skill name: 1 to 64 lowercase letters, digits and hyphens, not starting or ending with a
/// hyphen.
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Reads a `SKILL.md` of the skill directory `directory`: its frontmatter has `name`, which is
/// the directory name, and `description`.
pub(crate) fn read(r: &mut Reader, directory: &str, text: &str) -> Option<SkillDocument> {
    let mut lines = text.split_inclusive('\n');
    let opened = lines
        .next()
        .is_some_and(|line| line.trim_end_matches(['\r', '\n']) == "---");
    let mut frontmatter = String::new();
    let mut closed = false;
    if opened {
        for line in lines.by_ref() {
            if line.trim_end_matches(['\r', '\n']) == "---" {
                closed = true;
                break;
            }
            frontmatter.push_str(line);
        }
    }
    if !closed {
        r.format("", "must start with a YAML frontmatter between --- lines");
        return None;
    }
    let body: String = lines.collect();
    let value = r.parse(&frontmatter)?;
    let m = r.mapping(&value, "")?;
    let name = r.get(m, "", "name", true).cloned();
    let name = r.string(name.as_ref(), "name");
    let description = r.get(m, "", "description", true).cloned();
    let description = r.string(description.as_ref(), "description");
    if let Some(name) = &name
        && name != directory
    {
        r.format(
            "name",
            format!("{name} is not the name of the skill directory {directory}"),
        );
    }
    if description.as_deref().is_some_and(|d| d.trim().is_empty()) {
        r.push(Check::Required, "description", "must not be empty");
    }
    Some(SkillDocument {
        name: name?,
        description: description?,
        body,
    })
}
