use anyhow::Result;
use blake3::Hasher as Blake3Hasher;
use digest::Digest;
use std::io::Read;
use tokio::sync::mpsc;

pub enum HasherType {
    Md5(md5::Context),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Sha512(sha2::Sha512),
    Blake3(Box<Blake3Hasher>),
}

impl HasherType {
    pub fn new(hash_type: &str) -> Result<Self> {
        match hash_type.to_lowercase().as_str() {
            "md5" => Ok(HasherType::Md5(md5::Context::new())),
            "sha1" => Ok(HasherType::Sha1(sha1::Sha1::new())),
            "sha256" => Ok(HasherType::Sha256(sha2::Sha256::new())),
            "sha512" => Ok(HasherType::Sha512(sha2::Sha512::new())),
            "blake3" => Ok(HasherType::Blake3(Box::new(Blake3Hasher::new()))),
            _ => anyhow::bail!("Unsupported hash type: {}", hash_type),
        }
    }
    
    pub fn update(&mut self, data: &[u8]) {
        match self {
            HasherType::Md5(hasher) => hasher.consume(data),
            HasherType::Sha1(hasher) => Digest::update(hasher, data),
            HasherType::Sha256(hasher) => Digest::update(hasher, data),
            HasherType::Sha512(hasher) => Digest::update(hasher, data),
            HasherType::Blake3(hasher) => {
                hasher.update(data);
            }
        }
    }
    
    pub fn finalize(self) -> String {
        match self {
            HasherType::Md5(hasher) => format!("{:x}", hasher.compute()),
            HasherType::Sha1(hasher) => hex::encode(Digest::finalize(hasher)),
            HasherType::Sha256(hasher) => hex::encode(Digest::finalize(hasher)),
            HasherType::Sha512(hasher) => hex::encode(Digest::finalize(hasher)),
            HasherType::Blake3(hasher) => hex::encode(hasher.finalize().as_bytes()),
        }
    }
}

pub fn is_valid_hash_type(hash_type: &str) -> bool {
    matches!(hash_type.to_lowercase().as_str(), "md5" | "sha1" | "sha256" | "sha512" | "blake3")
}

#[derive(Debug, Clone)]
pub struct HashProgress {
    pub bytes_processed: u64,
}

#[derive(Debug)]
pub enum HashMessage {
    Progress(HashProgress),
    Finished(String),
    Error(String),
}

pub async fn hash_file(
    file_path: &std::path::Path,
    hash_type: &str,
    progress_sender: mpsc::UnboundedSender<HashMessage>,
    mut pause_receiver: mpsc::UnboundedReceiver<bool>,
) -> Result<()> {
    let mut file = std::fs::File::open(file_path)?;
    let mut hasher = HasherType::new(hash_type)?;
    let mut buffer = vec![0u8; 16 * 1024 * 1024]; // 16MB buffer
    let mut total_processed = 0u64;
    let mut paused = false;
    
    loop {
        // Check for pause/unpause messages
        while let Ok(pause_state) = pause_receiver.try_recv() {
            paused = pause_state;
        }
        
        // If paused, wait a bit and continue checking for unpause
        if paused {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            continue;
        }
        
        match file.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(bytes_read) => {
                hasher.update(&buffer[..bytes_read]);
                total_processed += bytes_read as u64;
                
                if progress_sender
                    .send(HashMessage::Progress(HashProgress {
                        bytes_processed: total_processed,
                    }))
                    .is_err()
                {
                    break; // Receiver dropped
                }
            }
            Err(e) => {
                let _ = progress_sender.send(HashMessage::Error(e.to_string()));
                return Err(e.into());
            }
        }
    }
    
    let hash_result = hasher.finalize();
    let _ = progress_sender.send(HashMessage::Finished(hash_result));
    
    Ok(())
}