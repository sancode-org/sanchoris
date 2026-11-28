---
name: open-pr-green-ci
description: Push the branch, open a pull request, and hand CI failures back to you until the checks pass. Run it when your changes for an issue are committed.
---

Run it with the repository name of the project and the issue number:

```sh
sanchoris-agent skill run open-pr-green-ci --input '{"Repo":"<repository name>","Issue":<issue number>}'
```

The skill pushes the branch, opens a pull request and waits for its checks. When a check fails, it sends you the failure. Fix it, commit, and run the `sanchoris-agent skill continue` command in the message. After 3 fixes that still fail, the skill fails with `Skill.FixesExhausted`. When the checks pass, it comments on the issue with the pull request.
