use anyhow::Result;
use aws_sdk_sts::Client;
use clap::Parser;
use serde_json::json;

#[derive(Parser, Debug)]
#[command(name = "aa")]
#[command(about = "Display AWS account information", long_about = None)]
struct Args {
    /// AWS profile to use
    profile: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    
    if let Some(profile) = args.profile {
        config_loader = config_loader.profile_name(profile);
    }
    
    let config = config_loader.load().await;
    let client = Client::new(&config);
    
    let identity = client.get_caller_identity().send().await?;
    
    let output = json!({
        "UserId": identity.user_id().unwrap_or_default(),
        "Account": identity.account().unwrap_or_default(),
        "Arn": identity.arn().unwrap_or_default()
    });
    
    println!("{}", serde_json::to_string_pretty(&output)?);
    
    Ok(())
}