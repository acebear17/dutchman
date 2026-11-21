# Working a Solid Edge issue with an AI agent

For the Solid Edge sub-issues of [#840](https://github.com/dutchman-cad/dutchman/issues/840)
(#841-#854). Read [AGENTS.md](../../AGENTS.md) first: it has the build, the checks and the
pull-request rules. This page adds what is specific to Solid Edge.

Each issue starts with a block to paste into your agent, with that issue's link in it:

```
Work on https://github.com/dutchman-cad/dutchman/issues/<N>
Clone https://github.com/dutchman-cad/dutchman
Follow AGENTS.md and docs/agents/solid-edge.md
If the cadmpeg issue under Depends on is still open, do that one first (its own block is at the top of it)
Use the repro files linked from the issue
Open a pull request when the issue's "How to verify" passes
```

## Where the work splits

cadmpeg reads the file; Dutchman maps what cadmpeg read onto SolidCraft. Never read Solid Edge
bytes in Dutchman: that belongs in cadmpeg's `cadmpeg-codec-solidedge`. Each Dutchman issue names
the cadmpeg issue it needs under **Depends on**, with its **Status**. Work in this order:

1. **Check Depends on.** If the cadmpeg issue is still open, work it first, following
   [cadmpeg's agent guide](https://github.com/cadmpeg/cadmpeg/blob/main/docs/agents/README.md), and
   open the pull request there.
2. **Open the Dutchman pull request right away, as a draft**, built against that cadmpeg pull
   request's branch: point the `cadmpeg-*` crates in the workspace `Cargo.toml` at its branch commit
   (`rev = "<commit>"`, on the repository the branch is in) and say so in the pull request body. Do
   the mapping and its tests (the steps below) on that draft.
3. **When the cadmpeg pull request merges,** Dutchman's pin moves to a cadmpeg `main` commit that
   includes it (maintainers move the pin after a batch of cadmpeg merges; ask in your pull request).
   Merge `main` into your branch, drop the temporary `rev`, verify again against the repro's STEP,
   and mark the pull request ready.
4. **If cadmpeg's review changes what is read,** update the Dutchman mapping and its tests before
   marking ready.

## Steps

1. **Read the issue.** Its **Known**, **Unknown** and **How to verify** sections are the brief.
   Read the cadmpeg issue it depends on, and the repro files and STEP exports attached there.
2. **See what cadmpeg gives you.** Convert a repro with `--json` and read the `history.features`
   rows: each has a `kind`, a `status` and a `reason`. A row refused for a reason Dutchman owns
   is your target.
3. **Find the mapping.** Format detection is in `crates/dutchman-core/src/format.rs`. Feature
   history mapping is in `crates/dutchman-solidcraft/src/history/`, one file per operation
   (`extrude.rs`, `revolve.rs`, `hole.rs`, `blend.rs` for fillets and chamfers, `pattern.rs`,
   `sheet.rs`, `sketch.rs`...). Solid Edge features arrive as the same CADIR operations every
   format uses, so most work extends an existing mapping instead of adding one.
4. **Respect the SolidCraft versions.** Run `scripts/solidcraft-window.sh --print`. New mapping
   targets only the releases it lists; older targets keep the original geometry with a named
   reason. `python3 scripts/check-version-gates.py --base origin/main` checks it.
5. **Test with synthetic models.** Add a test to `crates/dutchman-solidcraft/tests/history.rs`
   that builds the CADIR operation in code (see the helpers near the top, such as
   `cube_with_history`), converts it and checks the mapped feature. Never commit a Solid Edge
   file or a private design.
6. **Verify in SolidCraft.** With a SolidCraft release's `solidcraft-cli` at `SOLIDCRAFT_CLI`, the
   history tests replay the written design and compare volume and shape:
   `SOLIDCRAFT_CLI=/path/to/solidcraft-cli cargo test -p dutchman-solidcraft --test history`.
7. **Verify against the repro's STEP.** Convert the public repro and compare it with its STEP
   export: `dutchman diagnose repro.par --step repro.step --out checked`. Every visible body must
   match. Put the before/after numbers (features mapped, rebuilt, verified) in the pull request.
8. **Open the pull request.** Docs for the change (`docs/formats.md` and the Solid Edge page), a
   `changelog.d/` fragment, "Part of #840" and "Closes #N" in the description. CI runs the full
   matrix, including the SolidCraft round trips.
