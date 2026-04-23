use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u64 = 300;
const WATCH_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "mjs", "cjs"];
const IGNORE_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    ".output",
    ".git",
    ".next",
    "target",
    "build",
    ".turbo",
    ".cache",
];

#[derive(Parser)]
#[command(name = "ng")]
#[command(version = version_string!())]
#[command(
    about = "Watch source files and re-run pnpm lint on changes",
    long_about = "Watch source files in the current directory (recursively) and re-run pnpm lint (or pnpm typecheck with -t) when they change."
)]
struct Cli {
    #[arg(short = 't', long, help = "Run pnpm typecheck instead of pnpm lint")]
    typecheck: bool,
}

/// Decide whether a changed path should trigger a re-run.
///
/// Filters out paths under common build/dependency directories, test files,
/// and any file whose extension is not a JS/TS source extension.
fn should_consider(path: &Path) -> bool {
    for component in path.components() {
        if let Some(name) = component.as_os_str().to_str() {
            if IGNORE_DIRS.contains(&name) {
                return false;
            }
        }
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.ends_with(".test.ts") || name.ends_with(".test.tsx") {
            return false;
        }
    }

    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| WATCH_EXTENSIONS.contains(&ext))
}

fn run_pnpm_script(script: &str) {
    print!("\x1B[2J\x1B[1;1H");
    println!(
        "{} {}",
        "ng".cyan(),
        format!("running pnpm {script}...").dimmed()
    );
    println!();

    let status = Command::new("pnpm").arg(script).status();

    println!();
    match status {
        Ok(s) if s.success() => {
            println!("{}", format!("pnpm {script} passed.").green());
        }
        Ok(s) => {
            let code = s.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
            println!("{}", format!("pnpm {script} failed (exit {code}).").red());
        }
        Err(e) => {
            eprintln!("{}", format!("Failed to start pnpm: {e}").red());
        }
    }
    println!("{}", "Watching for changes...".dimmed());
}

fn is_relevant_event(kind: &notify::EventKind) -> bool {
    matches!(
        kind,
        notify::EventKind::Create(_)
            | notify::EventKind::Modify(_)
            | notify::EventKind::Remove(_)
    )
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let script = if cli.typecheck { "typecheck" } else { "lint" };

    let cwd = std::env::current_dir().context("Failed to get current directory")?;

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default())
        .context("Failed to create file watcher")?;
    watcher
        .watch(&cwd, RecursiveMode::Recursive)
        .with_context(|| format!("Failed to watch {}", cwd.display()))?;

    run_pnpm_script(script);

    let debounce = Duration::from_millis(DEBOUNCE_MS);
    let mut pending: Option<Instant> = None;

    loop {
        let event = match pending {
            None => match rx.recv() {
                Ok(e) => e,
                Err(_) => break,
            },
            Some(t) => {
                let remaining = debounce.saturating_sub(t.elapsed());
                match rx.recv_timeout(remaining) {
                    Ok(e) => e,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        pending = None;
                        run_pnpm_script(script);
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        };

        if let Ok(event) = event {
            if is_relevant_event(&event.kind) && event.paths.iter().any(|p| should_consider(p)) {
                pending = Some(Instant::now());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn considers_ts_file() {
        assert!(should_consider(&PathBuf::from("src/foo.ts")));
    }

    #[test]
    fn considers_tsx_file() {
        assert!(should_consider(&PathBuf::from("src/foo.tsx")));
    }

    #[test]
    fn considers_mjs_file() {
        assert!(should_consider(&PathBuf::from("scripts/foo.mjs")));
    }

    #[test]
    fn ignores_unrelated_extension() {
        assert!(!should_consider(&PathBuf::from("README.md")));
        assert!(!should_consider(&PathBuf::from("foo.txt")));
    }

    #[test]
    fn ignores_files_without_extension() {
        assert!(!should_consider(&PathBuf::from("Makefile")));
    }

    #[test]
    fn ignores_node_modules() {
        assert!(!should_consider(&PathBuf::from(
            "node_modules/some-pkg/index.ts"
        )));
    }

    #[test]
    fn ignores_dist_dir() {
        assert!(!should_consider(&PathBuf::from("dist/bundle.js")));
    }

    #[test]
    fn ignores_dot_output_dir() {
        assert!(!should_consider(&PathBuf::from(".output/server/index.mjs")));
    }

    #[test]
    fn ignores_git_dir() {
        assert!(!should_consider(&PathBuf::from(".git/HEAD")));
    }

    #[test]
    fn ignores_test_files() {
        assert!(!should_consider(&PathBuf::from("src/foo.test.ts")));
        assert!(!should_consider(&PathBuf::from("src/foo.test.tsx")));
    }

    #[test]
    fn ignored_dir_anywhere_in_path() {
        assert!(!should_consider(&PathBuf::from(
            "packages/a/node_modules/b/c.ts"
        )));
    }

    #[test]
    fn considers_multibyte_filename() {
        assert!(should_consider(&PathBuf::from("src/日本語.ts")));
        assert!(should_consider(&PathBuf::from("src/🎉.tsx")));
        assert!(should_consider(&PathBuf::from("src/café.mjs")));
    }

    #[test]
    fn respects_ignore_dirs_beside_multibyte_components() {
        assert!(!should_consider(&PathBuf::from(
            "日本語/node_modules/foo.ts"
        )));
        assert!(should_consider(&PathBuf::from("日本語/src/foo.ts")));
    }

    #[test]
    fn relevant_event_kinds() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        assert!(is_relevant_event(&notify::EventKind::Create(
            CreateKind::File
        )));
        assert!(is_relevant_event(&notify::EventKind::Modify(
            ModifyKind::Any
        )));
        assert!(is_relevant_event(&notify::EventKind::Remove(
            RemoveKind::File
        )));
        assert!(!is_relevant_event(&notify::EventKind::Access(
            notify::event::AccessKind::Any
        )));
    }
}
