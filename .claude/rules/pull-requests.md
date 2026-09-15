# Pull Requests

When all commits on a branch are done, `make check` passes, and the review agent has reported back, push and open a PR automatically.

There is no `make test-prove` in this repository; it lives in proveno-zk. If a change affects what gets proved — the program hash, canonical serialization, the oracle tape — run it in a proveno-zk checkout before opening a PR, and note the result in the PR body.

- **Target:** always `main`
- **State:** always open as **draft**
- **Title:** `type(scope): short description` — same convention as the commit that drove the work (see `.claude/rules/commits.md`)
- **Body:** summarise what changed (bullet points from the commits) and reference the issue or planning doc the work came from

```bash
git push -u origin <branch>
gh pr create --draft --base main --title "..." --body "..."
```

## Agent Run Report (PR comment)

Immediately after the PR is created, post an agent run report as a PR comment. Assemble it from:
1. `git log main..HEAD --oneline` — the implementation commits
2. The review agent's returned report (captured earlier)

```bash
gh pr comment <PR-number> --body "$(cat <<'EOF'
## Agent Run Report

### Implementation Commits
- <commit hash> <commit message>
- ...

### Review Report
<paste the review agent's full structured output here>
EOF
)"
```

This comment is the permanent record of what every agent did on this branch. It must be posted before the branch is considered done.
