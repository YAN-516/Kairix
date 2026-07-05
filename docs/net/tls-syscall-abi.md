# TLS syscall ABI

本文记录当前内核提供的 TLS syscall 编号和用户态语义。TLS syscall 建立在已连接的 TCP socket 之上，用户态仍然负责创建、连接和关闭 TCP fd；内核负责维护 TLS session handle，并通过该 handle 读写明文数据。

## Syscall 编号

| 编号 | 名称 | 参数 | 返回值 |
| --- | --- | --- | --- |
| 1100 | `tls_connect` | `fd`, `host_ptr`, `host_len` | 成功返回 TLS handle |
| 1101 | `tls_write` | `tls_id`, `buf`, `len` | 成功返回写入的明文字节数 |
| 1102 | `tls_read` | `tls_id`, `buf`, `len` | 成功返回读取的明文字节数，返回 `0` 表示 EOF |
| 1103 | `tls_close` | `tls_id` | 成功返回 `0` |

所有失败在用户态表现为负数 errno。

## 调用流程

典型流程：

```text
socket(AF_INET, SOCK_STREAM, IPPROTO_TCP)
connect(fd, sockaddr, len)
tls_connect(fd, host)
tls_write(tls_id, request, request_len)
tls_read(tls_id, response_buf, response_len)
tls_close(tls_id)
close(fd)
```

`tls_connect` 要求传入的 `fd` 已经是 established TCP socket。`host` 会作为 TLS ServerName/SNI 使用。

## 当前语义

- TLS session 归创建它的进程所有，其他进程使用同一个 `tls_id` 会返回 `EBADF`。
- `tls_connect` 完成 TLS client handshake 后才返回 handle。
- `tls_write` 接收明文，内核侧加密后写入底层 TCP。
- `tls_read` 从底层 TCP 读取 TLS record，解密后返回明文。
- `tls_read` 在连接关闭时返回 `0`。
- `tls_close` 会发送 TLS close_notify，并移除本地 TLS handle。
- `tls_close` 不关闭底层 TCP fd；用户态仍应自行 `close(fd)`。

## 错误说明

| 场景 | 错误 |
| --- | --- |
| fd 或 tls handle 无效 | `EBADF` |
| fd 不是 TCP socket | `ENOTSOCK` |
| TCP socket 未连接 | `ENOTCONN` |
| host 不是合法 TLS ServerName | `EINVAL` |
| 用户指针不可访问 | `EFAULT` |
| TLS 握手或读取等待超时 | `ETIMEDOUT` |
| TLS/rustls 内部错误 | `EIO` |

## 当前安全限制

当前 TLS 使用自定义 `InsecureServerVerifier`，会接受服务器证书和签名校验结果。因此它适合功能验证和比赛环境内联调，但还不具备真实生产 HTTPS 客户端的证书安全性。

后续如果要接近真实 HTTPS/Git HTTPS，需要补：

- CA root store
- 服务器证书链校验
- hostname 校验
- 证书过期时间校验
- 可选的证书指纹 pinning

## 用户态封装

`user_lib` 暴露了对应封装：

```rust
tls_connect(fd, host)
tls_write(tls_id, buf)
tls_read(tls_id, buf)
tls_close(tls_id)
```

目前主要用户程序是 `httpsget`，用于通过 TCP + TLS 发起 HTTP/HTTPS 调试请求。
