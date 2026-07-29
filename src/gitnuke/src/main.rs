use buildinfo::version_string;
use clap::Parser;

/// Remove a git worktree and delete its branch.
#[derive(Parser)]
#[command(name = "gitnuke")]
#[command(about = "Remove a git worktree and force-delete its branch")]
#[command(version = version_string!())]
struct Cli {
    /// Worktree to nuke: its path, its directory name, or its branch name.
    #[arg(required = true)]
    targets: Vec<String>,
}

fn main() {
    let cli = Cli::parse();
    eprintln!(
        "gitnuke: not implemented ({} target(s) requested)",
        cli.targets.len()
    );
    std::process::exit(1);
}
