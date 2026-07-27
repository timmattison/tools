# tvfind

Find smart TVs on the local network and identify them.

```
$ tvfind --vendor tcl
Scanning 192.168.0.0/23 (510 hosts) for TVs...
┌───────────────┬─────────────────┬────────┬──────────────┬───────────┬─────────────┐
│ IP            ┆ Name            ┆ Vendor ┆ Model        ┆ Platform  ┆ Software    │
╞═══════════════╪═════════════════╪════════╪══════════════╪═══════════╪═════════════╡
│ 192.168.0.119 ┆ Office - top    ┆ TCL    ┆ 43S435       ┆ Roku TV   ┆ 15.0.4      │
│ 192.168.0.248 ┆ Bedroom         ┆ TCL    ┆ 43S405       ┆ Roku TV   ┆ 15.2.4      │
│ 192.168.1.88  ┆ Office - bottom ┆ TCL    ┆ 43S435       ┆ Roku TV   ┆ 15.1.4      │
│ 192.168.1.165 ┆ Living Room     ┆ TCL    ┆ Smart TV Pro ┆ Google TV ┆ 3.72.446070 │
└───────────────┴─────────────────┴────────┴──────────────┴───────────┴─────────────┘

Probably a TV but answering nothing (powered off?):

  192.168.1.193    28:7b:11:59:af:27   Hui Zhou Gaoshengda Technology
  192.168.1.217    d0:65:b3:a8:60:33   TCL King Electrical Appliances(Huizhou)Co.
```

## Usage

```
tvfind [--subnet CIDR] [--vendor NAME] [--no-arp]
```

| Flag | Meaning |
| --- | --- |
| `--subnet CIDR` | Subnet to scan. Defaults to the subnet of this machine's first non-loopback IPv4 interface. Blocks wider than a `/16` are refused. |
| `--vendor NAME` | Report only TVs whose manufacturer contains `NAME`, case-insensitively. Omit to list every TV found. |
| `--no-arp` | Skip the ARP cross-check that reports televisions which are powered off. |

## How it works

Two firmware families cover essentially every consumer smart TV that answers on
a LAN, and each publishes an authoritative vendor string:

| Platform | Port | Endpoint | Identifying field |
| --- | --- | --- | --- |
| Roku TV | 8060 | `/query/device-info` | `<vendor-name>` |
| Google TV | 8008 | `/ssdp/device-desc.xml` | `<manufacturer>` |

Google TVs are additionally asked for `/setup/eureka_info`, which carries the
name the owner assigned. That request is best-effort: if it fails, the UPnP
document alone still identifies the set.

### Why not SSDP or mDNS?

Because discovery multicast is unreliable in practice. Access points routinely
filter multicast between the 2.4 GHz and 5 GHz radios, so an SSDP `M-SEARCH`
finds only the subset of TVs sharing a radio with the machine running the scan —
while every one of them answers correctly when addressed directly. `tvfind`
therefore probes hosts directly and treats discovery protocols as unavailable.

### Televisions that are powered off

A TV in standby refuses every TCP connection but still answers ARP, so it is
invisible to a port scan yet plainly present in the neighbour table. When nmap's
`nmap-mac-prefixes` database is installed, `tvfind` resolves each unexplained
neighbour's MAC prefix and reports the ones registered to the vendor being
looked for.

This lookup understands contract manufacturers. TCL sets, for example, register
MAC blocks to their Huizhou ODM rather than to TCL itself, so a `--vendor tcl`
filter matches those too. Without that alias a powered-off TCL TV would be
missed entirely.

## Installation

```bash
cargo install --git https://github.com/timmattison/tools tvfind
```

The powered-off report additionally requires `nmap` (for its OUI database) and
`arp`. Both are optional — without them the scan still runs, it just cannot
account for sets that are switched off.
