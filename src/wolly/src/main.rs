use anyhow::{Context, Result, bail};
use clap::Parser;
use get_if_addrs::{get_if_addrs, IfAddr};
use std::net::{UdpSocket, Ipv4Addr};
use std::thread;
use std::time::Duration;

/// Wake-on-LAN tool to wake computers remotely via magic packets
#[derive(Parser, Debug)]
#[command(name = "wolly")]
#[command(about = "Wake-on-LAN tool to wake computers remotely", long_about = None)]
struct Cli {
    /// MAC address of the target computer (formats: AA:BB:CC:DD:EE:FF, AA-BB-CC-DD-EE-FF, or AABBCCDDEEFF)
    #[arg(help = "MAC address of the target computer (not required with --list-interfaces)")]
    mac_address: Option<String>,

    /// UDP port to send the magic packet to (default: 9)
    #[arg(short, long, default_value = "9")]
    port: u16,

    /// Broadcast address to send the packet to (default: 255.255.255.255)
    #[arg(short, long, default_value = "255.255.255.255")]
    broadcast: String,

    /// Network interface to use for sending the packet (e.g., en0, eth0)
    #[arg(short, long)]
    interface: Option<String>,

    /// Number of packets to send (default: 3 for reliability)
    #[arg(short = 'c', long, default_value = "3")]
    count: u8,

    /// Delay between packets in milliseconds (default: 100ms)
    #[arg(short = 'd', long, default_value = "100")]
    delay: u64,

    /// Try sending on both port 7 and port 9 for maximum compatibility
    #[arg(long)]
    try_both_ports: bool,

    /// List available network interfaces and exit
    #[arg(long)]
    list_interfaces: bool,

    /// Print verbose output showing the packet details
    #[arg(short, long)]
    verbose: bool,
}

/// Represents a MAC address as 6 bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacAddress([u8; 6]);

impl MacAddress {
    /// Parses a MAC address from various string formats:
    /// - Colon-separated: AA:BB:CC:DD:EE:FF
    /// - Dash-separated: AA-BB-CC-DD-EE-FF
    /// - No separators: AABBCCDDEEFF
    ///
    /// Returns an error if the format is invalid or contains non-hex characters
    fn parse(s: &str) -> Result<Self> {
        // Remove common separators
        let cleaned = s.replace([':', '-'], "");

        if cleaned.len() != 12 {
            bail!("MAC address must be 12 hex characters (got {} characters)", cleaned.len());
        }

        let mut bytes = [0u8; 6];
        for (i, byte) in bytes.iter_mut().enumerate() {
            let hex_str = &cleaned[i * 2..i * 2 + 2];
            *byte = u8::from_str_radix(hex_str, 16)
                .with_context(|| format!("Invalid hex in MAC address: {}", hex_str))?;
        }

        Ok(MacAddress(bytes))
    }

    /// Returns the MAC address as a byte array
    fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }

    /// Formats the MAC address as a colon-separated string (e.g., AA:BB:CC:DD:EE:FF)
    fn format(&self) -> String {
        self.0
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(":")
    }
}

/// Represents a network interface with its name, IPv4 address, and netmask
#[derive(Debug, Clone)]
struct NetworkInterface {
    name: String,
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
}

impl NetworkInterface {
    /// Calculates the subnet broadcast address for this interface
    ///
    /// Takes the IP address and netmask and calculates the broadcast address
    /// by setting all host bits to 1
    fn broadcast_address(&self) -> Ipv4Addr {
        let ip_octets = self.ip.octets();
        let mask_octets = self.netmask.octets();

        let broadcast_octets = [
            ip_octets[0] | !mask_octets[0],
            ip_octets[1] | !mask_octets[1],
            ip_octets[2] | !mask_octets[2],
            ip_octets[3] | !mask_octets[3],
        ];

        Ipv4Addr::from(broadcast_octets)
    }
}

/// Gets all available network interfaces with IPv4 addresses
///
/// Filters out loopback interfaces and returns only interfaces with IPv4 addresses
fn get_network_interfaces() -> Result<Vec<NetworkInterface>> {
    let interfaces = get_if_addrs()
        .context("Failed to get network interfaces")?;

    let ipv4_interfaces: Vec<NetworkInterface> = interfaces
        .into_iter()
        .filter_map(|iface| {
            // Skip loopback interfaces first
            if iface.is_loopback() {
                return None;
            }

            if let IfAddr::V4(v4) = iface.addr {
                return Some(NetworkInterface {
                    name: iface.name,
                    ip: v4.ip,
                    netmask: v4.netmask,
                });
            }
            None
        })
        .collect();

    Ok(ipv4_interfaces)
}

/// Selects the best network interface to use for sending the magic packet
///
/// If an interface name is specified, finds that interface.
/// Otherwise, returns the first non-loopback IPv4 interface found.
fn select_interface(interface_name: Option<&str>) -> Result<NetworkInterface> {
    let interfaces = get_network_interfaces()?;

    if interfaces.is_empty() {
        bail!("No suitable network interfaces found");
    }

    match interface_name {
        Some(name) => {
            interfaces
                .into_iter()
                .find(|iface| iface.name == name)
                .ok_or_else(|| anyhow::anyhow!("Interface '{}' not found", name))
        }
        None => {
            // Return the first interface
            Ok(interfaces.into_iter().next().unwrap())
        }
    }
}

/// Lists all available network interfaces with their IPv4 addresses and broadcast addresses
fn list_interfaces() -> Result<()> {
    let interfaces = get_network_interfaces()?;

    if interfaces.is_empty() {
        println!("No network interfaces with IPv4 addresses found");
        return Ok(());
    }

    println!("Available network interfaces:");
    for iface in interfaces {
        println!("  {} - {} (broadcast: {})", iface.name, iface.ip, iface.broadcast_address());
    }

    Ok(())
}

/// Creates a Wake-on-LAN magic packet for the given MAC address.
///
/// A magic packet consists of:
/// - 6 bytes of 0xFF
/// - 16 repetitions of the target MAC address (6 bytes each)
/// Total: 102 bytes
///
/// Returns the magic packet as a Vec<u8>
fn create_magic_packet(mac: &MacAddress) -> Vec<u8> {
    let mut packet = Vec::with_capacity(102);

    // Add 6 bytes of 0xFF
    packet.extend_from_slice(&[0xFF; 6]);

    // Add MAC address 16 times
    for _ in 0..16 {
        packet.extend_from_slice(mac.as_bytes());
    }

    packet
}

/// Sends Wake-on-LAN magic packets to the specified broadcast address and port(s).
///
/// Creates a UDP socket bound to the specified interface IP with broadcast enabled,
/// and sends the magic packet multiple times with delays between sends for reliability.
///
/// Returns the total number of bytes sent across all packets.
fn send_magic_packets(
    mac: &MacAddress,
    broadcast_addr: &str,
    ports: &[u16],
    interface_ip: Ipv4Addr,
    count: u8,
    delay_ms: u64,
    verbose: bool,
) -> Result<usize> {
    let packet = create_magic_packet(mac);
    let delay = Duration::from_millis(delay_ms);

    // Bind to the specific interface IP
    let bind_addr = format!("{}:0", interface_ip);
    let socket = UdpSocket::bind(&bind_addr)
        .with_context(|| format!("Failed to bind UDP socket to {}", bind_addr))?;

    socket.set_broadcast(true)
        .context("Failed to set broadcast option on socket")?;

    let mut total_bytes_sent = 0;
    let total_sends = count as usize * ports.len();

    for (send_num, port) in (1..=count).flat_map(|i| ports.iter().map(move |p| (i, *p))) {
        let destination = format!("{}:{}", broadcast_addr, port);

        let bytes_sent = socket.send_to(&packet, &destination)
            .with_context(|| format!("Failed to send magic packet to {}", destination))?;

        total_bytes_sent += bytes_sent;

        if verbose {
            println!("  Sent packet {} of {} to {}:{} ({} bytes)",
                     send_num, total_sends / ports.len(), broadcast_addr, port, bytes_sent);
        }

        // Don't delay after the last send
        if send_num < count || port != *ports.last().unwrap() {
            thread::sleep(delay);
        }
    }

    Ok(total_bytes_sent)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --list-interfaces
    if cli.list_interfaces {
        return list_interfaces();
    }

    // Validate that MAC address is provided
    let mac_address = cli.mac_address.as_ref()
        .ok_or_else(|| anyhow::anyhow!("MAC address is required (use --help for usage)"))?;

    // Parse MAC address
    let mac = MacAddress::parse(mac_address)
        .context("Failed to parse MAC address")?;

    // Select network interface
    let interface = select_interface(cli.interface.as_deref())
        .context("Failed to select network interface")?;

    // Determine broadcast address
    let subnet_broadcast = interface.broadcast_address();
    let using_subnet_broadcast = cli.broadcast == "255.255.255.255";
    let broadcast_addr = if using_subnet_broadcast {
        subnet_broadcast.to_string()
    } else {
        cli.broadcast.clone()
    };

    // Determine ports to use
    let ports: Vec<u16> = if cli.try_both_ports {
        vec![7, 9]
    } else {
        vec![cli.port]
    };

    // Display configuration
    if cli.verbose {
        println!("Network interface: {} ({})", interface.name, interface.ip);
        println!("Subnet broadcast: {}", subnet_broadcast);
        println!("Target MAC address: {}", mac.format());
        println!("Broadcast address: {}", broadcast_addr);
        if using_subnet_broadcast {
            println!("  (auto-detected from interface)");
        }
        if cli.try_both_ports {
            println!("Ports: 7 and 9 (trying both)");
        } else {
            println!("Port: {}", cli.port);
        }
        println!("Packet count: {}", cli.count);
        println!("Delay between packets: {}ms", cli.delay);
        println!("Magic packet size: 102 bytes");
        println!();
        println!("Sending packets:");
    } else {
        println!("Using interface: {} ({})", interface.name, interface.ip);
        if using_subnet_broadcast {
            println!("Broadcasting to subnet: {}", broadcast_addr);
        }
    }

    // Send the magic packets
    let bytes_sent = send_magic_packets(
        &mac,
        &broadcast_addr,
        &ports,
        interface.ip,
        cli.count,
        cli.delay,
        cli.verbose,
    )?;

    if cli.verbose {
        println!();
        println!("Successfully sent {} total bytes", bytes_sent);
    } else {
        println!("Sent {} magic packet(s) to {}", cli.count * ports.len() as u8, mac.format());
    }

    // Show troubleshooting hints
    if !cli.verbose {
        println!();
        println!("If the device doesn't wake up, try:");
        println!("  1. Verify WoL is enabled in BIOS and network adapter settings");
        let alt_broadcast = if using_subnet_broadcast {
            "255.255.255.255".to_string()
        } else {
            subnet_broadcast.to_string()
        };
        println!("  2. Try a different broadcast: --broadcast {}", alt_broadcast);
        if !cli.try_both_ports {
            println!("  3. Try both ports: --try-both-ports");
        }
        println!("  4. Run with --verbose to see detailed packet information");
        println!("  5. Ensure the device is connected via Ethernet (not WiFi)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mac_parse_colon_separated() {
        let mac = MacAddress::parse("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(mac.0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_mac_parse_dash_separated() {
        let mac = MacAddress::parse("AA-BB-CC-DD-EE-FF").unwrap();
        assert_eq!(mac.0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_mac_parse_no_separator() {
        let mac = MacAddress::parse("AABBCCDDEEFF").unwrap();
        assert_eq!(mac.0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_mac_parse_lowercase() {
        let mac = MacAddress::parse("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(mac.0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_mac_parse_mixed_case() {
        let mac = MacAddress::parse("Aa-Bb-Cc-Dd-Ee-Ff").unwrap();
        assert_eq!(mac.0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn test_mac_parse_invalid_length() {
        assert!(MacAddress::parse("AA:BB:CC:DD:EE").is_err());
        assert!(MacAddress::parse("AA:BB:CC:DD:EE:FF:00").is_err());
    }

    #[test]
    fn test_mac_parse_invalid_hex() {
        assert!(MacAddress::parse("GG:BB:CC:DD:EE:FF").is_err());
        assert!(MacAddress::parse("AA:ZZ:CC:DD:EE:FF").is_err());
    }

    #[test]
    fn test_mac_format() {
        let mac = MacAddress([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        assert_eq!(mac.format(), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn test_mac_format_lowercase_input() {
        let mac = MacAddress::parse("aa:bb:cc:dd:ee:ff").unwrap();
        assert_eq!(mac.format(), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn test_magic_packet_structure() {
        let mac = MacAddress([0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
        let packet = create_magic_packet(&mac);

        // Check packet size
        assert_eq!(packet.len(), 102);

        // Check first 6 bytes are 0xFF
        for i in 0..6 {
            assert_eq!(packet[i], 0xFF, "Byte {} should be 0xFF", i);
        }

        // Check MAC address is repeated 16 times
        for repetition in 0..16 {
            let start = 6 + (repetition * 6);
            let mac_slice = &packet[start..start + 6];
            assert_eq!(mac_slice, &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
                      "MAC address repetition {} is incorrect", repetition);
        }
    }

    #[test]
    fn test_magic_packet_consistency() {
        let mac = MacAddress::parse("AA:BB:CC:DD:EE:FF").unwrap();
        let packet1 = create_magic_packet(&mac);
        let packet2 = create_magic_packet(&mac);
        assert_eq!(packet1, packet2);
    }

    #[test]
    fn test_magic_packet_different_macs() {
        let mac1 = MacAddress::parse("AA:BB:CC:DD:EE:FF").unwrap();
        let mac2 = MacAddress::parse("11:22:33:44:55:66").unwrap();
        let packet1 = create_magic_packet(&mac1);
        let packet2 = create_magic_packet(&mac2);
        assert_ne!(packet1, packet2);
    }

    #[test]
    fn test_mac_equality() {
        let mac1 = MacAddress::parse("AA:BB:CC:DD:EE:FF").unwrap();
        let mac2 = MacAddress::parse("aa-bb-cc-dd-ee-ff").unwrap();
        let mac3 = MacAddress::parse("AABBCCDDEEFF").unwrap();
        assert_eq!(mac1, mac2);
        assert_eq!(mac2, mac3);
        assert_eq!(mac1, mac3);
    }

    #[test]
    fn test_get_network_interfaces() {
        // This test just checks that we can call the function without panicking
        // The actual interfaces available depend on the system
        let result = get_network_interfaces();
        assert!(result.is_ok());
    }

    #[test]
    fn test_network_interface_fields() {
        let iface = NetworkInterface {
            name: "eth0".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 100),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
        };
        assert_eq!(iface.name, "eth0");
        assert_eq!(iface.ip, Ipv4Addr::new(192, 168, 1, 100));
        assert_eq!(iface.netmask, Ipv4Addr::new(255, 255, 255, 0));
    }

    #[test]
    fn test_broadcast_address_calculation() {
        // Test /24 network (255.255.255.0)
        let iface1 = NetworkInterface {
            name: "eth0".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 100),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
        };
        assert_eq!(iface1.broadcast_address(), Ipv4Addr::new(192, 168, 1, 255));

        // Test /16 network (255.255.0.0)
        let iface2 = NetworkInterface {
            name: "eth0".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 100),
            netmask: Ipv4Addr::new(255, 255, 0, 0),
        };
        assert_eq!(iface2.broadcast_address(), Ipv4Addr::new(192, 168, 255, 255));

        // Test /8 network (255.0.0.0)
        let iface3 = NetworkInterface {
            name: "eth0".to_string(),
            ip: Ipv4Addr::new(10, 0, 1, 100),
            netmask: Ipv4Addr::new(255, 0, 0, 0),
        };
        assert_eq!(iface3.broadcast_address(), Ipv4Addr::new(10, 255, 255, 255));

        // Test /28 network (255.255.255.240)
        let iface4 = NetworkInterface {
            name: "eth0".to_string(),
            ip: Ipv4Addr::new(192, 168, 1, 20),
            netmask: Ipv4Addr::new(255, 255, 255, 240),
        };
        assert_eq!(iface4.broadcast_address(), Ipv4Addr::new(192, 168, 1, 31));
    }
}
