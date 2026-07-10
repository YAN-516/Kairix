# SSH syscall ABI

本文记录当前内核提供的 SSH syscall 编号和用户态语义。SSH syscall 建立在已连接的 TCP socket 之上，由内核侧 Sunset SSH runner 维护 SSH session 和 channel 状态。

## Syscall 编号

| 编号 | 名称 | 说明 |
| --- | --- | --- |
| 1110 | `ssh_connect` | 在已连接 TCP socket 上建立 SSH transport session |
| 1111 | `ssh_write` | 保留的 raw SSH 写接口；Sunset 接管 transport 后非空写入返回错误 |
| 1112 | `ssh_read` | 保留的 raw SSH 读接口；Sunset 接管 transport 后非空读取返回错误 |
| 1113 | `ssh_close` | 关闭 SSH session |
| 1114 | `ssh_peer_ident` | 读取远端 SSH identification string |
| 1115 | `ssh_auth_password` | 用户名密码认证 |
| 1116 | `ssh_exec` | 打开 session channel 并执行远程命令 |
| 1117 | `ssh_channel_read` | 阻塞读取 channel stdout/stderr，返回 0 表示 EOF |
| 1118 | `ssh_channel_close` | 关闭并释放本地 channel handle |
| 1119 | `ssh_channel_status` | 获取远程命令退出码；未退出时返回 `EAGAIN` |
| 1120 | `ssh_channel_write` | 向 channel stdin 写入数据 |
| 1121 | `ssh_shell` | 打开无 PTY 的远程 shell channel |
| 1122 | `ssh_channel_try_read` | 非阻塞读取 channel stdout/stderr；暂无数据时返回 `EAGAIN` |
| 1123 | `ssh_auth_publickey` | 使用 OpenSSH Ed25519 私钥做公钥认证 |

## 当前语义

- `ssh_connect(fd, ident_ptr, ident_len)` 要求 `fd` 是已连接的 TCP socket。
- `ssh_auth_password` 支持用户名密码认证。
- `ssh_auth_publickey` 支持未加密 OpenSSH `ssh-ed25519` 私钥；暂不支持加密私钥、RSA/ECDSA 私钥和 SSH agent。
- `ssh_exec` 和 `ssh_shell` 都会创建一个 SSH session channel。
- 每个 SSH session 当前只允许一个 active channel。
- `ssh_channel_read` 合并读取 stdout 和 stderr。
- `ssh_channel_write` 写入的是普通 channel data，对远端表现为 stdin。
- `ssh_channel_try_read` 主要用于交互式程序轮询远端输出。
- hostkey 当前自动接受，尚未实现 known_hosts 校验。

## 用户态封装

用户态 `user_lib` 暴露了对应封装：

```rust
ssh_connect(fd, ident)
ssh_auth_password(ssh_id, username, password)
ssh_auth_publickey(ssh_id, username, private_key_bytes)
ssh_exec(ssh_id, command)
ssh_shell(ssh_id)
ssh_channel_read(ssh_id, channel_id, buf)
ssh_channel_try_read(ssh_id, channel_id, buf)
ssh_channel_write(ssh_id, channel_id, buf)
ssh_channel_status(ssh_id, channel_id)
ssh_channel_close(ssh_id, channel_id)
ssh_close(ssh_id)
```

## 已有用户程序

- `sshtest`：SSH ABI 和密码认证测试。
- `sshexec`：执行单条远程命令并读取输出。
- `sshshell`：打开无 PTY 远程 shell，转发本地输入和远端输出。
