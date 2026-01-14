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
