# Public fixture score

The `Public fixture score` workflow converts public fixtures on GitHub runners.
It runs each Monday at 07:00 UTC. To start a run, open Actions, select
**Public fixture score**, then select **Run workflow**. Pick the Dutchman branch or tag
to score under **Use workflow from** (default `main`); set `shards` to a count from 1 to 256
(default 12), and `manifest_url` to the public manifest endpoint (default
`https://progress.dutchmancad.xyz/scoreboard/public-files.json`). The workflow must
be on the default branch for scheduled runs.

The workflow builds one release binary with the lockfile of the selected branch or tag.
Its shard runner comes from the workflow commit. The build job downloads one
manifest snapshot and keeps only the public score fields, and each shard takes every Nth file from that snapshot.
SolidCraft 0.45 is downloaded with the existing release checksum check. Each
file gets 900 seconds for conversion and reopen together. Reopen uses
`document.inspect` with `measure: true` and the same result shape as the fixture
history sweep. An HTTP error, byte-count mismatch, or SHA-256 mismatch produces
a result with `exit: null` and a `skip_reason`; it does not stop other files.
Conversion timeouts use exit 124 and a `failure_reason`.

The manifest is a JSON list, or an object with a `files` list. Each row requires
`file` (a unique relative file key), `url` (HTTPS), `sha256`, `bytes`, and
`consent` (`cc0` or `cc-by`). Score metadata uses `app`, `step`, `recipe`,
`recipe_family`, and optional `measured`. Paths cannot contain parent components,
backslashes, or control characters. The manifest is limited to 16 MiB. Downloads
stop when they exceed the declared byte count and have a 300-second bound.

Artifacts are:

- `public-score-build`: the executable, manifest snapshot, and build provenance,
  retained for 14 days.
- `public-shard-<index>`: `<file>.result.json` records and shard metadata,
  retained for 14 days, including available records from a failed shard job.
- `public-results`: all complete shard records, a public manifest without
  contributor fields, and `run.json`, retained for 30 days. Collection fails if
  results are missing or manifest hashes or SolidCraft versions differ.

`run.json` records `dutchman.commit`, `cadmpeg` (the Git revision in `Cargo.lock`),
`solidcraft`, `manifest_sha256` (the original downloaded bytes), `files`, and
`shards`. `snapshot_sha256` records the filtered snapshot used by the shards.
Raw logs, source files, and written designs are scratch files and are
not uploaded. The only workflow token is `GITHUB_TOKEN`, with read access.

On the fixture history host, `python3 fetch_public.py` downloads the most recent
successful run with an unexpired `public-results` artifact. It replaces
`public/results` after validation. Public keys are mapped to dataset keys by
SHA-256. Unmatched files get extra rows under `public/`; `mapping.json` records
which rows matched. Scoring takes several results directories and gives listed
public rows their public evidence. Existing score commands also use
`public/results` when present. Files without results stay pending.

For a local check with approved inputs, generate a two-row manifest with their
byte counts and SHA-256 values, then run:

```sh
python3 scripts/fixture-score.py shard --manifest /tmp/two-public-files.json \
  --index 0 --shards 1 --out /tmp/public-score-dry-run \
  --dry-run --local-root /path/to/fixture-dataset
```

This checks local bytes and result paths. It sends no files and runs no CAD tools.

## SolidWorks pin gap

The separate **SolidWorks pin gap** workflow (`.github/workflows/swscore.yml`) runs on
pushes to `swscore-run/**` and by `workflow_dispatch`. Push a branch with that prefix, or
select the workflow in Actions and choose **Run workflow**. From an authenticated checkout:

```sh
gh workflow run swscore.yml --ref swscore-run/pin-gap
```

It snapshots the same public manifest once, selects only `app: SolidWorks` (ignoring case),
`consent: cc0`, and `.sldprt`/`.sldasm` files, and distributes them across 16 shards for each
of two builds. `pin` uses the checked-in manifest and lockfile; `main` moves every cadmpeg
pin to `d3acf478605ea753556423a39745c0539a451d9b`, updates only the cadmpeg packages,
and builds without `--locked`. Both builds use Debian Bookworm. An update or compile failure
keeps its log and build outcome so the summary can report API drift without claiming gains.

Each shard downloads SolidCraft once using the newest minor in the writer's version window,
checks each public file's byte count and SHA-256, and gives each conversion 900 seconds.
It then reopens any written design with `timeline.roll_to` at `end`, as the Gate workflow
does, with a separate 900-second limit. Download, conversion, and reopen failures are
per-file results and do not stop the shard. Files convert independently; assemblies whose
external parts are not present can report missing references. Native files and designs stay
in scratch directories and are never uploaded.

Artifacts, retained for 14 days, are:

- `swscore-fixtures`: the filtered manifest used by both variants.
- `swscore-build-pin` and `swscore-build-main`: the successful binary, available update/build
  logs, and `build.json` (commit, cadmpeg revisions, update/compile outcomes, and `ok`).
- `swscore-<variant>-<shard>`: conversion reports, reopen results, logs, shard metadata, and
  per-file `*-result.json` records with file key, SHA-256, download verification, conversion
  and reopen exit codes, conversion report, completion flag, and any error. Exit 124 means
  the corresponding command exceeded its deadline. Available records survive shard failure.
- `swscore-summary`: `swscore-summary.md` and `swscore-summary.json`. The Markdown also goes
  to the Actions job summary. The JSON contains per-variant builds, counts, per-file first
  blockers, grouped gaps with file lists, and files without comparable evidence.

User timeline counts exclude `not_needed` and `left_out`; mapped counts only user rows with
status `mapped`, and rebuilt sums `history.rebuilt`. Full mapping requires at least one user
feature and successful conversion. Reopen success means the SolidCraft command exited zero.
The first blocker skips downstream reasons matching `after the first blocking` or
`earlier feature .* blocked`. Native kinds are grouped by the feature name's prefix before
its first digit.

The gap table compares first-blocker `(kind, reason)` signatures per file only where both
builds and conversions succeeded and history is available. A signature gone on `main` is
**closed by the pin move**; a new signature is listed under **new on main (regressions)**.
Changed first blockers can expose later gaps, so these labels do not prove full recovery or
a regression by themselves. Unchanged blockers are **Dutchman mapping still lacks** for
explicit mapping/SolidCraft limitations, otherwise **cadmpeg**. This ownership heuristic
first excludes native, unresolved, missing-data, and explicit cadmpeg reasons. Build failures,
missing evidence, and user history disappearing on `main` never count as closed gaps.

To summarize downloaded artifacts locally, preserving their artifact-name directories:

```sh
python3 scripts/swscore-summary.py /path/to/artifacts --out /tmp/swscore-summary
python3 -m unittest discover -s scripts/tests -p test_swscore.py -v
```
