mod auth;
mod commands;
mod git;
mod github;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "org-borg",
    about = "Assimilate GitHub organization repositories - resistance is futile",
    version
)]
struct Cli {
    #[arg(
        short,
        long,
        help = "GitHub personal access token (can also be set via GITHUB_TOKEN env var)"
    )]
    token: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Show current GitHub user information")]
    Whoami,

    #[command(about = "List all organizations you have access to")]
    ListOrgs,

    #[command(about = "Clone all repositories from a specific organization")]
    CloneOrg {
        #[arg(help = "Organization name")]
        org: String,

        #[arg(
            short,
            long,
            default_value = "./repos",
            help = "Output directory for cloned repositories"
        )]
        output: PathBuf,

        #[arg(
            short,
            long,
            help = "Use SSH URLs for cloning (default: HTTPS)"
        )]
        ssh: bool,

        #[arg(
            short,
            long,
            help = "Archive repositories after cloning"
        )]
        archive: bool,
    },

    #[command(about = "Clone all repositories from all organizations")]
    CloneAll {
        #[arg(
            short,
            long,
            default_value = "./repos",
            help = "Output directory for cloned repositories"
        )]
        output: PathBuf,

        #[arg(
            short,
            long,
            help = "Use SSH URLs for cloning (default: HTTPS)"
        )]
        ssh: bool,

        #[arg(
            short,
            long,
            help = "Archive repositories after cloning"
        )]
        archive: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Try to get token in this order:
    // 1. CLI argument
    // 2. GITHUB_TOKEN environment variable
    // 3. gh CLI authentication
    let token = cli.token.clone()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .or_else(|| {
            if auth::is_gh_installed() {
                auth::get_gh_token().ok().flatten()
            } else {
                None
            }
        });

    // Check if we need a token and don't have one
    if token.is_none() && !matches!(cli.command, Commands::CloneOrg { ssh: true, .. } | Commands::CloneAll { ssh: true, .. }) {
        eprintln!("{}", "Error: GitHub authentication required.".red());
        eprintln!();
        eprintln!("{}", "You can authenticate using one of these methods:".yellow());
        eprintln!("  1. Use the GitHub CLI: {}", "gh auth login".cyan());
        eprintln!("  2. Set environment variable: {}", "export GITHUB_TOKEN=<your-token>".cyan());
        eprintln!("  3. Pass token as argument: {}", "--token <your-token>".cyan());
        eprintln!();
        
        if auth::is_gh_installed() {
            if let Ok(Some(status)) = auth::get_gh_auth_status() {
                eprintln!("{}", "Current gh auth status:".dimmed());
                for line in status.lines() {
                    eprintln!("  {}", line.dimmed());
                }
            }
        } else {
            eprintln!("{}", "GitHub CLI (gh) is not installed.".dimmed());
            eprintln!("{}", "Install it from: https://cli.github.com".dimmed());
        }
        
        eprintln!();
        eprintln!("{}", "To create a personal access token:".dimmed());
        eprintln!("{}", "  https://github.com/settings/tokens".dimmed());
        eprintln!("{}", "  Required scopes: repo, read:org".dimmed());
        std::process::exit(1);
    }

    // Show authentication source if verbose
    if token.is_some() {
        let auth_source = if cli.token.is_some() {
            "CLI argument"
        } else if std::env::var("GITHUB_TOKEN").is_ok() {
            "GITHUB_TOKEN environment variable"
        } else {
            "GitHub CLI (gh)"
        };
        
        if std::env::var("VERBOSE").is_ok() {
            eprintln!("{} {}", "Using authentication from:".dimmed(), auth_source.green());
        }
    }

    let client = if let Some(token) = token.clone() {
        github::GitHubClient::new(token)?
    } else {
        github::GitHubClient::new(String::new())?
    };

    match cli.command {
        Commands::Whoami => {
            commands::show_user_info(&client).await?;
        }
        Commands::ListOrgs => {
            commands::list_organizations(&client).await?;
        }
        Commands::CloneOrg {
            org,
            output,
            ssh,
            archive,
        } => {
            std::fs::create_dir_all(&output)?;
            commands::clone_organization_repos(
                &client,
                &org,
                &output,
                ssh,
                archive,
                token,
            )
            .await?;
        }
        Commands::CloneAll {
            output,
            ssh,
            archive,
        } => {
            std::fs::create_dir_all(&output)?;
            commands::clone_all_organizations_repos(
                &client,
                &output,
                ssh,
                archive,
                token,
            )
            .await?;
        }
    }

    Ok(())
}