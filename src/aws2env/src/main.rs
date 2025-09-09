use clap::Parser;
use dirs::home_dir;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
enum Aws2EnvError {
    #[error("Home directory not found")]
    HomeNotFound,
    #[error("AWS config directory not found at {0}")]
    AwsConfigNotFound(String),
    #[error("Failed to read file {0}: {1}")]
    FileReadError(String, std::io::Error),
    #[error("Profile '{0}' not found")]
    ProfileNotFound(String),
}

type Result<T> = std::result::Result<T, Aws2EnvError>;

#[derive(Parser, Debug)]
#[command(name = "aws2env")]
#[command(about = "Convert AWS credentials to environment variables", long_about = None)]
struct Args {
    /// AWS profile to use (default: "default")
    #[arg(short, long, default_value = "default")]
    profile: String,

    /// Show all available profiles
    #[arg(short, long)]
    list: bool,
}

#[derive(Debug, Clone)]
struct AwsCredentials {
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    region: Option<String>,
}

fn parse_ini_file(content: &str) -> HashMap<String, HashMap<String, String>> {
    let mut result = HashMap::new();
    let mut current_section = String::new();
    
    for line in content.lines() {
        let line = line.trim();
        
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        
        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len() - 1].to_string();
            result.entry(current_section.clone()).or_insert_with(HashMap::new);
        } else if line.contains('=') {
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() == 2 && !current_section.is_empty() {
                let key = parts[0].trim().to_string();
                let value = parts[1].trim().to_string();
                result.entry(current_section.clone())
                    .or_insert_with(HashMap::new)
                    .insert(key, value);
            }
        }
    }
    
    result
}

fn get_aws_config_path() -> Result<PathBuf> {
    let home = home_dir().ok_or(Aws2EnvError::HomeNotFound)?;
    let aws_dir = home.join(".aws");
    
    if !aws_dir.exists() {
        return Err(Aws2EnvError::AwsConfigNotFound(aws_dir.display().to_string()));
    }
    
    Ok(aws_dir)
}

fn load_credentials(profile: &str) -> Result<AwsCredentials> {
    let aws_dir = get_aws_config_path()?;
    let mut credentials = AwsCredentials {
        access_key_id: None,
        secret_access_key: None,
        session_token: None,
        region: None,
    };
    
    // Load credentials file
    let credentials_path = aws_dir.join("credentials");
    if credentials_path.exists() {
        let content = fs::read_to_string(&credentials_path)
            .map_err(|e| Aws2EnvError::FileReadError(credentials_path.display().to_string(), e))?;
        
        let parsed = parse_ini_file(&content);
        
        if let Some(profile_data) = parsed.get(profile) {
            credentials.access_key_id = profile_data.get("aws_access_key_id").cloned();
            credentials.secret_access_key = profile_data.get("aws_secret_access_key").cloned();
            credentials.session_token = profile_data.get("aws_session_token").cloned();
        }
    }
    
    // Load config file
    let config_path = aws_dir.join("config");
    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| Aws2EnvError::FileReadError(config_path.display().to_string(), e))?;
        
        let parsed = parse_ini_file(&content);
        
        // Config file uses "profile <name>" format for non-default profiles
        let config_section = if profile == "default" {
            profile.to_string()
        } else {
            format!("profile {}", profile)
        };
        
        if let Some(profile_data) = parsed.get(&config_section) {
            credentials.region = profile_data.get("region").cloned();
            
            // Config file can also contain credentials
            if credentials.access_key_id.is_none() {
                credentials.access_key_id = profile_data.get("aws_access_key_id").cloned();
            }
            if credentials.secret_access_key.is_none() {
                credentials.secret_access_key = profile_data.get("aws_secret_access_key").cloned();
            }
            if credentials.session_token.is_none() {
                credentials.session_token = profile_data.get("aws_session_token").cloned();
            }
        }
    }
    
    // Check if we found any credentials
    if credentials.access_key_id.is_none() && credentials.secret_access_key.is_none() {
        return Err(Aws2EnvError::ProfileNotFound(profile.to_string()));
    }
    
    Ok(credentials)
}

fn list_profiles() -> Result<Vec<String>> {
    let aws_dir = get_aws_config_path()?;
    let mut profiles = Vec::new();
    
    // Check credentials file
    let credentials_path = aws_dir.join("credentials");
    if credentials_path.exists() {
        let content = fs::read_to_string(&credentials_path)
            .map_err(|e| Aws2EnvError::FileReadError(credentials_path.display().to_string(), e))?;
        
        let parsed = parse_ini_file(&content);
        for profile in parsed.keys() {
            if !profiles.contains(profile) {
                profiles.push(profile.clone());
            }
        }
    }
    
    // Check config file
    let config_path = aws_dir.join("config");
    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| Aws2EnvError::FileReadError(config_path.display().to_string(), e))?;
        
        let parsed = parse_ini_file(&content);
        for section in parsed.keys() {
            let profile = if section.starts_with("profile ") {
                section.strip_prefix("profile ").unwrap().to_string()
            } else {
                section.clone()
            };
            
            if !profiles.contains(&profile) {
                profiles.push(profile);
            }
        }
    }
    
    profiles.sort();
    Ok(profiles)
}

/// Escape a string for safe use in shell commands using single quotes
/// This handles all special characters by wrapping in single quotes and
/// escaping any embedded single quotes
fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    
    // Check if the string needs escaping
    // Safe characters that don't need escaping when not quoted
    let needs_escaping = s.chars().any(|c| {
        !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != '.' && c != '/'
    });
    
    if !needs_escaping {
        return s.to_string();
    }
    
    // Use single quotes and escape any embedded single quotes
    // The strategy is: close the quote, add escaped single quote, reopen the quote
    // For example: 'can'\''t' becomes can't when evaluated by the shell
    let escaped = s.replace('\'', "'\\''");
    format!("'{}'", escaped)
}

fn print_export_commands(credentials: &AwsCredentials) {
    if let Some(access_key) = &credentials.access_key_id {
        println!("export AWS_ACCESS_KEY_ID={}", shell_escape(access_key));
    }
    
    if let Some(secret_key) = &credentials.secret_access_key {
        println!("export AWS_SECRET_ACCESS_KEY={}", shell_escape(secret_key));
    }
    
    if let Some(session_token) = &credentials.session_token {
        println!("export AWS_SESSION_TOKEN={}", shell_escape(session_token));
    }
    
    if let Some(region) = &credentials.region {
        println!("export AWS_DEFAULT_REGION={}", shell_escape(region));
        println!("export AWS_REGION={}", shell_escape(region));
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    if args.list {
        let profiles = list_profiles()?;
        if profiles.is_empty() {
            println!("No AWS profiles found");
        } else {
            println!("Available AWS profiles:");
            for profile in profiles {
                println!("  {}", profile);
            }
        }
        return Ok(());
    }
    
    let credentials = load_credentials(&args.profile)?;
    print_export_commands(&credentials);
    
    Ok(())
}
