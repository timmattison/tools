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

A shell function that merely forwards arguments to the binary (`function prmv() { prcp --rm "$@"; }`) is **cosmetic, not load-bearing** — it adds no capability the binary lacks. That is not a sufficient reason to touch the user's shell config.

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

This logic is centralized in `resolve_config_target`, so every consumer (`crap`, `cwt`, `prcp`) benefits without code changes.

### Tools Currently Using shellsetup

- `cwt` - Change Worktree (provides `wt`, `wtf`, `wtb`, `wtm` commands)
- `nwt` - New Worktree (provides the worktree-creation cd function)
- `crap` - Claude, Resume Anywhere Please (provides the `crap` cd-and-resume function)
- `prcp` - Progress Copy (provides `prmv` command) — **slated for removal**, see issue #265; this is cosmetic shell integration, not load-bearing

## Progress Bar Display

All tools in this repository that display progress bars **should** use the `termbar` library crate located at `src/termbar/`.

### Why

The `termbar` library provides:
- Terminal width detection with fallback
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
- `org-borg` - Organization Backup (custom template with dynamic width)

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
`#[allow(clippy::disallowed_methods, reason = "...")]` - see `gsw` and `seescc`.
The exemption stays visible in the file it applies to, rather than in a central
allowlist.

### Crates Currently Using testcolor

- `gsw` - render, watch, and push tests
- `cwt` - family render tests, and the end-to-end helpers that read the output
  of the binary

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

### Go Tools

Use the `internal/version` package:

```go
import (
    "github.com/timmattison/tools/internal/version"
)

func main() {
    var showVersion bool
    flag.BoolVar(&showVersion, "version", false, "Show version information")
    flag.BoolVar(&showVersion, "V", false, "Show version information (shorthand)")
    flag.Parse()

    if showVersion {
        fmt.Println(version.String("toolname"))
        os.Exit(0)
    }
    // ...
}
```

**Important:** Always define version flags in `main()`, not in `init()`. This keeps all flag definitions in one place and makes the code more readable. All Go tools in this repository follow this pattern.

**Build with ldflags** using `scripts/build-go.sh` to inject git info:

```bash
./scripts/build-go.sh           # Build all Go tools
./scripts/build-go.sh dirc prgz  # Build specific tools
```

The build script reads the version from the `VERSION` file at the repository root.

### Tools Currently Using buildinfo

All Rust tools use buildinfo for version information.

### Tools Currently Using internal/version

All Go tools use internal/version:
- `dirc` - Directory Clipboard
- `localnext` - Local Next.js Server
- `prgz` - Progress Gzip
- `procinfo` - Process Info
- `subito` - AWS IoT Subscriber
- `symfix` - Symlink Fix

## Lint Configuration

Every crate in this workspace **must** inherit the repo-wide lint set, and a crate that wants to be stricter **must** declare the extra lints as crate-root attributes in **every one of its target roots**.

### Why

The root `Cargo.toml` declares `[workspace.lints.rust]` and `[workspace.lints.clippy]`, but cargo hands those lints only to members that opt in. A member that omits the stanza is silently exempt: nothing warns and nothing fails, because the exemption is spelled as an *absence*. That is also why it spreads — a new crate that never types the stanza is born exempt. All 73 members opt in today, and `repo_guards::workspace_lints` keeps it that way: a new crate that omits the stanza fails `cargo test`.

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
and no build step ever said so. Two binaries had drifted out of an index by the
time the guard was written.

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
shellcheck scripts/build-go.sh
```

### Shell Script Style Guidelines

1. **Use `set -e`** at the top of scripts to exit on error
2. **Quote variables** to prevent word splitting: `"$var"` not `$var`
3. **Avoid UUOC**: Use `< file` instead of `cat file |`
4. **Use `[[ ]]`** instead of `[ ]` for conditionals in bash
5. **Handle arguments properly**: Use `while` loops with `shift` for multi-argument parsing
6. **Provide help text**: Include `-h`/`--help` options
7. **Avoid emojis**: Use text indicators like `[PASS]`/`[FAIL]` instead of `✓`/`✗`
