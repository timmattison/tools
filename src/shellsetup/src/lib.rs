//! Shell integration library for adding tool functions to shell config files.
//!
//! This library provides a standardized way to add shell functions and aliases
//! to user shell configuration files (`.bashrc`, `.zshrc`, etc.) with support
//! for detecting existing installations and upgrading them in-place.
//!
//! # Example
//!
//! ```rust,ignore
//! use shellsetup::{ShellIntegration, ShellCommand};
//!
//! let integration = ShellIntegration::new(
//!     "mytool",
//!     "My Tool",
//!     r#"
//! function mt() {
//!     mytool "$@"
//! }
//! alias mtv='mt --verbose'
//! "#,
//! )
//! .with_command("mt", "Run mytool")
//! .with_command("mtv", "Run mytool with verbose output");
//!
//! integration.setup()?;
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use colored::Colorize;
use thiserror::Error;

/// Errors that can occur during shell integration setup.
#[derive(Error, Debug)]
pub enum ShellSetupError {
    /// Could not determine the user's home directory.
    #[error("Could not determine home directory")]
    NoHomeDir,

    /// The user's shell is not supported for automatic setup.
    #[error("Unsupported shell: {shell}. Please manually add the shell integration to your config.\n{manual_instructions}")]
    UnsupportedShell {
        shell: String,
        manual_instructions: String,
    },

    /// Failed to read the shell config file.
    #[error("Could not read {path}: {source}")]
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Failed to write to the shell config file.
    #[error("Could not write to {path}: {source}")]
    WriteError {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Result type for shell setup operations.
pub type Result<T> = std::result::Result<T, ShellSetupError>;

/// A command that will be available after shell integration is set up.
#[derive(Debug, Clone)]
pub struct ShellCommand {
    /// The command name (e.g., "wt", "wtf").
    pub name: String,
    /// A short description of what the command does.
    pub description: String,
}

impl ShellCommand {
    /// Creates a new shell command.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Configuration for shell integration.
///
/// This struct holds all the information needed to set up shell integration
/// for a tool, including the shell code to add, markers for detection, and
/// information about available commands.
#[derive(Debug, Clone)]
pub struct ShellIntegration {
    /// Short name of the tool (e.g., "cwt", "prcp").
    tool_name: String,
    /// Human-readable description (e.g., "Change Worktree", "Progress Copy").
    tool_description: String,
    /// The shell code to add (functions, aliases, etc.).
    shell_code: String,
    /// Commands that will be available after setup.
    commands: Vec<ShellCommand>,
    /// Previous end marker patterns for detecting old installations.
    /// Used when upgrading from versions without the standard end marker.
    old_end_markers: Vec<String>,
}

impl ShellIntegration {
    /// Creates a new shell integration configuration.
    ///
    /// # Arguments
    ///
    /// * `tool_name` - Short name of the tool (e.g., "cwt")
    /// * `tool_description` - Human-readable description (e.g., "Change Worktree")
    /// * `shell_code` - The shell code to add (without markers - they're added automatically)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let integration = ShellIntegration::new(
    ///     "mytool",
    ///     "My Tool",
    ///     r#"
    /// function mt() {
    ///     mytool "$@"
    /// }
    /// "#,
    /// );
    /// ```
    pub fn new(
        tool_name: impl Into<String>,
        tool_description: impl Into<String>,
        shell_code: impl Into<String>,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            tool_description: tool_description.into(),
            shell_code: shell_code.into(),
            commands: Vec::new(),
            old_end_markers: Vec::new(),
        }
    }

    /// Adds a command to the list of available commands shown after setup.
    pub fn with_command(mut self, name: impl Into<String>, description: impl Into<String>) -> Self {
        self.commands.push(ShellCommand::new(name, description));
        self
    }

    /// Adds an old end marker pattern for detecting legacy installations.
    ///
    /// Use this when upgrading from a version that didn't have the standard
    /// end marker. The setup will look for this pattern to find the end of
    /// the old installation block.
    pub fn with_old_end_marker(mut self, marker: impl Into<String>) -> Self {
        self.old_end_markers.push(marker.into());
        self
    }

    /// Returns the start marker comment.
    fn start_marker(&self) -> String {
        format!("# {} - {} shell integration", self.tool_name, self.tool_description)
    }

    /// Returns the end marker comment.
    fn end_marker(&self) -> String {
        format!("# End {} shell integration", self.tool_name)
    }

    /// Returns the full shell integration block with markers.
    fn full_block(&self) -> String {
        format!(
            "\n{}\n# Added by: {} --shell-setup{}\n{}\n",
            self.start_marker(),
            self.tool_name,
            self.shell_code.trim_end(),
            self.end_marker()
        )
    }

    /// Sets up shell integration by adding or upgrading the shell config.
    ///
    /// This function:
    /// 1. Detects the user's shell (bash or zsh)
    /// 2. Finds the appropriate config file
    /// 3. Checks for existing installation
    /// 4. Either installs fresh, upgrades old installation, or updates existing
    pub fn setup(&self) -> Result<()> {
        let home = dirs::home_dir().ok_or(ShellSetupError::NoHomeDir)?;

        // Detect shell from SHELL environment variable
        let shell = std::env::var("SHELL").unwrap_or_default();
        let shell_name = Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Determine which config file to use
        let config_file = match shell_name {
            "zsh" => home.join(".zshrc"),
            "bash" => {
                // Prefer .bashrc, but use .bash_profile on macOS if .bashrc doesn't exist
                let bashrc = home.join(".bashrc");
                let bash_profile = home.join(".bash_profile");
                if bashrc.exists() {
                    bashrc
                } else if bash_profile.exists() {
                    bash_profile
                } else {
                    bashrc // Create .bashrc if neither exists
                }
            }
            _ => {
                return Err(ShellSetupError::UnsupportedShell {
                    shell: shell_name.to_string(),
                    manual_instructions: format!(
                        "Add this to your shell config:\n{}",
                        self.full_block()
                    ),
                });
            }
        };

        // Check if already installed and handle upgrades
        if config_file.exists() {
            let contents = fs::read_to_string(&config_file).map_err(|e| {
                ShellSetupError::ReadError {
                    path: config_file.clone(),
                    source: e,
                }
            })?;

            let start_marker = self.start_marker();
            let end_marker = self.end_marker();

            if contents.contains(&start_marker) {
                // Check if this is a new-style installation (has end marker)
                if contents.contains(&end_marker) {
                    // New-style: replace the entire block
                    let new_contents = self.replace_block(&contents);
                    fs::write(&config_file, new_contents).map_err(|e| {
                        ShellSetupError::WriteError {
                            path: config_file.clone(),
                            source: e,
                        }
                    })?;
                    println!(
                        "{} Shell integration updated in {}",
                        "✓".green(),
                        config_file.display()
                    );
                } else {
                    // Old-style (no end marker): upgrade to new format
                    let new_contents = self.upgrade_old_installation(&contents);
                    fs::write(&config_file, new_contents).map_err(|e| {
                        ShellSetupError::WriteError {
                            path: config_file.clone(),
                            source: e,
                        }
                    })?;
                    println!(
                        "{} Shell integration upgraded in {}",
                        "✓".green(),
                        config_file.display()
                    );
                }
                self.print_activation_instructions(&config_file);
                return Ok(());
            }
        }

        // Fresh installation: append shell integration to config file
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&config_file)
            .map_err(|e| ShellSetupError::WriteError {
                path: config_file.clone(),
                source: e,
            })?;

        file.write_all(self.full_block().as_bytes())
            .map_err(|e| ShellSetupError::WriteError {
                path: config_file.clone(),
                source: e,
            })?;

        println!(
            "{} Shell integration added to {}",
            "✓".green(),
            config_file.display()
        );
        self.print_activation_instructions(&config_file);

        Ok(())
    }

    /// Replaces the shell integration block between start and end markers.
    fn replace_block(&self, contents: &str) -> String {
        let lines: Vec<&str> = contents.lines().collect();
        let mut result: Vec<String> = Vec::new();
        let mut in_block = false;
        let mut block_replaced = false;

        let start_marker = self.start_marker();
        let end_marker = self.end_marker();
        let new_block = self.full_block();

        for line in lines {
            if !in_block && line.contains(&start_marker) {
                // Start of block - skip until end marker
                in_block = true;
                continue;
            }

            if in_block {
                if line.contains(&end_marker) {
                    // End of block - insert new integration
                    result.push(new_block.trim().to_string());
                    in_block = false;
                    block_replaced = true;
                }
                // Skip lines within the block
                continue;
            }

            result.push(line.to_string());
        }

        // If we never found the end marker, something went wrong - append anyway
        if !block_replaced {
            result.push(new_block.trim().to_string());
        }

        result.join("\n") + "\n"
    }

    /// Upgrades old-style shell integration (without end marker) to new format.
    fn upgrade_old_installation(&self, contents: &str) -> String {
        let lines: Vec<&str> = contents.lines().collect();
        let mut result: Vec<String> = Vec::new();
        let mut in_block = false;
        let mut block_replaced = false;

        let start_marker = self.start_marker();
        let new_block = self.full_block();

        for line in lines {
            if !in_block && line.contains(&start_marker) {
                // Start of old block - skip until we find the old end
                in_block = true;
                continue;
            }

            if in_block {
                // Check if this line matches any old end marker
                let is_old_end = self
                    .old_end_markers
                    .iter()
                    .any(|marker| line.contains(marker));

                if is_old_end {
                    // End of old block - insert new integration
                    result.push(new_block.trim().to_string());
                    in_block = false;
                    block_replaced = true;
                }
                // Skip lines within the old block
                continue;
            }

            result.push(line.to_string());
        }

        // If we never found the old end marker, just append the new block
        if !block_replaced {
            result.push(new_block.trim().to_string());
        }

        result.join("\n") + "\n"
    }

    /// Prints activation instructions after shell integration is added or updated.
    fn print_activation_instructions(&self, config_file: &Path) {
        println!();
        println!("To activate, run:");
        println!("  {} {}", "source".cyan(), config_file.display());
        println!();
        println!("Or open a new terminal window.");

        if !self.commands.is_empty() {
            println!();
            println!("Available commands:");
            for cmd in &self.commands {
                println!("  {} - {}", cmd.name.yellow(), cmd.description);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_integration() -> ShellIntegration {
        ShellIntegration::new(
            "testtool",
            "Test Tool",
            r#"
function tt() {
    testtool "$@"
}
alias ttv='tt --verbose'
"#,
        )
        .with_command("tt", "Run testtool")
        .with_command("ttv", "Run testtool with verbose output")
        .with_old_end_marker("alias ttv='tt --verbose'")
    }

    #[test]
    fn test_markers() {
        let integration = create_test_integration();
        assert_eq!(
            integration.start_marker(),
            "# testtool - Test Tool shell integration"
        );
        assert_eq!(integration.end_marker(), "# End testtool shell integration");
    }

    #[test]
    fn test_full_block_contains_markers() {
        let integration = create_test_integration();
        let block = integration.full_block();
        assert!(block.contains(&integration.start_marker()));
        assert!(block.contains(&integration.end_marker()));
        assert!(block.contains("function tt()"));
    }

    #[test]
    fn test_replace_block() {
        let integration = create_test_integration();
        let old_contents = r#"# Some config
export FOO=bar

# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
# End testtool shell integration

# More config
export BAZ=qux
"#;
        let new_contents = integration.replace_block(old_contents);

        // Should preserve content before and after
        assert!(new_contents.contains("export FOO=bar"));
        assert!(new_contents.contains("export BAZ=qux"));
        // Should have new content
        assert!(new_contents.contains("testtool \"$@\""));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
        // Should have end marker
        assert!(new_contents.contains(&integration.end_marker()));
    }

    #[test]
    fn test_upgrade_old_installation() {
        let integration = create_test_integration();
        let old_contents = r#"# Some config
export FOO=bar

# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
alias ttv='tt --verbose'

# More config
export BAZ=qux
"#;
        let new_contents = integration.upgrade_old_installation(old_contents);

        // Should preserve content before and after
        assert!(new_contents.contains("export FOO=bar"));
        assert!(new_contents.contains("export BAZ=qux"));
        // Should have new content
        assert!(new_contents.contains("testtool \"$@\""));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
        // Should have end marker now
        assert!(new_contents.contains(&integration.end_marker()));
    }

    #[test]
    fn test_with_command() {
        let integration = ShellIntegration::new("test", "Test", "code")
            .with_command("cmd1", "Description 1")
            .with_command("cmd2", "Description 2");

        assert_eq!(integration.commands.len(), 2);
        assert_eq!(integration.commands[0].name, "cmd1");
        assert_eq!(integration.commands[1].description, "Description 2");
    }

    #[test]
    fn test_with_old_end_marker() {
        let integration = ShellIntegration::new("test", "Test", "code")
            .with_old_end_marker("marker1")
            .with_old_end_marker("marker2");

        assert_eq!(integration.old_end_markers.len(), 2);
        assert!(integration.old_end_markers.contains(&"marker1".to_string()));
    }

    // ========== Edge case tests for old format upgrade ==========

    #[test]
    fn test_upgrade_old_format_at_file_start() {
        let integration = create_test_integration();
        let old_contents = r#"# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
alias ttv='tt --verbose'

# Other config
export PATH=/usr/bin
"#;
        let new_contents = integration.upgrade_old_installation(old_contents);

        // Should have new content (new block starts with newline internally)
        assert!(new_contents.contains(&integration.start_marker()));
        // Should preserve content after
        assert!(new_contents.contains("export PATH=/usr/bin"));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
        // Should have end marker
        assert!(new_contents.contains(&integration.end_marker()));
    }

    #[test]
    fn test_upgrade_old_format_at_file_end() {
        let integration = create_test_integration();
        let old_contents = r#"# Other config
export PATH=/usr/bin

# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
alias ttv='tt --verbose'"#;
        let new_contents = integration.upgrade_old_installation(old_contents);

        // Should preserve content before
        assert!(new_contents.contains("export PATH=/usr/bin"));
        // Should have new content
        assert!(new_contents.contains("testtool \"$@\""));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
        // Should have end marker
        assert!(new_contents.contains(&integration.end_marker()));
    }

    #[test]
    fn test_upgrade_old_format_only_block() {
        let integration = create_test_integration();
        let old_contents = r#"# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
alias ttv='tt --verbose'"#;
        let new_contents = integration.upgrade_old_installation(old_contents);

        // Should have new content
        assert!(new_contents.contains("testtool \"$@\""));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
        // Should have both markers
        assert!(new_contents.contains(&integration.start_marker()));
        assert!(new_contents.contains(&integration.end_marker()));
    }

    #[test]
    fn test_upgrade_stops_at_first_old_end_marker() {
        let integration = ShellIntegration::new(
            "testtool",
            "Test Tool",
            "\nfunction tt() {\n    testtool \"$@\"\n}\n",
        )
        .with_old_end_marker("FIRST_MARKER")
        .with_old_end_marker("SECOND_MARKER");

        let old_contents = r#"# testtool - Test Tool shell integration
OLD LINE 1
FIRST_MARKER
SHOULD_BE_PRESERVED
SECOND_MARKER
"#;
        let new_contents = integration.upgrade_old_installation(old_contents);

        // Should preserve content after first marker
        assert!(new_contents.contains("SHOULD_BE_PRESERVED"));
        assert!(new_contents.contains("SECOND_MARKER"));
        // Should not have old content before first marker
        assert!(!new_contents.contains("OLD LINE 1"));
    }

    #[test]
    fn test_upgrade_no_old_end_marker_appends_new_block() {
        let integration = create_test_integration();
        // Old content has start marker but no matching end marker
        let old_contents = r#"# Some config
export FOO=bar

# testtool - Test Tool shell integration
# This is old content with no end marker
function old_tt() {
    echo "old"
}
# No matching end marker here

# More config
export BAZ=qux
"#;
        let new_contents = integration.upgrade_old_installation(old_contents);

        // When no old end marker is found, should append the new block
        // The old content after the start marker should be skipped until EOF
        // Then new block appended
        assert!(new_contents.contains(&integration.end_marker()));
        assert!(new_contents.contains("testtool \"$@\""));
    }

    // ========== Edge case tests for new format replacement ==========

    #[test]
    fn test_replace_block_at_file_start() {
        let integration = create_test_integration();
        let old_contents = r#"# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
# End testtool shell integration

# Other config
export PATH=/usr/bin
"#;
        let new_contents = integration.replace_block(old_contents);

        // Should have new content
        assert!(new_contents.contains("testtool \"$@\""));
        // Should preserve content after
        assert!(new_contents.contains("export PATH=/usr/bin"));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
    }

    #[test]
    fn test_replace_block_at_file_end() {
        let integration = create_test_integration();
        let old_contents = r#"# Other config
export PATH=/usr/bin

# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
# End testtool shell integration"#;
        let new_contents = integration.replace_block(old_contents);

        // Should preserve content before
        assert!(new_contents.contains("export PATH=/usr/bin"));
        // Should have new content
        assert!(new_contents.contains("testtool \"$@\""));
        // Should not have old content
        assert!(!new_contents.contains("OLD CONTENT"));
    }

    #[test]
    fn test_replace_block_only_block() {
        let integration = create_test_integration();
        let old_contents = r#"# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD CONTENT
}
# End testtool shell integration"#;
        let new_contents = integration.replace_block(old_contents);

        // Should have new content only
        assert!(new_contents.contains("testtool \"$@\""));
        assert!(!new_contents.contains("OLD CONTENT"));
        // Should have markers
        assert!(new_contents.contains(&integration.start_marker()));
        assert!(new_contents.contains(&integration.end_marker()));
    }

    #[test]
    fn test_replace_block_missing_end_marker_appends() {
        let integration = create_test_integration();
        // Has start marker but no end marker - should append new block
        let old_contents = r#"# Some config
# testtool - Test Tool shell integration
function old() { echo "old"; }
# No end marker
export FOO=bar
"#;
        let new_contents = integration.replace_block(old_contents);

        // Should have the new block appended
        assert!(new_contents.contains(&integration.end_marker()));
        assert!(new_contents.contains("testtool \"$@\""));
    }

    // ========== Idempotency tests ==========

    #[test]
    fn test_replace_is_idempotent() {
        let integration = create_test_integration();
        let initial = r#"# Config
export FOO=bar

# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD
}
# End testtool shell integration

export BAZ=qux
"#;
        let first_replace = integration.replace_block(initial);
        let second_replace = integration.replace_block(&first_replace);

        // Running replace twice should produce identical results
        assert_eq!(first_replace, second_replace);
    }

    #[test]
    fn test_upgrade_then_replace_is_idempotent() {
        let integration = create_test_integration();
        let old_format = r#"# Config
export FOO=bar

# testtool - Test Tool shell integration
# Added by: testtool --shell-setup
function tt() {
    OLD
}
alias ttv='tt --verbose'

export BAZ=qux
"#;
        // First upgrade from old format
        let upgraded = integration.upgrade_old_installation(old_format);
        // Then replace (simulating running --shell-setup again)
        let replaced = integration.replace_block(&upgraded);

        // Should be identical
        assert_eq!(upgraded, replaced);
    }

    // ========== Content preservation tests ==========

    #[test]
    fn test_preserves_blank_lines_before_block() {
        let integration = create_test_integration();
        let old_contents = "export FOO=bar\n\n\n# testtool - Test Tool shell integration\nOLD\n# End testtool shell integration\n";
        let new_contents = integration.replace_block(old_contents);

        // Should preserve the blank lines before the block
        assert!(new_contents.starts_with("export FOO=bar\n\n\n"));
    }

    #[test]
    fn test_preserves_blank_lines_after_block() {
        let integration = create_test_integration();
        let old_contents = "# testtool - Test Tool shell integration\nOLD\n# End testtool shell integration\n\n\nexport FOO=bar\n";
        let new_contents = integration.replace_block(old_contents);

        // Should preserve the blank lines after the block
        assert!(new_contents.contains("\n\nexport FOO=bar"));
    }

    #[test]
    fn test_preserves_comments_around_block() {
        let integration = create_test_integration();
        let old_contents = r#"# === MY CUSTOM SECTION ===
export FOO=bar
# === END CUSTOM ===

# testtool - Test Tool shell integration
OLD
# End testtool shell integration

# === ANOTHER SECTION ===
export BAZ=qux
"#;
        let new_contents = integration.replace_block(old_contents);

        assert!(new_contents.contains("# === MY CUSTOM SECTION ==="));
        assert!(new_contents.contains("# === END CUSTOM ==="));
        assert!(new_contents.contains("# === ANOTHER SECTION ==="));
    }

    // ========== Real-world cwt format tests ==========

    #[test]
    fn test_cwt_old_format_upgrade() {
        // Simulates the actual old cwt format (before wtm was added)
        let integration = ShellIntegration::new(
            "cwt",
            "Change Worktree",
            r#"
function wt() {
    if [ $# -eq 0 ]; then
        cwt
    else
        local target=$(cwt "$@")
        if [ $? -eq 0 ] && [ -n "$target" ]; then
            cd "$target"
        fi
    fi
}

# Quick navigation aliases
alias wtf='wt -f'
alias wtb='wt -p'
alias wtm='wt main'
"#,
        )
        .with_old_end_marker("alias wtb='wt -p'");

        let old_zshrc = r#"# User config
export EDITOR=vim

# cwt - Change Worktree shell integration
# Added by: cwt --shell-setup
function wt() {
    if [ $# -eq 0 ]; then
        cwt
    else
        local target=$(cwt "$@")
        if [ $? -eq 0 ] && [ -n "$target" ]; then
            cd "$target"
        fi
    fi
}

# Quick navigation aliases
alias wtf='wt -f'  # Next worktree
alias wtb='wt -p'  # Previous worktree (back)

# Other aliases
alias ll='ls -la'
"#;
        let new_contents = integration.upgrade_old_installation(old_zshrc);

        // Should preserve user config before
        assert!(new_contents.contains("export EDITOR=vim"));
        // Should preserve other aliases after
        assert!(new_contents.contains("alias ll='ls -la'"));
        // Should have new wtm alias
        assert!(new_contents.contains("alias wtm='wt main'"));
        // Should have end marker now
        assert!(new_contents.contains("# End cwt shell integration"));
        // Should not have duplicate alias definitions
        let wtf_count = new_contents.matches("alias wtf=").count();
        assert_eq!(wtf_count, 1, "Should only have one wtf alias");
    }

    #[test]
    fn test_cwt_new_format_replacement() {
        let integration = ShellIntegration::new(
            "cwt",
            "Change Worktree",
            r#"
function wt() {
    cwt "$@"
}
alias wtf='wt -f'
alias wtb='wt -p'
alias wtm='wt main'
alias wtn='wt -n'
"#,
        );

        let current_zshrc = r#"# User config
export EDITOR=vim

# cwt - Change Worktree shell integration
# Added by: cwt --shell-setup
function wt() {
    cwt "$@"
}
alias wtf='wt -f'
alias wtb='wt -p'
alias wtm='wt main'
# End cwt shell integration

# Other aliases
alias ll='ls -la'
"#;
        let new_contents = integration.replace_block(current_zshrc);

        // Should preserve user config
        assert!(new_contents.contains("export EDITOR=vim"));
        assert!(new_contents.contains("alias ll='ls -la'"));
        // Should have new alias
        assert!(new_contents.contains("alias wtn='wt -n'"));
        // Should have end marker
        assert!(new_contents.contains("# End cwt shell integration"));
    }

    // ========== Safety tests ==========

    #[test]
    fn test_shell_code_no_dangerous_patterns() {
        let integration = create_test_integration();
        let block = integration.full_block();

        // Should not contain dangerous commands
        assert!(!block.contains("rm -rf"));
        assert!(!block.contains("rm -f /"));
        assert!(!block.contains("> /dev/"));
        assert!(!block.contains("chmod 777"));
        assert!(!block.contains("curl | sh"));
        assert!(!block.contains("wget | sh"));
        assert!(!block.contains("eval"));
    }

    #[test]
    fn test_markers_are_valid_shell_comments() {
        let integration = create_test_integration();

        // Markers should start with # (valid shell comment)
        assert!(integration.start_marker().starts_with('#'));
        assert!(integration.end_marker().starts_with('#'));

        // Markers should not contain shell special characters that could break
        let start = integration.start_marker();
        let end = integration.end_marker();
        assert!(!start.contains('`'));
        assert!(!start.contains('$'));
        assert!(!start.contains('\\'));
        assert!(!end.contains('`'));
        assert!(!end.contains('$'));
        assert!(!end.contains('\\'));
    }

    #[test]
    fn test_block_ends_with_newline() {
        let integration = create_test_integration();
        let block = integration.full_block();

        // Block should end with newline for proper file formatting
        assert!(block.ends_with('\n'));
    }

    #[test]
    fn test_added_by_comment_included() {
        let integration = create_test_integration();
        let block = integration.full_block();

        // Should include "Added by" comment for attribution
        assert!(block.contains("# Added by: testtool --shell-setup"));
    }

    // ========== Multiple tool coexistence test ==========

    #[test]
    fn test_different_tools_dont_interfere() {
        let cwt = ShellIntegration::new("cwt", "Change Worktree", "\nfunction wt() { cwt; }\n");
        let prcp = ShellIntegration::new("prcp", "Progress Copy", "\nfunction prmv() { prcp --rm; }\n");

        let file_with_both = r#"# cwt - Change Worktree shell integration
function wt() { OLD_CWT; }
# End cwt shell integration

# prcp - Progress Copy shell integration
function prmv() { OLD_PRCP; }
# End prcp shell integration
"#;
        // Replacing cwt should not affect prcp
        let after_cwt_replace = cwt.replace_block(file_with_both);

        assert!(after_cwt_replace.contains("function wt() { cwt; }"));
        assert!(after_cwt_replace.contains("function prmv() { OLD_PRCP; }"));
        assert!(!after_cwt_replace.contains("OLD_CWT"));

        // Replacing prcp should not affect cwt
        let after_prcp_replace = prcp.replace_block(&after_cwt_replace);

        assert!(after_prcp_replace.contains("function wt() { cwt; }"));
        assert!(after_prcp_replace.contains("function prmv() { prcp --rm; }"));
        assert!(!after_prcp_replace.contains("OLD_PRCP"));
    }
}
