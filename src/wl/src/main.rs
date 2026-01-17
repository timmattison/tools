use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use std::net::SocketAddr;

/// Show which program is listening on a given port
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// The port number to check
    port: u16,

    /// Show detailed socket information
    #[clap(long, short)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    match listeners::get_all() {
        Ok(listeners) => {
            let mut found_matches = false;
            
            for listener in &listeners {
                // Parse the socket address to get the port
                let socket_str = format!("{}", listener.socket);
                if let Ok(socket_addr) = socket_str.parse::<SocketAddr>() {
                    if socket_addr.port() == args.port {
                        found_matches = true;
                        
                        if args.verbose {
                            println!("PID: {} Process: {} Socket: {} Full: {:?}", 
                                listener.process.pid, 
                                listener.process.name,
                                listener.socket,
                                listener
                            );
                        } else {
                            println!("PID: {} Process: {} Socket: {}", 
                                listener.process.pid, 
                                listener.process.name,
                                listener.socket
                            );
                        }
                    }
                } else {
                    // Handle cases where socket format might not parse as SocketAddr
                    // Look for port number in the socket string
                    if socket_str.contains(&format!(":{}", args.port)) {
                        found_matches = true;
                        
                        if args.verbose {
                            println!("PID: {} Process: {} Socket: {} Full: {:?}", 
                                listener.process.pid, 
                                listener.process.name,
                                listener.socket,
                                listener
                            );
                        } else {
                            println!("PID: {} Process: {} Socket: {}", 
                                listener.process.pid, 
                                listener.process.name,
                                listener.socket
                            );
                        }
                    }
                }
            }
            
            if !found_matches {
                println!("No processes listening on port {}", args.port);
            }
        },
        Err(e) => {
            eprintln!("Error getting listeners: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}