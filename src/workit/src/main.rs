use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(author, version, about = "Find all Cargo.toml files and add them to a workspace", long_about = None)]
struct Args {
    #[arg(short, long, help = "Path to search for Cargo.toml files", default_value = ".")]
    path: PathBuf,

    #[arg(short, long, help = "Output file for workspace Cargo.toml", default_value = "Cargo.toml")]
    output: PathBuf,

    #[arg(short, long, help = "Dry run - show what would be done without making changes")]
    dry_run: bool,

    #[arg(short, long, help = "Exclude paths matching these patterns")]
    exclude: Vec<String>,

    #[arg(short = 'P', long, help = "Prefix to add to member paths (e.g., 'src/')")]
    prefix: Option<String>,
}

fn find_cargo_tomls(root: &Path, excludes: &[String]) -> Result<Vec<PathBuf>> {
    let mut cargo_files = Vec::new();
    
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let path = e.path();
            
            // Skip hidden directories
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with('.') && path != root {
                    return false;
                }
            }
            
            // Skip excluded paths
            for exclude in excludes {
                if path.to_string_lossy().contains(exclude) {
                    return false;
                }
            }
            
            true
        })
    {
        let entry = entry?;
        let path = entry.path();
        
        if path.file_name() == Some("Cargo.toml".as_ref()) && path != root.join("Cargo.toml") {
            // Check if it's a package (not already a workspace)
            if is_package_toml(path)? {
                cargo_files.push(path.to_path_buf());
            }
        }
    }
    
    cargo_files.sort();
    Ok(cargo_files)
}

fn is_package_toml(path: &Path) -> Result<bool> {
    let content = fs::read_to_string(path)?;
    let doc = content.parse::<DocumentMut>()?;
    
    // It's a package if it has [package] but not [workspace]
    Ok(doc.get("package").is_some() && doc.get("workspace").is_none())
}

fn create_or_update_workspace(output: &Path, members: &[String], dry_run: bool) -> Result<()> {
    let mut doc = if output.exists() {
        let content = fs::read_to_string(output)
            .with_context(|| format!("Failed to read existing {}", output.display()))?;
        content.parse::<DocumentMut>()
            .with_context(|| format!("Failed to parse existing {}", output.display()))?
    } else {
        DocumentMut::new()
    };
    
    // Ensure [workspace] section exists
    if doc.get("workspace").is_none() {
        doc["workspace"] = Item::Table(Table::new());
    }
    
    // Get or create members array
    let workspace = doc["workspace"].as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Failed to create workspace table"))?;
    
    if workspace.get("members").is_none() {
        workspace["members"] = Item::Value(toml_edit::Value::Array(Array::new()));
    }
    
    let members_array = workspace["members"].as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("Failed to create members array"))?;
    
    // Clear existing members and add new ones
    members_array.clear();
    for member in members {
        members_array.push(member);
    }
    
    // Add common workspace configuration if not present
    if workspace.get("resolver").is_none() {
        workspace["resolver"] = toml_edit::value("2");
    }
    
    if dry_run {
        println!("Would write to {}:", output.display());
        println!("{}", doc);
    } else {
        fs::write(output, doc.to_string())
            .with_context(|| format!("Failed to write {}", output.display()))?;
        println!("Created/updated workspace at {}", output.display());
    }
    
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Find all Cargo.toml files
    let cargo_files = find_cargo_tomls(&args.path, &args.exclude)?;
    
    if cargo_files.is_empty() {
        println!("No Cargo.toml files found in subdirectories");
        return Ok(());
    }
    
    println!("Found {} Cargo.toml files:", cargo_files.len());
    
    // Convert paths to relative paths for workspace members
    let mut members = Vec::new();
    let root = args.path.canonicalize()?;
    
    for cargo_file in &cargo_files {
        let parent = cargo_file.parent()
            .ok_or_else(|| anyhow::anyhow!("Cargo.toml has no parent directory"))?;
        
        let relative = if parent.starts_with(&root) {
            parent.strip_prefix(&root)?
        } else {
            parent
        };
        
        let mut member = relative.to_string_lossy().replace('\\', "/");
        
        // Add prefix if specified
        if let Some(prefix) = &args.prefix {
            member = format!("{}{}", prefix, member);
        }
        
        println!("  - {}", member);
        members.push(member);
    }
    
    // Create or update workspace Cargo.toml
    create_or_update_workspace(&args.output, &members, args.dry_run)?;
    
    if !args.dry_run {
        println!("\nWorkspace created with {} members", members.len());
        println!("You can now use commands like:");
        println!("  cargo build --workspace");
        println!("  cargo test --workspace");
        println!("  cargo build -p <package-name>");
    }
    
    Ok(())
}