use std::collections::BTreeMap;

use sanchoris_config::{Architecture, BUILTIN_SKILLS, Check, Host, Outside, TriggerOn, validate};

const CONFIG: &str = include_str!("fixtures/sanchoris.yaml");
const OPEN_PR_GREEN_CI: &str = include_str!("../builtin/open-pr-green-ci/skill.yaml");

const SKILL_MD: &str = "---
name: wait-pr-ci
description: Wait for the checks of a pull request.
---

Run it with `sanchoris-agent skill run wait-pr-ci`.
";

const SKILL_YAML: &str = "Inputs: {}
StartAt: Check
TimeoutSeconds: 3600
States:
  Check:
    Type: Command
    Run: gh pr checks --watch
    TimeoutSeconds: 3600
    Retry:
      - ErrorEquals: [Command.Failed]
        IntervalSeconds: 10
        MaxAttempts: 2
        BackoffRate: 2
    Catch:
      - ErrorEquals: [States.ALL]
        Next: Failed
    Next: Done
  Done:
    Type: Succeed
  Failed:
    Type: Fail
    Error: Skill.ChecksFailed
    Cause: The checks failed
";

fn outside() -> Outside {
    Outside {
        credentials: vec!["my-claude".to_string()],
        hosts: vec![Host {
            name: "home".to_string(),
            architecture: Architecture::X86_64,
        }],
        repositories: vec!["acme/shop-web".to_string(), "acme/shop-api".to_string()],
    }
}

fn files(config: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("sanchoris.yaml".to_string(), config.to_string())])
}

fn with_skill(config: &str, skill_md: &str, skill_yaml: &str) -> BTreeMap<String, String> {
    let mut files = files(config);
    files.insert(
        "skills/wait-pr-ci/SKILL.md".to_string(),
        skill_md.to_string(),
    );
    files.insert(
        "skills/wait-pr-ci/skill.yaml".to_string(),
        skill_yaml.to_string(),
    );
    files
}

fn replace(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from:?} is not in the text");
    text.replacen(from, to, 1)
}

/// The issues of a revision as (check, file, path), sorted.
fn issues(files: &BTreeMap<String, String>) -> Vec<(Check, String, String)> {
    let errors = validate(files, &outside()).expect_err("the revision should fail");
    let mut out: Vec<_> = errors
        .into_iter()
        .map(|i| (i.check, i.file, i.path))
        .collect();
    out.sort_by(|a, b| (&a.1, &a.2).cmp(&(&b.1, &b.2)));
    out
}

fn one(check: Check, file: &str, path: &str) -> Vec<(Check, String, String)> {
    vec![(check, file.to_string(), path.to_string())]
}

fn config_issue(check: Check, path: &str) -> Vec<(Check, String, String)> {
    one(check, "sanchoris.yaml", path)
}

fn skill_issue(check: Check, path: &str) -> Vec<(Check, String, String)> {
    one(check, "skills/wait-pr-ci/skill.yaml", path)
}

#[test]
fn the_examples_of_the_design_pass() {
    let config = validate(&files(CONFIG), &outside()).expect("the example passes");
    let project = &config.projects["shop"];
    assert_eq!(project.repos["frontend"], "acme/shop-web");
    assert_eq!(project.scripts["install"].timeout_seconds, 3600);
    assert!(project.scripts["test"].run_async);
    assert!(!project.scripts["test"].run_on_create);
    let trigger = &config.triggers["labeled-issue"];
    assert!(matches!(trigger.on, TriggerOn::Event(_)));
    assert_eq!(trigger.start.project, "shop");
    let skill = &config.skills["open-pr-green-ci"];
    assert!(skill.builtin);
    let definition = skill.definition.as_ref().expect("a predictable skill");
    assert_eq!(definition.inputs.len(), 2);
    assert_eq!(definition.source["StartAt"], "Init");
}

#[test]
fn a_skill_of_the_configuration_passes() {
    let config = validate(&with_skill(CONFIG, SKILL_MD, SKILL_YAML), &outside()).unwrap();
    let skill = &config.skills["wait-pr-ci"];
    assert!(!skill.builtin);
    assert_eq!(
        skill.document.description,
        "Wait for the checks of a pull request."
    );
    assert!(skill.definition.is_some());
}

#[test]
fn open_pr_green_ci_passes_as_a_skill_of_the_configuration() {
    let (_, document, definition) = BUILTIN_SKILLS[0];
    let mut files = files(CONFIG);
    files.insert(
        "skills/open-pr-green-ci/SKILL.md".to_string(),
        document.to_string(),
    );
    files.insert(
        "skills/open-pr-green-ci/skill.yaml".to_string(),
        OPEN_PR_GREEN_CI.to_string(),
    );
    assert_eq!(definition, OPEN_PR_GREEN_CI);
    let config = validate(&files, &outside()).unwrap();
    assert!(!config.skills["open-pr-green-ci"].builtin);
}

#[test]
fn a_skill_with_only_skill_md_passes() {
    let mut files = files(CONFIG);
    files.insert(
        "skills/wait-pr-ci/SKILL.md".to_string(),
        SKILL_MD.to_string(),
    );
    let config = validate(&files, &outside()).unwrap();
    assert!(config.skills["wait-pr-ci"].definition.is_none());
}

#[test]
fn a_host_of_the_same_architecture_passes() {
    let config = replace(
        CONFIG,
        "Host: cloud, Architecture: arm64",
        "Host: home, Architecture: x86_64",
    );
    validate(&files(&config), &outside()).unwrap();
}

#[test]
fn missing_items_fail() {
    let cases = [
        ("Version: 1\n", "", "Version"),
        (
            "    Workspace: { Host: cloud, Architecture: arm64, IdleSeconds: 900 }\n",
            "",
            "Projects.shop.Workspace",
        ),
        (
            "Model: claude-sonnet-5-5, Credential: my-claude",
            "Model: claude-sonnet-5-5",
            "Projects.shop.DefaultModel.Credential",
        ),
        (
            "{ Command: 'cd frontend && pnpm test', TimeoutSeconds: 900 }",
            "{ Command: 'cd frontend && pnpm test' }",
            "Projects.shop.Scripts.test.TimeoutSeconds",
        ),
        ("    Enabled: true\n", "", "Triggers.labeled-issue.Enabled"),
        (
            "      Project: shop\n",
            "",
            "Triggers.labeled-issue.Start.Project",
        ),
    ];
    for (from, to, path) in cases {
        let config = replace(CONFIG, from, to);
        assert_eq!(
            issues(&files(&config)),
            config_issue(Check::Required, path),
            "{path}"
        );
    }
}

#[test]
fn missing_items_of_skill_yaml_fail() {
    let cases = [
        ("StartAt: Check\n", "", "StartAt"),
        ("TimeoutSeconds: 3600\nStates", "States", "TimeoutSeconds"),
        ("Inputs: {}\n", "", "Inputs"),
        (
            "    Run: gh pr checks --watch\n    TimeoutSeconds: 3600\n",
            "    Run: gh pr checks --watch\n",
            "States.Check.TimeoutSeconds",
        ),
        (
            "        MaxAttempts: 2\n        BackoffRate: 2\n",
            "        MaxAttempts: 2\n",
            "States.Check.Retry[0].BackoffRate",
        ),
        ("    Cause: The checks failed\n", "", "States.Failed.Cause"),
    ];
    for (from, to, path) in cases {
        let yaml = replace(SKILL_YAML, from, to);
        assert_eq!(
            issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
            skill_issue(Check::Required, path),
            "{path}"
        );
    }
}

#[test]
fn a_state_without_next_or_end_fails() {
    let yaml = replace(SKILL_YAML, "    Next: Done\n", "");
    // Done is then reached from nothing.
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
        vec![
            (
                Check::Required,
                "skills/wait-pr-ci/skill.yaml".to_string(),
                "States.Check".to_string()
            ),
            (
                Check::SkillDefinition,
                "skills/wait-pr-ci/skill.yaml".to_string(),
                "States.Done".to_string()
            ),
        ]
    );
}

#[test]
fn missing_items_of_skill_md_fail() {
    let document = replace(
        SKILL_MD,
        "description: Wait for the checks of a pull request.\n",
        "",
    );
    assert_eq!(
        issues(&with_skill(CONFIG, &document, SKILL_YAML)),
        one(Check::Required, "skills/wait-pr-ci/SKILL.md", "description")
    );
    let mut files = files(CONFIG);
    files.insert(
        "skills/wait-pr-ci/skill.yaml".to_string(),
        SKILL_YAML.to_string(),
    );
    assert_eq!(
        issues(&files),
        one(Check::Required, "skills/wait-pr-ci/SKILL.md", "")
    );
}

#[test]
fn a_skill_named_compact_fails() {
    let mut files = files(CONFIG);
    files.insert(
        "skills/compact/SKILL.md".to_string(),
        replace(SKILL_MD, "name: wait-pr-ci", "name: compact"),
    );
    assert_eq!(
        issues(&files),
        one(Check::Required, "skills/compact/SKILL.md", "")
    );
}

#[test]
fn unresolved_references_fail() {
    let cases = [
        (
            "      Project: shop\n",
            "      Project: shop-v2\n",
            Check::Reference,
            "Triggers.labeled-issue.Start.Project",
        ),
        (
            "Credential: my-claude }",
            "Credential: team-claude }",
            Check::Reference,
            "Projects.shop.DefaultModel.Credential",
        ),
        (
            "      Project: shop\n",
            "      Project: shop\n      Model: { Agent: codex, Model: gpt-5, Credential: my-codex }\n",
            Check::Reference,
            "Triggers.labeled-issue.Start.Model.Credential",
        ),
        (
            "Host: cloud",
            "Host: office",
            Check::Reference,
            "Projects.shop.Workspace.Host",
        ),
        (
            "Host: cloud",
            "Host: home",
            Check::Reference,
            "Projects.shop.Workspace.Architecture",
        ),
        (
            "github.com/acme/shop-api",
            "github.com/acme/shop-admin",
            Check::Repository,
            "Projects.shop.Repos.backend.Repo",
        ),
        (
            "repository: [acme/shop-web, acme/shop-api]",
            "repository: [acme/shop-web, acme/shop-mobile]",
            Check::Repository,
            "Triggers.labeled-issue.EventPattern.detail.repository[1]",
        ),
    ];
    for (from, to, check, path) in cases {
        let config = replace(CONFIG, from, to);
        assert_eq!(issues(&files(&config)), config_issue(check, path), "{path}");
    }
}

#[test]
fn unknown_states_fail() {
    let cases = [
        ("StartAt: Check\n", "StartAt: Begin\n", "StartAt"),
        (
            "    Next: Done\n",
            "    Next: Finish\n",
            "States.Check.Next",
        ),
        (
            "        Next: Failed\n",
            "        Next: Fails\n",
            "States.Check.Catch[0].Next",
        ),
    ];
    for (from, to, path) in cases {
        let yaml = replace(SKILL_YAML, from, to);
        let found = issues(&with_skill(CONFIG, SKILL_MD, &yaml));
        assert!(
            found.contains(&skill_issue(Check::SkillDefinition, path)[0]),
            "{path}: {found:?}"
        );
    }
    let yaml = replace(
        OPEN_PR_GREEN_CI,
        "    Default: AskFix\n",
        "    Default: AskForFix\n",
    );
    let found = issues(&with_skill(CONFIG, SKILL_MD, &yaml));
    assert!(found.contains(&skill_issue(Check::SkillDefinition, "States.CanFix.Default")[0]));
    let yaml = replace(
        OPEN_PR_GREEN_CI,
        "        Next: GiveUp\n",
        "        Next: Quit\n",
    );
    let found = issues(&with_skill(CONFIG, SKILL_MD, &yaml));
    assert!(
        found.contains(&skill_issue(Check::SkillDefinition, "States.CanFix.Choices[0].Next")[0])
    );
}

#[test]
fn an_unreachable_state_fails() {
    let yaml = format!("{SKILL_YAML}  Orphan:\n    Type: Succeed\n");
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
        skill_issue(Check::SkillDefinition, "States.Orphan")
    );
    let yaml = replace(
        OPEN_PR_GREEN_CI,
        "      - ErrorEquals: [Command.Failed]\n        ResultPath: $.checks\n        Next: CanFix\n",
        "      - ErrorEquals: [Command.Failed]\n        ResultPath: $.checks\n        Next: Report\n",
    );
    let found = issues(&with_skill(CONFIG, SKILL_MD, &yaml));
    for state in ["CanFix", "AskFix", "AwaitFix", "CountFix", "GiveUp"] {
        assert!(
            found.contains(&skill_issue(Check::SkillDefinition, &format!("States.{state}"))[0]),
            "{state}: {found:?}"
        );
    }
}

#[test]
fn a_definition_without_terminal_state_fails() {
    let yaml = "Inputs: {}
StartAt: Loop
TimeoutSeconds: 60
States:
  Loop:
    Type: Pass
    Next: Loop
";
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, yaml)),
        skill_issue(Check::SkillDefinition, "States")
    );
}

#[test]
fn states_all_before_another_element_fails() {
    let yaml = replace(
        SKILL_YAML,
        "      - ErrorEquals: [States.ALL]\n        Next: Failed\n",
        "      - ErrorEquals: [States.ALL]\n        Next: Failed\n      - ErrorEquals: [Command.Failed]\n        Next: Failed\n",
    );
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
        skill_issue(Check::SkillDefinition, "States.Check.Catch[0].ErrorEquals")
    );
    let yaml = replace(
        SKILL_YAML,
        "ErrorEquals: [States.ALL]",
        "ErrorEquals: [States.ALL, Command.Failed]",
    );
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
        skill_issue(Check::SkillDefinition, "States.Check.Catch[0].ErrorEquals")
    );
}

#[test]
fn format_errors_fail() {
    let cases = [
        ("Version: 1\n", "Version: 2\n", "Version"),
        (
            "github.com/acme/shop-web",
            "gitlab.com/acme/shop-web",
            "Projects.shop.Repos.frontend.Repo",
        ),
        (
            "Architecture: arm64",
            "Architecture: riscv64",
            "Projects.shop.Workspace.Architecture",
        ),
        (
            "UsageLimit: { Resume: auto }",
            "UsageLimit: { Resume: later }",
            "Projects.shop.UsageLimit.Resume",
        ),
        (
            "IdleSeconds: 900",
            "IdleSeconds: fifteen",
            "Projects.shop.Workspace.IdleSeconds",
        ),
        (
            "repository: [acme/shop-web, acme/shop-api]",
            "repository: [{ prefix: acme/ }]",
            "Triggers.labeled-issue.EventPattern.detail.repository",
        ),
        (
            "label: [sanchoris]",
            "label: [{ suffix: ris }]",
            "Triggers.labeled-issue.EventPattern.detail.label[0]",
        ),
        (
            "label: [sanchoris]",
            "label: sanchoris",
            "Triggers.labeled-issue.EventPattern.detail.label",
        ),
        (
            "detail-type: [Issue Labeled]",
            "detail-type: [Issue Closed]",
            "Triggers.labeled-issue.EventPattern.detail-type[0]",
        ),
        (
            "    Enabled: true\n",
            "    Enabled: yes please\n",
            "Triggers.labeled-issue.Enabled",
        ),
    ];
    for (from, to, path) in cases {
        let config = replace(CONFIG, from, to);
        assert_eq!(
            issues(&files(&config)),
            config_issue(Check::Format, path),
            "{path}"
        );
    }
}

#[test]
fn a_malformed_schedule_expression_fails() {
    let schedule = "  nightly:
    Enabled: true
    ScheduleExpression: cron(0 3 * * *)
    Start:
      Project: shop
      Message: Update the dependencies.
";
    let config = format!("{CONFIG}{schedule}");
    assert_eq!(
        issues(&files(&config)),
        config_issue(Check::Format, "Triggers.nightly.ScheduleExpression")
    );
    let config = format!(
        "{CONFIG}{}",
        schedule.replace("cron(0 3 * * *)", "cron(0 3 * * ? *)")
    );
    let config = validate(&files(&config), &outside()).unwrap();
    assert!(matches!(
        config.triggers["nightly"].on,
        TriggerOn::Schedule(_)
    ));
}

#[test]
fn format_errors_of_skill_files_fail() {
    let cases = [
        (
            "    Type: Succeed\n",
            "    Type: Success\n",
            "States.Done.Type",
        ),
        (
            "TimeoutSeconds: 3600\nStates",
            "TimeoutSeconds: an hour\nStates",
            "TimeoutSeconds",
        ),
        (
            "        BackoffRate: 2\n",
            "        BackoffRate: 0.5\n",
            "States.Check.Retry[0].BackoffRate",
        ),
        (
            "    Error: Skill.ChecksFailed\n",
            "    Error: States.ChecksFailed\n",
            "States.Failed.Error",
        ),
        ("Inputs: {}\n", "Inputs: { Pr: integer }\n", "Inputs.Pr"),
    ];
    for (from, to, path) in cases {
        let yaml = replace(SKILL_YAML, from, to);
        assert_eq!(
            issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
            skill_issue(Check::Format, path),
            "{path}"
        );
    }
    let yaml = replace(
        OPEN_PR_GREEN_CI,
        "  GiveUp: { x: 840, y: 300 }\n",
        "  GiveIn: { x: 840, y: 300 }\n",
    );
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
        skill_issue(Check::Format, "Layout.GiveIn")
    );
    let yaml = replace(SKILL_YAML, "States:\n  Check:", "States:\n  Check: [");
    assert_eq!(
        issues(&with_skill(CONFIG, SKILL_MD, &yaml)),
        skill_issue(Check::Format, "")
    );
    let document = replace(SKILL_MD, "name: wait-pr-ci", "name: wait-for-ci");
    assert_eq!(
        issues(&with_skill(CONFIG, &document, SKILL_YAML)),
        one(Check::Format, "skills/wait-pr-ci/SKILL.md", "name")
    );
}

#[test]
fn a_trigger_with_neither_or_both_sources_fails() {
    let config = replace(
        CONFIG,
        "    EventPattern:\n",
        "    ScheduleExpression: rate(1 day)\n    EventPattern:\n",
    );
    assert_eq!(
        issues(&files(&config)),
        config_issue(Check::Trigger, "Triggers.labeled-issue")
    );
    let neither = "  nightly:
    Enabled: true
    Start:
      Project: shop
      Message: Update the dependencies.
";
    assert_eq!(
        issues(&files(&format!("{CONFIG}{neither}"))),
        config_issue(Check::Trigger, "Triggers.nightly")
    );
}

#[test]
fn a_message_with_an_undefined_value_fails() {
    let config = replace(CONFIG, "Resolve issue <number>", "Resolve issue <issue>");
    assert_eq!(
        issues(&files(&config)),
        config_issue(Check::Trigger, "Triggers.labeled-issue.Start.Message")
    );
}

#[test]
fn a_missing_config_file_fails() {
    assert_eq!(issues(&BTreeMap::new()), config_issue(Check::Required, ""));
}
