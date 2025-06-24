use anyhow::{Context, Result};
use clap::Parser;
use memmap2::Mmap;
use std::fs::File;
use std::io::Write;

#[derive(Parser)]
#[command(name = "hexfind")]
#[command(about = "Search for a hex string in a binary file and display a hex dump with surrounding bytes")]
#[command(long_about = None)]
struct Cli {
    #[arg(help = "Hex string to search for (with or without 0x prefix)")]
    hex_string: String,
    
    #[arg(help = "File to search in")]
    file: String,
    
    #[arg(short, long, default_value = "16", help = "Number of bytes to show before and after the match")]
    context: usize,
    
    #[arg(short, long, help = "Show all matches instead of just the first one")]
    all: bool,
}

struct Match {
    offset: usize,
    data: Vec<u8>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Remove 0x prefix if present
    let hex_string = cli.hex_string.trim_start_matches("0x");
    
    // Decode the hex string
    let pattern = hex::decode(hex_string)
        .context("Error decoding hex string")?;
    
    // Open and memory map the file
    let file = File::open(&cli.file)
        .context("Error opening file")?;
    let mmap = unsafe { Mmap::map(&file)? };
    
    // Search for the pattern
    let matches = find_pattern(&mmap, &pattern, cli.context, cli.all);
    
    if matches.is_empty() {
        println!("Pattern '{}' not found in file '{}'", hex_string, cli.file);
        return Ok(());
    }
    
    // Display the matches
    println!("Found {} match(es) for pattern '{}' in file '{}'\n", 
             matches.len(), hex_string, cli.file);
    
    for (i, m) in matches.iter().enumerate() {
        println!("Match #{}:", i + 1);
        println!("Offset: 0x{:08x} ({} decimal)", m.offset, m.offset);
        display_hex_dump(&m.data, m.offset, cli.context, pattern.len());
        println!();
    }
    
    Ok(())
}

fn find_pattern(mmap: &Mmap, pattern: &[u8], context_bytes: usize, all_matches: bool) -> Vec<Match> {
    let mut matches = Vec::new();
    let pattern_len = pattern.len();
    let file_size = mmap.len();
    
    for i in 0..=file_size.saturating_sub(pattern_len) {
        if &mmap[i..i + pattern_len] == pattern {
            let match_offset = i;
            
            // Calculate the start of the context
            let context_start = match_offset.saturating_sub(context_bytes);
            
            // Calculate the end of the context
            let context_end = (match_offset + pattern_len + context_bytes).min(file_size);
            
            // Create a copy of the data with context
            let match_data = mmap[context_start..context_end].to_vec();
            
            matches.push(Match {
                offset: match_offset,
                data: match_data,
            });
            
            if !all_matches {
                break;
            }
        }
    }
    
    matches
}

fn display_hex_dump(data: &[u8], file_offset: usize, context_bytes: usize, pattern_len: usize) {
    // Calculate the actual offset of the first byte in the data relative to the file
    let data_start_offset = file_offset.saturating_sub(context_bytes);
    
    // Calculate aligned offset for display purposes
    let display_start_offset = data_start_offset - (data_start_offset % 4);
    
    // Calculate how many bytes we skip at the beginning due to alignment
    let alignment_skip = data_start_offset - display_start_offset;
    
    // Calculate the position of the pattern in the data array
    let pattern_pos_in_data = file_offset - data_start_offset;
    
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    
    // Display the hex dump
    for i in 0.. {
        let display_offset = display_start_offset + i * 16;
        let data_offset = i * 16;
        
        // Check if we've exhausted the data
        if data_offset >= data.len() + alignment_skip {
            break;
        }
        
        // Print offset
        write!(handle, "{:08x}: ", display_offset).unwrap();
        
        // Print hex values
        for j in 0..16 {
            let display_pos = i * 16 + j;
            
            // Check if this position exists in our data array
            if display_pos >= alignment_skip && display_pos - alignment_skip < data.len() {
                let data_index = display_pos - alignment_skip;
                let byte = data[data_index];
                
                // Check if this byte is part of the pattern
                let in_pattern = data_index >= pattern_pos_in_data && data_index < pattern_pos_in_data + pattern_len;
            
                if in_pattern {
                    // Red and bold for pattern bytes
                    write!(handle, "\x1b[1;31m{:02x}\x1b[0m ", byte).unwrap();
                } else {
                    write!(handle, "{:02x} ", byte).unwrap();
                }
            } else {
                // This position doesn't exist in our data (due to alignment)
                write!(handle, "   ").unwrap();
            }
            
            // Add extra space in the middle
            if j == 7 {
                write!(handle, " ").unwrap();
            }
        }
        
        // Print ASCII representation
        write!(handle, " |").unwrap();
        for j in 0..16 {
            let display_pos = i * 16 + j;
            
            if display_pos >= alignment_skip && display_pos - alignment_skip < data.len() {
                let data_index = display_pos - alignment_skip;
                let byte = data[data_index];
                let in_pattern = data_index >= pattern_pos_in_data && data_index < pattern_pos_in_data + pattern_len;
                
                let c = if byte >= 32 && byte <= 126 {
                    byte as char
                } else {
                    '.'
                };
                
                if in_pattern {
                    write!(handle, "\x1b[1;31m{}\x1b[0m", c).unwrap();
                } else {
                    write!(handle, "{}", c).unwrap();
                }
            } else {
                write!(handle, " ").unwrap();
            }
        }
        
        writeln!(handle, "|").unwrap();
    }
    
    handle.flush().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_pattern_search() {
        let data = b"Hello, World! This is a test.";
        let pattern = b"World";
        let mmap_data = data.as_slice();
        
        // Simulate a memory map by using a slice
        let matches = find_pattern_in_slice(mmap_data, pattern, 4, false);
        
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, 7);
    }
    
    fn find_pattern_in_slice(data: &[u8], pattern: &[u8], context_bytes: usize, all_matches: bool) -> Vec<Match> {
        let mut matches = Vec::new();
        let pattern_len = pattern.len();
        let data_len = data.len();
        
        for i in 0..=data_len.saturating_sub(pattern_len) {
            if &data[i..i + pattern_len] == pattern {
                let match_offset = i;
                let context_start = match_offset.saturating_sub(context_bytes);
                let context_end = (match_offset + pattern_len + context_bytes).min(data_len);
                
                matches.push(Match {
                    offset: match_offset,
                    data: data[context_start..context_end].to_vec(),
                });
                
                if !all_matches {
                    break;
                }
            }
        }
        
        matches
    }
}