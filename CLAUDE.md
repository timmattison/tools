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

### Tools Currently Using shellsetup

- `cwt` - Change Worktree (provides `wt`, `wtf`, `wtb`, `wtm` commands)
- `prcp` - Progress Copy (provides `prmv` command)
