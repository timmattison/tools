use buildinfo::version_string;
use clap::{Parser, Subcommand};
use op_cache::{OpCache, OpPath};

#[derive(Parser)]
#[command(
    name = "op-cache",
    about = "1Password credential caching with retry logic and worktree support",
    version = version_string!()
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Read a text secret (prints value to stdout)
    Read {
        /// 1Password path (e.g., "op://Private/Item/field")
        op_path: String,
        /// Optional environment variable that overrides cache and 1Password
        #[arg(long)]
        env_var: Option<String>,
    },
    /// Read a binary secret and write it to a file (prints output path to stdout)
    ReadBinary {
        /// 1Password path (e.g., "op://Private/Signature/file.png")
        op_path: String,
        /// Where to write the binary file
        output_path: String,
    },
    /// Remove a credential from the cache (next read re-fetches from 1Password)
    Invalidate {
        /// 1Password path to invalidate
        op_path: String,
    },
    /// Remove the entire cache file
    Clear,
    /// Show cached entries (values are redacted)
    Show,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> op_cache::Result<()> {
    match cli.command {
        Commands::Read { op_path, env_var } => {
            let cache = OpCache::new()?;
            let path = OpPath::new(&op_path)?;
            let value = cache.read(&path, env_var.as_deref())?;
            print!("{value}");
        }
        Commands::ReadBinary {
            op_path,
            output_path,
        } => {
            let cache = OpCache::new()?;
            let path = OpPath::new(&op_path)?;
            let resolved = cache.read_binary(&path, output_path.as_ref())?;
            print!("{}", resolved.display());
        }
        Commands::Invalidate { op_path } => {
            let path = OpPath::new(&op_path)?;
            let cache = OpCache::new()?;
            cache.invalidate(&path)?;
            eprintln!("Invalidated: {op_path}");
        }
        Commands::Clear => {
            let cache = OpCache::new()?;
            cache.clear()?;
            eprintln!("Cache cleared");
        }
        Commands::Show => {
            let cache = OpCache::new()?;
            let entries = cache.entries()?;
            if entries.is_empty() {
                eprintln!("Cache is empty");
            } else {
                eprintln!("Cached credentials ({}):", entries.len());
                for (path, fetched_at) in entries {
                    println!("  {path}  (fetched: {fetched_at})");
                }
            }
            eprintln!("\nCache file: {}", cache.cache_path().display());
        }
    }

    Ok(())
}
