# SSH 使用说明

本文说明当前系统内 SSH 用户程序和 syscall 能力的使用方法。当前 SSH 已支持 TCP 连接、SSH 握手、密码认证、执行单条远程命令、向远程命令写 stdin，以及打开一个无 PTY 的交互式 shell。

## 已支持功能

- `sshtest`：测试 SSH 连接、peer ident、密码认证和 ABI 错误路径。
- `sshexec`：通过 SSH 密码认证后执行一条远程命令，打印 stdout/stderr，并返回远程退出码。
- `sshshell`：通过 SSH 密码认证后打开远程 shell，支持本地输入和远端输出双向转发。
- 内核侧 syscall：
  - `ssh_connect`
  - `ssh_auth_password`
  - `ssh_peer_ident`
  - `ssh_exec`
  - `ssh_shell`
  - `ssh_channel_read`
  - `ssh_channel_try_read`
  - `ssh_channel_write`
  - `ssh_channel_status`
  - `ssh_channel_close`
  - `ssh_close`

## 运行前提

目标机器需要运行 SSH server，并允许密码登录。

在 QEMU 默认网络环境中，客户机访问宿主机通常使用：

```sh
10.0.2.2
```

例如宿主机有用户：

```text
用户名：kairixssh
密码：123456
端口：22
```

则可以在系统内运行下面的测试。

## sshtest

### 本地 ABI 自测

```sh
sshtest --selftest
```

该命令不需要真实 SSH server，主要检查错误路径，例如：

- 无效 fd 返回 `EBADF`
- 非 TCP socket 返回 `ENOTSOCK`
- 未连接 TCP 返回 `ENOTCONN`
- 非法 client ident 返回 `EINVAL`
- stale handle 返回 `EBADF`

### 测试 SSH 握手

```sh
sshtest 10.0.2.2 22
```

预期输出包含：

```text
[ok] tcp connect
[ok] ssh connect
ssh peer ident: SSH-...
```

### 测试密码认证

```sh
sshtest 10.0.2.2 22 kairixssh 123456
```

预期输出包含：

```text
[ok] tcp connect
[ok] ssh connect
[ok] ssh password auth
```

## sshexec

`sshexec` 用于执行单条远程命令。

```sh
sshexec <ipv4> <port> <username> <password> <command>
```

示例：

```sh
sshexec 10.0.2.2 22 kairixssh 123456 "uname -a"
```

成功时输出类似：

```text
[ok] tcp connect
[ok] ssh connect
[ok] password auth
[ok] exec channel 1
Linux docker-desktop 5.15.167.4-microsoft-standard-WSL2 ...

[exit] 0
```

### 常用测试命令

查看远端用户信息：

```sh
sshexec 10.0.2.2 22 kairixssh 123456 "id"
```

查看远端当前目录：

```sh
sshexec 10.0.2.2 22 kairixssh 123456 "pwd"
```

测试 stdout、stderr 和退出码：

```sh
sshexec 10.0.2.2 22 kairixssh 123456 "sh -c 'echo stdout; echo stderr >&2; exit 7'"
```

预期输出包含：

```text
stdout
stderr

[exit] 7
```

## sshshell

`sshshell` 用于打开一个远程 shell，并在本地终端和 SSH channel 之间转发输入输出。

```sh
sshshell <ipv4> <port> <username> <password>
```

示例：

```sh
sshshell 10.0.2.2 22 kairixssh 123456
```

成功时输出类似：

```text
[ok] tcp connect
[ok] ssh connect
[ok] password auth
[ok] shell channel 1
[info] no PTY yet; type commands and use `exit` to close the shell
kairixssh@10.0.2.2$ 
```

进入后可以输入远端命令：

```sh
id
pwd
uname -a
exit
```

`sshshell` 会在本地模拟一个简单提示符：

```text
<username>@<ipv4>$ 
```

注意：当前是无 PTY shell，适合执行普通命令；暂不适合 `top`、`vim`、全屏程序、需要终端控制序列的交互程序。退出时会恢复本地 stdin 的原始 flags，避免影响返回后的本地 shell 输入。

## 返回值说明

`sshexec` 会使用远程命令的退出码作为自身退出码，范围取低 8 位。

例如：

```sh
sshexec 10.0.2.2 22 kairixssh 123456 "sh -c 'exit 3'"
```

远程命令退出码为 `3`，程序最后会打印：

```text
[exit] 3
```

## 常见错误

### `tcp connect failed`

可能原因：

- SSH server 没启动。
- IP 或端口错误。
- QEMU 网络未配置好。
- 宿主机防火墙阻止连接。

可以先在宿主机确认：

```sh
ssh kairixssh@127.0.0.1
```

### `ssh connect failed`

表示 TCP 已连上，但 SSH 握手失败。

可能原因：

- 目标端口不是 SSH 服务。
- SSH server 过早断开连接。
- KEX/算法兼容性问题。

### `ssh password auth failed`

可能原因：

- 用户名或密码错误。
- SSH server 禁用了密码认证。
- 用户不允许 SSH 登录。

检查服务器配置时重点看：

```text
PasswordAuthentication yes
```

### `ssh exec failed`

可能原因：

- 还没有完成密码认证。
- 当前 SSH session 已经有一个 active channel。
- 远端拒绝 session 或 exec 请求。

### `ssh shell failed`

可能原因：

- 还没有完成密码认证。
- 当前 SSH session 已经有一个 active channel。
- 远端拒绝 shell 请求。

### `ssh channel read failed`

表示 exec channel 读取失败，可能是连接断开或 SSH channel 状态异常。

### `ssh channel write failed`

表示向远端 stdin 写入失败，可能是远端命令已经退出、channel 已关闭，或连接已经断开。

## 当前限制

- 只支持 IPv4。
- 只支持密码认证。
- 暂不支持 known_hosts 校验；hostkey 当前由内核自动接受。
- `sshexec` 只执行一条命令。
- 每个 SSH session 当前只允许一个 active channel。
- stdout 和 stderr 当前合并输出。
- 已支持 stdin 写入。
- 已支持无 PTY 交互式 shell；暂不支持 PTY。
- 暂不支持 SFTP。

## 后续可扩展方向

- 增加 PTY 支持。
- 增加公钥认证。
- 增加 known_hosts/hostkey 校验。
- 基于 SSH channel 实现 SFTP。
