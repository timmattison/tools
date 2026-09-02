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
use gitscratch::Repo;

// The pre-flight first — it is also the only route to a worktree. See below.
// A detached worktree at `main`, in a temp directory, torn down on drop.
let scratch = Repo::open(repo_path)?.scratch("main")?;

// Check the candidate out detached, then replay it.
scratch.git().run(&["checkout", "-q", "--detach", "feature"])?;
let conflicts = scratch.replay_rebase("main")?;

if conflicts.is_clean() {
    // Nothing conflicted.
} else {
    for (file, hunks) in conflicts.file_hunks() {
        // `file` is a `&Path` — git's own bytes, never decoded, so it is
        // converted lossily here at the moment of printing and nowhere
        // earlier. `hunks` is a `Hunks` — the same type the headline total
        // comes back as, so it already knows its own noun.
        println!("{}: {}", file.display(), hunks.phrase());
    }
}
```

A `Conflicts` records how many times the replay halted and, for every file that
conflicted, how many hunks it contributed. The headline totals — `hunks()`,
`files()`, `stops()` — are summaries of that breakdown rather than numbers
tracked beside it, so the total and the list underneath it cannot tell a reader
two different stories.

Every count that crosses the boundary is one of the newtypes in `metrics`, in
both directions: `file_hunks()` yields a `&Path` and a `Hunks` for the same
reason `hunks()` does, so a renderer never throws the type away and immediately
rebuilds it, and counts that are all `usize` underneath can never be transposed
on the way in or out. That holds off the conflict path too —
`Repo::uncommitted_files()` returns an `Uncommitted`, whose noun is the whole
`"uncommitted file"`, so the one count that is *not* about conflicts still
arrives knowing what to call itself.

`is_clean()` reads the file set rather than the counts, and the agreement
between them is structural rather than observed. Adding to the breakdown is the
one door in, and it floors every entry at one hunk, so hunks are non-zero
exactly when the set is non-empty — on the replay path and in a hand-built
fixture alike.

`Scratch` is the only way to get a worktree, and `Repo::scratch` is the only way
to get a `Scratch`. A `Scratch` hands out a `Git` that already carries the whole
safety configuration, so there is no way to get a worktree from here without
also getting the hardening — nor without first having established that the
directory is a repository at all, which is the pre-flight's job below.

That `Git` offers exactly one way to read a **list of paths** back out of git,
`nul_separated_paths`, which inserts `-z`, splits stdout on NUL without trimming
anything, and takes each field as the path those bytes spell:

```rust
let conflicted = git.nul_separated_paths(&["diff", "--name-only", "--diff-filter=U"])?;
```

The contract is byte-exact in both halves. `nul_separated` underneath it hands
back `Vec<Vec<u8>>` — git's bytes, for output whose fields are not all paths,
such as a `status --porcelain -z` record of `XY <path>` — and
`nul_separated_paths` converts each field with no decoding step at all, because
on unix a path *is* an arbitrary byte string. Decoding one lossily would put
`U+FFFD` where the bytes were, which is the same two-part failure C-quoting
causes: a name nobody typed, and a name that opens no file.

There is deliberately no line-oriented equivalent, because one cannot be made
correct. Git C-quotes a path containing `"`, `\` or a control character no
matter how `core.quotePath` is set, so a quoted name arrives naming no file on
disk; and a name that merely begins or ends with whitespace arrives intact and
is destroyed by the reader instead, since Rust's `str::trim` is Unicode-aware
and strips `U+3000` as readily as a space. Either way the path cannot be opened,
and in this crate a conflicted file that cannot be opened is floored at one hunk
— a wrong total that looks entirely plausible. `-z` is the one mode with no
quoting and a separator no path can contain, so the reader that uses it is the
only reader there is. `run` and `try_run` trim *and* decode lossily, and are for
output meant for a human.

## The pre-flight

Not every question is worth a worktree. `Repo` answers the cheap ones first, so
a mistyped branch name fails in milliseconds with a message naming it, rather
than arriving later disguised as a failed simulation:

```rust
use gitscratch::Repo;

let repo = Repo::open(cwd)?;           // any directory *inside* one; errors if there is none
let onto = repo.resolve("main")?;      // errors naming the revision that did not resolve
let dirty = repo.uncommitted_files()?; // an `Uncommitted`: staged + unstaged + untracked, per file

let scratch = repo.scratch(&onto)?;    // the only way to a worktree
```

These live here rather than in each consuming tool for the same reason as
everything else: `Git::new` is crate-private, so a repository-rooted runner can
only be built from inside this crate. The queries are all reads, which fire no
hooks, so unlike `Scratch` the pre-flight creates nothing at all — no temporary
directory, no worktree, nothing to clean up if it rejects.

And it is not optional. `Repo::scratch` is the only public entrance to a
`Scratch`, so the last line above is not a convenience — it is the only line
that compiles. `Repo` deliberately does *not* hand its path back: a pre-flight a
caller can walk around is a suggestion, and handing back the opened directory
would have made the checked path and an unchecked one the same `&Path`,
indistinguishable at the call site. The validated path never leaves the type, so
the worktree comes out of the thing that validated it.

## The report

`Report` turns a `Conflicts` into the words a developer reads. It lives here,
not in the binaries, because `grind` (rebase) and `grime` (merge) ask different
questions and have to print the same shape — and two renderers would drift apart
on exactly the details that make the two answers comparable at a glance:

```rust
use gitscratch::Report;

let report = Report::for_tool("grind").describing("replaying HEAD onto main");

if let Some(note) = report.dirty_note(repo.uncommitted_files()?) {
    eprintln!("{note}");
}
println!("{}", report.render(&conflicts));
```

The tool name and the action arrive through two differently-named calls rather
than as two arguments to one constructor, because both are `&str`: passing them
the wrong way round would compile, and produce a report prefixed with `replaying
HEAD onto main` and indented to the width of it. Named calls cannot be
transposed. `Report` is `Copy`, so neither `describing` nor `without_stops`
spends the value it was called on.

```console
grind: conflicts - replaying HEAD onto main
       4 hunks across 2 files, 3 stops

  src/lib.rs     3 hunks
  src/main.rs    1 hunk
```

The only variation the two tools get is `Report::without_stops()`, which drops
the stop count for `grime`: a merge halts exactly once, so the number would be a
constant dressed up as a measurement. Everything else is fixed — the indent is
measured from the tool's own name, the counts are padded in *display* width so a
CJK filename still lines its column up, and every noun is pluralised by the
metric newtype that owns it rather than by whoever is printing it. The dirty
note is the shape of that last rule: `dirty_note` takes an `Uncommitted` and
chooses only the verb, because "1 uncommitted file **is**" and "3 uncommitted
files **are**" are one agreement written in two places, and the half that names
the thing being counted belongs to the counter.

This is a deliberate, spec-sanctioned acceptance of a little presentation logic
in a library crate. The alternative is two copies of it.

A replay walks the *whole* operation rather than bailing at the first collision,
resolving as it goes by staging the conflict markers verbatim. That is the
conservative auto-resolution: unlike `--ours` or `--theirs` it never silently
discards a side. It does mean a later commit touching the same region conflicts
again, which is faithful to reality, since a human resolution also leaves later
commits conflicting against the resolved state. Treat a `Conflicts` as a cost
index measured under identical rules, not as an exact prediction.

## What it guarantees

| Guard | Why |
| --- | --- |
| `rebase.updateRefs=false` | Without it, rebasing a detached HEAD still rewrites every branch ref pointing into the replayed range — including the branch being simulated. Not paranoia: with the setting enabled and the guard removed, a dry run *destroys the branch it is replaying*. The claim holds on every machine only because the row below pins the backend alongside it; under the apply backend the setting is inert either way. |
| `rebase.backend=merge` | `--update-refs` is a merge-backend feature, and the apply backend ignores it outright. Left unpinned, the row above is unfalsifiable on a developer who prefers apply — it could be deleted and nothing on that machine would notice, because the backend already silences what it overrides. The backend also decides where a halted rebase files its state, `rebase-merge` rather than `rebase-apply`, so a consumer inspecting an interrupted replay reads the same repository everywhere. |
| `rerere.enabled=false`, `rerere.autoupdate=false` | A simulated resolution would otherwise land in the shared `rr-cache` and silently pre-resolve the developer's real merges later. |
| `core.hooksPath` → an empty directory | No hook fires. An empty *value* is not "hooks off" — git still resolves lookups against it — so the path is a real, empty, temporary directory, validated once at creation. `Repo`'s read-only pre-flight points it at a relative path this crate never creates instead: reads fire no hooks, and rejecting a typo must not be able to fail for want of a writable temp directory. |
| `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_TERMINAL_PROMPT` | A halted rebase would otherwise open an editor and hang forever. |
| `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_PREFIX`, `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES` **removed** | The guard that decides *which* repository every other row protects. Git obeys these before it obeys the directory it was pointed at, and it exports the first four into every hook it runs — so anything a hook spawns inherits them. Run from a `pre-push` gate, `git bisect run`, `rebase --exec`, or `cargo test` from `.husky/pre-commit`, an unscrubbed simulation aims itself at the hook's repository: `git init` re-initialises it, `git config` overwrites the developer's identity in it, and `git add` stages phantom entries into its index. Removed at the single place a git process is created, at every fixture spawn, and — via the public `NoInheritedRepository` — at the consumers' own spawns, so the list cannot drift between them. |
| `commit.gpgsign=false` | A signing config in the developer's global gitconfig would otherwise prompt or fail mid-replay. |
| `gpg.format=openpgp` | Belt to `commit.gpgsign`'s braces. `gpg.format = ssh` is a different signing backend entirely, with its own key and helper program; pinning the format back to git's default means that configuration is never consulted, so signing cannot be attempted through it. |
| `gc.auto=0` | Simulated commits are loose and nothing references them yet; an opportunistic gc could collect one out from under the run. |
| `rebase.autoStash=false`, `rebase.autosquash=false` | The replay must be the operation as written, not a rewritten variant of it. |
| `user.name=gitscratch`, `user.email=gitscratch@localhost` | Scratch commits are throwaway, but they still have to be attributable to the harness that made them rather than to whichever tool is driving it — and a developer's real name and address have no business being stamped on commits that only ever simulated something. |
| `core.quotePath=false` | Correctness, not cosmetics. By default git C-quotes and octal-escapes any path outside ASCII, so `日本語.txt` comes back from `diff --name-only` as `"\346\227\245\346\234\254\350\252\236.txt"`. That breaks a caller twice: it reports a name nobody typed, *and* the escaped string names no file on disk, so reading it fails and the hunk counter floors that file at 1 — a plausible-looking wrong total. This is the belt, not the braces: it governs only bytes ≥ `0x80`, and git quotes a `"`, a `\` or a control character whatever it is set to. Reading a path list is `Git::nul_separated_paths`'s job (above); this narrows what a call site that reaches around it can get wrong. |

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

A tenth guarantee — **the `user.name`/`user.email` identity** — is pinned by a
unit test in `src/git.rs` instead, which reads back `git var GIT_AUTHOR_IDENT`
rather than building a repository to commit into.

**`core.quotePath=false`**, the last row above, is pinned by a second unit test
in `src/git.rs`, for a reason worth stating: it used to be pinned from the other
direction, by `tests/conflicts.rs` asserting the *answer* a non-ASCII path
produces. That stopped testing this setting the moment `nul_separated_paths`
became the only path reader: `-z` output is unquoted whatever `quotePath` says
— remove the pin today and all eighteen integration tests stay green, verified.
The unit test asserts it against `Git::run` instead, the surface it still
covers.

`tests/conflicts.rs` now pins the reader rather than the setting, and pins it
over both classes of name the setting cannot rescue: one git quotes anyway
(`back\slash.txt`, `quo"te.txt`) and one a trimming reader erodes (` lead.txt`,
`trail.txt `, `　wide.txt `, whose leading `U+3000` Rust's Unicode-aware
`str::trim` eats as readily as a space). Both halves of the defect are asserted
together, because they break together — the name and the count. `tests/repo.rs`
covers the other call site's one wrinkle: `status --porcelain -z` spends two
fields on a rename, and a rename is one uncommitted file.

**The removed location variables**, the last row above, are pinned by
`tests/isolation.rs`, which has to reach for a mechanism the rest of the suite
does not. `std::env::set_var` is process-global and `unsafe`, and Rust runs a
binary's tests as threads of one process, so poisoning the environment there
would race every other test. The tests re-execute the test binary instead, with
the variables set on the *child* — which is the leak verbatim, a whole process
whose environment names another repository, and is parallel-safe because nothing
outside that child ever sees them. Each one builds a victim repository, snapshots
the file the leak corrupts, and asserts the bytes are identical afterwards: a
snapshot rather than a second interrogation through git, because once a phantom
index entry points at an object the victim does not have, git's own answers about
it stop being trustworthy. Both shapes are covered — the severe one, where the
fixture directory never gets a `.git` at all, and the `GIT_INDEX_FILE`-only one a
`pre-commit` hook produces on its own. `grind`'s `tests/cli.rs` pins the same
thing end to end through the binary.

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

`tests/repo.rs` covers the pre-flight separately, since what it must get right
is the *cheap rejection*: a directory that is not a repository and a revision
that does not resolve both have to fail there, by name. The premise for the
first of those comes from `not_a_repository()` rather than from a bare
`TempDir`, so a `TMPDIR` that turns out to sit inside a repository is reported
where the mistake is instead of as the pre-flight accepting a directory it
should have refused.

It also pins the other half of `Repo::open`'s contract: the directory it is
handed may be any directory *inside* the repository, which is what every run
from a subdirectory — that is, nearly every run — depends on. The validated path
is private, so nothing outside the crate can inspect it and only behaviour can
be asserted. `nested_conflict_repo()` is opened two levels down and has to
resolve a revision to the same commit the root-opened `Repo` does, count a file
edited at the repository *root* as uncommitted, and still hand back a worktree
standing on the fixture's own `main`. `grind`'s `tests/cli.rs` closes the same
loop through the binary, where the quiet failure lives: a run started in
`sub/nested` has to name the two conflicted files `shared.txt` and
`sub/nested/shared.txt` — the prefix kept, and neither file dropped for sitting
outside the directory the run began in — and print byte-identically to the same
run from the root.

Three mutations pin that pair, each failing only the assertion it belongs to
while the rest of the workspace's suite stays green: making `Repo::open` refuse
a non-empty `rev-parse --show-prefix`, scoping `uncommitted_files` to the cwd
with a `-- .` pathspec, and reducing each conflicted path to its last component.
The last of those is why the fixture is contested in two places at two depths —
every other fixture's conflicts sit at the repository root, where a
root-relative name and a cwd-relative one are the same string.

`tests/conflicts.rs` covers the answer rather than the safety of getting it:
whether a replay conflicted at all, that the per-file breakdown accumulates
across stops and adds up to the total it explains, and that a conflicted
`日本語.txt` comes back by its real name carrying its real hunk count. That last
one is deliberately built on a file contested in *two* regions — with one, the
undercount and the truth would both be 1 and the defect would pass. `Report`'s
own tests sit beside it in `src/report.rs`, because rendering a `Conflicts` is
pure string work that needs no repository at all.

**That a counter's noun is the counter's own business** is pinned by a unit test
in `src/metrics.rs` on `Uncommitted` — the newest counter, and the one whose
noun is two words rather than one, so it is where the macro's suffix-`s` rule is
most worth asserting. `src/report.rs` pins the other end of the same seam: the
note reads as a sentence at one file and at three, with the noun coming from the
counter and the verb from the renderer, so the two cannot drift apart unnoticed.
A default `Uncommitted` is a clean tree, which is what lets a caller that could
not measure fall back to saying nothing rather than to saying something about
zero files.

One test there is honest about being a compile-time guarantee in a test's
clothing: `Report`'s `Debug` and `Copy` derives cannot fail an assertion, only a
build — drop `Copy` and using a report after `without_stops()` is a
use-after-move, drop `Debug` and the format string does not compile. It is
written as a test so that whoever deletes a derive trips over the reason it was
added. What it *does* assert at runtime is the part a hand-written `Debug` could
still get wrong: that the representation names the tool and the action, which is
the only thing that makes it worth anything in a failing assertion's message.

**The replay's round budget** is pinned by unit tests in `src/scratch.rs`, which
the integration suite could not serve: the constant is 1000, and the case that
matters is a replay needing exactly that many rounds. So the tests name the
budget instead — `replay_rebase_within` is `replay_rebase` with the bound as a
parameter — and spend it on `contested_region_repo()`, whose three colliding
commits take exactly three rounds. Both sides of the boundary are asserted:
three rounds must produce the answer, two must still refuse. Noticing that the
rebase has *finished* costs no round, so a fully-measured replay is never
reported as one the harness gave up on. A `--skip` round does cost one, because
a `--skip` that leaves the rebase halted and still empty is exactly the runaway
the bound exists to catch.

**That a `Conflicts` cannot contradict its own accessors** is pinned by three
further unit tests in `src/scratch.rs`, which need no repository either. One
attributes a measured count of zero to a file and requires the total to come
back as one hunk: the floor lives at the single door into the breakdown, so it
covers the replay path — the one place a count is still measured at runtime —
and not just the constructor, whose `NonZeroUsize` makes a zero-hunk file
unspellable. The other two require the constructor to refuse a breakdown and a
stop count that disagree about whether anything conflicted, in both directions:
stops with no files would otherwise render the clean line and swallow them, and
files with no stops would report a replay that never halted.

Consumers pin what they compose on top of the harness. `grist`'s own
`tests/safety.rs` asserts that a full simulation — its `checkout --detach` →
`replay_rebase` → `squash_into` sequence, which this crate's tests cannot see —
leaves every real branch ref where it found it.

The `testing` feature exposes `gitscratch::testing`: throwaway git repositories
with known conflict shapes, shared by every crate built on the harness so the
fixtures exist once rather than once per test binary. Every fixture lives in its
own `TempDir`, so concurrent `cargo test` runs never share a path.

It also gates `Conflicts::from_files`, the hand-built-breakdown constructor
`Report`'s tests are built on. Every call site is a fixture, and a released
binary has no business minting a verdict that nothing measured, so the
constructor is simply not compiled into one.

| Fixture | Shape |
| --- | --- |
| `contested_region_repo()` | `iterated` rewrites one region across three commits, `single` touches it once — the asymmetry that makes a stop count worth printing. |
| `stacked_branches_repo()` | `built-on-top` branched from `groundwork`, not from main. |
| `equal_hunks_unequal_stops_repo()` | Two branches making the same two edits, packaged as one commit and as two, so they tie on hunks and differ on stops. |
| `independent_branches_repo()` | Two branches that each add a file of their own, so nothing can conflict. |
| `conflicting_repo()` | Two branches rewriting the same line, so a replay is guaranteed to conflict and resolve. |
| `nested_conflict_repo()` | The same collision in `shared.txt` and in `sub/nested/shared.txt`, so a tool can be run from a committed subdirectory two levels down — one conflict inside it, one outside it. |
| `multi_byte_names_repo()` | Branches `left-左` and `right-右` colliding in `readme.md` and `日本語.txt` — a name git would escape, a hunk count that collapses when it does, and two names whose byte, character and column widths disagree. |
| `awkward_names_repo()` | Conflicts in names git C-quotes whatever `core.quotePath` says — a backslash, a double quote — beside names with leading and trailing whitespace, including U+3000. Each is contested in two regions, so a mangled name floors at one hunk and the count fails, not just the spelling. Unix only, because the filesystem has to hold the names. |
| `not_a_repository()` | A directory outside every repository, which checks its own premise and says so if `TMPDIR` turns out to sit inside one. |
| `TestRepo::bare_clone(head)` | A `BareRepo`: `worktree add` succeeds there but `status --porcelain` cannot run, so a pre-flight query can fail where the replay still answers. |

```toml
[dev-dependencies]
gitscratch = { workspace = true, features = ["testing"] }
```

## Used by

- [`grist`](../grist/README.md) — ranks squash-merge orderings by conflict cost
- [`grind`](../grind/README.md) — would rebasing HEAD onto this branch conflict,
  and by how much?
