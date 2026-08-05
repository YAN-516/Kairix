# VisionFive 2 Kairix 烧录与网络测试指南


## 1. 当前硬件和分区约定

- 开发板：StarFive VisionFive 2（JH7110）
- 网口：`end0` 对应的 GMAC0，物理地址 `0x16030000`
- PHY：Motorcomm YT8531，MDIO 地址 0
- Kairix 静态地址：`192.168.10.2/24`
- Windows 有线网卡地址：`192.168.10.1/24`
- TF 卡第 3 分区：100 MiB FAT 启动分区
- TF 卡第 4 分区：Kairix ext4 根分区
- U-Boot 中 TF 卡设备：`mmc 1`

## 2. 构建最新 Kairix 镜像

在 Docker 的 `/workspace` 中执行：

```bash
# 构建内核
make -C os ARCH=riscv64 BOARD=visionfive2 \
  LOG=INFO VF2_ROOT_PART=4 build

# 封装为 U-Boot uImage
# 参数含义：
# -A riscv：目标 CPU 架构是 RISC-V。
# -O linux：操作系统类型字段写为 Linux。这里只是 uImage 元数据，Kairix 并不会因此变成 Linux。
# -T kernel：镜像类型为内核。
# -C none：内核数据没有压缩，U-Boot 不需要先解压。
# -a 0x80200000：加载地址。U-Boot 将内核数据放到物理地址 0x80200000。
# -e 0x80200000：入口地址。加载完成后从 0x80200000 开始执行。
# -n "..."：uImage 的显示名称。
# -d .../os.bin：作为 uImage 数据部分的输入文件。
# 最后一个参数：生成的 uImage 文件路径。
mkimage -A riscv -O linux -T kernel -C none \
  -a 0x80200000 -e 0x80200000 \
  -n "Kairix VisionFive 2" \
  -d os/target/riscv64gc-unknown-none-elf/release/os.bin \
  os/kairix-uImage-vf2-network-root4

# -l 表示列出 uImage 头信息
mkimage -l os/kairix-uImage-vf2-network-root4

# 计算整个uImage的哈希值
sha256sum os/kairix-uImage-vf2-network-root4
```

## 3. 将镜像复制到 WSL

在 WSL Ubuntu 中执行。容器名变化时先通过 `docker ps` 查询：

```bash
# 查看正在运行的 Docker 容器
docker ps --format 'table {{.ID}}\t{{.Names}}'
# 把容器内的 uImage 复制到 WSL 用户主目录
# thirsty_booth 为当前 Docker 容器的名称
docker cp thirsty_booth:/workspace/os/kairix-uImage-vf2-network-root4 ~/
# 计算WSL中镜像的哈希值，确保镜像为最新镜像
sha256sum ~/kairix-uImage-vf2-network-root4
```


## 4. 将读卡器接入 WSL

关闭开发板并拔出 TF 卡，将 TF 卡插入 USB 读卡器。在管理员 PowerShell 中执行：

```powershell
usbipd list
```

找到 USB 大容量存储设备。
BUSID 可能在每次重新插入后变化，以下以 `3-3` 为例：

```powershell
usbipd bind --force --busid 3-3
usbipd attach --wsl --busid 3-3
usbipd list
```

如果设备已经是 `Shared`，可跳过 `bind`。最终状态应为 `Attached`。

## 5. 确认 TF 卡设备名

在 WSL 中执行：

```bash
lsblk -o NAME,SIZE,RM,RO,TYPE,FSTYPE,LABEL,MOUNTPOINTS,MODEL
```

本次 TF 卡识别为 `/dev/sdg`：

```text
sdg      59.5G  removable
├─sdg1      2M
├─sdg2      4M
├─sdg3    100M  vfat
└─sdg4    3.8G  ext4
```

设备名变化时，后续所有 `/dev/sdg*` 命令都必须相应修改。

检查并卸载已有挂载：

```bash
findmnt /dev/sdg3
findmnt /dev/sdg4
sudo umount /dev/sdg3 2>/dev/null || true
sudo umount /dev/sdg4 2>/dev/null || true
```

## 6. 准备第 4 分区

最终的 `sdcard-rv-pub.img.gz` 解压后是 14 GiB 的完整 ext4 rootfs：

```bash
DISK=/dev/sdg
ROOT_PART=/dev/sdg4
IMAGE_BYTES=15032385536
# 卸载第 4 分区
sudo umount "$ROOT_PART" 2>/dev/null || true
lsblk -o NAME,SIZE,RM,TYPE,FSTYPE,LABEL,MOUNTPOINTS "$DISK"
# 查看分区表和空闲空间
sudo parted "$DISK" unit GiB print free
```

第 4 分区必须是最后一个分区，后面必须有连续空闲空间。如果它小于 14 GiB，将结束位置扩展到 16 GiB；这会得到约 15.9 GiB 的第 4 分区，不会占满整张卡：

```bash
# 获取第 4 分区的准确容量
PART_BYTES=$(sudo blockdev --getsize64 "$ROOT_PART")

if [ "$PART_BYTES" -lt "$IMAGE_BYTES" ]; then
# 扩展第 4 分区，修改第 4 分区的结束位置，将结束位置设置在整张卡的 16 GiB 处
  sudo parted "$DISK" --script resizepart 4 16GiB
  sudo partprobe "$DISK"
  sudo udevadm settle
fi
# 重新读取扩展后的容量
PART_BYTES=$(sudo blockdev --getsize64 "$ROOT_PART")
echo "image bytes:     $IMAGE_BYTES"
echo "partition bytes: $PART_BYTES"
test "$PART_BYTES" -ge "$IMAGE_BYTES" \
  && echo "SIZE OK" \
  || echo "SIZE ERROR: 不要继续写入"
```

只有看到 `SIZE OK` 才能继续。

## 7. 写入完整的 14 GiB rootfs

先查询实际容器名，将完整 rootfs 压缩镜像复制到 WSL，并校验压缩文件：

```bash
# 查看包括已停止容器在内的 Docker 容器，确认实际容器名或 ID
docker ps -a --format 'table {{.ID}}\t{{.Names}}\t{{.Status}}'

# 设置容器名以及复制到 WSL 后的镜像路径
CONTAINER="your-container"
ROOTFS_GZ="$HOME/sdcard-rv-pub.img.gz"

# 从 Docker 容器复制完整的 rootfs 压缩镜像到 WSL
docker cp \
  "$CONTAINER:/workspace/sdcard-rv-pub.img.gz" \
  "$ROOTFS_GZ"

# 计算压缩镜像的 SHA-256，与下方已知正确值比较
sha256sum "$ROOTFS_GZ"
# 检查 gzip 文件结构和压缩数据是否完整
gzip -t "$ROOTFS_GZ" && echo "gzip OK"
```

确保第 4 分区没有挂载，然后边解压边覆盖整个第 4 分区。目标必须是`ROOT_PART`，不能是整块磁盘 `DISK`：

```bash
# 检查第 4 分区是否已挂载；正常情况下不应输出挂载记录
findmnt "$ROOT_PART" || true
# 再次显示写入源和目标，防止误写整块 TF 卡
echo "source: $ROOTFS_GZ"
echo "target: $ROOT_PART"

# 只要解压或 dd 任一环节失败，整个管道就返回失败
set -o pipefail
# 解压完整 rootfs，并直接写入未挂载的第 4 分区
gzip -dc "$ROOTFS_GZ" |
  sudo dd of="$ROOT_PART" \
    bs=16M \
    iflag=fullblock \
    status=progress \
    conv=fsync

# 检查写入管道的退出状态；必须为 0
echo "dd exit status: $?"
# 将仍在系统缓存中的数据写入 TF 卡
sync
```

在修改文件系统之前，读取刚写入的 14 GiB 并验证哈希：

```bash
# 从第 4 分区开头读取 896 个 16 MiB 数据块，即完整的 14 GiB 镜像
# 计算卡上原始镜像的 SHA-256，与下方已知正确值比较
sudo dd if="$ROOT_PART" \
  bs=16M \
  count=896 \
  iflag=fullblock \
  status=progress |
  sha256sum
```

哈希一致后检查 ext4，并让文件系统使用第 4 分区中的剩余空间：

```bash
# 在扩展前检查并自动修复 ext4 文件系统
sudo e2fsck -f -y "$ROOT_PART"
# 将 ext4 扩展到第 4 分区的当前容量
sudo resize2fs "$ROOT_PART"
# 扩展后再次只读检查文件系统，不自动修改
sudo e2fsck -f -n "$ROOT_PART"
```

最后只读挂载，确认完整 rootfs 和工具链均存在：

```bash
# 设置并创建临时挂载目录
ROOT_MNT=/mnt/kairix-root
sudo mkdir -p "$ROOT_MNT"
# 以只读方式挂载，避免验证过程修改最终 rootfs
sudo mount -o ro "$ROOT_PART" "$ROOT_MNT"

# 检查文件系统容量、类型、标签和 UUID
df -hT "$ROOT_MNT"
sudo blkid "$ROOT_PART"
# 检查基础系统、工具链和 cagent 测例是否存在
sudo ls -l "$ROOT_MNT/bin/busybox"
sudo ls -l "$ROOT_MNT/usr/bin/git"
sudo ls -l "$ROOT_MNT/usr/bin/gcc"
sudo ls -l "$ROOT_MNT/root/.cargo/bin/rustc"
sudo ls -l "$ROOT_MNT/glibc/cagent_testcode.sh"

# 验证完成后卸载分区，并刷新缓存
sudo umount "$ROOT_MNT"
sync
```

## 8. 替换第 3 分区中的 Kairix 镜像

挂载启动分区：

```bash
# 创建启动分区的临时挂载目录；目录已存在时不会报错
sudo mkdir -p /mnt/sdboot
# 将 TF 卡第 3 分区挂载到 /mnt/sdboot
sudo mount /dev/sdg3 /mnt/sdboot
# 列出启动分区内容，确认内核、DTB 和 U-Boot 配置文件仍然存在
sudo ls -lh /mnt/sdboot
```

只删除旧 Kairix 镜像。不要删除 `dtbs`、`extlinux`、`uEnv.txt` 或启动固件：

```bash
# 只删除 U-Boot 当前使用的旧 Kairix 内核，不改动其他启动文件
sudo rm -f /mnt/sdboot/kairix-uImage-rv
```

复制最新镜像，并统一使用 U-Boot 启动文件名 `kairix-uImage-rv`：

```bash
# 将 WSL 中的新内核复制到启动分区，并使用固定的 U-Boot 文件名
sudo cp ~/kairix-uImage-vf2-network-root4 \
  /mnt/sdboot/kairix-uImage-rv

# 将文件数据和文件系统元数据写入 TF 卡
sync
# 计算 WSL 源镜像和 TF 卡目标镜像的 SHA-256；两者必须一致
sha256sum ~/kairix-uImage-vf2-network-root4
sudo sha256sum /mnt/sdboot/kairix-uImage-rv
# 检查卡上内核镜像的文件大小和修改时间
sudo ls -lh /mnt/sdboot/kairix-uImage-rv
# 确认 VisionFive 2 的设备树文件没有被删除
sudo ls -lh /mnt/sdboot/dtbs/starfive/jh7110-visionfive-v2.dtb
```

源文件和卡上文件的 SHA-256 必须完全一致。

## 9. 安全卸载和断开读卡器

在 WSL 中执行：

```bash
sync
sudo umount /mnt/sdboot
lsblk
```

确认 `sdg3` 和 `sdg4` 都没有挂载点。然后在管理员 PowerShell 中执行，
BUSID 应使用本次 `usbipd list` 显示的值：

```powershell
usbipd detach --busid 3-3
usbipd list
```


## 10. 在 VisionFive 2 上启动 Kairix

将 TF 卡插回开发板，连接串口和网线。上电后在串口终端中打断 U-Boot
自动启动，执行：

```bash
# 从 mmc 设备 1 的第 3 分区加载 Kairix uImage 到内存地址 0x84000000
load mmc 1:3 0x84000000 kairix-uImage-rv
# 禁止 bootm 把 DTB 重定位回内核占用的物理内存
setenv fdt_high 0xffffffffffffffff
# 1 GiB bootstrap heap 使内核物理范围延伸到约 0xc0b00000；将 DTB 放到安全地址
load mmc 1:3 0xd0000000 dtbs/starfive/jh7110-visionfive-v2.dtb
# 启动内核；中间的 - 表示不提供 initrd，最后一个地址是 DTB
bootm 0x84000000 - 0xd0000000
```

## 11. 配置 Windows 直连网络

本文使用以下直连网络：

```text
Windows 有线网卡：192.168.10.1/24
VisionFive 2：    192.168.10.2/24
默认网关：        192.168.10.1（仅板端配置）
```

Windows 继续通过 Wi-Fi 或其他接口访问互联网。连接开发板的有线网卡只配置
静态地址，不配置默认网关和 DNS。以下命令需要在管理员 PowerShell 中执行。

### 11.1 确认有线网卡

```powershell
# 查看网卡名称、连接状态、速率和 MAC 地址
Get-NetAdapter |
  Format-Table Name,Status,LinkSpeed,MacAddress

# 修改为实际连接 VisionFive 2 的网卡名称
$Wired = "以太网"
$HostAddress = "192.168.10.1"
$PrefixLength = 24
$BoardAddress = "192.168.10.2"

# 单独查看目标网卡，确认 Status 为 Up
Get-NetAdapter -Name $Wired |
  Format-Table Name,Status,LinkSpeed,MacAddress
```

如果网卡名称不是“以太网”，只修改 `$Wired`，后续命令无需改动。

### 11.2 设置静态 IPv4 地址

下面的命令会清除该有线网卡现有的手动 IPv4 地址，再设置
`192.168.10.1/24`。不要对 Windows 当前用于上网的 Wi-Fi 网卡执行这些命令。

```powershell
# 关闭该有线网卡的 DHCP
Set-NetIPInterface `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 `
  -Dhcp Disabled

# 删除该有线网卡原有的手动 IPv4 地址
Get-NetIPAddress `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 `
  -ErrorAction SilentlyContinue |
  Where-Object PrefixOrigin -eq Manual |
  Remove-NetIPAddress -Confirm:$false

# 设置 Windows 直连地址；这里不设置 DefaultGateway
New-NetIPAddress `
  -InterfaceAlias $Wired `
  -IPAddress $HostAddress `
  -PrefixLength $PrefixLength

# 检查最终 IPv4 配置
Get-NetIPAddress `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 |
  Format-Table InterfaceAlias,IPAddress,PrefixLength,AddressState
```

### 11.3 启用转发并创建 NAT

Windows NAT 将来自 `192.168.10.0/24` 的板端流量转发到 Windows 当前的默认
外网接口，不需要在 `New-NetNat` 中指定 Wi-Fi 名称。不要同时为这个子网启用
“Internet 连接共享（ICS）”和 `NetNat`。

```powershell
$NatName = "KairixNat"
$InternalPrefix = "192.168.10.0/24"

# 允许该有线接口转发 IPv4 数据包
Set-NetIPInterface `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 `
  -Forwarding Enabled

# 查询是否已经存在同名 NAT
$ExistingNat = Get-NetNat `
  -Name $NatName `
  -ErrorAction SilentlyContinue

# 仅在 NAT 不存在时创建，重复执行不会重复添加
if ($null -eq $ExistingNat) {
  New-NetNat `
    -Name $NatName `
    -InternalIPInterfaceAddressPrefix $InternalPrefix
} elseif ($ExistingNat.InternalIPInterfaceAddressPrefix -ne $InternalPrefix) {
  throw "$NatName 已存在，但内部网段不是 $InternalPrefix"
}

# 检查转发状态和 NAT 配置
Get-NetIPInterface `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 |
  Format-Table InterfaceAlias,Dhcp,Forwarding,ConnectionState

Get-NetNat -Name $NatName |
  Format-List Name,InternalIPInterfaceAddressPrefix,Active
```

`New-NetNat` 创建的配置会保留，Windows 重启后通常不需要重新创建。重新测试前
使用 `Get-NetNat -Name KairixNat` 确认其仍然存在即可。

### 11.4 Windows 到开发板的 ping 测试

开发板启动且网口链路建立后，在管理员 PowerShell 中执行：

```powershell
# 清除旧 ARP 项，确保本次重新解析开发板 MAC
arp -d $BoardAddress

# 记录测试前的有线网卡统计
$Before = Get-NetAdapterStatistics -Name $Wired

# 连续发送 100 个带 32 字节负载的 ICMP 请求
ping -n 100 -l 32 $BoardAddress

# 检查 ARP 表中是否出现 192.168.10.2 对应的 Kairix MAC
arp -a

# 计算本次测试期间的接收包、错误包和丢弃包增量
$After = Get-NetAdapterStatistics -Name $Wired
[pscustomobject]@{
  RxUnicast = $After.ReceivedUnicastPackets - $Before.ReceivedUnicastPackets
  RxErrors  = $After.ReceivedPacketErrors - $Before.ReceivedPacketErrors
  RxDropped = $After.ReceivedDiscardedPackets - $Before.ReceivedDiscardedPackets
}
```

实板已验证结果为 100 次全部回复、`RxErrors=0`、`RxDropped=0`。

如果开发板无法 ping Windows 的 `192.168.10.1`，但 Windows 可以 ping 开发板，
可能是 Windows 防火墙拒绝入站 ICMP。仅在需要板端 ping Windows 时添加一条
限定到该有线网卡的规则：

```powershell
New-NetFirewallRule `
  -DisplayName "Kairix ICMPv4 Echo" `
  -Direction Inbound `
  -InterfaceAlias $Wired `
  -Protocol ICMPv4 `
  -IcmpType 8 `
  -Action Allow
```

## 12. 配置板端网关和 DNS

Windows NAT 生效后，板端还需要默认路由和 DNS。先通过 `ip link` 确认实际
接口名；以下以 `eth0` 为例：

```bash
# 查看网络接口名称和链路状态
ip link

NETDEV=eth0

# 启用接口并设置板端静态地址
ip link set "$NETDEV" up
ip addr replace 192.168.10.2/24 dev "$NETDEV"

# 将 Windows 有线地址设置为默认网关
ip route replace default via 192.168.10.1 dev "$NETDEV"

# 配置 DNS；rootfs 可写时该设置立即生效
printf 'nameserver 1.1.1.1\n' > /etc/resolv.conf

# 检查板端地址、路由和 DNS
ip addr show dev "$NETDEV"
ip route
cat /etc/resolv.conf
```

## 13. 可选：撤销 Windows 配置

不再使用直连网络时，可在管理员 PowerShell 中删除 NAT、静态地址，并恢复
该有线网卡的 DHCP：

```powershell
# 删除 Kairix NAT
Remove-NetNat -Name $NatName -Confirm:$false -ErrorAction SilentlyContinue

# 关闭该有线接口的 IPv4 转发
Set-NetIPInterface `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 `
  -Forwarding Disabled

# 删除手动 IPv4 地址并恢复 DHCP
Get-NetIPAddress `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 `
  -ErrorAction SilentlyContinue |
  Where-Object PrefixOrigin -eq Manual |
  Remove-NetIPAddress -Confirm:$false

Set-NetIPInterface `
  -InterfaceAlias $Wired `
  -AddressFamily IPv4 `
  -Dhcp Enabled
```
