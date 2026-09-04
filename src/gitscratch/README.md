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
scratch.check_out_detached("feature")?;
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

A counter has no `Display`, and that is what makes the paragraph above true
rather than merely intended. A count comes back out of one only through a method
that names the rendering: `phrase()` for a sentence, which supplies the noun and
the `s` it takes in the plural, and `digits()` for a table cell whose column
heading carries the noun already. With a `Display` on the counters,
`format!("{} across {}", c.hunks(), c.files())` compiles and prints
`4 across 2` — the exact wording failure these types exist to stop — and `{}` is
the spelling a caller reaches for first while `phrase()` is the one they have to
remember. Neither rendering is free now, so a caller chooses one. `BranchName`
keeps its `Display`, because that type *is* its string.

`is_clean()` reads the file set rather than the counts, and the agreement
between them is structural rather than observed. Adding to the breakdown is the
one door in, and it floors every entry at one hunk, so hunks are non-zero
exactly when the set is non-empty — on the replay path and in a hand-built
fixture alike.

A `Conflicts` a consumer holds is one a replay produced. There is no `Default`
derive on it, because the value that derive hands out — no files, no stops — is
the clean verdict, the one that renders "hit no conflicts" and exits 0, and a
derive puts it behind the spelling every caller reaches for first and behind
every generic route to it besides. The seed a fold really does need is
`Conflicts::nothing_replayed()`, which says at the call site that nothing has
been replayed into it. `Conflicts::from_files()`, the constructor that states a
breakdown, is compiled only under the `testing` feature — and that gate is a
boundary rather than a form of words only because no derive stands beside it.
A `compile_fail` doc-test holds the derive out — see **Testing** below.

`Scratch` is the only way to get a worktree, and `Repo::scratch` is the only way
to get a `Scratch`. A `Scratch` answers the operations it names —
`check_out_detached`, `replay_rebase`, `head_tree`, `commit_tree` — and each of
them builds its own git call under the whole safety configuration. So there is
no way to get a worktree from here without also getting the hardening — nor
without first having established that the directory is a repository at all,
which is the pre-flight's job below.

What a `Scratch` does **not** hand back is the runner that makes those calls. A
scratch worktree is a *linked* worktree of the developer's real repository, so
it shares that repository's refs, configuration and object store — and the
hardening is configuration. It pins the settings that make a *replay*
non-destructive, and it says nothing at all about `branch -D`, `update-ref`,
`config --local` or `push`, because those are different commands and no setting
refuses them. A consumer holding a runner reaches every one of them, in the
developer's own repository.

So the runner stays inside the crate, and both halves of that are needed.
`Git::new` is crate-private, so nothing outside can *build* one; and `Scratch`
answers with the operation rather than with the thing that performs it, so
nothing outside is *handed* one. The crate carried the first half alone for a
while, and the promise was false for the whole of it. A `compile_fail`
doc-test on `Scratch` now holds the second half — see **Testing** below. An
operation this list is missing is a pull request, not a reason to reopen the
door.

The rest of this section is the runner's own rules. A consumer never calls it,
but a reader auditing the harness needs them, because they are what the named
operations above are built on.

Every reader on the runner takes the **subcommand as its own parameter**, ahead
of the arguments, and that shape is a guard rather than a courtesy. Git reads
whatever stands ahead of the subcommand as *its* options, and its rule for two
`-c` pairs naming one key is that the last pair wins — so a caller whose
arguments reached that position could re-pin every setting in the table below,
`rebase.updateRefs=false` included, and could aim the runner at any repository
on the machine with `-C`. Naming the subcommand separately puts every caller
argument after it, where git reads it as an argument of the subcommand. The
bypass therefore does not compile, rather than being refused at run time.

The runner offers exactly one way to read a **list of paths** back out of git,
`nul_separated_paths`, which inserts `-z` right after the subcommand, splits
stdout on NUL without trimming anything, and takes each field as the path those
bytes spell:

```rust
let conflicted = git.nul_separated_paths("diff", &["--name-only", "--diff-filter=U"])?;
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
only reader for a *list*.

**One** path is a different question, and it has the other of the two path
readers, `path`. `rev-parse` has no `-z` at all — it prints the flag back as an
unknown option and exits 0, so a NUL-delimited read of it hands back `-z` and
the path as two fields — and a single answer needs no separator anyway, since
the end of stdout ends the path. So `path` takes git's raw stdout, strips
exactly one trailing newline, and hands the rest to the same byte-for-byte
conversion:

```rust
let state = git.path("rev-parse", &["--git-path", "rebase-merge"])?;
```

One newline rather than a trim, because every other byte of that answer belongs
to the name. `run` and `try_run` trim *and* decode lossily, and are for output
meant for a human. Reading a path back through either of them is a bug, and the
replay read the halted rebase's state directory that way for a while: in a
linked worktree git builds that answer out of the *developer's* own repository
path, so a byte outside UTF-8 anywhere in that path came back as `U+FFFD` and
named a directory nothing holds. `exists()` was then false, the replay reported
no rebase in progress, and a real halt was announced as "the rebase failed
without leaving a rebase to resolve".

The two losses do not reach the same answers, which is exactly why the guard is
one reader rather than a rule per call site. `--git-path` glues a state
directory name onto the end of its answer, so the repository's own last
character sits in the middle and no trim reaches it there — while
`rev-parse --show-toplevel` ends on that character, and a trimming reader takes
it off. A call site that reasoned about which loss its own question is open to
would have to be right twice, every time. Taking the reader is right once.

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

`Repo::resolve` puts the question to git in the one form git can refuse. A bare
`git rev-parse <revision>` prints an argument it cannot place straight back and
exits **0**, so a name that names no commit passed the very check that exists to
catch it, and `grind -- --root` answered `clean` for a branch that does not
exist. The question carries `--verify`, so git answers with one commit id or
fails, and `--end-of-options`, so a revision that starts with a dash arrives as a
revision rather than as a flag. The same separator stands ahead of every other
caller-supplied revision this crate hands to git — the commit-ish of
`worktree add` and the upstream of `rebase` — because each of those is a place
where git accepts an option and answers cheaply instead of refusing.

These live here rather than in each consuming tool for the same reason as
everything else: `Git::new` is crate-private, so a repository-rooted runner can
only be built from inside this crate, and nothing here hands one out either. The
queries are all reads, which fire no hooks, so unlike `Scratch` the pre-flight
creates nothing at all — no temporary directory, no worktree, nothing to clean
up if it rejects.

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
use termbar::TerminalWidth;

// The tool's name alone, because the note below reads nothing else.
let unworded = Report::for_tool("grind");

if let Some(note) = unworded.dirty_note(repo.uncommitted_files()?) {
    eprintln!("{note}");
}

let report = unworded.describing("replaying HEAD onto main");
let columns = usize::from(TerminalWidth::get_or_default());
println!("{}", report.render_within(&conflicts, columns));
```

The tool name and the action arrive through two differently-named calls rather
than as two arguments to one constructor, because both are `&str`: passing them
the wrong way round would compile, and produce a report prefixed with `replaying
HEAD onto main` and indented to the width of it. Named calls cannot be
transposed.

The first call also hands back a *different type*, and that is what closes the
other half of the same defect. `Report::for_tool` returns an `UnwordedReport`,
which has no `render`, no `render_within` and no `without_stops`. Its one route
to the words is `describing`, and `describing` is what builds the `Report`. So
the second call is not a step a caller can leave out: a report carrying half a
sentence has no type to be. One used to, and it printed `grind: clean -  hit no
conflicts`, with two spaces where the action belongs — a hole only whoever reads
the output ever sees. The split takes a second silent mistake away with it: a
`Report` has no `describing`, so calling it twice and keeping the last wording
in silence names a method the worded type does not have. A `compile_fail`
doc-test holds the door — see **Testing** below.

`dirty_note` sits on the unworded type rather than on `Report`, because it reads
the tool's name and nothing else. A caller prints that caveat before it has an
action string at all, which is the order `grind` uses. Both types are `Copy`, so
neither `describing` nor `without_stops` spends the value it was called on.

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

Three things about a file name are the renderer's to handle, and every one of
them happens once, ahead of the measurement and ahead of the print, so the
string that was measured is the string that reaches the screen.

The first is that a name is bytes and a terminal takes text. This is the one
place the crate decodes a name at all, and it decodes it lossily, so a byte
outside UTF-8 becomes U+FFFD here and nowhere earlier — earlier would put that
name back into the map, where it names no file and costs the file its real hunk
count. One replacement character per undecodable byte leaves the rest of the
name alone, which is what lets a developer match `bad-<U+FFFD>.txt` against
their own repository.

The second is that a name holds every byte but NUL — which is the premise the
`-z` reader rests on — so a newline, a carriage return and an ESC are all legal
in one. `render_within` spells every control character out as `\u{...}`. A raw
newline would split one row of this line-oriented layout in two and strand the
count on the second row, and a raw ESC would hand an escape sequence out of the
repository straight to the terminal of whoever ran the tool. The rule is
`char::is_control` and nothing wider, so a leading space, a trailing space,
U+3000, a backslash, a double quote and an emoji all arrive unchanged — every
one of them names a real file, and this crate went to some trouble to carry them
here intact.

The third is that a name has no bound. The count column sits past the widest
name, and one deeply nested path — an ordinary thing to have — carries the
counts of every row off the right-hand edge of the terminal, which then wraps
each of them. `render_within` takes the width of the terminal and clamps the
name column to what is left after the indent, the gap and the widest count. A
name too wide for the clamp takes a row of its own and its count takes the next
row, in the same column as every other count. Here, on a terminal 40 columns
wide:

```console
grind: conflicts - replaying HEAD onto main
       4 hunks across 2 files, 2 stops

  readme.md                      1 hunk
  src/a/very/deeply/nested/directory/with/a/long/name/module.rs
                                 3 hunks
```

The name is never cut short, because a truncated path opens no file. The width
arrives as a parameter and is never read here, because measuring a terminal is
a decision about one program's output and this crate renders for every consumer
that asks. `Report::render` is the same layout with no right-hand edge, for a
caller that has no terminal to name — a test, or anything building this text for
somewhere other than a screen.

This is a deliberate, spec-sanctioned acceptance of a little presentation logic
in a library crate. The alternative is two copies of it.

A replay walks the *whole* operation rather than bailing at the first collision,
resolving as it goes by staging the conflict markers verbatim. That is the
conservative auto-resolution: unlike `--ours` or `--theirs` it never silently
discards a side. It does mean a later commit touching the same region conflicts
again, which is faithful to reality, since a human resolution also leaves later
commits conflicting against the resolved state. Treat a `Conflicts` as a cost
index measured under identical rules, not as an exact prediction.

A hunk is a **closed conflict region**: an opening marker, and the closing
marker that comes after it. Both are matched exactly — seven brackets, then a
space or the end of the line — because that shape is the only thing separating a
marker git wrote from a line of file content that begins with brackets, and
files full of such lines are ordinary. A document about resolving conflicts, a
fixture for a conflict parser and a saved merge transcript all hold them. An
opening marker nothing closes is content by the same rule: it holds one version
of the lines under it rather than two, so there is no decision in it for anyone
to make. A closing marker with no opening one before it closes nothing and
counts as nothing, which is that reasoning read the other way round — counting
it alone would put the over-count back in by the other door. A conflict with no
markers at all — a binary file, a delete/modify — costs one decision, which is
the floor every conflicted file meets.

The "identical rules" are what `merge.conflictStyle=merge` in the table below
buys. `diff3` and `zdiff3` put the base version inside the region, so a base
carrying a line that reads as a marker is measured on a developer who set the
key and not on one who did not.

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

   That probe takes two path lists from git and intersects them here, rather
   than handing the first list back as a pathspec: one argv cannot hold every
   path of a vendored-dependency drop, and a name read back in is a pathspec
   rather than a path. Both of its invocations carry `--ignore-submodules=none`,
   because one of them is plumbing and the other is porcelain, and the porcelain
   one reads `diff.ignoreSubmodules` out of the developer's own configuration.
   Under `diff.ignoreSubmodules=all` a commit that moves a submodule pointer and
   touches nothing else is a path to one command and nothing to the other, which
   reads as a commit that adds nothing to the new base.

Ahead of both probes stands a refusal, because a **merge commit** at a halt is
one neither of them can answer about. `git diff-tree` reports no changed path
for a merge unless it is asked for `-c`, `--cc` or `-m`, and probe 2 asks for
none of them, so an unguarded probe reads the halt as a commit that changes
nothing and `rebase --skip` drops a whole side of history. The probe therefore
counts `REBASE_HEAD`'s parents first and refuses a commit with more than one,
naming it. `rebase.rebaseMerges=false` in the table below closes the route a
developer's own configuration opens; the refusal is the structural half, and it
reads the shape of the commit rather than a setting, so it holds whatever a
later setting does.

That same silence has a second cause, and a flag rather than a refusal answers
it. `git diff-tree` compares a commit against its parent, so it prints no path
for a **root commit** either. Probe 2 therefore asks for `--root`, which compares
such a commit against nothing and names every path it adds. A root commit reaches
a halt in ordinary use: replaying a branch onto one that shares no history with
it replays every commit of that branch, its root commit included. The parent
count lets it through, correctly — a root commit has no parent at all, so the
count is zero.

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
| `core.hooksPath` → an empty directory | No hook fires. An empty *value* is not "hooks off" — git still resolves lookups against it — so the path is a real, empty, temporary directory, validated once at creation. `Repo`'s read-only pre-flight points it at a relative path this crate never creates instead: reads fire no hooks, and rejecting a typo must not be able to fail for want of a writable temp directory. |
| `core.fsmonitor=false` | The one program git runs that the row above cannot take away. The filesystem monitor is executed directly rather than resolved through the hooks directory, so the classic `core.fsmonitor=.git/hooks/fsmonitor-watchman` survives the redirect verbatim and every index refresh a replay performs would run it — in the developer's repository and in the scratch worktree both. `core.fsmonitor=true` costs more than that: git starts a daemon that watches a temporary directory the replay is about to delete. A freshly created scratch worktree gains nothing from a monitor, so the pin costs the replay nothing. Read from git's settings resolution rather than executed; what the test executes is the pin. |
| `GIT_EDITOR`, `GIT_SEQUENCE_EDITOR`, `GIT_TERMINAL_PROMPT` | A halted rebase would otherwise open an editor and hang forever. |
| The inherited git environment, shed | The guard that decides *which* repository every other row protects, and who its commits belong to. Git obeys `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` before it obeys the directory it was pointed at, and it exports them into every hook it runs — `GIT_INDEX_FILE` often the *relative* `.git/index`, which silently re-anchors on whichever directory each command runs in. It exports `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL`/`GIT_AUTHOR_DATE` and their committer siblings too, and an identity variable outranks every config source, `-c` included. Run from a `pre-push` gate, `git bisect run`, `rebase --exec`, or `cargo test` from `.husky/pre-commit`, an unscrubbed simulation aims itself at the hook's repository and stamps the developer's own name on what it commits there. The rule is the `GIT_` prefix and never a list of names: a list strips nothing new the day git adds a variable, and it goes on returning the same clean-looking answer as a list that works — which is how `GIT_CONFIG_PARAMETERS`, a variable git exports to every hook that injects arbitrary configuration, walked through the fifteen-name list this used to carry. Shed at the single place a git process is created, at every fixture spawn, and — via the public `shed_inherited_git_environment` and `NoInheritedGitEnvironment` — at the consumers' own spawns, so the rule cannot drift between them. |
| `commit.gpgsign=false` | A signing config in the developer's global gitconfig would otherwise prompt or fail mid-replay. |
| `gpg.format=openpgp` | Belt to `commit.gpgsign`'s braces. `gpg.format = ssh` is a different signing backend entirely, with its own key and helper program; pinning the format back to git's default means that configuration is never consulted, so signing cannot be attempted through it. |
| `gc.auto=0` | Simulated commits are loose and nothing references them yet; an opportunistic gc could collect one out from under the run. This covers the gc task and nothing else, which is narrower than it reads: the switch on the rest of automatic maintenance is the row below. |
| `maintenance.auto=false` | The other half of automatic maintenance, and the half that reaches the network. Git's `run_auto_maintenance` starts the maintenance tasks unless this key is explicitly false, and the default is to run them — so `gc.auto=0` holds back one task while the rest go ahead. Every resolved conflict runs `rebase --continue`, which commits, and a commit reaches that call. On a developer who has run `git maintenance start` the incremental strategy turns the prefetch task on, and prefetch carries no auto-condition of its own, so `--auto` does not hold it back: it fetches from every remote and writes `refs/prefetch/*` into the developer's repository, because a linked scratch worktree shares the common dir. A dry run that reaches the network and writes refs is the class `gc.auto=0` was added for. Read from git's source rather than executed; what the test executes is the pin. |
| `rebase.autoStash=false`, `rebase.autosquash=false` | The replay must be the operation as written, not a rewritten variant of it. |
| `rebase.rebaseMerges=false` | A rebase that keeps merges puts a merge commit on the replay's todo list, and a merge commit at a halt is a commit the replay cannot measure: `git diff-tree` prints no path at all for one unless it is asked for `-c`, `--cc` or `-m`, and the empty-commit probe asks for none of them. That probe would read the halt as a commit that changes nothing, and `rebase --skip` would drop a whole side of history. Git 2.55 was watched to re-create the merge commit under `rebase.rebaseMerges=true`, so this is a developer's own configuration rather than a hypothetical. The probe refuses a multi-parent stopped commit outright as well, which holds whatever a later setting does; this pin closes the one route into it that exists today. |
| `-z` on the way out, `--literal-pathspecs` on the way in | Git C-quotes a non-ASCII name whenever it prints a path on a line, and a name that begins or ends with whitespace survives git only to lose that whitespace to a trimming reader. `-z` turns the escaping off and separates on the one byte a path cannot contain, so a path comes back as the bytes it is stored under. `--literal-pathspecs` covers the other direction, where a path handed back to git stops being a path and becomes a pathspec: a leading `:` is magic, and `*`, `?` and `[` are wildcards. A pathspec that matches *nothing* is the mild half — it can only add to the paths a probe finds missing, and that only ever buys a refusal nobody needed. The half worth the guard is one that matches the *wrong* file: `:/foo.txt` read as magic means from the top of the working tree, so it silently answers about the root `foo.txt`. No call site in this crate hands a path back to git today — the empty-commit probe intersects two path lists in Rust instead — so the pin protects the next call site that does, at the cost of one argument on the single door every git call goes through. |
| `user.name=gitscratch`, `user.email=gitscratch@localhost` | Scratch commits are throwaway, but they still have to be attributable to the harness that made them rather than to whichever tool is driving it — and a developer's real name and address have no business being stamped on commits that only ever simulated something. The config half settles nothing on its own: an identity variable outranks every config source, `-c` included, which is why the row above sheds the whole inherited environment first. |
| `core.quotePath=false` | Correctness, not cosmetics. By default git C-quotes and octal-escapes any path outside ASCII, so `日本語.txt` comes back from `diff --name-only` as `"\346\227\245\346\234\254\350\252\236.txt"`. That breaks a caller twice: it reports a name nobody typed, *and* the escaped string names no file on disk, so reading it fails and the hunk counter floors that file at 1 — a plausible-looking wrong total. This is the belt, not the braces: it governs only bytes ≥ `0x80`, and git quotes a `"`, a `\` or a control character whatever it is set to. Reading a path list is `Git::nul_separated_paths`'s job and reading one path is `Git::path`'s (both above), and this narrows what a call site that reaches around them can get wrong. |
| `merge.conflictStyle=merge` | The count has to mean the same thing on every machine. All three styles open and close a conflict region with the same markers, so a region whose two sides carry no bracket line of their own costs one hunk under any of them. What `diff3` and `zdiff3` add is the **base** version of the region, between a `|||||||` line and the `=======` one — so a base carrying a line that reads as a marker lands inside the region under those two and outside it under `merge`. The replay then measures a different file on a developer who set the key, and `grist` ranks candidates on that count: two developers comparing the same branches read two orders and neither is told why. Read out of a real merge rather than from git's documentation. |

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
the one loss `-z` cannot undo: a byte that is not UTF-8 has no `String` to come
back *as*, and repairing it into U+FFFD hands back a name no file has — a path
the developer cannot find in their own repository, and a file nothing can open,
which in this crate floors a conflicted file at one hunk and undercounts the
work behind a plausible total. The classification is safe from this one, and the
distinction is worth keeping straight: the empty-commit probe reads both of its
path lists through `Git::paths`, so a lossy decode mangles the two lists the
same way and their intersection is unchanged. macOS will not let a working tree
hold such a name at all, so the commit is built directly in the object database
and the guard is pinned here rather than end-to-end.

**The refusal of a revision that names no commit** is pinned by
`refuses_a_revision_that_starts_with_a_dash_rather_than_echoing_it_back`. Plain
`git rev-parse <revision>` reads a dash-leading argument as an option it does not
know, prints the argument back, and exits 0. The pre-flight read that exit code
as a commit, so `grind -- --root` announced a clean verdict for a branch that
does not exist. `Git::rev_parse` asks with `--verify` and `--end-of-options`
instead: `--verify` makes git refuse a revision it cannot resolve, and
`--end-of-options` ends git's own option position, so the revision arrives as a
revision. The test carries an armed control — plain git, through the fixture,
must still print the argument back at exit 0, or the refusal stands against a
hazard that is already gone.
`resolves_a_revision_that_names_a_commit_to_its_full_id` holds the other side of
the same guard, because a reader that refuses every revision passes the test
above and breaks every caller.

Three more cover the readers themselves. The first two are about an undecodable
name, which the readers answer differently on purpose, and the third is about
the ends of a path, where a reader that trims takes a character off a name that
had one.
`a_non_ascii_path_read_back_through_run_is_not_octal_escaped` pins
**`core.quotePath=false`**, a row above, against `Git::run`. That setting used to
be pinned from the other direction, by `tests/conflicts.rs` asserting the
*answer* a non-ASCII path produces, and it stopped testing this setting the
moment `nul_separated_paths` became the path-list reader: `-z` output is unquoted
whatever `quotePath` says, so removing the pin today leaves every integration
test green — verified. `Git::run` is the surface it still covers, and the one a
future call site would reach for by mistake.
`a_path_git_reports_comes_back_byte_for_byte_even_when_it_is_not_utf8` pins the
other half of the pair, against the refusal named just above it:
`Git::nul_separated` hands back the bytes git wrote, so a name outside UTF-8
survives it, where `Git::paths` refuses one outright. A path on unix is a byte
string with no encoding promised, and a lossy conversion would destroy exactly
the names that reader exists to preserve. The name never touches the filesystem —
APFS rejects it outright — so the test puts it in the index directly.
`a_path_that_ends_in_whitespace_comes_back_with_that_whitespace_intact` pins the
third, `Git::path`, which reads *one* path rather than a list. Its fixture is a
repository whose own directory name ends in whitespace — a space, and U+3000,
which Rust's Unicode-aware `str::trim` eats just as readily — so the repository
spells that character as the last character of its own path, and
`rev-parse --show-toplevel` prints it there. A trimmed answer names a directory
nothing holds, and every question asked of that path is then answered about
nothing. The test carries an armed control: the same answer, read back through
`Git::run`, must be missing exactly that character, or the assertion under it
stands against a loss that is already gone. `--show-toplevel` rather than the
`--git-path` the replay reads, and the limit is worth stating: `--git-path`
glues a state directory name onto the end, so no trim reaches the repository's
own last character there, and the loss that does reach it — a byte outside UTF-8
— is a name APFS refuses to hold at all. One reader carries both losses, so the
half a fixture on this machine can arm pins the reader.

Two more pin **the position a caller's arguments land in**, which is what keeps
every row of the table above from being undone by the caller. Git reads the
arguments ahead of the subcommand as its own options, so an argument list that
reaches that position re-pins any setting `safety_config` fixed — git's rule for
two `-c` pairs naming one key is that the last pair wins — and aims the runner
at any repository on the machine with `-C`.
`an_argument_cannot_re_pin_a_setting_the_safety_config_fixed` smuggles
`-c rebase.updateRefs=OVERRIDDEN` past the runner and requires the pinned
`false` to hold, and
`an_argument_cannot_aim_the_runner_at_another_repository` smuggles a `-C` naming
a second fixture and requires the answer to be about the first. Each carries an
armed control ahead of its assertion, because both assertions say that
something did *not* happen and an assertion of that shape passes just as
cheerfully when the hazard was never live: the first reads the pin back
unmodified and then watches plain git honour the last `-c` pair, and the second
watches plain git follow `-C` into the other repository. The guard itself is
structural rather than a check — `Git` takes the subcommand as a parameter of
its own, so an argument can only land *after* it, where git reads it as an
argument of the subcommand.

Three more pin the settings that let git act on the repository **on its own**,
without any command asking it to. Each fixture arms the opposite value in its own
repository, reads it back through plain git — the armed control, since a setting
the fixture never took is one the runner cannot be shown to override — and then
requires the runner to answer with the pinned value.
`pins_automatic_maintenance_off_even_when_the_repository_turns_it_on` covers
`maintenance.auto=false`, the row `gc.auto=0` does not reach: every resolved
conflict runs `rebase --continue`, which commits, and a commit reaches git's
automatic maintenance.
`pins_the_filesystem_monitor_off_even_when_the_repository_names_one` covers
`core.fsmonitor=false`, the one program git runs that the redirected
`core.hooksPath` cannot take away, since git executes the named path directly.
`pins_merge_preserving_rebase_off_even_when_the_repository_turns_it_on` covers
`rebase.rebaseMerges=false`, which keeps a merge commit off the replay's todo
list — `git diff-tree` reports no changed path for a merge, so a halt on one
reads as a commit that changes nothing. The first two settings are read from
git's own source rather than executed; what these tests execute is the pin
itself. The third was executed: git 2.55 was watched to re-create the merge
commit under `rebase.rebaseMerges=true`.

One more pins the setting that decides **what git writes into a conflicted
file**, which is the thing the hunk counter measures.
`pins_the_conflict_style_even_when_the_repository_asks_for_diff3` covers
`merge.conflictStyle=merge`, against a fixture that asks for `diff3`. The three
styles open and close a region with the same markers, so the count holds for a
region whose two sides carry no bracket line of their own; what `diff3` and
`zdiff3` add is the base version, between a `|||||||` line and the `=======`
one. A base carrying a line that reads as a marker therefore lands inside the
region under those two styles and outside it under `merge`, and the same replay
measures a different file on a developer who set the key. The style each setting
writes was read out of a real merge; what the test executes is the pin.

`src/scratch.rs` holds the structural half of that last one.
`refuses_a_merge_commit_at_a_halt_rather_than_reading_it_as_a_commit_that_changes_nothing`
points `REBASE_HEAD` at a merge commit and requires the empty-commit probe to
refuse it by name. The probe counts the stopped commit's parents before it asks
anything else, so the classification is correct whatever a later setting does,
where the pin above only closes the route into it that exists today. It carries
two controls ahead of the assertion — the fixture's stopped commit really has two
parents, and `diff-tree` really is silent about it — and one after it: the same
probe, pointed at a single-parent commit, has to answer rather than refuse.

The fixture builder stamps commits too, and is covered on its own ground in
`src/testing.rs`, by
`a_fixture_commits_under_its_own_identity_in_a_hook_environment`. It needs an
actual commit to ask about, so it reads `git log` back for author, committer and
both raw dates. It re-executes its own test binary with a hook's identity
variables set on the *child* for the reason the two above do — the same mechanism
`tests/isolation.rs` reaches for.

**Four properties are pinned by doc-tests**, because each is about what a
consumer can *compile* and no ordinary test can state that. Rustdoc compiles a
doc-test as a program outside this crate, which is exactly the seat a consumer
sits in, so a ` ```compile_fail ` block is the only place such a property can be
written down. Each of the four was watched to fail before it was believed —
rustdoc reports `Test compiled successfully, but it's marked compile_fail` —
and each refusal was then read, because a block that failed over a typo, a
renamed method or a missing import reports exactly the same green forever.

- **The door**, on `Scratch`. The block reaches for `scratch.git()`, and it
  passes only while that reach fails to compile. Watched to fail with the runner
  still public, and the refusal read as `error[E0624]: method git is private`.
- **The verdict**, on `Conflicts`. The block writes `Conflicts::default()`, and
  it passes only while the `Default` derive is gone. Watched to fail with the
  derive put back, and the refusal read as `error[E0599]: no associated function
  or constant named default found for struct Conflicts`, whose note points a
  reader at `Conflicts::nothing_replayed` and `Conflicts::from_files`.
- **The bare count**, on the `metrics` module. The block writes
  `format!("{hunks}")`, and it passes only while the counters have no `Display`.
  Watched to fail with the `Display` impl put back on the counter macro, and the
  refusal read as `error[E0277]: Hunks doesn't implement std::fmt::Display`.
- **The unfinished sentence**, on `Report`. The block writes
  `Report::for_tool("grind").render(…)`, and it passes only while `for_tool`
  hands back an `UnwordedReport` that cannot render. Watched to fail with a
  `render` put back on the unworded type, and the refusal read as
  `error[E0599]: no method named render found for struct UnwordedReport<'a>`.

A block asserting that something does *not* compile passes just as readily when
it never compiled for an unrelated reason, so each guard carries a control: a
block beside it that has to compile and that differs by exactly the line under
test. `Scratch`'s control puts the named operations in place of the reach.
`Conflicts`'s control puts a measured `replay_rebase` in place of the derive.
The counter's control puts `hunks.phrase()` in place of `{}`. `Report`'s control
puts `describing` back on the first of the two lines. One line differs, so the
one line is what each ` ```compile_fail ` block measures.

Each mutation reddens its own guard and nothing else across `gitscratch`,
`grind` and `grist`. [`MUTATIONS.md`](./MUTATIONS.md) records the first two in
its map, because what each of them costs is a plausible wrong answer nobody
sees: a runner in a consumer's hands, and a clean verdict for a replay that
never happened. The other two are out of that map, on the basis the two
render-boundary tests in `src/report.rs` set — a `Display` on a counter costs
`4 across 2` and an unworded report costs `grind: clean -  hit no conflicts`,
and both of those are sentences a reader sees on screen. Each still carries its
own record, in `MUTATIONS.md` under a heading that says why it is not a map row.

The rest are about this document rather than about the code.
`every_guard_the_safety_config_pins_is_named_in_the_readme_inventory` asks
`safety_config` what it pins and requires the **What it guarantees** section
above to name every one of them — the whole `key=value` for a settled value, the
key alone for a per-run computed one like `core.hooksPath`, the option verbatim
for a main option like `--literal-pathspecs`. So the inventory is checked, not
merely maintained: a guard added to the configuration and forgotten here fails
the build instead of leaving a reader with a table they will reasonably take for
the complete list. `--literal-pathspecs` is why the test exists — it was
load-bearing in `safety_config` for a while before it was ever a row. It now
makes the case for this check from the other side as well: the empty-commit
probe stopped handing paths back to git, so no test in the workspace goes red
when the pin is removed, and a guard whose own test has gone quiet still has to
be a row.

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

`tests/conflicts.rs` now pins the reader rather than the setting, and pins it
over both classes of name the setting cannot rescue: one git quotes anyway
(`back\slash.txt`, `quo"te.txt`) and one a trimming reader erodes (` lead.txt`,
`trail.txt `, `　wide.txt `, whose leading `U+3000` Rust's Unicode-aware
`str::trim` eats as readily as a space). Both halves of the defect are asserted
together, because they break together — the name and the count. `tests/repo.rs`
covers the other call site's one wrinkle: `status --porcelain -z` spends two
fields on a record that names where the content came from, and such a record is
one uncommitted file. All four spellings of it have a test there, because the
pairing reads two status bytes for two letters and an arm nothing can fail is an
arm nobody can trust. `git mv` writes `R` in the index column;
`status.renames=copies` — a key this crate pins nothing about, so it arrives out
of the developer's own configuration — writes `C` there beside the modification
of the source; and both letters reach the *working-tree* column where the
destination is in the index with no content behind it, which is what
`git add -N` records and what `git add -p` records for a new file. The two new
tests each carry an armed control, since an undetected copy comes back as
`A  copy.txt`, one field for one file, and the count is then right without the
pairing ever running.

**The removed location variables**, the `GIT_DIR` row above, are pinned by
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

`tests/halts.rs` covers the other half of telling the truth: not that the
harness leaves the repository alone, but that it does not report a cheap number
for work it dropped. It puts a replay in each halt state *for real* — a resolved
conflict whose commit cannot be written, a clean pick whose commit cannot be
written, a commit that genuinely became empty, and a `--skip` git refuses — by
making the object database unwritable, which is the only cause of a failed
commit write still reachable through the harness once signing, hooks and the
editor are pinned off. It is Unix-only for that reason. The last of those four
states is named for what the replay believes about it rather than for what git
did: `git rebase --skip` exits non-zero whenever the rebase is left unfinished,
so the replay cannot tell a skip git refused from a skip that worked and halted
again. [`MUTATIONS.md`](./MUTATIONS.md) records that, and records why it is
written down rather than fixed here.

Three of those clean picks are there for what the probe reads a path *as*, and
every one of them asserts the *classification* rather than merely that something
failed — the commit must never be called empty.
`refuses_to_report_a_cost_when_a_clean_pick_of_quoted_paths_could_not_be_committed`
is the spelling git prints: a commit touching nothing but a `café.txt` and a name
with a leading space, the two spellings a line-oriented read mangles, with no
plainly-spelled file alongside to carry the refusal on its own.
`refuses_to_report_a_cost_when_a_clean_pick_of_a_pathspec_magic_path_could_not_be_committed`
is the spelling a pathspec reads back: a `foo.txt` inside a directory literally
named `:`, with an untouched `foo.txt` at the root for the magic spelling to
answer about instead, so a probe built out of pathspecs gets an empty diff back —
a true answer to a question nobody asked.
`refuses_to_report_a_cost_when_a_clean_pick_of_a_submodule_pointer_could_not_be_committed`
is the path git declines to print at all: a commit that moves a submodule pointer
and touches nothing else, in a fixture that sets `diff.ignoreSubmodules=all` in
its own configuration. `diff-tree` is plumbing and reports the submodule;
`git diff` is porcelain and reads that key, so it reports nothing. One tree, two
sets of rules, and the answer that comes out of them is "this commit adds
nothing". Both invocations now ask for `--ignore-submodules=none`, and the test
carries an armed control proving the setting really does hide the pointer from
the porcelain, so a git that stopped honouring it fails the test rather than
quietly emptying it.

A fourth clean pick asks a different question: whether git names a path there at
all.
`refuses_to_report_a_cost_when_a_clean_pick_of_a_root_commit_could_not_be_committed`
puts a **root commit** at the halt, on a fixture whose second history was started
with `git checkout --orphan`. `diff-tree` compares a commit against its parent,
so it prints nothing for a commit that has none until it is asked for `--root`,
and a probe without the flag reads the first commit of a whole history as a
commit that changes nothing. Two controls stand ahead of the replay: the branch's
only commit really has no parent, read back through `rev-list --parents`, and
`diff-tree` really is silent about that commit until the flag is added. The
parent count that refuses a merge commit is not the guard here and must not be —
a root commit has no parent, so the count is zero and the refusal passes it
through.

The first two of those were written when the probe handed its paths back to git
as pathspecs, and that round trip is gone — `missing` is now the intersection of
the two path lists, computed in Rust, so a spelling that changes on the way out
changes identically on both sides and no argv has to carry every path of a
commit. Both tests keep their worth as the answer asserted end to end. What the
magic-path one no longer does is redden when `--literal-pathspecs` is removed,
which [`MUTATIONS.md`](./MUTATIONS.md) records rather than leaves to be
re-derived.

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

`tests/repo.rs` covers the pre-flight separately, since what it must get right
is the *cheap rejection*: a directory that is not a repository and a revision
that does not resolve both have to fail there, by name. The premise for the
first of those comes from `not_a_repository()` rather than from a bare
`TempDir`, so a `TMPDIR` that turns out to sit inside a repository is reported
where the mistake is instead of as the pre-flight accepting a directory it
should have refused. `scratch_refuses_a_revision_that_starts_with_a_dash_rather_than_building_one_at_head`
covers the revision that reaches the worktree rather than the reader:
`git worktree add -q --detach <path> --force` is a complete and valid command,
so without `--end-of-options` git reads `--force` as its own flag and builds the
worktree at HEAD at exit 0 — a scratch of a revision nobody asked for, and every
number measured in it about another branch.

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
`日本語.txt` comes back by its real name carrying its real hunk count. It also
asserts that a fold adds the stop counts of the steps it takes. Stops are the
one measure a fold carries as a number of its own rather than as a sum of the
breakdown, so a fold that dropped a step's stops would leave every other
assertion in that file green. The name test is deliberately built on a file
contested in *two* regions — with one, the undercount and the truth would both
be 1 and the defect would pass. `Report`'s
own tests sit beside it in `src/report.rs`, because rendering a `Conflicts` is
pure string work that needs no repository at all. Two of them cover the render
boundary, where every byte this crate carried intact finally becomes text.
`a_name_that_is_not_valid_utf_8_is_rendered_with_the_replacement_character`
builds the name straight out of bytes — `Conflicts::from_files` takes a
`PathBuf`, and APFS refuses such a name on disk — and asserts both the U+FFFD
the decode writes and that the count of that row still starts in the column its
ASCII sibling's does.
`a_name_that_holds_a_space_or_an_emoji_is_rendered_as_it_stands` covers the
printable characters the escape rule was written to leave alone: a leading
space, a trailing space, U+3000 and an emoji, asserted through the rendered text
rather than through the map. Both were watched to fail — replacing the decode
with a `to_str` that gives up reddens the first, and trimming the converted name
reddens the second.

Every column assertion in that file reads through one helper, `count_column`,
and the helper has a test of its own —
`the_count_column_is_read_from_the_last_place_the_count_appears`. The count is
the last thing on a row, so the helper reads the last place the count appears. A
name can spell its own count, `11 hunks.txt` being a legal file name, and a
helper that stopped at the first occurrence answers with the column the *name*
starts in — leaving every alignment assertion built on it passing or failing for
a reason that has nothing to do with alignment. `grind`'s command-line suite
carries the same helper over the rendered output of the binary, and the same
test beside it.

**That a counter's noun is the counter's own business** is pinned by a unit test
in `src/metrics.rs` on `Uncommitted` — the newest counter, and the one whose
noun is two words rather than one, so it is where the macro's suffix-`s` rule is
most worth asserting. The doc-test pair in that module's own documentation pins
the half a unit test cannot reach: that a caller has no way to route around the
noun by printing the bare number. See the doc-test account above.
`src/report.rs` pins the other end of the same seam: the note reads as a
sentence at one file and at three, with the noun coming from the counter and the
verb from the renderer, so the two cannot drift apart unnoticed.
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
reported as one the harness gave up on.

A `--skip` round costs one too, and that half is pinned by nothing, because
nothing can pin it. The skip arm reads git's outcome the moment it comes back and
stops the replay unless git exited zero, and `git rebase --skip` exits zero only
when it has finished the rebase — git 2.55 exits 1 for a skip that worked and
then met a conflict, and again for one that worked and then met a second empty
commit. So no round can follow a skip round, and moving the charge into the
conflict arm alone changes no answer this suite can produce. The charge stays at
the top of the loop because the rule is that a round of work costs a round, and a
charge written into one arm leaves the next arm uncounted.
[`MUTATIONS.md`](./MUTATIONS.md) records it as unfalsifiable rather than as a
guard somebody watched fail.

**That a `Conflicts` cannot contradict its own accessors** is pinned by three
further unit tests in `src/scratch.rs`, which need no repository either. The
stop count is a `Stops` in the field as well as in the accessor, so there is no
unwrapping step between the two for those tests to have to cover. One test
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

A fixture runs git two ways. `TestRepo::git` raises a failed command as a panic,
which is what building a fixture wants. `TestRepo::try_git` hands the `Output`
back instead, which is what a **control** wants — the command a test runs to
prove the hazard it is guarding against is really armed, whose failure *is* the
demonstration and so has to be read rather than raised. Both shed the inherited
git environment; only the assertion differs. That is the point of the second
one: without it a control buys its permission by reaching around the fixture for
a raw `Command`, and loses the scrub in the same move — and `current_dir` does
not settle which repository git uses, because `GIT_DIR` outranks it, so the
control then merges or commits in the developer's own repository. `try_git`
applies a caller's own variables *after* the sweep, which is what lets a control
pin `GIT_TERMINAL_PROMPT=0`, or the `LC_ALL` it needs to read git's words rather
than their translation, without the `GIT_` prefix rule taking them straight back
off.

It also gates `Conflicts::from_files`, the hand-built-breakdown constructor
`Report`'s tests are built on. Every call site is a fixture, and a released
binary has no business stating a cost that nothing measured, so the constructor
is simply not compiled into one. What makes that a boundary rather than a form
of words is that nothing ungated stands beside it: `Conflicts` lost its
`Default` derive, and the one constructor a released binary keeps is
`nothing_replayed()`, which states no breakdown and names itself as the seed of
a fold.

It also holds `path_at_or_above`, the safety matcher that reads the output of a
destructive tool for a path at or above the work tree of a
`DetachedGitDirRepo` — the work tree that stands in for `$HOME`. `gitnuke`,
`nodenuke` and `repotidy` each assert that it answers `None` for a run in a
detached git directory, and each of the three carried a byte-identical copy of
it. A copy that widened left the other two narrow, and a matcher that finds too
little answers `None` for the wrong reason, which is the defect those three
guards exist to stop. The matcher reads one line at a time and tries the
longest candidate of each start first, so a path that holds a space reaches the
comparison. Its mutation test sits beside it in `src/testing.rs`:
`the_path_check_flags_the_work_tree_and_the_directory_above_it` plants four
paths the matcher must flag, one of them under a directory whose name holds a
space, and one path under the work tree that it must pass.

`DetachedGitDirRepo` carries a precondition of its own, and it runs before the
fixture builds anything. The fixture leaves no `.git` entry, so a tool that
walks upward for one finds whatever stands above the temporary directory. A
repository up there becomes the root that tool works in, and `nodenuke` deletes
what it walks into without asking. `TempDir` reads `TMPDIR`, so a `TMPDIR`
inside a checkout aims every guard built on this fixture at that checkout.
`init` therefore asks `ancestor_repository` whether the temporary directory sits
inside a repository, and panics with the offending path when it does. A path
check that reads a run's output is a post-mortem; this one runs first.
`the_ancestor_check_finds_the_repository_a_directory_sits_inside` pins it, and
[`MUTATIONS.md`](./MUTATIONS.md) records both directions it was watched to fail
in.

| Fixture | Shape |
| --- | --- |
| `contested_region_repo()` | `iterated` rewrites one region across three commits, `single` touches it once — the asymmetry that makes a stop count worth printing. |
| `stacked_branches_repo()` | `built-on-top` branched from `groundwork`, not from main. |
| `equal_hunks_unequal_stops_repo()` | Two branches making the same two edits, packaged as one commit and as two, so they tie on hunks and differ on stops. |
| `independent_branches_repo()` | Two branches that each add a file of their own, so nothing can conflict. |
| `conflicting_repo()` | Two branches rewriting the same line, so a replay is guaranteed to conflict and resolve. |
| `nested_conflict_repo()` | The same collision in `shared.txt` and in `sub/nested/shared.txt`, so a tool can be run from a committed subdirectory two levels down — one conflict inside it, one outside it. |
| `modify_delete_repo()` | A branch that modifies the file `main` deleted, so a replay is a modify/delete conflict. Its auto-resolution stages a blob the object store already holds, so this is the shape that gets all the way to `rebase --continue` under a sealed object database and fails on the commit write alone — the resolution staged, and nothing unmerged. Sealing is a permission trick, so the tests that reach these halts are Unix only. |
| `branches_behind_main_repo()` | Two branches that each add a file of their own, and a `main` that has moved past both, so replaying either has to *write* a commit rather than move a ref. The pick applies cleanly, so a sealed object database halts it with nothing unmerged and nothing dirty — the state a commit that genuinely became empty is otherwise indistinguishable from. |
| `branches_behind_main_with_quoted_and_space_led_paths_repo()` | The same shape with the branch's work in `café.txt`, which git C-quotes whenever it prints a path alone on a line, and ` leading space.txt`, which any trim of that line silently shortens. Neither path is plainly spelled, deliberately: an ordinary sibling in the same commit would come back matching and carry the refusal on its own, leaving what the mangled names cost invisible. |
| `branches_behind_main_with_a_pathspec_magic_path_repo()` | The same shape again, with the branch's work in `:/foo.txt` and a decoy `foo.txt` committed at the root. Nothing mangles on the way out; the leading `:` is pathspec magic on the way back in, so the name asks about the decoy neither side touched — the direction that *shrinks* the set of paths a probe finds missing to empty, which is a halt read as a commit to skip. The probe builds no pathspec today, so what this fixture asks for now is the answer: such a commit is never called empty. Unix only, because the filesystem has to hold a directory named `:`. |
| `branches_behind_main_with_a_submodule_pointer_bump_repo()` | The same shape once more, with the branch's work in a submodule pointer and `diff.ignoreSubmodules=all` set in the fixture's own configuration. `diff-tree` reports the moved gitlink and `git diff` reports nothing, so a probe that asks one command what the commit touched and the other whether the new base holds it reads one tree under two sets of rules — and calls a commit empty because the porcelain declined to mention its only path. The pointer's two values are commits of this repository's own object database, so no second repository is cloned or kept alive; a `.gitmodules` entry sits in the base commit, where a real superproject records one, leaving the bump commit touching the gitlink alone. |
| `unrelated_histories_repo()` | `main` and `unrelated` share no commit at all: `git checkout --orphan` gives the second branch a root commit of its own, and the two histories name their files differently so the pick applies cleanly. Replaying `unrelated` onto `main` therefore puts a **root commit** at a halt, which is where `diff-tree` prints no path until it is asked for `--root`. Every other fixture here starts from a base commit both branches share, so this is the one shape that reaches that question. Sealing the object database halts the pick with nothing unmerged and nothing dirty, as `branches_behind_main_repo()` does for a commit with a parent. |
| `commit_emptied_by_main_repo()` | A branch whose first commit reaches content `main` arrived at by a different route, followed by a second commit that is real work. No commit shares a patch id with one on the other side, so git cannot drop the first early: under `--empty=stop` the rebase halts on it legitimately, and the second still has to survive. |
| `multi_byte_names_repo()` | Branches `left-左` and `right-右` colliding in `readme.md` and `日本語.txt` — a name git would escape, a hunk count that collapses when it does, and two names whose byte, character and column widths disagree. |
| `awkward_names_repo()` | Conflicts in names git C-quotes whatever `core.quotePath` says — a backslash, a double quote — beside names with leading and trailing whitespace, including U+3000. Each is contested in two regions, so a mangled name floors at one hunk and the count fails, not just the spelling. Unix only, because the filesystem has to hold the names. |
| `not_a_repository()` | A directory outside every repository, which checks its own premise and says so if `TMPDIR` turns out to sit inside one. |
| `TestRepo::bare_clone(head)` | A `BareRepo`: `worktree add` succeeds there but `status --porcelain` cannot run, so a pre-flight query can fail where the replay still answers. |
| `DetachedGitDirRepo::nested()` and `::beside()` | A repository whose git directory is detached from its work tree, the way `yadm` keeps a directory of dotfiles. The fixture leaves no `.git` entry anywhere, so a walk upward for one finds no repository at all. `nested` puts the git directory inside the work tree, which is what `yadm` does, and git reports `--is-inside-work-tree` as true from there. `beside` puts it outside, and git reports that same question as false and `--is-inside-git-dir` as true, so code that reads either answer needs both shapes. Like `not_a_repository()`, it checks its own premise and refuses a `TMPDIR` that sits inside a repository. |

```toml
[dev-dependencies]
gitscratch = { workspace = true, features = ["testing"] }
```

## Used by

- [`grist`](../grist/README.md) — ranks squash-merge orderings by conflict cost
- [`grind`](../grind/README.md) — would rebasing HEAD onto this branch conflict,
  and by how much?
