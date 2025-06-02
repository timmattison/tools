use anyhow::Result;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc;
use crate::hash::{hash_file, HashMessage};

#[derive(Debug, Clone)]
pub enum AppState {
    Preparing,
    Hashing,
    Paused,
    Finished,
    Error(String),
}

pub struct App {
    pub state: AppState,
    pub hash_type: String,
    pub input_file: PathBuf,
    pub file_size: u64,
    pub bytes_processed: u64,
    pub start_time: Option<Instant>,
    pub hash_result: Option<String>,
    pub error_message: Option<String>,
    
    // Progress tracking
    progress_receiver: mpsc::UnboundedReceiver<HashMessage>,
    pause_sender: mpsc::UnboundedSender<bool>,
    paused: bool,
    
    // Task handle
    hash_task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl App {
    pub async fn new(hash_type: &str, input_file: &PathBuf) -> Result<Self> {
        let file_metadata = std::fs::metadata(input_file)?;
        let file_size = file_metadata.len();
        
        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        let (pause_sender, pause_receiver) = mpsc::unbounded_channel();
        
        // Start the hash calculation task
        let file_path = input_file.clone();
        let hash_type_owned = hash_type.to_string();
        let hash_task = tokio::spawn(async move {
            hash_file(&file_path, &hash_type_owned, progress_sender, pause_receiver).await
        });
        
        Ok(App {
            state: AppState::Preparing,
            hash_type: hash_type.to_string(),
            input_file: input_file.clone(),
            file_size,
            bytes_processed: 0,
            start_time: None,
            hash_result: None,
            error_message: None,
            progress_receiver,
            pause_sender,
            paused: false,
            hash_task: Some(hash_task),
        })
    }
    
    pub fn state(&self) -> &AppState {
        &self.state
    }
    
    pub fn get_hash_result(&self) -> Option<&String> {
        self.hash_result.as_ref()
    }
    
    pub fn toggle_pause(&mut self) {
        if matches!(self.state, AppState::Hashing | AppState::Paused) {
            self.paused = !self.paused;
            let _ = self.pause_sender.send(self.paused);
            
            self.state = if self.paused {
                AppState::Paused
            } else {
                AppState::Hashing
            };
        }
    }
    
    pub async fn tick(&mut self) {
        // Check for messages from the hash task
        while let Ok(message) = self.progress_receiver.try_recv() {
            match message {
                HashMessage::Progress(progress) => {
                    if self.start_time.is_none() {
                        self.start_time = Some(Instant::now());
                        self.state = AppState::Hashing;
                    }
                    self.bytes_processed = progress.bytes_processed;
                }
                HashMessage::Finished(hash_value) => {
                    self.hash_result = Some(hash_value);
                    self.state = AppState::Finished;
                }
                HashMessage::Error(error_msg) => {
                    self.error_message = Some(error_msg.clone());
                    self.state = AppState::Error(error_msg);
                }
            }
        }
        
        // Check if the task has completed
        if let Some(task) = &mut self.hash_task {
            if task.is_finished() {
                match task.await {
                    Ok(Ok(())) => {
                        // Task completed successfully, result should be in hash_result
                        if self.hash_result.is_none() {
                            self.state = AppState::Finished;
                        }
                    }
                    Ok(Err(e)) => {
                        self.error_message = Some(e.to_string());
                        self.state = AppState::Error(e.to_string());
                    }
                    Err(e) => {
                        self.error_message = Some(e.to_string());
                        self.state = AppState::Error(e.to_string());
                    }
                }
                self.hash_task = None;
            }
        }
    }
    
    pub fn progress_percentage(&self) -> f64 {
        if self.file_size == 0 {
            0.0
        } else {
            (self.bytes_processed as f64 / self.file_size as f64) * 100.0
        }
    }
    
    pub fn throughput(&self) -> Option<f64> {
        if let Some(start_time) = self.start_time {
            let elapsed = start_time.elapsed();
            if elapsed.as_secs_f64() > 0.0 {
                Some(self.bytes_processed as f64 / elapsed.as_secs_f64())
            } else {
                None
            }
        } else {
            None
        }
    }
    
    pub fn format_throughput(&self) -> String {
        match self.throughput() {
            Some(bytes_per_sec) => {
                if bytes_per_sec >= 1_000_000.0 {
                    format!("{:15.0} MB/s", bytes_per_sec / 1_000_000.0)
                } else if bytes_per_sec >= 1_000.0 {
                    format!("{:15.0} KB/s", bytes_per_sec / 1_000.0)
                } else {
                    format!("{:15.0} B/s", bytes_per_sec)
                }
            }
            None => "Unknown".to_string(),
        }
    }
    
    pub fn format_bytes(&self, bytes: u64) -> String {
        use num_format::{Locale, ToFormattedString};
        bytes.to_formatted_string(&Locale::en)
    }
}