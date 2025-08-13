use anyhow::{anyhow, Result};
use git2::{Cred, FetchOptions, RemoteCallbacks, Repository as GitRepository};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;

pub struct GitCloner {
    use_ssh: bool,
    token: Option<String>,
}

impl GitCloner {
    pub fn new(use_ssh: bool, token: Option<String>) -> Self {
        Self { use_ssh, token }
    }

    pub fn clone_repository(
        &self,
        repo_url: &str,
        target_path: &Path,
        repo_name: &str,
    ) -> Result<()> {
        if target_path.exists() {
            return Ok(());
        }

        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} Cloning {msg}...")
                .unwrap(),
        );
        pb.set_message(repo_name.to_string());

        let mut callbacks = RemoteCallbacks::new();

        if self.use_ssh {
            callbacks.credentials(|_url, username_from_url, _allowed_types| {
                Cred::ssh_key_from_agent(username_from_url.unwrap_or("git"))
            });
        } else if let Some(token) = &self.token {
            callbacks.credentials(move |_url, _username_from_url, _allowed_types| {
                Cred::userpass_plaintext("x-access-token", token)
            });
        }

        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(callbacks);

        let mut builder = git2::build::RepoBuilder::new();
        builder.fetch_options(fetch_options);

        match builder.clone(repo_url, target_path) {
            Ok(_) => {
                pb.finish_with_message(format!("✓ Cloned {}", repo_name));
                Ok(())
            }
            Err(e) => {
                pb.finish_with_message(format!("✗ Failed to clone {}", repo_name));
                Err(anyhow!("Failed to clone {}: {}", repo_name, e))
            }
        }
    }

    pub fn pull_repository(&self, repo_path: &Path, repo_name: &str) -> Result<()> {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} Pulling {msg}...")
                .unwrap(),
        );
        pb.set_message(repo_name.to_string());

        let repo = GitRepository::open(repo_path)?;
        let mut remote = repo.find_remote("origin")?;

        let mut callbacks = RemoteCallbacks::new();

        if self.use_ssh {
            callbacks.credentials(|_url, username_from_url, _allowed_types| {
                Cred::ssh_key_from_agent(username_from_url.unwrap_or("git"))
            });
        } else if let Some(token) = &self.token {
            callbacks.credentials(move |_url, _username_from_url, _allowed_types| {
                Cred::userpass_plaintext("x-access-token", token)
            });
        }

        let mut fetch_options = FetchOptions::new();
        fetch_options.remote_callbacks(callbacks);

        remote.fetch(&["refs/heads/*:refs/heads/*"], Some(&mut fetch_options), None)?;

        pb.finish_with_message(format!("✓ Updated {}", repo_name));
        Ok(())
    }
}