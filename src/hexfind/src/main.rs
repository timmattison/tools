use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use memmap2::Mmap;
use std::fs::File;
use std::io::Write;

#[derive(Parser)]
#[command(name = "hexfind")]
#[command(version = version_string!())]
#[command(
    about = "Search for a hex string in a binary file and display a hex dump with surrounding bytes"
)]
#[command(long_about = None)]
struct Cli {
    #[arg(help = "Hex string to search for (with or without 0x prefix)")]
    hex_string: String,

    #[arg(help = "File to search in")]
    file: String,

    #[arg(
        short,
        long,
        default_value = "16",
        help = "Number of bytes to show before and after the match"
    )]
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
    let pattern = hex::decode(hex_string).context("Error decoding hex string")?;
    if pattern.is_empty() {
        anyhow::bail!("The hex string is empty, so there is no pattern to search for");
    }

    // Open and memory map the file
    let file = File::open(&cli.file).context("Error opening file")?;
    // SAFETY: memory-mapping a file is unsafe because the mapped region can
    // change underneath us if the file is modified or truncated by another
    // process. hexfind is a read-only search tool over user-supplied files; the
    // caller accepts that risk by selecting the path, and we never write
    // through the mapping or hand it out across thread boundaries.
    let mmap = unsafe { Mmap::map(&file)? };

    // Search for the pattern
    let matches = find_pattern(&mmap, &pattern, cli.context, cli.all);

    if matches.is_empty() {
        println!("Pattern '{}' not found in file '{}'", hex_string, cli.file);
        return Ok(());
    }

    // Display the matches
    println!(
        "Found {} match(es) for pattern '{}' in file '{}'\n",
        matches.len(),
        hex_string,
        cli.file
    );

    for (i, m) in matches.iter().enumerate() {
        println!("Match #{}:", i + 1);
        println!("Offset: 0x{:08x} ({} decimal)", m.offset, m.offset);
        display_hex_dump(&m.data, m.offset, cli.context, pattern.len());
        println!();
    }

    Ok(())
}

/// Finds the places where `pattern` occurs in `haystack`.
///
/// Returns one `Match` for each occurrence, in order of offset, or only the
/// first when `all_matches` is false. Each `Match` holds its offset and a copy
/// of the matching bytes with up to `context_bytes` bytes on each side. The
/// context stops at the two ends of the haystack, also for the largest
/// `usize`. An empty pattern and a pattern longer than the haystack give no
/// match, so the result is empty.
fn find_pattern(
    haystack: &[u8],
    pattern: &[u8],
    context_bytes: usize,
    all_matches: bool,
) -> Vec<Match> {
    let mut matches = Vec::new();

    // `windows(0)` panics, so an empty pattern returns first.
    if pattern.is_empty() {
        return matches;
    }

    // `windows` gives nothing when the pattern is longer than the haystack.
    for (match_offset, window) in haystack.windows(pattern.len()).enumerate() {
        if window != pattern {
            continue;
        }

        let context_start = match_offset.saturating_sub(context_bytes);

        // The window ends inside the haystack, so only the addition of
        // `context_bytes`, which comes from the command line, can overflow.
        let context_end = (match_offset + pattern.len())
            .saturating_add(context_bytes)
            .min(haystack.len());

        matches.push(Match {
            offset: match_offset,
            data: haystack[context_start..context_end].to_vec(),
        });

        if !all_matches {
            break;
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
                let in_pattern = data_index >= pattern_pos_in_data
                    && data_index < pattern_pos_in_data + pattern_len;

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
                let in_pattern = data_index >= pattern_pos_in_data
                    && data_index < pattern_pos_in_data + pattern_len;

                let c = if (32..=126).contains(&byte) {
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

        let matches = find_pattern(data, pattern, 4, false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, 7);
    }

    /// Removes its file when dropped, also when the test panics.
    struct TempFile(std::path::PathBuf);

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_pattern_longer_than_the_file_finds_no_match() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "hexfind-longer-pattern-{}-{nanos}.bin",
            std::process::id()
        ));
        let guard = TempFile(path);
        std::fs::write(&guard.0, [0x01, 0x02, 0x03]).unwrap();
        let file = File::open(&guard.0).unwrap();
        // SAFETY: the test owns the file, made it with a unique name, and nothing else writes to it while it is mapped.
        let mmap = unsafe { Mmap::map(&file) }.unwrap();

        let matches = find_pattern(&mmap, &[0x01, 0x02, 0x03, 0x04], 4, true);

        assert!(
            matches.is_empty(),
            "a pattern longer than the file must find no match, got {} match(es)",
            matches.len()
        );
    }

    /// `--context` comes from the command line, so the largest `usize` is a
    /// value that a user can give. The context then stops at the two ends of
    /// the haystack.
    #[test]
    fn a_context_of_the_largest_usize_stops_at_the_ends_of_the_haystack() {
        let matches = find_pattern(b"abc", b"b", usize::MAX, false);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].offset, 1);
        assert_eq!(matches[0].data, b"abc");
    }

    /// An empty pattern says nothing about the haystack, so it matches
    /// nowhere.
    #[test]
    fn an_empty_pattern_finds_no_match() {
        let matches = find_pattern(b"abc", b"", 4, true);

        assert!(
            matches.is_empty(),
            "an empty pattern must find no match, got {} match(es)",
            matches.len()
        );
    }

    /// `all_matches` gives each occurrence in order of offset, overlaps
    /// included, and each context stops at the ends of the haystack.
    #[test]
    fn all_matches_gives_each_occurrence_with_its_context() {
        let matches = find_pattern(b"xaaay", b"aa", 1, true);

        let found: Vec<(usize, &[u8])> = matches
            .iter()
            .map(|m| (m.offset, m.data.as_slice()))
            .collect();
        assert_eq!(found, [(1, b"xaaa".as_slice()), (2, b"aaay".as_slice())]);
    }
}
