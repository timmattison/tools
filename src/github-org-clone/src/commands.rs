use crate::git::GitCloner;
use crate::github::{GitHubClient, Repository};
use anyhow::Result;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Semaphore;

const MAX_CONCURRENT_CLONES: usize = 5;

pub async fn show_user_info(client: &GitHubClient) -> Result<()> {
    let user = client.get_current_user().await?;
    
    println!("\n{}", "GitHub Account Information".bold().green());
    println!("{}", "─".repeat(40));
    println!("Username: {}", user.login.cyan());
    if let Some(name) = user.name {
        println!("Name: {}", name);
    }
    if let Some(email) = user.email {
        println!("Email: {}", email);
    }
    println!("Public Repos: {}", user.public_repos.to_string().yellow());
    println!("Public Gists: {}", user.public_gists.to_string().yellow());
    println!("Followers: {}", user.followers.to_string().yellow());
    println!("Following: {}", user.following.to_string().yellow());
    
    Ok(())
}

pub async fn list_organizations(client: &GitHubClient) -> Result<()> {
    let orgs = client.list_organizations().await?;
    
    if orgs.is_empty() {
        println!("\n{}", "No organizations found".yellow());
        return Ok(());
    }
    
    println!("\n{}", "Organizations".bold().green());
    println!("{}", "─".repeat(40));
    
    for org in orgs {
        println!("• {} {}", 
            org.login.cyan(),
            if let Some(repos) = org.public_repos {
                format!("({} repos)", repos).dimmed().to_string()
            } else {
                String::new()
            }
        );
        if let Some(desc) = org.description {
            println!("  {}", desc.dimmed());
        }
    }
    
    Ok(())
}

pub async fn clone_organization_repos(
    client: &GitHubClient,
    org: &str,
    output_dir: &Path,
    use_ssh: bool,
    archive: bool,
    token: Option<String>,
) -> Result<()> {
    println!("\n{} {}", "Fetching repositories for organization:".bold(), org.cyan());
    
    let repos = client.list_org_repositories(org).await?;
    
    if repos.is_empty() {
        println!("{}", "No repositories found".yellow());
        return Ok(());
    }
    
    println!("Found {} repositories", repos.len().to_string().yellow());
    
    let org_dir = output_dir.join(org);
    std::fs::create_dir_all(&org_dir)?;
    
    clone_repositories(client, repos, &org_dir, use_ssh, archive, token).await?;
    
    Ok(())
}

pub async fn clone_all_organizations_repos(
    client: &GitHubClient,
    output_dir: &Path,
    use_ssh: bool,
    archive: bool,
    token: Option<String>,
) -> Result<()> {
    println!("\n{}", "Fetching all organizations...".bold());
    
    let orgs = client.list_organizations().await?;
    
    if orgs.is_empty() {
        println!("{}", "No organizations found".yellow());
        println!("Fetching personal repositories...");
        
        let repos = client.list_user_repositories().await?;
        if !repos.is_empty() {
            let personal_dir = output_dir.join("personal");
            std::fs::create_dir_all(&personal_dir)?;
            clone_repositories(client, repos, &personal_dir, use_ssh, archive, token).await?;
        }
        return Ok(());
    }
    
    println!("Found {} organizations", orgs.len().to_string().yellow());
    
    for org in orgs {
        println!("\n{} {}", "Processing organization:".bold(), org.login.cyan());
        
        match clone_organization_repos(
            client,
            &org.login,
            output_dir,
            use_ssh,
            archive,
            token.clone(),
        ).await {
            Ok(_) => {},
            Err(e) => {
                eprintln!("{} Failed to process {}: {}", 
                    "✗".red(), 
                    org.login, 
                    e
                );
            }
        }
    }
    
    println!("\n{}", "Fetching personal repositories...".bold());
    let personal_repos = client.list_user_repositories().await?;
    if !personal_repos.is_empty() {
        let personal_dir = output_dir.join("personal");
        std::fs::create_dir_all(&personal_dir)?;
        clone_repositories(client, personal_repos, &personal_dir, use_ssh, archive, token).await?;
    }
    
    Ok(())
}

async fn clone_repositories(
    client: &GitHubClient,
    repos: Vec<Repository>,
    output_dir: &Path,
    use_ssh: bool,
    archive: bool,
    token: Option<String>,
) -> Result<()> {
    let multi_progress = MultiProgress::new();
    let main_pb = multi_progress.add(ProgressBar::new(repos.len() as u64));
    main_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} repositories")
            .unwrap()
            .progress_chars("=>-"),
    );
    
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CLONES));
    let cloner = Arc::new(GitCloner::new(use_ssh, token));
    let client = Arc::new(client.clone());
    
    let mut tasks = vec![];
    
    for repo in repos {
        let semaphore = semaphore.clone();
        let cloner = cloner.clone();
        let client = client.clone();
        let output_dir = output_dir.to_path_buf();
        let main_pb = main_pb.clone();
        
        let task = tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            
            let repo_path = output_dir.join(&repo.name);
            let url = if use_ssh {
                &repo.ssh_url
            } else {
                &repo.clone_url
            };
            
            let result = if repo_path.exists() {
                cloner.pull_repository(&repo_path, &repo.name)
            } else {
                cloner.clone_repository(url, &repo_path, &repo.name)
            };
            
            if let Err(e) = result {
                eprintln!("{} Failed to clone/update {}: {}", 
                    "✗".red(), 
                    repo.name, 
                    e
                );
            }
            
            if archive && !repo.archived {
                let parts: Vec<&str> = repo.full_name.split('/').collect();
                if parts.len() == 2 {
                    if let Err(e) = client.archive_repository(parts[0], parts[1]).await {
                        eprintln!("{} Failed to archive {}: {}", 
                            "✗".red(), 
                            repo.name, 
                            e
                        );
                    } else {
                        println!("{} Archived {}", "📦".to_string(), repo.name.yellow());
                    }
                }
            }
            
            main_pb.inc(1);
        });
        
        tasks.push(task);
    }
    
    for task in tasks {
        let _ = task.await;
    }
    
    main_pb.finish_with_message("✓ All repositories processed");
    
    Ok(())
}