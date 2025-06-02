#[cfg(test)]
mod tests {
    use crate::{App, AppEvent, copy_file};
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    #[test]
    fn test_format_bytes() {
        assert_eq!(App::format_bytes(0), "0 B");
        assert_eq!(App::format_bytes(500), "500 B");
        assert_eq!(App::format_bytes(1000), "1.0 KB");
        assert_eq!(App::format_bytes(1500), "1.5 KB");
        assert_eq!(App::format_bytes(1000000), "1.0 MB");
        assert_eq!(App::format_bytes(1500000000), "1.5 GB");
    }

    #[test]
    fn test_app_creation() {
        let temp_dir = tempdir().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        fs::write(&source_path, "test content").unwrap();
        
        let dest_path = temp_dir.path().join("dest.txt");
        
        let app = App::new(source_path, dest_path.clone()).unwrap();
        assert_eq!(app.total_size, 12); // "test content" is 12 bytes
        assert_eq!(app.bytes_copied.load(Ordering::Relaxed), 0);
        assert!(!app.is_paused());
        assert!(!app.copy_complete);
        assert!(!app.should_quit);
        assert_eq!(app.destination_path, dest_path);
    }

    #[test]
    fn test_progress_calculation() {
        let temp_dir = tempdir().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        fs::write(&source_path, "test content").unwrap();
        
        let dest_path = temp_dir.path().join("dest.txt");
        let app = App::new(source_path, dest_path).unwrap();
        
        // Initially 0% progress
        assert_eq!(app.get_progress(), 0.0);
        
        // Simulate 6 bytes copied (50% progress)
        app.bytes_copied.store(6, Ordering::Relaxed);
        assert_eq!(app.get_progress(), 0.5);
        
        // Simulate complete copy (100% progress)
        app.bytes_copied.store(12, Ordering::Relaxed);
        assert_eq!(app.get_progress(), 1.0);
    }

    #[test]
    fn test_pause_toggle() {
        let temp_dir = tempdir().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        fs::write(&source_path, "test").unwrap();
        
        let dest_path = temp_dir.path().join("dest.txt");
        let app = App::new(source_path, dest_path).unwrap();
        
        assert!(!app.is_paused());
        app.toggle_pause();
        assert!(app.is_paused());
        app.toggle_pause();
        assert!(!app.is_paused());
    }

    #[tokio::test]
    async fn test_file_copy_simple() {
        let temp_dir = tempdir().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");
        
        let test_content = "Hello, world! This is a test for the Rust prcp implementation.";
        fs::write(&source_path, test_content).unwrap();
        
        let bytes_copied = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = mpsc::unbounded_channel();
        
        // Start the copy operation
        let copy_result = copy_file(source_path, dest_path.clone(), bytes_copied.clone(), paused, tx).await;
        
        // Verify the copy was successful
        assert!(copy_result.is_ok());
        
        // Verify the destination file exists and has correct content
        assert!(dest_path.exists());
        let copied_content = fs::read_to_string(&dest_path).unwrap();
        assert_eq!(copied_content, test_content);
        
        // Verify the bytes copied counter
        assert_eq!(bytes_copied.load(Ordering::Relaxed), test_content.len() as u64);
        
        // Verify we received completion event
        let mut received_complete = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, AppEvent::CopyComplete) {
                received_complete = true;
                break;
            }
        }
        assert!(received_complete);
    }

    #[test]
    fn test_throughput_calculation() {
        let temp_dir = tempdir().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        fs::write(&source_path, "test").unwrap();
        
        let dest_path = temp_dir.path().join("dest.txt");
        let app = App::new(source_path, dest_path).unwrap();
        
        // Simulate some bytes copied
        app.bytes_copied.store(1000, Ordering::Relaxed);
        
        // For very fast operations, throughput should be non-zero
        let throughput = app.get_throughput();
        assert!(throughput > 0);
    }
}