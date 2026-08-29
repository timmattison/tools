use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use ignore::{WalkBuilder, WalkState};
use rayon::prelude::*;
use sha2::{Digest, Sha256, Sha512};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Parser)]
#[command(name = "dirhash")]
#[command(version = version_string!())]
#[command(about = "Calculate a hash of all files in a directory")]
#[command(
    long_about = "Calculates SHA-512 hash for each file, then creates a final SHA-256 hash from sorted file hashes. Skips hidden files and files that .gitignore, .ignore, and the other standard ignore files name. Reports how many files it left out, and which flag pulls each group back in."
)]
struct Cli {
    #[arg(help = "Directory to hash")]
    directory: String,

    #[arg(
        long,
        help = "Don't respect any ignore file (.ignore, .gitignore, the global gitignore, .git/info/exclude)"
    )]
    no_ignore: bool,

    #[arg(
        long,
        help = "Don't respect the VCS ignore files (.gitignore, the global gitignore, .git/info/exclude); keep .ignore files"
    )]
    no_ignore_vcs: bool,

    #[arg(long, help = "Include hidden files and directories")]
    hidden: bool,
}

/// The filters that keep a file out of the hash.
///
/// A field is true when the filter is on, so [`Filters::NONE`] describes a walk
/// that reaches every file on disk. The two ignore fields move together under
/// `--no-ignore` and apart under `--no-ignore-vcs`, which is why they are two
/// fields rather than one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Filters {
    /// Skip a file whose path holds a component that starts with a dot.
    skip_hidden: bool,
    /// Obey `.ignore` files.
    obey_dot_ignore: bool,
    /// Obey `.gitignore`, the global gitignore, and `.git/info/exclude`.
    obey_vcs_ignore: bool,
}

impl Filters {
    /// A walk that leaves every file in.
    const NONE: Self = Self {
        skip_hidden: false,
        obey_dot_ignore: false,
        obey_vcs_ignore: false,
    };

    fn from_cli(cli: &Cli) -> Self {
        // `--no-ignore` covers the VCS ignore files as well. The help text and
        // the README always said so, and ripgrep, whose walker this tool uses,
        // reads the flag the same way. Only `--no-ignore-vcs` splits the two.
        let obey_dot_ignore = !cli.no_ignore;
        Self {
            skip_hidden: !cli.hidden,
            obey_dot_ignore,
            obey_vcs_ignore: obey_dot_ignore && !cli.no_ignore_vcs,
        }
    }

    /// The same filters with the hidden filter off.
    ///
    /// The count of the files that the hidden filter alone removed is the
    /// difference between this walk and the configured walk.
    fn without_hidden(self) -> Self {
        Self {
            skip_hidden: false,
            ..self
        }
    }

    fn obeys_an_ignore_file(self) -> bool {
        self.obey_dot_ignore || self.obey_vcs_ignore
    }

    /// Build a walker that obeys these filters.
    fn walker(self, directory: &str) -> WalkBuilder {
        let mut walker = WalkBuilder::new(directory);
        walker
            .hidden(self.skip_hidden)
            .ignore(self.obey_dot_ignore)
            .git_ignore(self.obey_vcs_ignore)
            .git_global(self.obey_vcs_ignore)
            .git_exclude(self.obey_vcs_ignore)
            // A parent directory of the target can hold an ignore file too.
            // `Filters::NONE` must reach every file, so the parent search goes
            // off with the rest of the ignore machinery.
            .parents(self.obeys_an_ignore_file());
        walker
    }

    /// Count the files this walk reaches, without reading any of them.
    fn count_files(self, directory: &str) -> usize {
        let count = AtomicUsize::new(0);
        self.walker(directory).build_parallel().run(|| {
            Box::new(|entry| {
                let is_file = entry
                    .as_ref()
                    .ok()
                    .and_then(ignore::DirEntry::file_type)
                    .is_some_and(|ft| ft.is_file());
                if is_file {
                    count.fetch_add(1, Ordering::Relaxed);
                }
                WalkState::Continue
            })
        });
        count.into_inner()
    }
}

/// How many files each group of filters left out of the hash.
struct Excluded {
    hidden: usize,
    ignored: usize,
}

impl Excluded {
    /// Measure the two groups against the files the hash actually covered.
    ///
    /// Three walks bound the two groups, and each walk turns one group of
    /// filters off:
    ///
    /// - `hashed` reaches the files the hash covers.
    /// - `without_hidden` reaches those plus the hidden files that no ignore
    ///   file names.
    /// - `unfiltered` reaches every file on disk.
    ///
    /// The two differences partition the excluded files exactly, so the parts
    /// always add up to the total. A walk whose filters match one already done
    /// is not repeated.
    fn measure(directory: &str, filters: Filters, hashed: usize) -> Self {
        let without_hidden_filters = filters.without_hidden();

        let without_hidden = if without_hidden_filters == filters {
            hashed
        } else {
            without_hidden_filters.count_files(directory)
        };

        let unfiltered = if without_hidden_filters == Filters::NONE {
            without_hidden
        } else {
            Filters::NONE.count_files(directory)
        };

        Self {
            hidden: without_hidden.saturating_sub(hashed),
            ignored: unfiltered.saturating_sub(without_hidden),
        }
    }

    fn total(&self) -> usize {
        self.hidden + self.ignored
    }

    /// The line for stderr, or `None` when the hash covered every file.
    ///
    /// The line names the flag that is sure to bring a group back in, and not
    /// every flag that changes the outcome. For the ignored group that flag is
    /// `--no-ignore`. The line never names `--no-ignore-vcs`, because a
    /// `.ignore` file keeps a file out of a `--no-ignore-vcs` walk. That flag
    /// brings the group back only when the VCS ignore files name every file in
    /// it, and the counts do not tell the two cases apart. The line names a
    /// flag only when that flag changes this run, so a run that already passed
    /// `--hidden` is never told to pass it again.
    fn note(&self) -> Option<String> {
        match (self.hidden, self.ignored) {
            (0, 0) => None,
            (hidden, 0) => Some(format!(
                "Note: {hidden} hidden file(s) excluded. Use --hidden to include them."
            )),
            (0, ignored) => Some(format!(
                "Note: {ignored} ignored file(s) excluded. Use --no-ignore to include them."
            )),
            (hidden, ignored) => Some(format!(
                "Note: {total} file(s) excluded: {hidden} hidden, {ignored} ignored. Use --hidden and --no-ignore to include them.",
                total = self.total()
            )),
        }
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    let mut hasher = Sha512::new();
    let mut buffer = [0; 8192];

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_string(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let filters = Filters::from_cli(&cli);

    // Collect the files the hash covers.
    let entries: Vec<_> = filters
        .walker(&cli.directory)
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .collect();

    let hashed = entries.len();

    let mut file_hashes: Vec<String> = entries
        .into_par_iter()
        .filter_map(|entry| {
            let path = entry.path();
            match hash_file(path) {
                Ok(hash) => Some(hash),
                Err(e) => {
                    eprintln!("Error hashing {}: {}", path.display(), e);
                    None
                }
            }
        })
        .collect();

    // Sort hashes
    file_hashes.sort();

    // Concatenate sorted hashes
    let concatenated: String = file_hashes.concat();

    // Calculate final hash
    let final_hash = hash_string(&concatenated);

    // Tell the reader what the walk left out, and which flag brings it back.
    if let Some(note) = Excluded::measure(&cli.directory, filters, hashed).note() {
        writeln!(io::stderr(), "{note}")?;
    }

    // Print only the final hash to stdout
    println!("{final_hash}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_string() {
        // Test with empty string
        let hash = hash_string("");
        assert_eq!(hash.len(), 64); // SHA-256 produces 64 hex characters

        // Test deterministic behavior
        let hash1 = hash_string("test");
        let hash2 = hash_string("test");
        assert_eq!(hash1, hash2);

        // Test different inputs produce different hashes
        let hash3 = hash_string("test2");
        assert_ne!(hash1, hash3);
    }

    fn cli(no_ignore: bool, no_ignore_vcs: bool, hidden: bool) -> Cli {
        Cli {
            directory: ".".to_string(),
            no_ignore,
            no_ignore_vcs,
            hidden,
        }
    }

    #[test]
    fn no_ignore_turns_off_every_ignore_file() {
        let filters = Filters::from_cli(&cli(true, false, false));
        assert!(!filters.obey_dot_ignore);
        assert!(!filters.obey_vcs_ignore);
        assert!(!filters.obeys_an_ignore_file());
    }

    #[test]
    fn no_ignore_vcs_keeps_dot_ignore_files() {
        let filters = Filters::from_cli(&cli(false, true, false));
        assert!(filters.obey_dot_ignore);
        assert!(!filters.obey_vcs_ignore);
    }

    #[test]
    fn hidden_only_moves_the_hidden_filter() {
        let filters = Filters::from_cli(&cli(false, false, true));
        assert!(!filters.skip_hidden);
        assert!(filters.obey_dot_ignore);
        assert!(filters.obey_vcs_ignore);
    }

    #[test]
    fn every_flag_together_reaches_every_file() {
        assert_eq!(Filters::from_cli(&cli(true, true, true)), Filters::NONE);
    }

    #[test]
    fn the_note_names_only_the_applicable_flags() {
        let note = Excluded {
            hidden: 3,
            ignored: 0,
        }
        .note()
        .expect("a note for three hidden files");
        assert!(note.contains("--hidden"));
        assert!(!note.contains("--no-ignore"));

        let note = Excluded {
            hidden: 0,
            ignored: 2,
        }
        .note()
        .expect("a note for two ignored files");
        assert!(note.contains("--no-ignore"));
        assert!(!note.contains("--hidden"));

        assert!(Excluded {
            hidden: 0,
            ignored: 0
        }
        .note()
        .is_none());
    }

    #[test]
    fn the_parts_of_the_note_add_up_to_the_total() {
        let excluded = Excluded {
            hidden: 3,
            ignored: 1,
        };
        assert_eq!(excluded.total(), 4);
        assert!(excluded.note().expect("a note").contains("4 file(s)"));
    }
}
