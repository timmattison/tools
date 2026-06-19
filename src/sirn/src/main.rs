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
    /// Files to serve. Each is served at /<basename>.
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

    if cli.files.is_empty() {
        // Directory mode arrives in Phase 4.
        return Err(
            "directory mode is not yet implemented; specify one or more files to serve".into(),
        );
    }

    // Collision check BEFORE any binding so a bad invocation exits immediately.
    // Render the error via Display (not the default Debug) so the user sees the
    // helpful "duplicate basename '<name>': ..." message rather than its Debug form.
    let routes = sirn::build_routes(&cli.files).map_err(|e| e.to_string())?;

    let (port, source_desc) = match cli.port {
        Some(p) => (p, None),
        None => {
            let d = portplz_core::derive(&std::env::current_dir()?, cli.no_git)?;
            let src = if cli.verbose {
                Some(d.source.describe(d.port))
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

    println!(
        "{}",
        sirn::files_banner(
            version_string!(),
            &bind,
            port,
            source_desc.as_deref(),
            &routes
        )
    );

    // Share one Arc of the route map across the worker pool and the monitor.
    let routes = Arc::new(routes);

    // Background availability monitor (files mode), mirroring the Java tool.
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let monitor = sirn::spawn_monitor(Arc::clone(&routes), shutdown_rx);

    let handles = sirn::serve(server, Arc::clone(&routes), WORKER_THREADS);
    for h in handles {
        let _ = h.join();
    }

    // Workers exit only when the server is unblocked; on a clean exit, stop the
    // monitor too and join it so teardown is orderly.
    drop(shutdown_tx);
    let _ = monitor.join();
    Ok(())
}
