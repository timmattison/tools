use anyhow::{anyhow, Result};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    #[allow(dead_code)]
    token: String,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
    #[allow(dead_code)]
    pub id: i64,
    pub name: Option<String>,
    pub email: Option<String>,
    pub public_repos: i32,
    pub public_gists: i32,
    pub followers: i32,
    pub following: i32,
}

#[derive(Debug, Deserialize)]
pub struct Organization {
    pub login: String,
    #[allow(dead_code)]
    pub id: i64,
    pub description: Option<String>,
    pub public_repos: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Repository {
    #[allow(dead_code)]
    pub id: i64,
    pub name: String,
    pub full_name: String,
    #[allow(dead_code)]
    pub html_url: String,
    pub ssh_url: String,
    pub clone_url: String,
    #[allow(dead_code)]
    pub description: Option<String>,
    #[allow(dead_code)]
    pub fork: bool,
    pub archived: bool,
    #[allow(dead_code)]
    pub disabled: bool,
    #[allow(dead_code)]
    pub private: bool,
    #[allow(dead_code)]
    pub default_branch: Option<String>,
}

#[derive(Debug, Serialize)]
struct ArchiveRequest {
    archived: bool,
}

impl GitHubClient {
    pub fn new(token: String) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token))?,
        );
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("github-org-clone"),
        );
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/vnd.github.v3+json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self { client, token })
    }

    pub async fn get_current_user(&self) -> Result<User> {
        let response = self
            .client
            .get("https://api.github.com/user")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to get current user: {}",
                response.status()
            ));
        }

        Ok(response.json().await?)
    }

    pub async fn list_organizations(&self) -> Result<Vec<Organization>> {
        let mut organizations = Vec::new();
        let mut page = 1;
        let per_page = 100;

        loop {
            let response = self
                .client
                .get("https://api.github.com/user/orgs")
                .query(&[("per_page", per_page), ("page", page)])
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(anyhow!(
                    "Failed to list organizations: {}",
                    response.status()
                ));
            }

            let orgs: Vec<Organization> = response.json().await?;
            if orgs.is_empty() {
                break;
            }

            organizations.extend(orgs);
            page += 1;
        }

        Ok(organizations)
    }

    pub async fn list_org_repositories(&self, org: &str) -> Result<Vec<Repository>> {
        let mut repositories = Vec::new();
        let mut page = 1;
        let per_page = 100;

        loop {
            let response = self
                .client
                .get(&format!("https://api.github.com/orgs/{}/repos", org))
                .query(&[
                    ("per_page", per_page.to_string()),
                    ("page", page.to_string()),
                    ("type", "all".to_string()),
                ])
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(anyhow!(
                    "Failed to list repositories for org {}: {}",
                    org,
                    response.status()
                ));
            }

            let repos: Vec<Repository> = response.json().await?;
            if repos.is_empty() {
                break;
            }

            repositories.extend(repos);
            page += 1;
        }

        Ok(repositories)
    }

    pub async fn list_user_repositories(&self) -> Result<Vec<Repository>> {
        let mut repositories = Vec::new();
        let mut page = 1;
        let per_page = 100;

        loop {
            let response = self
                .client
                .get("https://api.github.com/user/repos")
                .query(&[
                    ("per_page", per_page.to_string()),
                    ("page", page.to_string()),
                    ("type", "all".to_string()),
                ])
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(anyhow!(
                    "Failed to list user repositories: {}",
                    response.status()
                ));
            }

            let repos: Vec<Repository> = response.json().await?;
            if repos.is_empty() {
                break;
            }

            repositories.extend(repos);
            page += 1;
        }

        Ok(repositories)
    }

    pub async fn archive_repository(&self, owner: &str, repo: &str) -> Result<()> {
        let response = self
            .client
            .patch(&format!(
                "https://api.github.com/repos/{}/{}",
                owner, repo
            ))
            .json(&ArchiveRequest { archived: true })
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to archive repository {}/{}: {}",
                owner,
                repo,
                response.status()
            ));
        }

        Ok(())
    }
}