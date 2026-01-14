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
}
