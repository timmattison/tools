use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;
use std::time::Duration;
use tokio::task;
use tokio::time::sleep;
use futures::future::join_all;

#[derive(Debug, Deserialize)]
pub struct WranglerListResponse {
    pub result: Vec<R2Object>,
}

#[derive(Debug, Deserialize)]
pub struct R2Object {
    pub key: String,
    #[allow(dead_code)]
    pub size: u64,
    #[allow(dead_code)]
    pub etag: String,
    #[allow(dead_code)]
    pub last_modified: String,
}

pub struct R2WranglerClient;

impl R2WranglerClient {
    pub fn new() -> Self {
        Self
    }

    pub async fn list_objects(&self, bucket_name: &str) -> Result<(Vec<String>, bool)> {
        // Run wrangler command to get bucket listing
        // This uses the same approach as the original command
        let bucket_arg = format!("{}/", bucket_name);
        let output = task::spawn_blocking(move || {
            Command::new("wrangler")
                .args(&["r2", "object", "get", "--remote", &bucket_arg])
                .output()
        })
        .await
        .context("Failed to spawn wrangler task")?
        .context("Failed to execute wrangler command. Is wrangler installed?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Wrangler command failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        
        // The output might have some non-JSON content at the beginning
        // Find the start of the JSON object
        let json_start = stdout.find('{').ok_or_else(|| {
            anyhow::anyhow!("No JSON found in wrangler output")
        })?;
        
        let json_str = &stdout[json_start..];
        
        // Parse the JSON output
        let full_response: serde_json::Value = serde_json::from_str(json_str)
            .context("Failed to parse wrangler output as JSON")?;
        
        let response: WranglerListResponse = serde_json::from_value(full_response.clone())
            .context("Failed to parse wrangler response structure")?;

        // Check if results are truncated
        let has_more = if let Some(result_info) = full_response.get("result_info") {
            result_info.get("is_truncated").and_then(|v| v.as_bool()).unwrap_or(false)
        } else {
            false
        };

        let keys = response.result.into_iter().map(|obj| obj.key).collect();
        Ok((keys, has_more))
    }

    pub async fn delete_objects(&self, bucket_name: &str, keys: Vec<String>) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }

        // Increase parallelism for better performance
        const CONCURRENCY: usize = 10;
        println!("Deleting {} objects ({} concurrent operations)...", keys.len(), CONCURRENCY);
        
        let mut failed_keys = Vec::new();
        let mut deleted_count = 0;
        
        // Process in chunks with higher parallelism
        for chunk in keys.chunks(CONCURRENCY) {
            let mut tasks = Vec::new();
            
            for key in chunk {
                let bucket = bucket_name.to_string();
                let key = key.clone();
                
                let task = task::spawn_blocking(move || {
                    // Try up to 3 times with exponential backoff
                    let mut attempts = 0;
                    let max_attempts = 3;
                    
                    loop {
                        attempts += 1;
                        let object_path = format!("{}/{}", bucket, key);
                        let output = Command::new("wrangler")
                            .args(&["r2", "object", "delete", "--remote", &object_path])
                            .output();

                        match output {
                            Ok(output) if output.status.success() => return Ok(()),
                            Ok(output) => {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                if attempts >= max_attempts {
                                    return Err((key.clone(), format!("Command failed after {} attempts: {}", attempts, stderr)));
                                }
                                // Sleep before retry (blocking sleep since we're in spawn_blocking)
                                std::thread::sleep(std::time::Duration::from_millis(200 * attempts as u64));
                            }
                            Err(e) => {
                                if attempts >= max_attempts {
                                    return Err((key.clone(), format!("Failed to execute after {} attempts: {}", attempts, e)));
                                }
                                std::thread::sleep(std::time::Duration::from_millis(200 * attempts as u64));
                            }
                        }
                    }
                });
                
                tasks.push(task);
            }
            
            // Wait for this batch to complete
            let results = join_all(tasks).await;
            
            for result in results {
                match result {
                    Ok(Ok(())) => {
                        deleted_count += 1;
                        if deleted_count % 20 == 0 {
                            println!("Progress: {} objects deleted...", deleted_count);
                        }
                    }
                    Ok(Err((key, error))) => {
                        eprintln!("Failed to delete {}: {}", key, error);
                        failed_keys.push(key);
                    }
                    Err(e) => {
                        eprintln!("Task join error: {}", e);
                    }
                }
            }
            
            // Add a small delay between batches to avoid overwhelming the API
            if chunk.len() > 0 {
                sleep(Duration::from_millis(50)).await;
            }
        }

        println!("Successfully deleted {} objects", deleted_count);

        if !failed_keys.is_empty() {
            return Err(anyhow::anyhow!(
                "Failed to delete {} objects. First few failures: {:?}", 
                failed_keys.len(), 
                &failed_keys[..failed_keys.len().min(5)]
            ));
        }

        Ok(())
    }
}