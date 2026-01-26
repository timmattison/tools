# Buffalo Tools - Development Guidelines

## Shell Integration

All tools in this repository that provide shell integration (shell functions, aliases, etc.) **must** use the `shellsetup` library crate located at `src/shellsetup/`.

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

### Tools Currently Using shellsetup

- `cwt` - Change Worktree (provides `wt`, `wtf`, `wtb`, `wtm` commands)
- `prcp` - Progress Copy (provides `prmv` command)

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
./scripts/build-go.sh bm dirc   # Build specific tools
```

The build script reads the version from the `VERSION` file at the repository root.

### Tools Currently Using buildinfo

All Rust tools use buildinfo for version information.

### Tools Currently Using internal/version

All Go tools use internal/version:
- `bm` - Bulk Move
- `dirc` - Directory Clipboard
- `localnext` - Local Next.js Server
- `prgz` - Progress Gzip
- `procinfo` - Process Info
- `subito` - AWS IoT Subscriber
- `symfix` - Symlink Fix

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
see actual tool implementations (e.g., `src/sp/src/main.rs`) for comprehensive test coverage.

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
