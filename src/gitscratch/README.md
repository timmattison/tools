# gitscratch

The hardened harness for dry-running a git operation without touching anything
real.

Answering "would this rebase conflict, and how badly?" means actually performing
the rebase, and performing it means running git against the developer's own
repository. That is only safe because of a specific set of pinned settings. This
crate owns them, so that every tool asking that question inherits the same
guarantees instead of each reimplementing a weaker version.

## The interface

```rust
use gitscratch::Scratch;

// A detached worktree at `main`, in a temp directory, torn down on drop.
let scratch = Scratch::create(repo_path, "main")?;

// Check the candidate out detached, then replay it.
scratch.git().run(&["checkout", "-q", "--detach", "feature"])?;
let conflicts = scratch.replay_rebase("main")?;

println!(
    "{} hunks across {} files, {} stops",
    conflicts.hunks(),
    conflicts.files(),
    conflicts.stops()
);
```

`Scratch` is the only door in. It hands out a `Git` that already carries the
whole safety configuration, so there is no way to get a worktree from here
without also getting the hardening — which is the point.

A replay walks the *whole* operation rather than bailing at the first collision,
resolving as it goes by staging the conflict markers verbatim. That is the
conservative auto-resolution: unlike `--ours` or `--theirs` it never silently
discards a side. It does mean a later commit touching the same region conflicts
again, which is faithful to reality, since a human resolution also leaves later
commits conflicting against the resolved state. Treat a `Conflicts` as a cost
index measured under identical rules, not as an exact prediction.

## Three ways a rebase halts, and why the third one matters

A halted rebase with **no unmerged paths** is a classification point, not a
single known case. Git stops there for a commit that adds nothing to the new
base — free to drop — and it stops there for a commit it could not *write*,
where dropping it throws the work away and answers with a cost for a branch
that was never replayed. Signing, hooks, a full or read-only object database, an
unusable editor all land in that same state, and git exits non-zero for the
harmless case too, so nothing about the invocation separates them.

| Halt | What it means | What the replay does |
| --- | --- | --- |
| Unmerged paths | A human would hand-merge these | Count them, stage the markers, continue |
| Nothing unmerged, commit adds nothing | The work is already in the new base | `rebase --skip`, costs nothing |
| Nothing unmerged, commit could not be written | git refused to create the commit | **Fail**, naming the commit and quoting git |

The third row is separated from the second by two probes, both of them
repository state rather than a match on git's wording, since wording changes and
state does not:

1. **Anything left uncommitted.** A commit that truly became empty leaves the
   index matching `HEAD` and the worktree matching the index. Content left
   behind is content that failed to be committed.
2. **Whether the stopped commit's work is already in the new base.** A failed
   commit write on a *clean* pick leaves nothing behind at all — git rolls the
   index back and reschedules the pick — so probe 1 has nothing to see. What
   still separates the two is the commit itself: for every path `REBASE_HEAD`
   touches, does `HEAD` already hold exactly that content? If so the commit is
   genuinely empty, and airtight rather than heuristic — applying `C` onto
   `HEAD` is a three-way merge (base `C^`, ours `HEAD`, theirs `C`), and on a
   path where both sides already agree the merge changes nothing, while a path
   `C` never touched cannot change either.

A refused `git rebase --skip` fails the replay immediately, carrying git's own
message, rather than being re-issued until the round limit runs out.

Both probes err toward the loud answer, which is the safe direction: a dry run
may say "this is expensive" or "I cannot answer", but never "this is cheap"
because it quietly discarded the work.

## What it guarantees

| Guard | Why |
| --- | --- |
| `rebase.updateRefs=false` | Without it, rebasing a detached HEAD still rewrites every branch ref pointing into the replayed range — including the branch being simulated. Not paranoia: with the setting enabled and the guard removed, a dry run *destroys the branch it is replaying*. The claim holds on every machine only because the row below pins the backend alongside it; under the apply backend the setting is inert either way. |
| `rebase.backend=merge` | `--update-refs` is a merge-backend feature, and the apply backend ignores it outright. Left unpinned, the row above is unfalsifiable on a developer who prefers apply — it could be deleted and nothing on that machine would notice, because the backend already silences what it overrides. The backend also decides where a halted rebase files its state, `rebase-merge` rather than `rebase-apply`, so a consumer inspecting an interrupted replay reads the same repository everywhere. |
| `rerere.enabled=false`, `rerere.autoupdate=false` | A simulated resolution would otherwise land in the shared `rr-cache` and silently pre-resolve the developer's real merges later. |
| `core.hooksPath` → an empty directory | No hook fires. An empty *value* is not "hooks off" — git still resolves lookups against it — so the path is a real, empty, temporary directory, validated once at creation. |
| `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_TERMINAL_PROMPT` | A halted rebase would otherwise open an editor and hang forever. |
| `commit.gpgsign=false` | A signing config in the developer's global gitconfig would otherwise prompt or fail mid-replay. |
| `gpg.format=openpgp` | Belt to `commit.gpgsign`'s braces. `gpg.format = ssh` is a different signing backend entirely, with its own key and helper program; pinning the format back to git's default means that configuration is never consulted, so signing cannot be attempted through it. |
| `gc.auto=0` | Simulated commits are loose and nothing references them yet; an opportunistic gc could collect one out from under the run. |
| `rebase.autoStash=false`, `rebase.autosquash=false` | The replay must be the operation as written, not a rewritten variant of it. |
| `-z` on the way out, `--literal-pathspecs` on the way in | A path read out of one invocation goes straight back into the next as a pathspec, and a pathspec is not a path: a leading `:` is magic, `*`, `?` and `[` are wildcards, and git C-quotes a non-ASCII name on the way out while dequoting nothing on the way back in. `-z` turns the escaping off; `--literal-pathspecs` turns the magic off. A pathspec that matches *nothing* is the mild half — it can only add to the paths a probe finds missing, and that only ever buys a refusal nobody needed. The half worth the guard is one that matches the *wrong* file: `:/foo.txt` read as magic means from the top of the working tree, so it silently answers about the root `foo.txt`, and if that one is unchanged the diff comes back empty. An empty diff reads as a commit that adds nothing to the new base, which is a `rebase --skip`, which is the work gone and a cost of zero reported for a branch that was never replayed. |
| The inherited git environment, shed | Configuration is only as strong as the environment it runs in, because git reads the environment *first*. A tool built on this crate can be invoked from inside a git hook, and git hands its hooks the commit it is making: `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL`/`GIT_AUTHOR_DATE` naming the developer, and `GIT_INDEX_FILE` — often the *relative* `.git/index`, which silently re-anchors on whichever directory each command runs in. `GIT_DIR` and `GIT_WORK_TREE` travel the same way. Every one is stripped, so a replay cannot be re-attributed or aimed at the repository the developer was committing to. `shed_inherited_git_environment` is public: the danger is not this crate's alone, and the list belongs in one place. |
| `user.name=gitscratch`, `user.email=gitscratch@localhost` | Scratch commits are throwaway, but they still have to be attributable to the harness that made them rather than to whichever tool is driving it — and a developer's real name and address have no business being stamped on commits that only ever simulated something. |

Teardown removes the scratch worktree **by path** and deliberately never runs
`git worktree prune`. Pruning is repo-wide and immediate: it deletes the
administrative state — including any halted rebase — of every worktree whose
directory is merely *missing right now*, which is the normal condition for a
worktree on an unmounted drive or a sleeping network mount. A dry run must not
cost the developer a worktree.

## Testing

`tests/safety.rs` pins nine properties across eight tests, each verified by
mutation — break the guard, watch that specific test fail, put it back. The two
counts differ because the teardown test pins two of them, the removal itself and
the backend its halted rebase is inspected under, and each gets a bullet:

- **`rebase.updateRefs=false`**, asserted with the setting deliberately turned
  *on* in the repository being replayed — and with `rebase.backend = apply`
  armed beside it and left armed through the replay, since the harness picks
  its own backend rather than inheriting the developer's.
- **The detached checkout**, which is what lets a branch already checked out in
  another worktree be replayed at all. It is spelled out in the test rather than
  hidden behind a library call precisely because it is a guard.
- **The absence of `git worktree prune` in teardown.** This one is mutated in
  the opposite direction — *add* a prune and watch the test fail — because the
  guarantee is that it is not there.
- **`rerere.enabled=false`**, asserted with rerere deliberately turned on: a
  conflicting replay must leave `rr-cache` unwritten.
- **`core.hooksPath`**, asserted by planting `post-checkout`, `pre-rebase`,
  `post-rewrite` and `pre-merge-commit` hooks that each touch a sentinel, and
  proving no sentinel appears.
- **The scratch worktree itself**, asserted by dirtying the real working tree
  and index three ways — a tracked edit, a staged change, and an untracked file
  that no reflog or stash could get back — and requiring every one of them to
  survive a replay unchanged, the two on disk compared byte for byte.
- **`worktree remove --force` in teardown**, asserted after a clean run, after a
  resolved conflict, and after a `Scratch` dropped while a rebase was still
  halted — the path most likely to leak a registration.
- **`rebase.backend=merge`**, pinned by that same halted-rebase case: it locates
  the halted rebase at `rebase-merge` in a fixture that arms
  `rebase.backend = apply`, so removing the harness's pin sends the state to
  `rebase-apply` and turns the test red.
- **`commit.gpgsign=false`**, asserted with signing turned on and a key that
  cannot resolve. The replay runs under a timeout, so the test catches a hang on
  a passphrase prompt and not only an outright failure.

The unit tests in `src/git.rs` pin what needs no repository built around it.
Some are about the code itself. **The `user.name`/`user.email` identity**, the
last row above, is read back through `git var GIT_AUTHOR_IDENT` rather than by
committing into a fixture, by
`commits_under_the_crate_s_own_identity_not_a_consuming_tool_s`, which reads it
in an environment carrying nothing of its own — the way the suite runs from a
shell. `the_pinned_identity_survives_a_hook_environment` reads it again under
the environment a git hook hands down, where `GIT_AUTHOR_NAME` and its siblings
outrank every `-c` the harness pins. **The inherited git environment, shed** is
asserted by
`ignores_an_inherited_git_environment_naming_another_identity_or_repository`,
which adds another repository's `GIT_DIR` and `GIT_INDEX_FILE` to that
environment and watches neither reach git. Both of those last two run in a
re-executed child of the test binary rather than in it: the environment is
process-wide, and mutating it in place would reach every sibling test and every
concurrent run of the suite. Each of them names the child by a libtest filter,
which is a string the compiler never checks against the test it names, so a
child that ran counts only when it *says* it ran — one sentinel line, printed
at the end of the child's body and required in its output alongside a
successful exit. The exit status alone cannot carry that, because libtest
exits 0 when a filter matches no test at all, so a renamed test and a passing
one look the same from the parent's side.
`a_child_half_that_matched_no_test_is_a_failure_not_a_pass` pins the refusal:
it hands the shared child runner a filter naming no test in this file and
requires a failure that says so. The rename that breaks a filter is the rename
that hides the breakage, which is why the two guards cannot be left to police
their own filters. **The UTF-8
refusal in `Git::paths`** —
`refuses_a_path_that_is_not_valid_utf_8_rather_than_replacing_the_byte` — covers
the one loss the `-z` round trip cannot undo: a byte that is not UTF-8 has no
`String` to come back *as*, and repairing it into U+FFFD would hand back a name
no file has, which is a pathspec matching nothing, which is how a commit gets
called empty and skipped. macOS will not let a working tree hold such a name at
all, so the commit is built directly in the object database and the guard is
pinned here rather than end-to-end.

The rest are about this document rather than about the code.
`every_guard_the_safety_config_pins_is_named_in_the_readme_inventory` asks
`safety_config` what it pins and requires the **What it guarantees** section
above to name every one of them — the whole `key=value` for a settled value, the
key alone for a per-run computed one like `core.hooksPath`, the option verbatim
for a main option like `--literal-pathspecs`. So the inventory is checked, not
merely maintained: a guard added to the configuration and forgotten here fails
the build instead of leaving a reader with a table they will reasonably take for
the complete list. `--literal-pathspecs` is why the test exists — it was
load-bearing in `safety_config` for a while before it was ever a row.

More of them pin the *scope* those checks read, since a check pointed at the
wrong span of the file reports clean without ever having seen the table.
`the_inventory_section_stops_at_the_next_heading_of_any_level` ends a section at
the next heading whatever its level: demoting the heading below the inventory —
one character — would otherwise widen it to swallow the prose here, which names
`--literal-pathspecs` and `core.hooksPath`, the exact two guards matched by bare
name, so both would be satisfiable with no row for either. Its last fixture ends
the section at a setext heading — `Testing` over a rule of dashes — the one
heading spelling that carries no `#`, so a lexical cut cannot pass this test.
`an_inventory_section_that_nothing_closes_is_refused_rather_than_run_to_the_end`
shuts the same gap from the other side: a section with no heading after it is
refused outright rather than read to the end of the file.
`a_hash_that_is_not_a_heading_does_not_end_a_section` shuts the third, the one
this very section fell down. A `#` is not a heading: the `#329` below is the
wrapped tail of an `Issue`, and a `#` opening a line inside a fenced shell block
is a comment. A cut that stopped at either ended this section well short of its
own end — so a test named down there read as named nowhere — and, worse, handed
the refusal above a boundary to be satisfied by, leaving an unbounded scope
reporting clean. The bounds come from a CommonMark parse for that reason: it is
the only thing that answers "is this line a heading?" instead of answering some
particular spelling of the question.

One more turns that treatment on this section.
`every_unit_test_in_this_file_is_named_in_the_readme_testing_section` reads
`src/git.rs` back as text, collects every test defined in it, and requires the
paragraphs above to name each one. Two of this README's lists have drifted out
from under it — the guard table, and this walkthrough, which went on describing
four tests through the commits that added two more — so this one is checked
rather than trusted too.

`tests/halts.rs` covers the other half of telling the truth: not that the
harness leaves the repository alone, but that it does not report a cheap number
for work it dropped. It puts a replay in each halt state *for real* — a resolved
conflict whose commit cannot be written, a clean pick whose commit cannot be
written, a commit that genuinely became empty, and a `--skip` git refuses — by
making the object database unwritable, which is the only cause of a failed
commit write still reachable through the harness once signing, hooks and the
editor are pinned off. It is Unix-only for that reason.

Two of those clean picks are there for the path round trip specifically, one per
direction, and both assert the *classification* rather than merely that
something failed — the commit must never be called empty.
`refuses_to_report_a_cost_when_a_clean_pick_of_quoted_paths_could_not_be_committed`
is the way out: a commit touching nothing but a `café.txt` and a name with a
leading space, the two spellings a line-oriented read mangles, with no
plainly-spelled file alongside to carry the refusal on its own.
`refuses_to_report_a_cost_when_a_clean_pick_of_a_pathspec_magic_path_could_not_be_committed`
is the way back in: a `foo.txt` inside a directory literally named `:`, with an
untouched `foo.txt` at the root for the magic spelling to answer about instead,
so the probe's diff comes back empty — a true answer to a question nobody asked.

`tests/hook_environment.rs` runs a whole replay under the environment `git
commit` actually exports to a pre-commit hook — the everyday way a consumer is
invoked, since a pre-commit hook running a test suite runs whatever that suite
drives. It is a test binary of its own because the environment is process-wide,
and cargo gives every integration test file its own process.

The genuinely-empty halt is driven explicitly rather than assumed: on git 2.55 a
patch already upstream is dropped without halting, a resolution that empties a
commit is dropped silently by `rebase --continue`, and the `rebase.empty` config
key is ignored on this path — only `--empty=stop` on git's command line reaches
that stop. The test starts that rebase itself and asserts the halt happened, so
a git that stops halting fails the test instead of quietly passing without
exercising anything.

The remaining rows of the table above are established by construction rather
than by a test of their own, in two different places. `gpg.format`, `gc.auto`,
and the `rebase.autoStash`/`autosquash` pair are entries in `safety_config`,
which returns `-c key=value` arguments and nothing else; the editor and prompt
environment — `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_TERMINAL_PROMPT` — is
set on the command itself, in `Git::try_run`. The editor guard is at least
exercised indirectly: every conflict test above drives a rebase that halts, and
a halted rebase without `GIT_EDITOR` set sits waiting on a commit message.

`gpg.format` looks like the signing test covers it and it does not, which is
worth saying out loud so nobody re-derives the wrong answer. That test's fixture
pins `gpg.format=openpgp` itself, deliberately — the format selects *which*
program config git reads, so without it the fake signing program the fixture
names would go unused on a developer who has `gpg.format = ssh` set globally —
and `openpgp` is the same value `safety_config` pins. Removing the harness's
entry would therefore change nothing that test can observe. The pin earns its
place for the reason the table gives; it is just not what makes that test pass.

[`MUTATIONS.md`](./MUTATIONS.md) records which guard each test pins, where that
guard lives, and the failure output captured when it was removed. It also
records the other half of the question — what keeps each test *honest*: the
start-state control proving the fixture began where the test needs it to, and
the armed control proving the hazard would really have fired without the guard.
That second half is the one that rots, and it rots green. Anyone changing
`safety_config`, `Scratch::create` or the teardown should re-run the relevant
mutation rather than trusting a green suite.

Consumers pin what they compose on top of the harness. `grist`'s own
`tests/safety.rs` asserts that a full simulation — its `checkout --detach` →
`replay_rebase` → `squash_into` sequence, which this crate's tests cannot see —
leaves every real branch ref where it found it.

The `testing` feature exposes `gitscratch::testing`: throwaway git repositories
with known conflict shapes, shared by every crate built on the harness so the
fixtures exist once rather than once per test binary. Every fixture lives in its
own `TempDir`, so concurrent `cargo test` runs never share a path.

```toml
[dev-dependencies]
gitscratch = { workspace = true, features = ["testing"] }
```

## Used by

- [`grist`](../grist/README.md) — ranks squash-merge orderings by conflict cost
