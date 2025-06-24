use anyhow::Result;
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone)]
pub enum FilterType {
    Suffix(String),
    Prefix(String),
    Substring(String),
}

pub struct FileWalker {
    paths: Vec<String>,
    filter: Option<FilterType>,
}

impl FileWalker {
    pub fn new(paths: Vec<String>) -> Self {
        let paths = if paths.is_empty() {
            vec![".".to_string()]
        } else {
            paths
        };
        
        Self {
            paths,
            filter: None,
        }
    }
    
    pub fn with_filter(mut self, filter: Option<FilterType>) -> Self {
        self.filter = filter;
        self
    }
    
    pub fn walk<F>(&self, mut handler: F) -> Result<()>
    where
        F: FnMut(&DirEntry) -> Result<()>,
    {
        // Deduplicate paths
        let mut unique_paths = std::collections::HashSet::new();
        for path in &self.paths {
            unique_paths.insert(path.as_str());
        }
        
        for path in unique_paths {
            for entry in WalkDir::new(path) {
                let entry = entry?;
                
                // Skip directories
                if entry.file_type().is_dir() {
                    continue;
                }
                
                // Apply filter if specified
                if let Some(filter) = &self.filter {
                    if !self.matches_filter(&entry, filter) {
                        continue;
                    }
                }
                
                handler(&entry)?;
            }
        }
        
        Ok(())
    }
    
    pub fn walk_with_path_separation<F>(&self, mut handler: F) -> Result<()>
    where
        F: FnMut(&str, &[DirEntry]) -> Result<()>,
    {
        // Deduplicate paths
        let mut unique_paths = std::collections::HashSet::new();
        for path in &self.paths {
            unique_paths.insert(path.as_str());
        }
        
        for path in unique_paths {
            let mut entries = Vec::new();
            
            for entry in WalkDir::new(path) {
                let entry = entry?;
                
                // Skip directories
                if entry.file_type().is_dir() {
                    continue;
                }
                
                // Apply filter if specified
                if let Some(filter) = &self.filter {
                    if !self.matches_filter(&entry, filter) {
                        continue;
                    }
                }
                
                entries.push(entry);
            }
            
            handler(path, &entries)?;
        }
        
        Ok(())
    }
    
    fn matches_filter(&self, entry: &DirEntry, filter: &FilterType) -> bool {
        let file_name = entry.file_name().to_string_lossy();
        
        match filter {
            FilterType::Suffix(suffix) => file_name.ends_with(suffix),
            FilterType::Prefix(prefix) => file_name.starts_with(prefix),
            FilterType::Substring(substring) => file_name.contains(substring),
        }
    }
}

// Utility functions for formatting output
pub fn format_count(count: u64) -> String {
    // Format with thousands separators
    let s = count.to_string();
    let mut result = String::new();
    let mut chars = s.chars().rev().enumerate();
    
    while let Some((i, c)) = chars.next() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    
    result.chars().rev().collect()
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    const THRESHOLD: f64 = 1024.0;
    
    if bytes == 0 {
        return "0 B".to_string();
    }
    
    let mut size = bytes as f64;
    let mut unit_index = 0;
    
    while size >= THRESHOLD && unit_index < UNITS.len() - 1 {
        size /= THRESHOLD;
        unit_index += 1;
    }
    
    if unit_index == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_format_count() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
    
    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
        assert_eq!(format_bytes(1073741824), "1.00 GB");
    }
}