//! The text of the notifications that a SkillRun sends to its thread (`skill.md`「Skill の呼び出し」).

use crate::generated::sanchoris_engine::Notice;
use crate::json::from_lean;

/// How much of the end of stdout and stderr a notification carries.
pub const OUTPUT_TAIL_BYTES: usize = 16 * 1024;

/// The last [`OUTPUT_TAIL_BYTES`] of a text, cut at a character boundary.
pub fn tail(text: &str) -> &str {
    if text.len() <= OUTPUT_TAIL_BYTES {
        return text;
    }
    let mut start = text.len() - OUTPUT_TAIL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn output_line(name: &str, text: Option<&String>) -> String {
    match text.map(|t| t.trim_end_matches('\n')) {
        Some(t) if !t.is_empty() => format!("\n{name}: {}", tail(t)),
        _ => String::new(),
    }
}

/// The input of the thread for a notification of the SkillRun `run` of `skill`.
pub fn text(skill: &str, run: u64, notice: &Notice) -> String {
    let head = format!("[sanchoris] Skill {skill} (run {run}): ");
    match notice {
        Notice::Transition {
            path,
            exit_code,
            stdout,
            stderr,
        } => {
            let exit = exit_code
                .as_ref()
                .map(|code| format!(" Exit {code}."))
                .unwrap_or_default();
            format!(
                "{head}{}.{exit}{}{}",
                path.join(" -> "),
                output_line("stdout", stdout.as_ref()),
                output_line("stderr", stderr.as_ref())
            )
        }
        Notice::Succeeded { path, output } => {
            let mut path = path.clone();
            path.push("SUCCEEDED".to_string());
            format!(
                "{head}{}.\nOutput: {}",
                path.join(" -> "),
                from_lean(output)
            )
        }
        Notice::Failed { path, error, cause } => {
            let mut path = path.clone();
            path.push("FAILED".to_string());
            format!(
                "{head}{}.\nError: {error}\nCause: {cause}",
                path.join(" -> ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_end_at_a_character_boundary() {
        let text = format!("{}{}", "é".repeat(OUTPUT_TAIL_BYTES), "end");
        let t = tail(&text);
        assert!(t.len() <= OUTPUT_TAIL_BYTES);
        assert!(t.ends_with("end"));
        assert_eq!(tail("short"), "short");
    }
}
