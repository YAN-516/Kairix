# VisionFive 2 Network Bring-up Baseline

Date: 2026-07-23

## Purpose

Record the Debian network baseline for implementing and validating the Kairix
Ethernet driver on the StarFive VisionFive 2.

## Test Topology

The board was connected directly to the Windows host with an RJ45 Ethernet
cable.

| Endpoint | Interface | IPv4 address | MAC address |
| --- | --- | --- | --- |
| Windows host | Realtek PCIe GbE Family Controller (`以太网`) | `192.168.10.1/24` | `08-BF-B8-C1-51-9D` |
| VisionFive 2 | `end0` | `192.168.10.2/24` | `6c:cf:39:00:5b:80` |
| VisionFive 2 | `end1` | Not configured | `6c:cf:39:00:5b:81` |

There was no DHCP server on the direct link. Before static addresses were
configured, Windows selected `169.254.14.180/16` and Debian only had an IPv6
link-local address.

## Static Address Configuration

Windows was configured with:

```powershell
New-NetIPAddress `
  -InterfaceAlias "以太网" `
  -IPAddress 192.168.10.1 `
  -PrefixLength 24
```

Debian on the VisionFive 2 was configured with:

```bash
pkill dhclient
ip -4 addr flush dev end0
ip addr add 192.168.10.2/24 dev end0
ip link set end0 up
```

The direct link intentionally had no default gateway or DNS server.

## Link State

After connecting the cable, Debian reported:

```text
lo               UNKNOWN        00:00:00:00:00:00 <LOOPBACK,UP,LOWER_UP>
end0             UP             6c:cf:39:00:5b:80 <BROADCAST,MULTICAST,UP,LOWER_UP>
end1             DOWN           6c:cf:39:00:5b:81 <NO-CARRIER,BROADCAST,MULTICAST,UP>
sit0@NONE        DOWN           0.0.0.0 <NOARP>
```

`end0` was the connected and tested port. `end1` had no cable attached.

## Connectivity Results

Windows to VisionFive 2:

```text
Pinging 192.168.10.2 with 32 bytes of data:
Reply from 192.168.10.2: bytes=32 time<1ms TTL=64
Reply from 192.168.10.2: bytes=32 time<1ms TTL=64
Reply from 192.168.10.2: bytes=32 time<1ms TTL=64
Reply from 192.168.10.2: bytes=32 time<1ms TTL=64

Packets: Sent = 4, Received = 4, Lost = 0 (0% loss)
```

VisionFive 2 to Windows, after permitting inbound ICMPv4 in Windows Firewall:

```text
PING 192.168.10.1 (192.168.10.1) 56(84) bytes of data.
64 bytes from 192.168.10.1: icmp_seq=1 ttl=128 time=1.01 ms
64 bytes from 192.168.10.1: icmp_seq=2 ttl=128 time=0.929 ms
64 bytes from 192.168.10.1: icmp_seq=3 ttl=128 time=0.830 ms
64 bytes from 192.168.10.1: icmp_seq=4 ttl=128 time=0.885 ms

4 packets transmitted, 4 received, 0% packet loss, time 3004ms
rtt min/avg/max/mdev = 0.830/0.913/1.008/0.065 ms
```

This confirms working PHY link, Ethernet TX/RX, ARP, IPv4, and ICMP on the
Debian reference system.

## Linux Driver and Hardware Observations

`ethtool` was not installed in the Debian image:

```text
-bash: ethtool: command not found
```

It can be installed later with:

```bash
apt update
apt install ethtool
```

The network-relevant kernel log was:

```text
[    0.000000] Kernel command line: root=/dev/mmcblk1p4 rw console=tty0 console=ttyS0,115200 earlycon rootwait stmmaceth=chain_mode:1 selinux=0
[    1.559793] libphy: Fixed MDIO Bus: probed
[    1.590300] starfive-eth-plat 16030000.ethernet:     DWMAC4/5
[    1.912458] libphy: stmmac: probed
[    1.916237] YT8531 Gigabit Ethernet stmmac-0:00: attached PHY driver (mii_bus:phy_addr=stmmac-0:00, irq=POLL)
[    1.927196] YT8531 Gigabit Ethernet stmmac-0:01: attached PHY driver (mii_bus:phy_addr=stmmac-0:01, irq=POLL)
[    1.959014] starfive-eth-plat 16040000.ethernet:     DWMAC4/5
[    2.279755] libphy: stmmac: probed
[    2.283560] YT8531 Gigabit Ethernet stmmac-1:00: attached PHY driver (mii_bus:phy_addr=stmmac-1:00, irq=POLL)
[    2.294504] YT8531 Gigabit Ethernet stmmac-1:01: attached PHY driver (mii_bus:phy_addr=stmmac-1:01, irq=POLL)
[    8.816256] starfive-eth-plat 16030000.ethernet end0: renamed from eth0
[   16.887967] starfive-eth-plat 16030000.ethernet end0: PHY [stmmac-0:00] driver [YT8531 Gigabit Ethernet] (irq=POLL)
[   16.901404] starfive-eth-plat 16030000.ethernet end0: Register MEM_TYPE_PAGE_POOL RxQ-0
[   16.919960] dwmac4: Master AXI performs fixed burst length
[   16.925556] starfive-eth-plat 16030000.ethernet end0: No Safety Features support found
[   16.933613] starfive-eth-plat 16030000.ethernet end0: IEEE 1588-2008 Advanced Timestamp supported
[   16.969888] starfive-eth-plat 16030000.ethernet end0: configuring for phy/rgmii-id link mode
[   17.026088] starfive-eth-plat 16040000.ethernet end1: PHY [stmmac-1:00] driver [YT8531 Gigabit Ethernet] (irq=POLL)
[   17.051956] dwmac4: Master AXI performs fixed burst length
[   17.087715] starfive-eth-plat 16040000.ethernet end1: configuring for phy/rgmii-id link mode
[  707.535896] starfive-eth-plat 16030000.ethernet end0: Link is Up - 1Gbps/Full - flow control rx/tx
[  707.545149] IPv6: ADDRCONF(NETDEV_CHANGE): end0: link becomes ready
[ 1740.252678] starfive-eth-plat 16030000.ethernet end0: Link is Down
[ 1770.415799] starfive-eth-plat 16030000.ethernet end0: Link is Up - 1Gbps/Full - flow control rx/tx
```

Device-tree path for the tested port:

```text
/sys/firmware/devicetree/base/soc/ethernet@16030000
```

## Kairix Driver Requirements Derived from the Baseline

The first Kairix Ethernet implementation should target the tested port with
these properties:

| Property | Value |
| --- | --- |
| MAC controller | Synopsys DWMAC4/5 |
| StarFive wrapper | `starfive-eth-plat` |
| MMIO base | `0x16030000` |
| PHY | Motorcomm YT8531 Gigabit Ethernet |
| PHY address used by `end0` | MDIO address `0` |
| PHY interface | RGMII-ID |
| Initial receive mode | Polling is acceptable |
| Confirmed link mode | 1 Gbps, full duplex |
| Confirmed flow control | RX/TX |
| Initial Kairix IPv4 | `192.168.10.2/24` |
| Test host IPv4 | `192.168.10.1/24` |

The minimum Kairix validation sequence is:

1. Initialize the StarFive clock/reset glue and DWMAC at `0x16030000`.
2. Probe the YT8531 PHY at MDIO address `0` and reach link-up.
3. Initialize RX/TX DMA descriptor rings.
4. Register the device with the existing Kairix `NetDevice` layer.
5. Configure `192.168.10.2/24` and a direct `192.168.10.0/24` route.
6. Receive ARP and ICMP echo requests from the Windows host.
7. Reply successfully to `ping 192.168.10.2` with no packet loss.

This Debian baseline isolates later Kairix failures to the Kairix DWMAC/PHY/DMA
implementation or its network integration, rather than the cable, host NIC, or
VisionFive 2 Ethernet hardware.
