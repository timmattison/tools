use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
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

    #[arg(short, long, help = "Exclude paths matching these patterns (in addition to defaults: target, node_modules)")]
    exclude: Vec<String>,

    #[arg(short = 'P', long, help = "Prefix to add to member paths (e.g., 'src/')")]
    prefix: Option<String>,

    #[arg(long, help = "Include packages in git worktrees (excluded by default)")]
    include_worktrees: bool,

    #[arg(long, help = "Disable default exclusions (target, node_modules)")]
    no_default_excludes: bool,
}

fn get_default_excludes() -> Vec<String> {
    vec!["target".to_string(), "node_modules".to_string()]
}

fn is_in_worktree(path: &Path) -> bool {
    // Walk up the directory tree looking for .git
    let mut current = path;
    loop {
        let git_path = current.join(".git");
        if git_path.exists() {
            // If .git is a file (not directory), it's likely a worktree
            if git_path.is_file() {
                if let Ok(content) = fs::read_to_string(&git_path) {
                    return content.trim().starts_with("gitdir:");
                }
            }
            return false; // Regular git repo
        }
        
        match current.parent() {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

fn find_cargo_tomls(root: &Path, excludes: &[String], include_worktrees: bool) -> Result<Vec<PathBuf>> {
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
            // Skip if in a worktree (unless explicitly included)
            if !include_worktrees && is_in_worktree(path) {
                continue;
            }
            
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

fn get_package_name(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let doc = content.parse::<DocumentMut>()?;
    
    if let Some(package) = doc.get("package") {
        if let Some(name) = package.get("name") {
            if let Some(name_str) = name.as_str() {
                return Ok(name_str.to_string());
            }
        }
    }
    
    Err(anyhow::anyhow!("No package name found in {}", path.display()))
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
    
    // Merge default excludes with user excludes
    let all_excludes = if args.no_default_excludes {
        args.exclude
    } else {
        let mut excludes = get_default_excludes();
        excludes.extend(args.exclude);
        excludes
    };
    
    // Find all Cargo.toml files
    let cargo_files = find_cargo_tomls(&args.path, &all_excludes, args.include_worktrees)?;
    
    if cargo_files.is_empty() {
        println!("No Cargo.toml files found in subdirectories");
        return Ok(());
    }
    
    println!("Found {} Cargo.toml files:", cargo_files.len());
    
    // Check for duplicate package names
    let mut package_names: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for cargo_file in &cargo_files {
        match get_package_name(cargo_file) {
            Ok(name) => {
                package_names.entry(name).or_insert_with(Vec::new).push(cargo_file.clone());
            }
            Err(e) => {
                eprintln!("Warning: Failed to get package name from {}: {}", cargo_file.display(), e);
            }
        }
    }
    
    // Report duplicate package names
    let mut has_duplicates = false;
    for (name, paths) in &package_names {
        if paths.len() > 1 {
            has_duplicates = true;
            eprintln!("\nError: Package name '{}' appears in multiple locations:", name);
            for path in paths {
                eprintln!("  - {}", path.display());
            }
        }
    }
    
    if has_duplicates {
        eprintln!("\nWorkspace creation failed: duplicate package names found.");
        eprintln!("Each package in a workspace must have a unique name in its Cargo.toml [package] section.");
        eprintln!("\nSuggestions:");
        eprintln!("1. Rename one of the duplicate packages");
        eprintln!("2. Exclude some paths using --exclude");
        eprintln!("3. Use a more specific --path to avoid including duplicates");
        return Err(anyhow::anyhow!("Duplicate package names found"));
    }
    
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