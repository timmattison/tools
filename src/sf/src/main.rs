use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use filewalker::{FileWalker, FilterType, format_bytes};

#[derive(Parser)]
#[command(name = "sf")]
#[command(version = version_string!())]
#[command(about = "Calculate total size of files in directories")]
#[command(long_about = "Calculate the total size of files in the specified directories. If no paths are provided, calculates size of files in the current directory.")]
struct Cli {
    #[arg(help = "Paths to calculate file sizes in")]
    paths: Vec<String>,
    
    #[arg(long, help = "Calculate size only for files with this suffix")]
    suffix: Option<String>,
    
    #[arg(long, help = "Calculate size only for files with this prefix")]
    prefix: Option<String>,
    
    #[arg(long, help = "Calculate size only for files containing this substring")]
    substring: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Check that at most one filter is specified
    let filter_count = [&cli.suffix, &cli.prefix, &cli.substring]
        .iter()
        .filter(|f| f.is_some())
        .count();
    
    if filter_count > 1 {
        eprintln!("Error: Only one of --suffix, --prefix, or --substring can be specified");
        std::process::exit(1);
    }
    
    // Create filter if specified
    let filter = if let Some(suffix) = cli.suffix {
        Some(FilterType::Suffix(suffix))
    } else if let Some(prefix) = cli.prefix {
        Some(FilterType::Prefix(prefix))
    } else if let Some(substring) = cli.substring {
        Some(FilterType::Substring(substring))
    } else {
        None
    };
    
    let walker = FileWalker::new(cli.paths).with_filter(filter);
    
    let mut total_size = 0u64;
    
    walker.walk(|entry| {
        if let Ok(metadata) = entry.metadata() {
            total_size += metadata.len();
        }
        Ok(())
    })?;
    
    println!("{}", format_bytes(total_size));
    
    Ok(())
}