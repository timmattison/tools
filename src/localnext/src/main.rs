use buildinfo::version_string;
use clap::Parser;
use std::path::Path;
use std::sync::Arc;

/// The size of the worker pool. A small fixed pool: a local preview server
/// answers one browser, and a thread per request would cost more than it saves.
const WORKER_THREADS: usize = 4;

/// The interface the server binds. A preview of an unbuilt site stays on the
/// loopback interface, off the LAN.
const BIND_ADDRESS: &str = "127.0.0.1";

#[derive(Parser)]
#[command(name = "localnext")]
#[command(version = version_string!())]
#[command(
    about = "Serve a statically exported Next.js build from its `out` directory",
    long_about = None
)]
struct Cli {
    /// Override the port derived from the project directory and the git branch.
    #[arg(short, long)]
    port: Option<u16>,
}

/// The directory whose `portplz` derivation supplies the default port.
///
/// It is the export root's PARENT when the root has one, and the root itself
/// otherwise.
///
/// `find_root` resolves to the same absolute `out` path whether the user ran from
/// the project directory or from inside `out`, so both invocations already agree
/// on a port. The parent matters for a different reason: outside a git repository
/// `portplz` hashes the directory BASENAME, and every static-export root on the
/// machine is named `out` — so deriving from the root itself would hand every
/// project the same port, which is the collision this port of the tool exists to
/// remove. The parent is the project directory, whose name is distinct. Inside a
/// git repository the choice makes no difference, because the repository name and
/// the current branch decide the hash.
fn port_basis(root: &Path) -> &Path {
    root.parent().unwrap_or(root)
}

/// Renders every startup failure through its `Display` form.
///
/// Returning a `Box<dyn Error>` straight out of `main` would print it through
/// `Debug` — `Error: "failed to bind …"`, message quoted — because that is what
/// `Termination` does. The split keeps the message readable.
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// Locates the export, picks a port, binds it, announces itself, and serves.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let cwd = std::env::current_dir()?;
    let root = localnext::find_root(&cwd)?;

    let port = match cli.port {
        Some(port) => port,
        None => {
            // Render a malformed `PORTPLZ_UID` through `Display` so the user gets
            // the helpful message rather than its `Debug` form.
            let user = portplz_core::UserSalt::current().map_err(|e| e.to_string())?;
            portplz_core::derive(port_basis(&root), false, &user)?
                .port
                .get()
        }
    };

    // `Server::http` errors as `Box<dyn Error + Send + Sync>`, which does not
    // coerce into this function's `Box<dyn Error>` through `?`; render it to a
    // String that names the address and the cause instead. Reporting this at all
    // is the point: the Go tool this ports discarded the equivalent error, so a
    // taken port exited 0 in silence and nothing was ever served.
    let address = format!("{BIND_ADDRESS}:{port}");
    let server = Arc::new(
        tiny_http::Server::http(&address).map_err(|e| format!("failed to bind {address}: {e}"))?,
    );

    // Read the address back from the server rather than reusing `port`: `--port 0`
    // lets the operating system assign one, and the banner has to name the port
    // that is actually listening. `server_addr` is an enum covering Unix sockets
    // too, and only an IP address was ever asked for above.
    let bound = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| format!("bound {address}, but it resolved to no IP address"))?;
    println!("{}", localnext::banner(version_string!(), &root, bound));

    // A dead acceptor (an accept() failure such as EMFILE) now surfaces here
    // as an `Err` instead of leaving the pool silently hung: `Pool::join`
    // returns it once every worker has exited, and `?` reports it the same
    // way every other startup failure in this function already is.
    localnext::serve(server, Arc::new(root), WORKER_THREADS).join()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::port_basis;
    use std::path::Path;

    #[test]
    fn the_basis_is_the_project_directory_holding_the_export_root() {
        assert_eq!(
            port_basis(Path::new("/projects/site/out")),
            Path::new("/projects/site")
        );
    }

    #[test]
    fn a_root_with_no_parent_is_its_own_basis() {
        assert_eq!(port_basis(Path::new("/")), Path::new("/"));
    }
}
