use buildinfo::version_string;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

const WORKER_THREADS: usize = 4; // small fixed pool, per spec

#[derive(Parser)]
#[command(name = "sirn")]
#[command(version = version_string!())]
#[command(about = "Serve It Right Now — a tiny zero-config HTTP file server", long_about = None)]
struct Cli {
    /// Files to serve, each at /<basename>. With no files, serves the current directory as a browsable tree.
    #[arg(value_name = "FILES")]
    files: Vec<PathBuf>,

    /// Override the portplz-derived port.
    #[arg(short, long)]
    port: Option<u16>,

    /// Bind address (default 127.0.0.1; use 0.0.0.0 to expose on the LAN).
    #[arg(short, long)]
    bind: Option<String>,

    /// Ignore the git branch when deriving the port (directory-name based).
    #[arg(long)]
    no_git: bool,

    /// Print the port-derivation source at startup.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Classify the positional arguments into a serving mode. Render any error via
    // Display (not the default Debug) so the user sees the helpful message rather
    // than its Debug form. A directory mixed with files is rejected here.
    let decision = sirn::decide_mode(&cli.files, |p| p.is_dir()).map_err(|e| e.to_string())?;

    // Build the serving mode up front so a files-mode collision (duplicate
    // basename) or a mixed dir+files error exits BEFORE any port derivation or
    // bind.
    let mode = match decision {
        sirn::ModeDecision::Files => {
            let routes = sirn::build_routes(&cli.files).map_err(|e| e.to_string())?;
            sirn::ServeMode::Files(Arc::new(routes))
        }
        sirn::ModeDecision::Directory(opt) => {
            let root = match opt {
                Some(p) => p,
                None => std::env::current_dir()?,
            };
            // Canonicalize so directory-mode path confinement has a stable root.
            let root = root.canonicalize()?;
            sirn::ServeMode::Directory(Arc::new(root))
        }
    };

    let (port, source_desc) = match cli.port {
        Some(p) => (p, None),
        None => {
            // Directory mode derives from the served root so `sirn <dir>` picks
            // the same port as `cd <dir> && sirn`; files mode derives from the
            // current directory.
            let derive_path = match sirn::port_basis(&mode) {
                Some(p) => p.to_path_buf(),
                None => std::env::current_dir()?,
            };
            let user = portplz_core::UserSalt::current();
            let d = portplz_core::derive(&derive_path, cli.no_git, &user)?;
            let src = if cli.verbose {
                Some(d.describe())
            } else {
                None
            };
            (d.port.get(), src)
        }
    };

    let bind = cli.bind.unwrap_or_else(|| "127.0.0.1".to_string());
    // `Server::http` errors as `Box<dyn Error + Send + Sync>`, which does not
    // coerce into our `Box<dyn Error>` via `?`; render it to a String instead.
    let server = Arc::new(
        tiny_http::Server::http(format!("{bind}:{port}"))
            .map_err(|e| format!("failed to bind {bind}:{port}: {e}"))?,
    );

    // Print the mode-appropriate banner and, in files mode only, spawn the
    // background availability monitor (it stats the served files).
    let monitor = match &mode {
        sirn::ServeMode::Files(routes) => {
            println!(
                "{}",
                sirn::files_banner(
                    version_string!(),
                    &bind,
                    port,
                    source_desc.as_deref(),
                    routes
                )
            );
            let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
            Some((
                shutdown_tx,
                sirn::spawn_monitor(Arc::clone(routes), shutdown_rx),
            ))
        }
        sirn::ServeMode::Directory(root) => {
            println!(
                "{}",
                sirn::directory_banner(
                    version_string!(),
                    &bind,
                    port,
                    source_desc.as_deref(),
                    root
                )
            );
            None
        }
    };

    let handles = sirn::serve(server, mode, WORKER_THREADS);
    for h in handles {
        let _ = h.join();
    }

    // Workers exit only when the server is unblocked; on a clean exit, stop the
    // monitor too (files mode) and join it so teardown is orderly.
    if let Some((shutdown_tx, monitor)) = monitor {
        drop(shutdown_tx);
        let _ = monitor.join();
    }
    Ok(())
}
