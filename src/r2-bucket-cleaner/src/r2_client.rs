use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use aws_sdk_s3::Client;
use op_cache::{OpCache, OpPath};

const ACCOUNT_ID_ENV: &str = "R2_ACCOUNT_ID";
const ACCESS_KEY_ID_ENV: &str = "R2_ACCESS_KEY_ID";
const SECRET_ACCESS_KEY_ENV: &str = "R2_SECRET_ACCESS_KEY";

const ACCOUNT_ID_OP_PATH: &str = "op://Private/R2 Credentials/R2_ACCOUNT_ID";
const ACCESS_KEY_ID_OP_PATH: &str = "op://Private/R2 Credentials/R2_ACCESS_KEY_ID";
const SECRET_ACCESS_KEY_OP_PATH: &str = "op://Private/R2 Credentials/R2_SECRET_ACCESS_KEY";

// Preview-only page size for the initial `list_objects` call that drives the
// pre-delete confirmation. Full-bucket enumeration (`list_all_objects`) uses
// `LIST_ALL_PAGE_SIZE` instead.
const PREVIEW_PAGE_SIZE: i32 = 20;
const LIST_ALL_PAGE_SIZE: i32 = 1000;
const DELETE_BATCH_SIZE: usize = 1000;

pub fn endpoint_url(account_id: &str) -> String {
    format!("https://{account_id}.r2.cloudflarestorage.com")
}

pub struct R2Credentials {
    pub account_id: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl R2Credentials {
    pub fn load() -> Result<Self> {
        if let (Ok(account_id), Ok(access_key_id), Ok(secret_access_key)) = (
            std::env::var(ACCOUNT_ID_ENV),
            std::env::var(ACCESS_KEY_ID_ENV),
            std::env::var(SECRET_ACCESS_KEY_ENV),
        ) {
            if !account_id.is_empty() && !access_key_id.is_empty() && !secret_access_key.is_empty()
            {
                return Ok(Self {
                    account_id,
                    access_key_id,
                    secret_access_key,
                });
            }
        }

        let cache = OpCache::new().map_err(|e| anyhow::anyhow!("{e}")).context(
            "initializing op-cache (run from inside a git repo, \
             or set R2_ACCOUNT_ID/R2_ACCESS_KEY_ID/R2_SECRET_ACCESS_KEY)",
        )?;

        let account_id = read_op(&cache, ACCOUNT_ID_OP_PATH, ACCOUNT_ID_ENV)?;
        let access_key_id = read_op(&cache, ACCESS_KEY_ID_OP_PATH, ACCESS_KEY_ID_ENV)?;
        let secret_access_key = read_op(&cache, SECRET_ACCESS_KEY_OP_PATH, SECRET_ACCESS_KEY_ENV)?;

        Ok(Self {
            account_id,
            access_key_id,
            secret_access_key,
        })
    }
}

fn read_op(cache: &OpCache, op_path: &str, env_var: &str) -> Result<String> {
    let path = OpPath::new(op_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    cache
        .read(&path, Some(env_var))
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("reading {op_path} from 1Password"))
}

pub struct R2Client {
    s3: Client,
}

impl R2Client {
    pub async fn new() -> Result<Self> {
        let creds = R2Credentials::load()?;
        let credentials = Credentials::new(
            &creds.access_key_id,
            &creds.secret_access_key,
            None,
            None,
            "r2-bucket-cleaner",
        );
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint_url(&creds.account_id))
            .region(Region::new("auto"))
            .credentials_provider(credentials)
            .build();
        Ok(Self {
            s3: Client::from_conf(config),
        })
    }

    pub async fn list_objects(&self, bucket: &str) -> Result<(Vec<String>, bool)> {
        let resp = self
            .s3
            .list_objects_v2()
            .bucket(bucket)
            .max_keys(PREVIEW_PAGE_SIZE)
            .send()
            .await
            .with_context(|| format!("listing objects in bucket '{bucket}'"))?;

        let keys: Vec<String> = resp
            .contents()
            .iter()
            .filter_map(|obj| obj.key().map(str::to_string))
            .collect();
        let has_more = resp.is_truncated().unwrap_or(false);
        Ok((keys, has_more))
    }

    pub async fn list_all_objects<F>(&self, bucket: &str, mut on_progress: F) -> Result<Vec<String>>
    where
        F: FnMut(usize),
    {
        let mut all_keys: Vec<String> = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut req = self
                .s3
                .list_objects_v2()
                .bucket(bucket)
                .max_keys(LIST_ALL_PAGE_SIZE);
            if let Some(token) = &continuation_token {
                req = req.continuation_token(token);
            }

            let resp = req
                .send()
                .await
                .with_context(|| format!("listing objects in bucket '{bucket}'"))?;

            all_keys.extend(
                resp.contents()
                    .iter()
                    .filter_map(|obj| obj.key().map(str::to_string)),
            );
            on_progress(all_keys.len());

            if !resp.is_truncated().unwrap_or(false) {
                break;
            }

            match resp.next_continuation_token() {
                Some(token) => continuation_token = Some(token.to_string()),
                None => break,
            }
        }

        Ok(all_keys)
    }

    pub async fn delete_objects<F>(
        &self,
        bucket: &str,
        keys: &[String],
        mut on_progress: F,
    ) -> Result<()>
    where
        F: FnMut(usize),
    {
        if keys.is_empty() {
            return Ok(());
        }

        let mut failed: Vec<(String, String)> = Vec::new();

        for chunk in keys.chunks(DELETE_BATCH_SIZE) {
            let objects: Vec<ObjectIdentifier> = chunk
                .iter()
                .map(|k| ObjectIdentifier::builder().key(k).build())
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("building ObjectIdentifier for DeleteObjects")?;
            let delete = Delete::builder()
                .set_objects(Some(objects))
                .quiet(true)
                .build()
                .context("building Delete for DeleteObjects")?;

            let resp = self
                .s3
                .delete_objects()
                .bucket(bucket)
                .delete(delete)
                .send()
                .await
                .with_context(|| format!("deleting objects in bucket '{bucket}'"))?;

            let errors = resp.errors();
            for e in errors {
                let key = e.key().unwrap_or("").to_string();
                let msg = e.message().unwrap_or("").to_string();
                failed.push((key, msg));
            }
            on_progress(chunk.len());
        }

        if !failed.is_empty() {
            let preview_len = failed.len().min(5);
            let preview = failed[..preview_len]
                .iter()
                .map(|(key, msg)| format!("  {key}: {msg}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(anyhow::anyhow!(
                "Failed to delete {} objects. First {} failures:\n{}",
                failed.len(),
                preview_len,
                preview
            ));
        }

        Ok(())
    }

    pub async fn delete_bucket(&self, bucket: &str) -> Result<()> {
        self.s3
            .delete_bucket()
            .bucket(bucket)
            .send()
            .await
            .with_context(|| format!("deleting bucket '{bucket}'"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_builds_r2_host_for_account_id() {
        assert_eq!(
            endpoint_url("abc123def456"),
            "https://abc123def456.r2.cloudflarestorage.com"
        );
    }

    #[test]
    fn endpoint_url_interpolates_different_account_ids() {
        assert_eq!(
            endpoint_url("acme"),
            "https://acme.r2.cloudflarestorage.com"
        );
    }
}
