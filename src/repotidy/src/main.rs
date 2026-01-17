use buildinfo::version_string;
use repowalker::{find_git_repo, RepoWalker};
use std::path::Path;
use std::process::{exit, Command};

fn run_command_in_directory(dir: &Path, command: &[&str]) -> Result<(), std::io::Error> {
    let output = Command::new(command[0])
        .args(&command[1..])
        .current_dir(dir)
        .output()?;
    
    if !output.status.success() {
        eprintln!("Error running {} in {}: {}", 
                 command.join(" "), 
                 dir.display(), 
                 String::from_utf8_lossy(&output.stderr));
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "Command failed"));
    }
    
    println!("Ran {} in {}", command.join(" "), dir.display());
    Ok(())
}

fn main() {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("repotidy {}", version_string!());
        return;
    }

    let repo_root = match find_git_repo() {
        Some(root) => root,
        None => {
            eprintln!("Error: Could not find git repository");
            exit(1);
        }
    };

    let walker = RepoWalker::new(repo_root.clone())
        .respect_gitignore(false)  // Don't respect gitignore - find ALL Go projects
        .skip_node_modules(true)
        .skip_worktrees(true)
        .include_hidden(true);     // Include hidden directories
    
    for entry in walker.walk_with_ignore() {
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            let go_mod_path = entry.path().join("go.mod");
            if go_mod_path.exists() {
                if let Err(e) = run_command_in_directory(entry.path(), &["go", "mod", "tidy"]) {
                    eprintln!("Error running go mod tidy in {}: {}", entry.path().display(), e);
                }
            }
        }
    }
}