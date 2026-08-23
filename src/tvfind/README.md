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

Registered to a vendor that matches "tcl", but answered no probe (powered off?):

  192.168.1.193    28:7b:11:59:af:27   Hui Zhou Gaoshengda Technology
  192.168.1.217    d0:65:b3:a8:60:33   TCL King Electrical Appliances(Huizhou)Co.
```

## Usage

```
tvfind [--subnet CIDR] [--vendor NAME] [--no-arp]
```

| Flag | Meaning |
| --- | --- |
| `--subnet CIDR` | Subnet to scan. Defaults to the first IPv4 interface of this machine that can reach a neighbour. Blocks wider than a `/16` are refused. |
| `--vendor NAME` | Report only TVs whose manufacturer contains `NAME`, case-insensitively. Omit to list every TV found. Also narrows the powered-off report to that vendor. |
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

### Answering the port is not enough

Neither port belongs to televisions alone, so a device that answers must also
prove it is a television.

**Roku.** Every Roku streaming player — the Express, the Streaming Stick, the
Streambar — answers ECP with the same document a Roku TV does, under the same
vendor string. Roku settles it in the document itself, so `tvfind` reports a
Roku device only when it declares `<is-tv>true</is-tv>`.

**Google TV.** Port 8008 is Chromecast built-in, not Google TV. Every speaker
with Chromecast built-in answers there and publishes a `<manufacturer>`. No
field of the UPnP document, of `/setup/eureka_info`, or of
`/setup/eureka_info?options=detail` states whether the device has a display, so
the screen is proved another way: a DIAL server only lists an application the
device can actually run, and `tvfind` asks for two that need a display —
`Netflix` and `YouTube`. A device that offers neither is not reported.

That test proves a **screen**, not a television. A streaming box or a smart
display that offers a video app passes it, which is the known limit of what
port 8008 can tell you.

A device that fails either test still has power. `tvfind` records that the
device answered. Thus a device that these tests refuse can never come back as a
television that is powered off.

### Why not SSDP or mDNS?

Because discovery multicast is unreliable in practice. Access points routinely
filter multicast between the 2.4 GHz and 5 GHz radios, so an SSDP `M-SEARCH`
finds only the subset of TVs sharing a radio with the machine running the scan —
while every one of them answers correctly when addressed directly. `tvfind`
therefore probes hosts directly and treats discovery protocols as unavailable.

### Televisions that are powered off

A TV in standby refuses every TCP connection but still answers ARP, so it is
invisible to a port scan yet plainly present in the neighbour table. When nmap's
`nmap-mac-prefixes` database is installed, `tvfind` resolves the MAC prefix of
each neighbour that answered nothing and reports the ones registered to a
television maker.

**A host that answered any probe is never in this list**, whatever the probe
found there. A completed TCP handshake proves that the device has power, and a
device with power is not powered off. The rule matters most for the devices the
two tests above refuse: nmap registers every Roku address block to the bare name
`Roku`, so a Roku streaming player matches a television brand. The player
answers port 8060, therefore it is a host with power, and it stays out of this
report.

An OUI lookup names the company an address block belongs to, and nothing more.
It cannot say what the device is. The heading of this report therefore states
that evidence and no more, and two rules keep the list short:

- **With `--vendor`, the filter is used as given.** The filter is your own
  judgement about what you are looking for.
- **Without `--vendor`, only a television brand qualifies.** Otherwise every
  neighbour in the table appears, which says nothing about televisions. On a
  home network that is the difference between one line and a hundred.

Brands are matched as whole words inside the registered name, because the
registry routinely leads with a city or a parent company — `Huizhou TCL
Communication Electron`, `Sichuan Changhong Electric`. Whole words are also what
separate `LG Electronics` from `LG Innotek`, which supplies camera and radio
modules to other makers.

The list understands contract manufacturers. TCL sets, for example, register MAC
blocks to their Huizhou ODM rather than to TCL itself, so both `--vendor tcl`
and the unfiltered report match them. Without that alias a powered-off TCL TV
would be missed entirely.

Every host is reported once. macOS `arp` prints a separate line for each
interface that reaches a neighbour, so a machine on three networks lists every
one of them three times.

### The subnet that is scanned by default

Without `--subnet`, `tvfind` reads the interfaces of this machine and takes the
first IPv4 interface that can reach a neighbour. Loopback is skipped, and so is
any interface whose netmask holds one or two addresses. Such an interface is a
point-to-point link and not a LAN.

That second rule is what keeps a VPN out of the result. Tailscale and most
WireGuard clients present a `utun` address with the netmask 255.255.255.255,
which is a `/32`. A `/32` holds one address, this machine, so a scan of it
reaches nothing. The operating system owns the order the interfaces arrive in
and is free to list the VPN before the LAN, so the order alone cannot be
trusted to pick the right one.

The address is not examined, only the netmask. A LAN that uses public
addressing is still a LAN, and `tvfind` scans it.

When no interface qualifies, `tvfind` names the interfaces it skipped and asks
for `--subnet`.

## Installation

```bash
cargo install --git https://github.com/timmattison/tools tvfind
```

The powered-off report additionally requires `nmap` (for its OUI database) and
`arp`. Both are optional — without them the scan still runs, it just cannot
account for sets that are switched off.
