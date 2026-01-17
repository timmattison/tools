use anyhow::{Context, Result};
use arboard::Clipboard;
use aws_config::meta::region::RegionProviderChain;
use aws_credential_types::Credentials;
use aws_sdk_sts::Client as StsClient;
use buildinfo::version_string;
use std::fs::{create_dir_all, File};
use std::io::Write;

const UPPER_AWS_ACCESS_KEY_ID: &str = "AWS_ACCESS_KEY_ID";
const UPPER_AWS_SECRET_ACCESS_KEY: &str = "AWS_SECRET_ACCESS_KEY";
const UPPER_AWS_SESSION_TOKEN: &str = "AWS_SESSION_TOKEN";

fn string_containing(input: &[String], pattern: &str) -> String {
    for line in input {
        if line.contains(pattern) {
            return line.to_string();
        }
    }
    String::new()
}

async fn verify_credentials(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: &str,
) -> Result<aws_sdk_sts::operation::get_caller_identity::GetCallerIdentityOutput> {
    let creds = Credentials::new(
        access_key_id.to_string(),
        secret_access_key.to_string(),
        Some(session_token.to_string()),
        None,
        "update-aws-credentials",
    );

    let region_provider = RegionProviderChain::default_provider();
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region_provider)
        .credentials_provider(creds)
        .load()
        .await;

    let sts_client = StsClient::new(&config);
    sts_client
        .get_caller_identity()
        .send()
        .await
        .context("Failed to validate AWS credentials")
}

fn write_credentials(
    access_key_id: &str,
    secret_access_key: &str,
    session_token: &str,
) -> Result<()> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let aws_dir = home.join(".aws");
    let credentials_path = aws_dir.join("credentials");

    // Ensure the .aws directory exists
    if !aws_dir.exists() {
        create_dir_all(&aws_dir).context("Could not create .aws directory")?;
    }

    let mut file = File::create(&credentials_path).context("Could not create AWS credentials file")?;

    let output = format!(
        "[default]\naws_access_key_id = {}\naws_secret_access_key = {}\naws_session_token = {}\n",
        access_key_id, secret_access_key, session_token
    );

    file.write_all(output.as_bytes())
        .context("Could not write to AWS credentials file")?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("update-aws-credentials {}", version_string!());
        return Ok(());
    }

    let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;
    let clipboard_data = clipboard
        .get_text()
        .context("Failed to read from clipboard")?;

    let clipboard_strings: Vec<String> = clipboard_data.lines().map(String::from).collect();

    let (aws_access_key_id, aws_secret_access_key, aws_session_token) = if clipboard_strings.len() == 3 {
        (
            string_containing(&clipboard_strings, UPPER_AWS_ACCESS_KEY_ID),
            string_containing(&clipboard_strings, UPPER_AWS_SECRET_ACCESS_KEY),
            string_containing(&clipboard_strings, UPPER_AWS_SESSION_TOKEN),
        )
    } else if clipboard_strings.len() == 4 {
        (
            string_containing(&clipboard_strings, &UPPER_AWS_ACCESS_KEY_ID.to_lowercase()),
            string_containing(&clipboard_strings, &UPPER_AWS_SECRET_ACCESS_KEY.to_lowercase()),
            string_containing(&clipboard_strings, &UPPER_AWS_SESSION_TOKEN.to_lowercase()),
        )
    } else {
        anyhow::bail!("👎 Expected 3 or 4 lines in clipboard");
    };

    if aws_access_key_id.is_empty() || !aws_access_key_id.contains('=') {
        anyhow::bail!("👎 Could not find the AWS access key ID in the clipboard");
    }

    if aws_secret_access_key.is_empty() || !aws_secret_access_key.contains('=') {
        anyhow::bail!("👎 Could not find the AWS secret access key in the clipboard");
    }

    if aws_session_token.is_empty() || !aws_session_token.contains('=') {
        anyhow::bail!("👎 Could not find the AWS session token in the clipboard");
    }

    let access_key_id = aws_access_key_id
        .split('=')
        .nth(1)
        .context("Invalid AWS access key ID format")?
        .trim()
        .replace('"', "");

    let secret_access_key = aws_secret_access_key
        .split('=')
        .nth(1)
        .context("Invalid AWS secret access key format")?
        .trim()
        .replace('"', "");

    let session_token = aws_session_token
        .split('=')
        .nth(1)
        .context("Invalid AWS session token format")?
        .trim()
        .replace('"', "");

    let caller_identity = verify_credentials(&access_key_id, &secret_access_key, &session_token).await?;

    write_credentials(&access_key_id, &secret_access_key, &session_token)?;

    println!("👍 Credentials updated successfully. Your AWS default profile is now set to the credentials in your clipboard.");
    println!();
    println!(
        "Your AWS account ID is {}",
        caller_identity.account().context("No account ID returned")?
    );
    println!(
        "Your AWS user ID is {}",
        caller_identity.user_id().context("No user ID returned")?
    );

    Ok(())
}
