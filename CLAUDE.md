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

All tools in this repository **must** display version information including git hash and dirty status when `--version` is used.

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
    var showVersion = flag.Bool("version", false, "Show version information")
    flag.Parse()

    if *showVersion {
        fmt.Println(version.String("toolname"))
        os.Exit(0)
    }
    // ...
}
```

**Build with ldflags** using `scripts/build-go.sh` to inject git info:

```bash
./scripts/build-go.sh           # Build all Go tools
./scripts/build-go.sh bm dirc   # Build specific tools
```

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
