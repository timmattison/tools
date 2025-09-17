use anyhow::Result;
use aws_sdk_sts::Client;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
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