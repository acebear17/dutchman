# Merge queue

GitHub's merge queue tests each pull request on top of the latest `main` (and of the queued pull
requests ahead of it) before it merges, so two green pull requests cannot break `main` together.
When a pull request passes its required checks and is enqueued, GitHub builds a temporary branch
(`gh-readonly-queue/main/...`), runs the workflows that have a `merge_group` trigger on it, and
squash-merges it only if the required checks pass.

## What runs in the queue

| Workflow | In the queue |
|---|---|
| `ci.yml` | Full tests on Linux x86_64, ARM64, Windows and macOS, plus fmt, Clippy, MSRV, WebAssembly, Docker, release checks and SolidCraft round trips. Path skips apply to the whole group: docs and changelog changes skip unrelated builds; a mixed group runs code checks. No Rust cache writes. |
| `placements.yml` | Called by `ci.yml`; builds the command, MCP and web programs once in Bookworm, checks all three assemblies including Voron for code changes, and shares the programs with Docker. |
| `docker.yml` | No separate queue run; `ci.yml` owns the Docker check. Main, scheduled and manual runs stay available. |
| `release-build.yml` | Called by `ci.yml`; `plan` enables all release targets and packaging for every group. No Rust caches. |

PRs run only fast Linux checks, with focused docs tests for docs-only changes, plus release
notes, changelog and version gates. `gh workflow run ci.yml --ref <branch>` runs the full
queue matrix on a PR branch by hand. Packaging changes and the `release-build` label no longer
start full builds on a PR. Candidate and stable tag pushes also run the full matrix; their
release builds run directly in `release-build.yml` to preserve candidate artifact discovery.

## macOS in the merge queue

macOS tests, release builds and packaging still run in the merge queue, but are non-blocking
there. Hosted macOS runners are scarce: missing runners caused jobs to be cancelled. For
merge groups, `ci-result` prints the macOS test result and requires the non-macOS release
build/check output to be `success`, regardless of the overall release-build result. Every
non-macOS failure, cancellation or unexpected skip still blocks the queue.
macOS stays required on main pushes, tags and manual runs, so a macOS break shows within
hours and blocks a release.

## Ruleset

Create it once, then enable it when ready (`"enforcement": "active"` switches it on at creation;
use `"disabled"` in the file to stage it first and `gh api -X PUT` the ruleset later):

```
gh api repos/dutchman-cad/dutchman/rulesets -X POST --input ruleset.json
```

`ruleset.json`:

```json
{
  "name": "main merge queue",
  "target": "branch",
  "enforcement": "active",
  "conditions": {
    "ref_name": {
      "include": [
        "refs/heads/main"
      ],
      "exclude": []
    }
  },
  "bypass_actors": [
    {
      "actor_id": 5,
      "actor_type": "RepositoryRole",
      "bypass_mode": "always"
    }
  ],
  "rules": [
    {
      "type": "merge_queue",
      "parameters": {
        "merge_method": "SQUASH",
        "grouping_strategy": "ALLGREEN",
        "max_entries_to_build": 5,
        "min_entries_to_merge": 1,
        "max_entries_to_merge": 5,
        "min_entries_to_merge_wait_minutes": 5,
        "check_response_timeout_minutes": 180
      }
    },
    {
      "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [
          {
            "context": "ci-result"
          }
        ]
      }
    }
  ]
}
```

- Squash merges, groups of up to 5 built in parallel, merged when all are green (`ALLGREEN`),
  after waiting 5 minutes for more pull requests to join. A group whose checks do not report
  within 180 minutes is removed.
- Repository admins bypass the rule (role id 5), so an emergency fix can merge directly.
- The one required check is `ci-result`, a job in `ci.yml` that always runs, needs every other
  CI job, including the reusable placement and release builds in the queue. It checks explicit
  expected results for the event and paths, rejecting failures, cancellations and unexpected
  skips, except for macOS in the merge queue as described above. Requiring the test jobs by
  name doesn't work: a docs-only change skips the test matrix, and a skipped matrix job reports
  once under its bare name (`test`), so `test (ubuntu-latest)` and the others never appear and
  the pull request waits forever. Heavy jobs are intentionally skipped on PRs and required in
  the queue, including `solidcraft-roundtrip`. Main keeps its path filters and informational
  round trips. On tags, release builds report independently from `ci-result`. CodeQL checks
  do not run in the queue.
- Windows and ARM tests are the slowest required checks (ARM took 34 minutes on one run).
- GitHub's ARM runners are sometimes shut down mid-job (exit code 143, "The runner has received
  a shutdown signal"). The ARM tests run as their own job, `test (ubuntu-24.04-arm)`; when it
  fails, `test (ubuntu-24.04-arm, again)` checks the failed job's annotations and, only after
  such a shutdown, runs the tests once more. `ci-result` then counts the second try in place of
  the first. Any other failure is not retried and still fails `ci-result`.

## Enqueue

Once the required checks pass on the pull request:

```
gh pr merge <n> --squash --auto
```

With the ruleset active this adds the pull request to the queue. Remove it with
`gh pr merge <n> --disable-auto` or from the pull request page.
