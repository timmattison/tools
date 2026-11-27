# Buffalo Tools - Development Guidelines

## Shell Integration

All tools in this repository that provide shell integration (shell functions, aliases, etc.) **must** use the `shellsetup` library crate located at `src/shellsetup/`.

### When to add `--shell-setup` at all (read this first)

**Only add `--shell-setup` when it is absolutely necessary — when the tool genuinely cannot do its job from a normal child process.** Writing into a user's shell rc file is intrusive, has to be maintained across upgrades, and is the kind of thing users reasonably distrust. Default to *not* shipping shell integration.

A shell function is only **load-bearing** when the tool must affect the *parent shell's* state — something a child process physically cannot do. The legitimate cases:

- **Changing the parent shell's working directory** (`cd`). A child process cannot change its parent's cwd, so a `cd`-ing tool *must* be a shell function. Examples: `crap` (cd-and-resume), `cwt`/`nwt` (switch/create worktree and land you there).
- **Exporting environment variables into the current session**, modifying shell options, or otherwise mutating live shell state.

If the tool does **not** need to mutate the parent shell, **do not add `--shell-setup`.** Before adding it, exhaust the native alternatives:

1. **A direct invocation or flag** — if `mytool --rm` already does the job, ship that as the interface. Don't wrap it.
2. **A subcommand or second binary** for the variant behavior.
3. **Documentation** telling users to add their own `alias` if they want a shorthand. A convenience alias is the user's choice to make, not something we install into their rc file.

A shell function that merely forwards arguments to the binary (`function prmv() { prcp --rm "$@"; }`) is **cosmetic, not load-bearing** — it adds no capability the binary lacks. That is not a sufficient reason to touch the user's shell config. `prcp` shipped exactly that function and lost it in #265: `prcp --rm` is the interface, and a user who wants the shorthand writes `alias prmv='prcp --rm'`.

### Why

The `shellsetup` library provides:
- Consistent marker-based block detection for upgrades
- Automatic shell detection (bash/zsh)
- In-place replacement of existing shell integration when users re-run `--shell-setup`
- Support for upgrading old installations that lack end markers
- Standardized output and instructions

### Usage

```rust
use shellsetup::ShellIntegration;

const SHELL_CODE: &str = r#"
function mycommand() {
    mytool "$@"
}
alias mc='mycommand --fast'
"#;

fn setup_shell_integration() -> Result<()> {
    let integration = ShellIntegration::new("mytool", "My Tool", SHELL_CODE)
        .with_command("mycommand", "Run mytool")
        .with_command("mc", "Run mytool with --fast")
        .with_old_end_marker("alias mc='mycommand --fast'");  // For upgrading old installs
    integration.setup().map_err(|e| anyhow::anyhow!("{}", e))
}
```

### Important: Using `with_old_end_marker()`

**When to use:** If your tool has ever been released with shell integration that users may have installed, you **must** call `.with_old_end_marker()` with a distinctive pattern from the end of the old shell code block. This allows the library to safely upgrade old installations.

**What to use as the marker:** Choose the last distinctive line of your old shell code. Good candidates are:
- The last alias definition (e.g., `alias mc='mycommand --fast'`)
- A distinctive command inside your last function (e.g., `mytool --rm "$@"`)

**Why this matters:** Without an old end marker, upgrading from an old installation may lose user config that appears after the old shell integration block. The library will warn users if this happens, but it's better to prevent it.

### yadm-Managed Shell Configs

`ShellIntegration::setup()` is **yadm-aware**. Before writing, it inspects the directory next to the target rc file (e.g. `~/.zshrc`) for yadm alternates named `<file>##...`:

- **No alternates** → writes the rc file directly (normal case).
- **Exactly one `##template*` alternate** (e.g. `~/.zshrc##template.default`) → writes the integration block to the **template** instead of the rendered file, and prints `yadm alt` re-render instructions. This prevents the block from being silently discarded on the next render.
- **Multiple templates, or a non-template alternate** (`##os.Darwin`, `##class.work`, …) → refuses with `ShellSetupError::YadmAmbiguousConfig`, listing the candidates and the block to add by hand. Choosing the right alternate requires yadm's class/OS rules, which the library does not evaluate.

This logic is centralized in `resolve_config_target`, so every consumer (`crap`, `cwt`, `nwt`) benefits without code changes.

### Tools Currently Using shellsetup

- `cwt` - Change Worktree (provides `wt`, `wtf`, `wtb`, `wtm` commands)
- `nwt` - New Worktree (provides the worktree-creation cd function)
- `crap` - Claude, Resume Anywhere Please (provides the `crap` cd-and-resume function)

## Progress Bar Display

All tools in this repository that display progress bars **should** use the `termbar` library crate located at `src/termbar/`.

### Why

The `termbar` library provides:
- Terminal width detection with fallback. `TerminalWidth` reads two sources and
  decides between them. A width the environment states in `COLUMNS` wins, which
  is the rule POSIX gives that variable. The controlling terminal answers when
  the environment states none, through the `termsize` crate, so a terminal that
  carries no window reports no width and the fallback stands. Such a terminal
  answers the `TIOCGWINSZ` ioctl with zero columns, and the ioctl succeeds, so a
  caller that reads the raw answer lays a progress bar out at no columns.
  `script -q /dev/null` makes one.
- A test of a tool that lays text out for a terminal states the width in
  `COLUMNS` on the command it starts. `cargo test` gives the test binary the
  terminal of whoever started the run, and the read of the controlling terminal
  reaches it through `/dev/tty` whatever standard output points at. A test that
  compares a laid-out block against a golden therefore passes in a wide window
  and fails in a narrow one, which is the same class of defect as a test that
  compares painted output against plain text. `src/grind/tests/cli.rs` states
  the width on every run it builds, and
  `src/grind/tests/controlling-terminal.rs` holds the rule itself against a
  pseudo-terminal of a size it chose.
- Progress bar width calculation that adapts to terminal size
- Pre-built progress bar styles (copy, verify, batch, hash)
- Escape function for template braces in filenames
- Unicode-aware display width calculation for filenames
- Optional async terminal resize watching via SIGWINCH with clean shutdown

### Usage

```rust
use termbar::{ProgressStyleBuilder, TerminalWidth, calculate_bar_width, PROGRESS_CHARS};

// Create a copy-style progress bar
let width = TerminalWidth::get_or_default();
let style = ProgressStyleBuilder::copy("myfile.txt")
    .build(width)
    .map_err(|e| anyhow::anyhow!("{}", e))?;

// Or use width calculation for custom templates
let bar_width = calculate_bar_width(width, 80); // 80 = overhead
let template = format!("{{spinner}} [{{bar:{}.cyan}}] {{msg}}", bar_width);
```

### Terminal Resize Watching

For applications that need to respond to terminal resize events:

```rust
use termbar::TerminalWidthWatcher;

// Create watcher with automatic SIGWINCH handling
let (watcher, resize_task, shutdown_tx) = TerminalWidthWatcher::with_sigwinch_channel();

// Get current width or watch for changes
let width = watcher.current_width();
let receiver = watcher.receiver();

// When done, signal shutdown by dropping the sender or sending explicitly
drop(shutdown_tx);  // or shutdown_tx.send(())
resize_task.await;
```

Benefits of the channel-based shutdown:
- Clean shutdown without polling overhead
- Immediate task termination when signaled
- Idiomatic async Rust patterns

### Available Style Builders

- `ProgressStyleBuilder::copy(filename)` - File copy operations (cyan bar)
- `ProgressStyleBuilder::verify(filename)` - File verification (yellow bar)
- `ProgressStyleBuilder::batch()` - Batch operations with file counts (blue bar)
- `ProgressStyleBuilder::hash()` - Hash operations with message (cyan bar)

### Tools Currently Using termbar

- `prcp` - Progress Copy (copy, verify, and batch styles)
- `prhash` - Progress Hash (custom template with dynamic width)
- `grind` - the progress bar of the replay, through `gitscratch`'s `Console` (feature `progress`)
- `grime` - the spinner of the merge phases, through the same `Console`

## Colored Output in Tests

Every test that asserts on text a tool painted with the `colored` crate **must**
compare visible glyphs, through the `testcolor` library crate at
`src/testcolor/`.

### Why

`colored` decides at format time whether to write ANSI escape codes. One input
to that decision is whether file descriptor 1 is a terminal. A test that
compares painted output against plain text thus passes when the run writes to a
file and fails when the run writes to a terminal.

`cargo test` hands the test binary the terminal of whoever started it. A
redirected run hides such a test, and a hand-typed `git commit` fails it,
because the pre-commit hook passes its terminal straight through.

This is not a flake. The result is deterministic on a condition the test does
not control, and it stays hidden until somebody commits from a terminal. `cwt`
shipped two such tests in #361. They blocked the first commit that touched
`Cargo.lock`, because a lockfile change is what makes the hook run the Rust
gate at all, so an unrelated dependency bump wore the blame.

### Usage

Take the crate as a dev-dependency with `testcolor.workspace = true`, then
force the codes on and take them back out:

```rust
let glyphs = testcolor::strip_ansi(&testcolor::with_forced_ansi(|| render(&snap)));
assert_eq!(glyphs, "> /repo [main]\n");
```

- `strip_ansi` - the one stripper. Two hand-written strippers agree on the
  common sequences and part company on the rare ones.
- `with_forced_ansi` - turns the codes on for one body, under the one lock, and
  puts the override back even when the body panics.
- `max_red_channel` and `TRUECOLOR_FG` - read a 24-bit foreground color back out
  of painted text.

Forcing the codes on and stripping them beats reading `render` raw. The
assertion then covers the painted output, which is the output a user reads.

### The ban

The override of the `colored` crate is process-global, and `cargo test` runs the
tests of one binary on many threads. A test that sets the override directly
changes what an unrelated test sees. The `clippy.toml` beside the workspace
manifest bans all four spellings of that call, and `disallowed_methods` is
`deny` in the workspace lint set, so a bypass fails the build.

A tool that decides its own color output at startup is the one legitimate
caller. It says so at the call site with
`#[expect(clippy::disallowed_methods, reason = "...")]` - see `gsw` and `seescc`.
The exemption stays visible in the file it applies to, rather than in a central
allowlist.

### 24-bit colors read `COLORTERM`

`colored` reads `COLORTERM` each time it formats a 24-bit color. If the value
is not `truecolor` or `24bit`, it writes the nearest of the 16 basic colors.
`colored` gives no API to override this read, and `with_forced_ansi` does not
change it. Thus a test that asserts on a 24-bit color passes only where
`COLORTERM` says `truecolor`.

`scripts/rust-checks.sh` pins `COLORTERM=truecolor` for the test run, thus the
hook and CI give the same result in each terminal. To run such a test by hand
in a terminal that does not set `COLORTERM`, set it on the command:
`COLORTERM=truecolor cargo test -p gsw`.

### Crates Currently Using testcolor

- `cdva` - the long-help test of `--strict`
- `crap` - the tests of the `--expand` notes
- `cwt` - family render tests, and the end-to-end helpers that read the output
  of the binary
- `faulte` - the help and usage-error tests of the binary
- `gitscratch` - render and paint tests of the halt diffs
- `glance` - the usage-error tests of the binary
- `grime` - `--diff` paint tests, through a pipe and on a terminal
- `grind` - `--diff` paint tests, through a pipe and on a terminal
- `gsw` - render, watch, push, and update run tests
- `nwt` - the usage error that clap gives for the removed `--tmux` flag
- `prcp` - the `--shell-setup` refusal and help tests, and the end-to-end tests of
  `-R`, the landing preview, and the final check
- `prgz` - render tests of the closing report
- `swt` - the end-to-end helpers that read stdout and stderr of the binary
- `wn` - row, plan, and graph render tests, and the end-to-end tests that read
  the rows through a wrapper and the progress line on a terminal

## Shell Quoting

Every tool in this repository that puts a value into a shell command line
**must** quote that value with the `shellquote` library crate at
`src/shellquote/`.

### Why

Four tools built the same single-quoted string, and only one of them had
tests. A rule that lives in four places drifts apart, and the copy that drifts
is the copy nobody tested.

`shellquote::shell_quote` holds the rule once. It puts single quotes around
every value, and it rewrites an embedded single quote as `'\''` — close the
quotation, escaped quote, open the quotation again. The empty string becomes
`''`, which is one empty word. Its tests hand each result to `/bin/sh` and
check that the shell gives the value back as exactly one argument.

### Usage

```rust
use shellquote::shell_quote;

let script = format!("command -v {}", shell_quote(name));
```

Take the crate with `shellquote.workspace = true`. Call the function by its
full path, `shellquote::shell_quote(...)`, where the call site sits behind a
`#[cfg(...)]`, so a build for another platform has no unused import.

### Tools Currently Using shellquote

- `aws2env` - the value of each credential it prints, when the value needs
  quotes. A value that holds only letters, digits, `_`, `-`, `.` and `/`
  prints bare, because a person reads that output before they paste it
- `chf` - the session id in the shell script it runs, and in the command it
  prints for a person to paste
- `crap` - the paths that the tests of the shell function put into the fake
  scripts
- `drun` - the fixture directory that the tests write into the fake docker
  script
- `faulte` - the text and the PID file path that the stream tests put into
  the stub scripts
- `gitscratch` - the log path, the path of the real git, and the subcommand
  names that the `git` shim of `testing::git_shim` puts into its script
- `glance` - the remote binary in the command that `ssh` sends to the second
  Mac. A leading `~/` stays outside the quotes, so the remote shell expands it
- `gsw` - the command it asks an interactive shell to run
- `ha` - the paths of the scripts that the browser tests put into `BROWSER`
- `nwt` - the worktree path and the branch in the cleanup commands it prints
  when it cannot remove a broken sparse worktree
- `popstop` - the state directory in the stop command that a refusal prints
- `sidecar` - the fixture root that the tests put into the fake `gh`
- `swt` - the path of a `.swt-check` override, and the command lines it prints
  for a person to paste
- `vpn-tunnel` - the `--op-path` value in the `op-cache invalidate` command that
  the all-in-use error prints
- `wdid` - the crate directory in the `cargo install --path` command of the
  workspace line, each shared word of a duplicate that needs quotes, and the
  paths and the history rows that the tests put into stub scripts

## GitHub REST Through `gh`

Every tool in this repository that names a GitHub repository by its remote URL,
or reads the reply of `gh api -i`, **must** use the `ghrest` library crate at
`src/ghrest/`.

### Why

`cwt` and `ha` each kept a private copy of this code, and `gsw` needed it too.
`cwt` is a binary crate, so `gsw` could not import its copy. A third copy is
the copy that drifts. `ghrest` holds the one parse of a `gh api -i` reply, the
one reader of a remote URL, and the one cache entry for a conditional request.

### Usage

Take the crate with `ghrest.workspace = true`.

- `GithubRepo::from_remote_url(url, extra_host)` - the host, the owner, and the
  name of a GitHub remote, or `None`. The caller passes `GH_HOST` as
  `extra_host`.
- `PullNumber` - the number of a pull request. It displays as `#547`.
- `parse_response(text)` - the status, the headers, and the body of a
  `gh api -i` reply. Read the status line, not the exit status: `gh` exits 1
  on a `304`.
- `EtagEntry` - the `ETag` and the body of the last answer for one path. A poll
  sends `condition_for(path)` as its `If-None-Match` line and reads
  `body_for(path)` on a `304`, which costs nothing against the rate limit.
- `Response::rate_limit_wait(now)` - how long a rate-limit refusal asks the
  caller to wait.

### Tools Currently Using ghrest

- `cwt` - the repository of `origin`, and the numbers of its pull requests
- `gsw` - the pull-request indicator of the watch header
- `ha` - every `gh api -i` reply, and its `ETag` cache

### Known Exception

- `sidecar` - `owner` in `src/sidecar/src/remote.rs` reads the GitHub owner of
  `origin` with its own parser. Its tests use local paths as `origin`, and
  `GithubRepo::from_remote_url` refuses a local path, so the move needs new
  fixtures. Issue #678 moves the owner and the host onto `ghrest` and removes
  this exception.

## Spawning Git

Every child process that this workspace starts **must** shed the inherited git
environment through the crate `childenv`, whether or not the child is git.
**Never write a list of variable names.** A raw `Command::new` fails the build.

`childenv` has two constructors, and a spawn takes the one that fits whether it
has a user whose intent to honor:

- **Test code** takes `childenv::fixture_command(program)`. It sheds the whole
  `GIT_` prefix and keeps nothing. A fixture builds a throwaway repository, so
  there is no user setting for it to honor.
- **Production code** takes `childenv::command(program)`. It sheds the same
  prefix and keeps the six names of `childenv::USER_INTENT_GIT_ENVIRONMENT` —
  `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_SSH`, `GIT_SSH_COMMAND`,
  `GIT_ASKPASS` and `GIT_TERMINAL_PROMPT`. A person states each of those on
  purpose, and a tool that runs for that person honors them.

One production site takes `fixture_command` on purpose:
`gitscratch::Git::command`, which each tool that takes `gitscratch` runs:
`bibibi`, `grime`, `grind`, `grist` and `gsw`. A replay works on local objects
and needs no credential, and a `GIT_CONFIG_GLOBAL` that one shell states must
not change what the replay reports. Do not move that site to
`childenv::command`.

The rule covers every child for two reasons. A program that does not run git
ignores `GIT_` variables, so the sweep costs nothing there. And `go`, `cargo`,
`gh`, `claude`, a shell, and `tmux` all run git later, so a rule about the
literal `"git"` does not see them.

### Why

Git obeys the environment before it obeys the directory a command was pointed
at, and it exports its own variables into every hook it runs. So a tool or a
test that runs from inside a pre-commit hook aims its git children at the
repository being committed to, whatever `current_dir` says. That has stamped
real commits with a fixture's identity and written a fixture's file onto a live
branch.

The repair kept arriving as a list of the three or four names somebody had
thought of. **A list is the defect, not the style.** It strips nothing new the
day git adds a variable, and from then on it returns the same clean-looking
answer as a list that works. Two variables walked through the three-name list
this workspace kept copying:

- `GIT_OBJECT_DIRECTORY` sends every object a child writes into another
  repository's store.
- `GIT_CONFIG_PARAMETERS`, which git hands every hook, injects arbitrary
  configuration (`user.email`, `core.bare`, `core.hooksPath`) into the child.

Neither names a location, so no amount of adding location names would have
caught either. The rule is the `GIT_` prefix, and the shared sweep enumerates
the environment rather than knowing any name at all.

### Why a keep-list is not a list of names again

The production entrance keeps six names, and that is a different shape from the
strip-list above rather than a return to it. **The split is not by git's
families, which the prefix genuinely cannot tell apart. It is by whose intent a
variable carries.** Git exports `GIT_DIR`, `GIT_INDEX_FILE` and
`GIT_CONFIG_PARAMETERS` into every hook, so those carry the launching hook's
intent and leave. A person exports `GIT_SSH_COMMAND` and `GIT_TERMINAL_PROMPT`,
so those carry the user's intent and stay. `childenv` states the two sets as
`USER_INTENT_GIT_ENVIRONMENT` and `HOOK_EXPORTED_GIT_ENVIRONMENT`, and a test
holds them disjoint — `GIT_EDITOR` is out of the keep-list for exactly that
reason, although a person does set it on purpose.

**A keep-list is safe in the way a strip-list is not, because its staleness runs
in the safe direction.** A stale strip-list inherits a variable git added after
it was written, and reports the same clean-looking answer as a list that works.
A stale keep-list sheds such a variable. The cost is one setting a user states
again, not one repository a tool writes into by mistake.

Dropping the six is not free. `GIT_SSH_COMMAND`, `GIT_SSH` and `GIT_ASKPASS`
are how a user authenticates, so dropping them turns a working install into an
authentication failure for anyone who holds a non-default key.
`GIT_TERMINAL_PROMPT=0` is what makes such a failure fast; without it git
prompts on `/dev/tty`, and a tool whose stdout a shell wrapper captures shows
the user a stalled command instead.

### Usage

Take the crate with `childenv.workspace = true` (a dev-dependency when only the
tests start a child process), then build the command from a constructor. A
production spawn:

```rust
let output = childenv::command("git")
    .args(["ls-files"])
    .current_dir(repo_root)
    .output();
```

A test fixture, which keeps nothing:

```rust
let mut command = childenv::fixture_command("git");
```

A value the call site wants pinned is set **after** the constructor, and so
wins. That is how `cwt` keeps its fixture identity and how several fixtures keep
`GIT_CONFIG_GLOBAL` pointed at `/dev/null`.

Removing `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` costs a fixture nothing: a
removal is not a redirect to `/dev/null`, so git falls back to the host's
`~/.gitconfig` and `/etc/gitconfig` exactly as it does in a shell that holds
nothing. What it costs is the answer for a user who exported one on purpose,
which is why the production constructor keeps both — `nwt`'s `missing_hooks_path`
reads the effective `core.hooksPath`, and the global and system steps of that
precedence are two of the four steps it exists to predict.

`childenv::shed_git_environment_from` takes the key source and the rule as
parameters. Tests use it, so no test has to set a real `GIT_*` variable: cargo
runs the tests of one binary on parallel threads, and such a test would redirect
the git children of every sibling thread.

Two builders are not a `std::process::Command`:

- `tokio::process::Command` starts from the entrance:
  `tokio::process::Command::from(childenv::command(program))`.
- `portable_pty::CommandBuilder` cannot come from `childenv`, because the crate
  depends only on `std`. Call `childenv::inherited_git_variables(rule)` and
  apply each name with `env_remove`.

### The Ban

`clippy.toml` bans `std::process::Command::new`,
`tokio::process::Command::new`, and `portable_pty::CommandBuilder::new`.
`disallowed_methods` is `deny` in the workspace lint set, so a raw spawn fails
the build. A hostile-environment test needs no exemption, because it sets its
variables after the constructor.

A raw spawn that is correct says so at its own site, with
`#[expect(clippy::disallowed_methods, reason = "...")]`, so the exemption stays
visible in the file it applies to.

### The Runners

The ban covers the Rust code of this workspace. It does not cover code that
other crates own, for example the build script of a dependency. So each runner
starts each cargo command through `without_git_environment` in
`scripts/without-git-environment.sh`, which removes the whole prefix:

- `scripts/rust-checks.sh` runs the four gates. The pre-commit hook and CI both
  start it, so a cargo command that you add to it gets the sweep.
- `scripts/plugin-checks.sh` runs the checks of the claude CLI on the plugins
  under `plugins/`. The pre-commit hook and CI both start it, and it starts
  each `claude` command through the same function.
- `.husky/pre-commit` itself starts only the cargo commands of the tool-index
  guard and of the `ic-inline` block.
- `test.sh` runs the workspace tests.

The hook is started by `sh`, so `without_git_environment` does not use
`compgen`, which a shell other than bash does not have.

### The Trap: A Lint Cannot Say This

The ban above names a constructor, so clippy can state it. A list of names is
different. `disallowed-methods` matches a method path and ignores its arguments.
The predicate here is entirely about the argument, so clippy can only ban
`env_remove` outright, which fires on every legitimate
removal of a variable that is not git's. The guard therefore reads the source.

It reads **tokens** rather than the syntax tree, because `syn` leaves the body
of a macro invocation unparsed and a tree walk cannot see a removal written
inside one. Tokens keep the property the tree gives: a `//` comment never
becomes a token, a doc comment becomes one string literal, and the text of a
string is one token rather than the words it spells — so prose that names the
call stays data. Each file is parsed first, so a file the guard cannot read as
Rust is a refusal rather than a clean verdict.

The read set is the Rust cargo compiles, not every `.rs` file on disk.
`src/cdva/tests/fixtures/rust` holds Rust that `cdva` reads as *input*, two
files of which are invalid on purpose. A named removal written into a parser
fixture is a fixture, not a defect.

### The Exemptions

`repo_guards::git_env_sweep::EXEMPTIONS` holds two entries, both in
`src/gitscratch/src/git.rs`: the restated removal of `GIT_AUTHOR_DATE` and
`GIT_COMMITTER_DATE` after the blanket sweep, so that second guard holds on its
own. Entries are keyed on **(file, variable)**, so a new named removal in an
exempt file is still a violation, and an entry that matches nothing is reported
too — an allowlist nobody prunes is how a guard quietly stops covering what it
was written for.

### What the Ban and the Guard Do Not Prove

The guard proves no call site passes a `GIT_`-prefixed name to `env_remove` as a
literal. A list of those names held in a constant and applied in a loop hands
the call a binding, so the guard reports it clean — the module doc of
`git_env_sweep` records why widening the matcher to see that shape was measured
and refused.

The ban proves that each spawn in the Rust code of this workspace takes a
constructor or states an exemption at its site. It does not prove three things:

- A dependency that starts a child process inside its own code is out of reach
  of clippy. The sweep in the runners is the only layer there.
- Clippy on macOS does not compile a `#[cfg(windows)]` site. Such a site takes
  the entrance by hand.
- A call site can put a `GIT_` variable back after the constructor. That is
  correct when the site means it, and it is visible at the site.

### Guards Enforcing This

- `clippy.toml` `disallowed-methods` — a raw `Command::new`,
  `tokio::process::Command::new` or `CommandBuilder::new` fails the build
- `src/repo-guards/tests/pre_commit_hook.rs` — no cargo command of the hook, of
  the `scripts/rust-checks.sh` that the hook starts, or of `test.sh`, and no
  `claude` command of the `scripts/plugin-checks.sh` that the hook starts, sees
  a `GIT_` variable, under a fake program that writes its environment to a file
- `repo_guards::git_env_sweep` (`src/repo-guards/src/git_env_sweep.rs`) — no
  file cargo compiles removes a `GIT_`-prefixed variable by name. Its companion
  test asks `cargo metadata` whether the read set holds every target root cargo
  builds, so a directory the walk never reaches shows up as a set difference
  instead of a clean report

## Alternate Screen

Every tool in this repository that enters the alternate screen of the terminal
**must** enter it through the `altscreen` library crate at `src/altscreen/`. A
raw entry fails the build or the guard.

### Why

A tool that enters the alternate screen depends on the terminal to clear it.
xterm clears the screen for mode 1049, but the emulator in the mosh server
ignores the modes 47, 1047 and 1049. Thus the text of the shell stays on the
screen.

ratatui does not help. It sends no clear at startup, and its first frame is a
diff against an empty buffer. Thus ratatui writes only the cells that are not
blank, and the shell text shows through each blank cell. `gitrdun` and `cctop`
shipped this defect. `ha` had it until PR #596 (branch `issue-592`) put a clear
into its view loop. Issue #593 moved that clear into one helper.

Ten tools had their own code to enter the alternate screen, and each one gave a
different subset of the guarantees below. `gitrdun` and `runat` left the shell
in raw mode after a panic. `diskhog` and `runat` called `Terminal::clear`, which
asks the terminal for the cursor position and waits for the answer. A terminal
that does not answer, for example a recording under `script`, stopped them at
the start.

### What the Helper Guarantees

- The erase of the whole screen goes out in the same write as the entry.
- A failed entry leaves no raw mode.
- Each exit restores the terminal: a drop, an error, and a panic.
- A panic restores the terminal first, so the panic message prints on the
  screen of the shell.
- The session never asks the terminal for the cursor position.

### Usage

Take the crate with `altscreen.workspace = true`. A tool that paints the screen
itself holds a `Session`:

```rust
let _session = altscreen::Session::enter(altscreen::Options::default())?;
```

A tool that draws with ratatui takes the feature `ratatui`, and draws on the
`Terminal` of the session:

```toml
altscreen = { workspace = true, features = ["ratatui"] }
```

```rust
let mut session = altscreen::RatatuiSession::enter(altscreen::Options::default())?;
session.terminal().draw(|frame| render(frame))?;
```

`Options` has two settings:

- `raw_mode(false)` is for a tool that holds raw mode another way. `ic` holds it
  through termion.
- `mouse_capture(true)` turns mouse capture on, and the restore turns it off.
  `runat` uses it.

To restore the terminal before the tool does more work, drop the session.
`runat` does this before it runs its command, and `diskhog` does it before it
prints its shutdown errors.

### The Ban

`clippy.toml` bans the spellings that clippy can see:

- `disallowed-types`: the `use` of `crossterm::terminal::EnterAlternateScreen`,
  `termion::screen::ToAlternateScreen` and `termion::screen::AlternateScreen`.
  A `use … as` rename and the re-export through `ratatui::crossterm` are caught
  too.
- `disallowed-methods`: `ratatui::init`, `ratatui::try_init`, `ratatui::run`, and
  the `into_alternate_screen` method of termion.

`ratatui::init_with_options` and `ratatui::try_init_with_options` stay open. They
do not enter the alternate screen, and an inline viewport is a correct use of
them.

`altscreen` is the one legitimate user. It says so at its import with
`#[expect(clippy::disallowed_types, reason = "...")]`, and clippy fails if that
expectation stops being fulfilled.

### The Guard

Clippy does not see three spellings. `disallowed-types` checks `use` items, not
a unit struct in the position of a value, so
`execute!(out, crossterm::terminal::EnterAlternateScreen)` with no import passes
it. A name that a glob import brings in passes it too. Clippy reads no string
literal, so `print!("\x1b[?1049h")` passes it.

`repo_guards::alternate_screen` reads tokens to close that gap. It has two
rules:

- **The identifier rule.** No identifier token is `EnterAlternateScreen`,
  `ToAlternateScreen`, `AlternateScreen`, `IntoAlternateScreen` or
  `into_alternate_screen`. A comment and a string are not identifier tokens, so
  prose stays data. A local type named `AlternateScreen` is reported too. That
  failure is loud, and a rename fixes it.
- **The literal rule.** No string literal, byte string literal or C string
  literal holds a DEC private mode set (`ESC [ ? Pm h`, or the 8-bit CSI) whose
  parameters include 47, 1047 or 1049. The rule reads the value of the literal
  after the escapes, so each spelling of ESC is one spelling. It parses the
  parameter list, so `\x1b[?25;1049h` is caught. A leave sequence (`…l`) is not
  reported.

The guard reads the same set of files as `repo_guards::git_env_sweep`: the Rust
that cargo compiles. It parses each file first, so a file that does not parse
is a refusal and not a clean report.

### The Exemptions

`repo_guards::alternate_screen::EXEMPTIONS` keys each entry on (file, spelling),
so a new spelling in an exempt file is still a violation. An entry that matches
nothing is reported, so a stale entry fails too. The list holds six entries:

- `src/altscreen/src/lib.rs`, `EnterAlternateScreen` — the one entrance. It
  writes the entry command into the bytes of its entry write.
- `src/altscreen/src/lib.rs`, `CSI ?1049 h` — its unit tests expect the exact
  entry bytes.
- `src/krt/tests/terminal.rs`, `CSI ?1049 h` — the pty tests of `krt` search the
  output for the entry sequence. They write to no user terminal.
- `src/termstate/src/tests.rs`, `CSI ?1049 h` — the tests of the emulator feed
  it the entry sequence as data. They write to no user terminal.
- `src/termstate/src/tests.rs`, `CSI ?47 h` — two tests of the emulator feed it
  mode 47 as data, because that mode sets only the flag of the alternate
  screen. They write to no user terminal.
- `src/nle/src/filters/keystroke_hud/tests.rs`, `CSI ?1049 h` — a test feeds the
  entry sequence of a full-screen program to the HUD as data. It writes to no
  user terminal.

### What the Ban and the Guard Do Not Prove

- A sequence that code builds at runtime, for example
  `format!("\x1b[?{}h", 1049)`.
- A sequence that `concat!` joins from parts.

### Guards Enforcing This

- `clippy.toml` `disallowed-types` and `disallowed-methods` — the spellings of
  "The Ban" fail the build
- `repo_guards::alternate_screen` (`src/repo-guards/src/alternate_screen.rs`) —
  no file cargo compiles names a listed identifier or holds an entry sequence,
  other than the exempt ones. `src/repo-guards/tests/alternate_screen.rs` holds
  one fixture for each syntactic form, and one test that audits this workspace

### Tools Currently Using altscreen

- `beta` - the player of a recording
- `cctop` - the full-screen view, through the ratatui session
- `chf` - the picker, through the ratatui session
- `diskhog` - the full-screen view, through the ratatui session
- `faulte` - the view of `faulte tui`, through the ratatui session
- `gitrdun` - the progress view, through the ratatui session
- `gsw` - the watch view
- `ha` - the full-screen view, through the ratatui session
- `ic` - the video player, with raw mode off, because termion holds raw mode
- `krt` - the live view
- `runat` - the countdown view, through the ratatui session, with mouse capture
- `seescc` - the watch view

## System Clipboard

Every tool in this repository that reads or writes the clipboard of the machine
**must** go through the `sysclip` library crate at `src/sysclip/`. A direct
dependency on `arboard` fails the guard, and a direct use of its clipboard fails
clippy.

### Why

The mosh-rs server puts a `pbcopy` shim and a `pbpaste` shim on the `PATH` of
each session, and it sets `MOSH_OPEN_SOCKET`. Each shim sends the text through
that socket to the clipboard of the client computer, which is the clipboard of
the person at the keyboard.

`arboard` does not know about the shims. It opens the clipboard of the server.
On a Mac where the session cannot reach the pasteboard of a GUI login, AppKit
gives no pasteboard, and `arboard` fails with "The selected clipboard is not
supported with the current system configuration". On a Linux server with no
display, it fails with an X11 timeout. On a Mac with a GUI login, it succeeds,
and that is worse: the text goes to the pasteboard of the server, and the
person at the client does not get it.

Five crates used `arboard` directly, and two of them kept their own copy of the
same error map. A private fallback in each crate is five copies that drift.

### The Order Rule

`sysclip::Clipboard::open` picks the clipboard in this order:

1. If `MOSH_OPEN_SOCKET` is set and not empty, the session has the shims.
   `sysclip` runs `pbcopy` or `pbpaste` from `PATH` and never opens `arboard`.
   A failure of the command is the answer, because the pasteboard of the server
   is not the clipboard of the user.
2. Else, `sysclip` uses `arboard`. If `arboard` fails, `sysclip` runs `pbcopy`
   or `pbpaste` from `PATH`. `ContentNotAvailable` on a read is not a failure.
   It is an empty clipboard.
3. If `PATH` holds no such command, the error is the `arboard` error.
4. If both fail, the error names both causes.

A command gets the text on its standard input, and gives the text on its
standard output. A non-zero exit is a failure, and the error holds the first
line of its standard error, for example `mosh-open: did not paste: …`.

`sysclip::Clipboard::open_native` skips the commands. It is for a tool that
polls. The mosh-rs client allows 5 pastes in each 10 seconds, and it shows each
paste on its notification bar, so a poll through `pbpaste` each 500 ms fails
and fills the bar. `clipboardmon` uses it.

### Usage

Take the crate with `sysclip.workspace = true`:

```rust
let mut clipboard = sysclip::Clipboard::open()?;
clipboard.write("/tmp/somewhere")?;
let text = clipboard.read()?; // an empty string when the clipboard holds no text
```

The error type, `sysclip::ClipboardError`, holds a message. It has no trailing
period, so a caller can put it inside a longer sentence.

No test reaches the clipboard of the machine. The order rule takes the opener
of `arboard`, the environment and the search path as parameters, so the unit
tests of `sysclip` cover each branch and set no process variable. An end-to-end
test starts the binary with `MOSH_OPEN_SOCKET` set and with a `PATH` that holds
a fake `pbcopy` and `pbpaste` in a temporary directory of its own. `dirc`,
`wn`, `clipboard-random` and `update-aws-credentials` hold such tests.
`sysclip::testing::FakeShims` (feature `testing`, as a dev-dependency) writes
those fake commands. Each such test takes them from that one place.

### The Ban

`clippy.toml` bans `arboard::Clipboard` in `disallowed-types` and
`arboard::Clipboard::new` in `disallowed-methods`. A call of
`arboard::Clipboard::new` raises both lints, and a name of the type in a `use`,
a signature or a field raises `disallowed_types`. `disallowed_methods` is
`deny` in the workspace lint set, and the pre-commit hook runs clippy with
`-D warnings`, so each spelling fails the gate. The ban matches the name of the
crate, so a renamed dependency and a `use … as` rename are caught too.

`sysclip` is the one legitimate user. It says so at each of its sites with
`#[expect(clippy::disallowed_types, reason = "...")]` (and
`clippy::disallowed_methods` at the call), and clippy fails if that
expectation stops being fulfilled.

### The Guard

`repo_guards::arboard_wall` reads `cargo metadata`. It fails when a workspace
member other than `sysclip` has a direct dependency on the package `arboard`,
of any kind: normal, dev or build, for every target. It matches the name of the
package, so a renamed dependency (`clip = { package = "arboard", … }`) is
caught. It refuses a result where `sysclip` itself is missing, because a guard
that finds no owner proves nothing.

### What the Ban and the Guard Do Not Prove

- A dependency of a dependency. A crate outside this workspace that opens the
  clipboard itself is out of reach of both.
- A tool that runs `pbcopy` or `xclip` itself with `childenv::command`. That is
  a spawn, not a use of `arboard`.
- Clippy on macOS does not compile a `#[cfg(windows)]` site.

### Guards Enforcing This

- `clippy.toml` `disallowed-types` and `disallowed-methods` — a use of
  `arboard::Clipboard` outside `sysclip` fails the build.
  `src/repo-guards/tests/arboard_ban.rs` runs clippy on a fixture of each form
  and reads the real `clippy.toml`.
- `repo_guards::arboard_wall` (`src/repo-guards/src/arboard_wall.rs`) — no
  workspace member other than `sysclip` depends on `arboard`.
  `src/repo-guards/tests/arboard_wall.rs` holds one fixture for each kind of
  dependency and one for the rename, and one test that audits this workspace.

### Crates Currently Using sysclip

- `clipboard-random` - each write of the generated data
- `clipboardmon` - the poll of the board tools, through `open_native`
- `dirc` - `SystemClipboard`. `FileClipboard` and `DIRC_CLIPBOARD_FILE` stay
- `update-aws-credentials` - the read of the credentials
- `wn` - the read of the chain, and the write of the plan. `WN_NO_CLIPBOARD`
  stays

## Version Information

All tools in this repository **must** display version information including git hash and dirty status when `--version` or `-V` is used.

### Why

Consistent version information helps with:
- Debugging issues by knowing exact build
- Identifying if local modifications exist
- Tracking which commit a binary was built from
- Consistent user experience across all tools

### Output Format

```
toolname 0.1.0 (abc1234, clean)
toolname 0.1.0 (abc1234, dirty)
toolname 0.1.0 (unknown, unknown)  # when git unavailable
```

### Rust Tools

Use the `buildinfo` library crate located at `src/buildinfo/`:

```rust
use buildinfo::version_string;
use clap::Parser;

#[derive(Parser)]
#[command(version = version_string!())]
struct Cli {
    // ...
}
```

The `version_string!()` macro captures at compile time:
- Package version from Cargo.toml
- Git commit hash (7 characters)
- Dirty/clean status

For tools without clap, add a manual check:

```rust
use buildinfo::version_string;

fn main() {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("toolname {}", version_string!());
        return;
    }
    // ...
}
```

### Tools Currently Using buildinfo

All Rust tools use buildinfo for version information.

## Lint Configuration

Every crate in this workspace **must** inherit the repo-wide lint set, and a crate that wants to be stricter **must** declare the extra lints as crate-root attributes in **every one of its target roots**.

### Why

The root `Cargo.toml` declares `[workspace.lints.rust]` and `[workspace.lints.clippy]`, but cargo hands those lints only to members that opt in. A member that omits the stanza is silently exempt: nothing warns and nothing fails, because the exemption is spelled as an *absence*. That is also why it spreads — a new crate that never types the stanza is born exempt. Each member opts in today, and `repo_guards::workspace_lints` keeps it that way: a new crate that omits the stanza fails `cargo test`.

### Usage

Every member manifest carries:

```toml
[lints]
workspace = true
```

Extra strictness cannot go in the manifest. Cargo refuses to merge a local `[lints]` table with `lints.workspace = true`:

```text
error: cannot override workspace.lints in lints, either remove the overrides or lints.workspace = true and manually specify the lints
```

So a crate that wants lints stricter than the workspace set declares them as crate-root inner attributes in its source (`src/cwt/src/main.rs`):

```rust
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
```

### The Trap: A Crate-Root Attribute Reaches One Target

**A manifest `[lints]` table applies to every target of the package. A crate-root attribute applies only to the target whose root file carries it.**

So `#![deny(unsafe_code)]` in `src/main.rs` does nothing for `tests/`, `benches/`, `examples/`, or `build.rs` — and nothing for `src/lib.rs` either, in a crate that has both. **The attributes must be repeated in every target root.**

A build script is the easiest one to forget, because nothing about it looks like a target. It is one: with `[lints.rust] unsafe_code = "deny"` in the manifest, an unsafe block in `build.rs` fails the build; move that lint to `#![deny(unsafe_code)]` in `src/lib.rs` and the same `build.rs` compiles clean. So `build.rs` must state a position like every other root — but a lint *it* raises never binds its siblings, since nothing links a build script into the crate.

This is not hypothetical. `cwt`'s integration tests silently lost `unsafe_code` and `clippy::pedantic` when its manifest `[lints]` table was converted to a crate-root attribute. Nothing warned; the lints simply stopped applying there. `src/cwt/tests/main-worktree.rs` now repeats them:

```rust
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
```

A target that legitimately needs a lint relaxed says so at the site, with a reason (`src/bm/tests/cli.rs`):

```rust
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]
```

`deny`, `forbid`, `warn`, `allow`, and `expect` all count as declaring a position — **silence is the only violation**. The bar is "mention", not "match the level", so a relaxation stays a visible, reviewable decision in the file it applies to instead of an entry in a central allowlist. A `cfg_attr`-wrapped lint (`#![cfg_attr(not(test), warn(clippy::unwrap_used))]`, as in `src/bm/src/lib.rs`) counts as a mention but does not raise. Because `clippy::allow_attributes_without_reason` is `warn` in the workspace set, every `allow` needs a `reason = "..."`.

An outer allowance is written `#[expect(lint, reason = "...")]`. `clippy::allow_attributes` is `warn` in the workspace set, and an expectation whose lint no longer fires is a warning. A stale allowance thus fails clippy. Inner `#![allow]` attributes are not flagged.

### Guards Enforcing This

- `repo_guards::workspace_lints` (`src/repo-guards/src/workspace_lints.rs`) - every workspace member manifest declares `[lints]` / `workspace = true`
- `repo_guards::target_lints` (`src/repo-guards/src/target_lints.rs`) - every target root — library, binary, test, bench, example, build script - declares a position on each lint its crate's lib/bin roots raise. Its companion test asks `cargo metadata` whether that set of roots is the set cargo actually builds, so a target kind the guard never learned about shows up as a set difference instead of a clean report

## Tool Indexes

Every binary this workspace builds **must** appear in both `README.md` and
`TLDR.md`.

### Why

The repository documents its tools twice, and on purpose. `README.md` carries
the long entry — what the tool is for, how to run it, how to install it.
`TLDR.md` carries one line per tool, alphabetized, for a reader who only needs
to know which tool to reach for. A tool that is missing from either one is a
tool nobody finds.

Nothing enforced this before. The omission is spelled as an *absence*, which is
why it spread: a crate that nobody remembers to document is born undocumented,
and no build step ever said so. One binary, `seescc`, had drifted out of an
index by the time the guard was written: it carried a `README.md` entry and no
`TLDR.md` row.

### Usage

`TLDR.md` is one table, alphabetized. Add a row whose **first cell** is the tool
name:

```markdown
| `krt` | Knights of the Round Trip — records the network path to a destination. |
```

`README.md` accepts either of the two forms in service today. Add a **top-level
list item** under `## The tools`, whose first word is the tool name:

```markdown
- krt (Knights of the Round Trip)
  - Records the network path to a destination, hop by hop.
  - To install: `cargo install --git https://github.com/timmattison/tools krt`
```

Or add a **level-2 section heading** whose first word is the tool name
(`## occ (old Claude Code)`), for a tool that needs more room than a list item.

### The Trap: A Mention Is Not An Entry

Both indexes are full of mentions. The row of `sirn` names `portplz`, and the
row of `prgz` names `prcp`. So the guard **parses** the Markdown rather than
searching it for the tool name — it reads the first cell of a table body row,
and the first word of a top-level item or a level-2 heading. A text search
would pass on a tool whose own entry is gone, and a search that reports clean
for the wrong reason is indistinguishable from a guard doing real work.

A nested list item is not an entry either. The `- To install: …` line under a
tool's own entry would otherwise document a tool named `To`.

### Guards Enforcing This

- `repo_guards::tool_index` (`src/repo-guards/src/tool_index.rs`) — every binary
  cargo builds has an entry in `README.md` and a row in `TLDR.md`. Its companion
  test asks `cargo metadata` whether the guard's set of binaries is the set
  cargo actually builds, so a binary the guard never learned to discover shows
  up as a set difference instead of a clean report

## 1Password Access

Every crate in this workspace that reads a secret **must** read it through the
`op-cache` library crate at `src/op-cache/`. No other crate runs the `op`
binary.

### Why

`op-cache` caches what it read, retries what failed, and writes the cache file
at mode 600. A crate that runs `op` itself gets none of that, and the failure
is quiet: the code works, the secret arrives, and only the properties nobody
can see are gone.

`ufa` did exactly that. `discover_op_controllers` ran
`op item get ufa --vault Private --format json` to learn which controllers the
user keeps, and that command prints the value of every concealed field beside
its label — so every controller key in the item reached a variable of `ufa`
for a list of names. `op_cache::field_labels` now runs that command. The values
still reach the process, in a buffer inside `op-cache`, and stop there: no
value reaches a type or a variable of `ufa`.

The reason it reached around the helper is the part worth remembering:
`op-cache` had no way to list the fields of an item, so the helper could not do
the job. **A crate that reaches around this helper is reporting a gap in it.**
Add the narrow method the helper lacks, then call it.

### Usage

```rust
// One field of an item, cached in a file the tool names.
let cache = op_cache::OpCache::with_path(config_dir.join(".op-cache.json"));
let path = op_cache::OpPath::new("op://Private/ufa/key - 192.168.1.1 port 443")?;
let key = cache.read(&path, None)?;

// The names of every field of an item, and none of the values.
let item = op_cache::OpItem::new("Private", "ufa")?;
let labels = op_cache::field_labels(&item)?;
```

`field_labels` is the one read in that crate that is not cached. A label is not
a secret, and a cached list of names goes stale silently: a stale value is
found out at the point of use, when the service refuses it and the caller
invalidates the entry, but a stale list of names simply hides the field the
user just added.

`OpCache::new` puts the cache file at the root of the git repository that
holds the working directory, and it fails outside a git repository. That suits
a tool that runs in its own checkout. An installed tool runs from any
directory, so it names the file with `OpCache::with_path`: `ufa` keeps it
beside its configuration file. At the root of another repository, the file
puts a plaintext key where nothing ignores it, and a `git add -A` stages it.

### The Trap: A Text Search Finds One Spelling

`Command::new("op")` is one spelling of many, and a text search reports each of
the others *clean*. A new spawn of `op` arrives as `childenv::command("op")` or
`childenv::fixture_command("op")`, because "Spawning Git" makes those two the
only constructors that clippy permits. The first version of the guard matched
only a callee that ends in `Command::new`, so it reported each of those spawns
clean. Other spellings are the call behind an import, the call behind a
`use … as` rename, the `tokio` type, `portable_pty::CommandBuilder::new`, the
program named by its path, and the call inside a macro body.

So the guard **parses** each source first, and it refuses a file it cannot
parse, rather than reporting it clean. Then it reads **tokens**, as
`repo_guards::git_env_sweep` does, because `syn` leaves the body of a macro
invocation unparsed. A token walk reads a macro body and a function body with
one rule, and a comment or a string stays data. The module doc of `op_wall`
states the spellings that the walk does not see, for example a program or an
entrance that reaches the call through a binding.

Clippy cannot express this rule. `disallowed-methods` matches a method path and
ignores its arguments, so it can ban `childenv::command` only for every
program, not for one.

### Guards Enforcing This

- `repo_guards::op_wall` (`src/repo-guards/src/op_wall.rs`) — no workspace
  member but `op-cache` runs the `op` binary. The audited set comes from the
  workspace members rather than a list of crates, so a new crate is covered on
  the day it is written, and a companion test asks whether every member gave
  the guard a file to read — a matcher pointed at the wrong directories reports
  clean with the same silence as a broken one

## Specs and Plans

A spec **must** live in `specs/` and a plan **must** live in `plans/`, both at
the repository root. A file in any other directory named `specs` or `plans` is
in the wrong place.

### Why

The superpowers skills write specs and plans to `docs/superpowers/` by default.
This repository took that default three times: #198 (`portplz`), #200
(`kitchen-sync`), and #191 (`vpn-tunnel`). Nothing failed, so the files stayed
there until a review of #191 found them.

### Guards Enforcing This

- `repo_guards::doc_placement` (`src/repo-guards/src/doc_placement.rs`) — every
  file in a `specs` or `plans` directory below the repository root fails
  `cargo test`. The walk does not enter a directory that holds a `.git` entry,
  because that directory is another checkout, for example an agent worktree
  in `.claude/worktrees/`

## UTF-8 String Safety

All tools in this repository **must** handle UTF-8 strings safely. Never use byte-level indexing that could panic on multi-byte characters.

### Why

Rust strings are UTF-8 encoded, meaning characters can be 1-4 bytes. Byte-level indexing (`&s[..n]`) will panic if `n` falls in the middle of a multi-byte character. Process names, filenames, and user input can all contain multi-byte characters.

### Common Pitfalls

```rust
// BAD: Will panic on "日本語" or "café"
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("{}...", &s[..max - 3]) }  // PANIC!
}

// GOOD: Use chars() for character-level operations
fn truncate(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max { s.to_string() }
    else {
        let truncated: String = s.chars().take(max.saturating_sub(3)).collect();
        format!("{truncated}...")
    }
}
```

### Rules

- **Never use `&s[..n]`** unless you've verified `n` is at a valid UTF-8 boundary
- Use `.chars()` or `.char_indices()` when iterating or truncating strings
- `s.len()` returns bytes, not characters - use `s.chars().count()` for character count
- For display width (terminal columns), use the `unicode-width` crate
- Always add tests with multi-byte characters (Japanese: 日本語, emoji: 🎉, accented: café)

### Testing UTF-8 Safety

Always include tests with multi-byte characters. The example below shows the pattern;
see actual tool implementations (e.g., `src/spv/src/main.rs`) for comprehensive test coverage.

```rust
#[test]
fn test_truncate_utf8_safety() {
    // Japanese characters (3 bytes each in UTF-8, but 1 char each)
    // "日本語テスト" is 6 characters; with max=5, truncate to 2 chars + "..."
    assert_eq!(truncate("日本語テスト", 5), "日本...");

    // Emoji (4 bytes each in UTF-8, but 1 char each)
    // "🎉🎊🎁🎈🎂" is 5 characters; with max=4, truncate to 1 char + "..."
    assert_eq!(truncate("🎉🎊🎁🎈🎂", 4), "🎉...");

    // Mixed ASCII and multi-byte
    // "café au lait" is 12 characters; with max=8, truncate to 5 chars + "..."
    assert_eq!(truncate("café au lait", 8), "café ...");
}
```

## Platform-Specific Code

When writing code that differs across platforms (Unix vs Windows), follow these guidelines to avoid dead code and ensure maintainability.

### Why

Rust's `#[cfg()]` attributes exclude code from compilation on non-matching platforms. This means:
- Clippy and the compiler won't warn about unused `#[cfg(not(unix))]` code on Unix
- It's easy to accidentally write duplicate implementations that diverge
- Dead code can accumulate unnoticed across platforms

### Pattern: Prefer Inline Conditionals for Simple Cases

When platform-specific logic is simple (a few lines), use inline `#[cfg()]` blocks:

```rust
// GOOD: Simple inline handling
let value = {
    #[cfg(unix)]
    {
        unix_specific_call()
    }
    #[cfg(not(unix))]
    {
        fallback_value()
    }
};
```

### Pattern: Use Functions for Complex Logic

When platform logic is complex, define functions for BOTH platforms and call them consistently:

```rust
// GOOD: Both platforms have functions, both are called
#[cfg(unix)]
fn get_system_info() -> Info {
    // Complex Unix implementation
}

#[cfg(not(unix))]
fn get_system_info() -> Info {
    // Complex Windows implementation
}

// Single call site that works on both platforms
let info = get_system_info();
```

### Anti-Pattern: Mixed Function and Inline

Never define a function for one platform while handling the other inline:

```rust
// BAD: Function defined but inline code bypasses it
#[cfg(unix)]
fn helper(x: u32) -> String { /* ... */ }

#[cfg(not(unix))]  // This function is never called!
fn helper(x: u32) -> String { x.to_string() }

// Later in code:
#[cfg(unix)]
{ helper(value) }
#[cfg(not(unix))]
{ value.to_string() }  // Duplicate logic, helper ignored
```

## Shell Scripts

Shell scripts in this repository **must** pass [ShellCheck](https://www.shellcheck.net/) validation.

### Why

ShellCheck catches common shell script issues:
- Useless use of cat (UUOC) - e.g., `cat file | grep` should be `grep < file`
- Unquoted variables that could cause word splitting
- Missing error handling
- Portability issues between shells

### Configuration

The repository includes a `.shellcheckrc` file that configures ShellCheck with sensible defaults.

### Running ShellCheck

```bash
# Check all shell scripts
shellcheck scripts/*.sh test.sh

# Check a specific script
shellcheck scripts/update-deps.sh
```

### Shell Script Style Guidelines

1. **Use `set -e`** at the top of scripts to exit on error
2. **Quote variables** to prevent word splitting: `"$var"` not `$var`
3. **Avoid UUOC**: Use `< file` instead of `cat file |`
4. **Use `[[ ]]`** instead of `[ ]` for conditionals in bash
5. **Handle arguments properly**: Use `while` loops with `shift` for multi-argument parsing
6. **Provide help text**: Include `-h`/`--help` options
7. **Avoid emojis**: Use text indicators like `[PASS]`/`[FAIL]` instead of `✓`/`✗`

## Continuous Integration

Every check that CI runs **must** live in `scripts/rust-checks.sh` or in
`scripts/plugin-checks.sh`.

### Why

The pre-commit hook and CI must run the same checks. When each caller holds
its own list, the two lists drift apart, and a check that one list lost
reports nothing. Thus one script holds each list. `scripts/rust-checks.sh`
holds the checks of the Rust workspace, and `scripts/plugin-checks.sh` holds the
checks of the Claude Code plugins under `plugins/`. `.husky/pre-commit` and
`.github/workflows/ci.yml` each start both scripts. The hook starts a script
only when a staged path is one that the script reads.

To run the full list by hand, run `./scripts/rust-checks.sh`, then
`./scripts/plugin-checks.sh`, from the repository root. Do this at the end of a
TDD run. A `git commit --allow-empty` stages no file, thus the hook runs no
check in this repository. A step of issue-fleet does not run the full list. It
obeys [Checks in an issue-fleet Step](#checks-in-an-issue-fleet-step).

### What CI does

`.github/workflows/ci.yml` runs for each pull request, for each push to `main`,
and when a person starts it by hand (`workflow_dispatch`). It runs the Rust
checks in one job on `ubuntu-latest` and in one job on `macos-latest`. Some
crates hold code that compiles only on macOS, and a Linux job does not compile,
lint, or test that code. It runs the plugin checks in one job on
`ubuntu-latest`. GitHub does not charge for standard runners in a public
repository.

Each job of the Rust checks does these steps:

1. On Linux only, install the system packages with `apt-get`: the libraries
   that some crates link against, and the tools that some tests run. `msgs` is
   a Tauri app and `nle` is a Dioxus app. On Linux both link against WebKitGTK
   4.1 and libxdo.
2. Install Node.js, at the version that `NODE_VERSION` in the workflow names.
   The tests of `nle` run TypeScript under the real `node`, which needs
   Node.js 22.18 or later.
3. Install the toolchain and the components that `rust-toolchain.toml` names.
4. Install `cargo-machete`.
5. Restore the Cargo cache. Only a run on `main` saves a cache, and a pull
   request reads that cache.
6. Run `scripts/rust-checks.sh`.

The job of the plugin checks does these steps:

1. Install Node.js, at the same `NODE_VERSION`. The package of the claude CLI
   needs Node.js 22 or later.
2. Install the claude CLI with `npm`, at the version that
   `CLAUDE_CODE_VERSION` in the workflow names.
3. Run `scripts/plugin-checks.sh`.

The plugin checks need no login. `claude plugin test` runs the tests in the
sandbox of the engine, which has no file system, network or process, thus one
platform is sufficient.

The workflow sets `CARGO_PROFILE_DEV_DEBUG=0` for each job, because debug
information is most of the size of the target directory, and the disk of a
Linux runner is small.

### To add a check

- Add a check of the Rust workspace to `scripts/rust-checks.sh`, and a check of
  a plugin to `scripts/plugin-checks.sh`. Do not add it to the hook or to the
  workflow.
- If the check needs a tool that the runner does not have, add an install step
  to the workflow.
- If a crate or a test needs a system library or a tool on Linux, add its
  package to the `apt-get install` step. The tests of `crap` need `zsh`, for
  example.
- Pin each third-party action to a full commit SHA, and write its release tag
  in a comment. A tag can move, but a commit SHA cannot.
- Run `actionlint .github/workflows/ci.yml` after each change to the workflow.

## Checks in an issue-fleet Step

A step of issue-fleet runs the checks of the crates that its change touches,
and not the full list. CI runs the full list on the pull request. CI does not
run the checks of the msgs frontend, thus a step that changes that frontend
runs them itself. This section applies when `ISSUE_FLEET_RUN` is set.
issue-fleet sets that variable in each step, also in each step of the
`review-loop` that it starts. Each other session keeps the full list at the end
of a TDD run, as [Continuous Integration](#continuous-integration) says.

### Why

On 2026-11-26, three runs of issue-fleet on one Mac of 14 cores held new work at
the load gate for 5.7 of 10.5 hours. The steps of this repository made most of
that load. A sample of 90 s counted 681 new processes each second, and 91% came
from one `/address-issues` step that ran the full list by hand. The test
binaries of `gsw`, `cwt` and `gitscratch` start `git` and the tool under test
for each test. Each new process costs kernel time and a check by `syspolicyd`.
In the steps of that day, `cargo test` ran 1,191 times, for 6.4 hours of wall
time.

CI runs `scripts/rust-checks.sh` on each pull request, as
[What CI does](#what-ci-does) says. issue-fleet opens a pull request for each
issue, and it merges only after the CI run is green. Thus a full run in a step
does the work of CI a second time, and it slows each other step on the machine.

### The Rule

1. Do not run `scripts/rust-checks.sh`, `test.sh`, `cargo test --workspace`,
   `cargo clippy --workspace`, or a `cargo test` or a `cargo clippy` with no
   `-p`.
2. For each crate that the change touches, run the four checks of
   `scripts/rust-checks.sh` for that crate, with the same flags and the same
   environment. Each member lives at `src/<crate>`:

   ```bash
   cargo fmt -p <crate> -- --check
   cargo machete src/<crate>
   cargo clippy --locked -p <crate> --all-targets -- -D warnings
   CLICOLOR_FORCE=1 COLORTERM=truecolor cargo test --locked --no-fail-fast -p <crate>
   ```

   `cargo fmt` and `cargo machete` compile nothing, and part 3 skips the hook
   that runs them. Also run these checks:
   - When the change touches a crate that other crates of the workspace use,
     run the tests of each crate that depends on it. Do this also when that is
     most of the workspace. `childenv` has 99 dependents, and `buildinfo` has
     88, of 115 members.
   - When the change touches a file that a guard of `repo-guards` reads, run
     `cargo test -p repo-guards`. The guards read each Rust file that cargo
     compiles, each `Cargo.toml`, `README.md`, `TLDR.md`, `clippy.toml`,
     `.gitignore`, `rust-toolchain.toml`, `.husky/pre-commit`, the scripts that
     the hook starts, `test.sh`, and each file in a `specs` or `plans`
     directory. Thus a change to Rust code also needs the tests of
     `repo-guards`.
   - When the change touches a file under `src/msgs/frontend/` or
     `src/msgs/scripts/`, run the checks of the msgs frontend: `tsc --noEmit`
     on both directories, and `vitest run`. Only the msgs block of
     `.husky/pre-commit` runs them, and part 3 skips that hook:

     ```bash
     pnpm --dir src/msgs/frontend install --frozen-lockfile --prefer-offline
     pnpm --dir src/msgs/frontend run check
     ```
3. Commit with `--no-verify`. The pre-commit hook runs the full list for each
   commit that stages a Rust file, a `Cargo.toml`, a `Cargo.lock`,
   `scripts/rust-checks.sh`, or the `rustfmt.toml` or the `rust-toolchain.toml`
   at the root.
4. Do not end a TDD run with the full list. Make no closing
   `git commit --allow-empty`, and do not run the list by hand in its place.
   The CI run of the pull request ends the TDD run.

This command tests a crate together with each workspace crate that depends on
it. A crate depends on it when it names the crate in a dependency table, or
when it gets the crate through a normal or a build dependency on another
member. `sysclip` has 5 crates of the first kind, and 5 more through
`clipboardmon`. A dev-dependency does not pass the crate on, because the tests
of a member do not build the dev-dependencies of its dependencies. In the
filter, the kind `null` is a normal dependency.

```bash
crate=childenv
cargo metadata --format-version 1 --no-deps \
  | jq -r --arg crate "$crate" '
      [.packages[] | {name, deps: [.dependencies[] | select(.path != null)]}] as $members
      | def users($kinds): . as $set | [$members[]
          | select(any(.deps[]; (.kind | IN($kinds[])) and (.name | IN($set[]))))
          | .name] + $set | unique;
      [$crate]
      | until(users([null, "build"]) == .; users([null, "build"]))
      | users([null, "build", "dev"]) - [$crate] | .[] | ("-p", .)' \
  | CLICOLOR_FORCE=1 COLORTERM=truecolor \
    xargs cargo test --locked --no-fail-fast -p "$crate"
```

Keep `-p "$crate"` in the command. GNU `xargs` runs its command one time when
the list is empty, and a `cargo test` with no `-p` tests the whole workspace.
BSD `xargs` on macOS runs nothing when the list is empty, thus the
`cargo test -p <crate>` of part 2 stays necessary.
