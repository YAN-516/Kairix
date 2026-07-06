# 网络功能文档

本目录集中放置网络相关用户程序、syscall ABI 和协议能力说明。

## 当前文档

- [httpsget 使用说明](./httpsget使用说明.md)
- [TLS syscall ABI](./tls-syscall-abi.md)
- [SSH 使用说明](./ssh使用说明.md)
- [SSH syscall ABI](./ssh-syscall-abi.md)
- [Git 协议实现说明](./git-protocol.md)
- [gitls 使用说明](./gitls使用说明.md)

## 当前能力概览

- HTTPS/TLS：已支持 TCP + TLS 连接、TLS 明文读写、`httpsget` 调试工具。
- SSH：已支持 SSH 握手、密码认证、exec、channel read/write、无 PTY shell。
- Git：已新增 pkt-line 编解码、refs advertisement 最小解析，以及 HTTPS/SSH `gitls` refs 列表工具。

后续 Git clone 的 HTTPS 和 SSH 能力建议继续在本目录下新增 Git 协议、Smart HTTP、Git over SSH 和 packfile 相关文档。
