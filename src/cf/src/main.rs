use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use filewalker::{FileWalker, FilterType, format_count};

#[derive(Parser)]
#[command(name = "cf")]
#[command(version = version_string!())]
#[command(about = "Count files in directories")]
#[command(long_about = "Count files in the specified directories. If no paths are provided, counts files in the current directory.")]
struct Cli {
    #[arg(help = "Paths to count files in")]
    paths: Vec<String>,
    
    #[arg(long, help = "Count only files with this suffix")]
    suffix: Option<String>,
    
    #[arg(long, help = "Count only files with this prefix")]
    prefix: Option<String>,
    
    #[arg(long, help = "Count only files containing this substring")]
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
    
    let mut total_count = 0u64;
    
    walker.walk(|_entry| {
        total_count += 1;
        Ok(())
    })?;
    
    println!("{}", format_count(total_count));
    
    Ok(())
}