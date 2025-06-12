use std::net::TcpListener;

use anyhow::{Context, Result};
use clap::Parser;
use rand::seq::SliceRandom;

/// Find a free TCP port on localhost
#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Allow searching privileged ports (1-1023)
    #[clap(long)]
    allow_privileged: bool,

    /// Start of port range to search (default: 1024 or 1 if --allow-privileged)
    #[clap(long)]
    start_port: Option<u16>,

    /// End of port range to search (default: 65535)
    #[clap(long)]
    end_port: Option<u16>,

    /// Find the first available port instead of a random one
    #[clap(long)]
    first_available: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let (start_port, end_port) = determine_port_range(&args)?;

    match find_free_port(start_port, end_port, args.first_available)? {
        Some(port) => {
            println!("{}", port);
            Ok(())
        }
        None => {
            anyhow::bail!("No free ports found in range {}-{}", start_port, end_port);
        }
    }
}

fn determine_port_range(args: &Args) -> Result<(u16, u16)> {
    let start_port = match args.start_port {
        Some(port) => port,
        None => {
            if args.allow_privileged {
                1
            } else {
                1024
            }
        }
    };

    let end_port = args.end_port.unwrap_or(65535);

    if start_port > end_port {
        anyhow::bail!("Start port ({}) cannot be greater than end port ({})", start_port, end_port);
    }

    if !args.allow_privileged && start_port < 1024 {
        anyhow::bail!("Start port ({}) is privileged. Use --allow-privileged to search privileged ports", start_port);
    }

    Ok((start_port, end_port))
}

fn find_free_port(start_port: u16, end_port: u16, first_available: bool) -> Result<Option<u16>> {
    if first_available {
        // Sequential search (original behavior)
        for port in start_port..=end_port {
            if is_port_free(port)? {
                return Ok(Some(port));
            }
        }
    } else {
        // Random search (new default behavior)
        let mut ports: Vec<u16> = (start_port..=end_port).collect();
        let mut rng = rand::rng();
        ports.shuffle(&mut rng);
        
        for port in ports {
            if is_port_free(port)? {
                return Ok(Some(port));
            }
        }
    }
    Ok(None)
}

fn is_port_free(port: u16) -> Result<bool> {
    match TcpListener::bind(format!("127.0.0.1:{}", port)) {
        Ok(_) => Ok(true),
        Err(ref e) if e.kind() == std::io::ErrorKind::AddrInUse => Ok(false),
        Err(e) => Err(e).with_context(|| format!("Failed to test port {}", port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_port_range_defaults() {
        let args = Args {
            allow_privileged: false,
            start_port: None,
            end_port: None,
            first_available: false,
        };
        let (start, end) = determine_port_range(&args).unwrap();
        assert_eq!(start, 1024);
        assert_eq!(end, 65535);
    }

    #[test]
    fn test_determine_port_range_privileged() {
        let args = Args {
            allow_privileged: true,
            start_port: None,
            end_port: None,
            first_available: false,
        };
        let (start, end) = determine_port_range(&args).unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, 65535);
    }

    #[test]
    fn test_determine_port_range_custom() {
        let args = Args {
            allow_privileged: false,
            start_port: Some(8000),
            end_port: Some(9000),
            first_available: false,
        };
        let (start, end) = determine_port_range(&args).unwrap();
        assert_eq!(start, 8000);
        assert_eq!(end, 9000);
    }

    #[test]
    fn test_determine_port_range_invalid() {
        let args = Args {
            allow_privileged: false,
            start_port: Some(9000),
            end_port: Some(8000),
            first_available: false,
        };
        assert!(determine_port_range(&args).is_err());
    }

    #[test]
    fn test_determine_port_range_privileged_without_flag() {
        let args = Args {
            allow_privileged: false,
            start_port: Some(80),
            end_port: Some(1000),
            first_available: false,
        };
        assert!(determine_port_range(&args).is_err());
    }

    #[test]
    fn test_find_free_port_first_available() {
        // This test tries to find the first available port in a reasonable range
        let result = find_free_port(49152, 65535, true).unwrap();
        assert!(result.is_some());
        
        // Test that the found port is actually free
        if let Some(port) = result {
            assert!(is_port_free(port).unwrap());
        }
    }

    #[test]
    fn test_find_free_port_random() {
        // This test tries to find a random free port in a reasonable range
        let result = find_free_port(49152, 65535, false).unwrap();
        assert!(result.is_some());
        
        // Test that the found port is actually free
        if let Some(port) = result {
            assert!(is_port_free(port).unwrap());
        }
    }

    #[test]
    fn test_first_available_vs_random_behavior() {
        // Test that first_available gives consistent results
        let first_result1 = find_free_port(49152, 49160, true).unwrap();
        let first_result2 = find_free_port(49152, 49160, true).unwrap();
        
        // Both should find the same port (the first available one)
        assert_eq!(first_result1, first_result2);
        
        // Random results might be different (though this is not guaranteed)
        // We just verify they both find valid ports
        let random_result1 = find_free_port(49152, 49160, false).unwrap();
        let random_result2 = find_free_port(49152, 49160, false).unwrap();
        
        assert!(random_result1.is_some());
        assert!(random_result2.is_some());
        
        if let (Some(port1), Some(port2)) = (random_result1, random_result2) {
            assert!(is_port_free(port1).unwrap());
            assert!(is_port_free(port2).unwrap());
        }
    }
}