# gitls 使用说明

`gitls` 是当前 Git clone 的 refs 列表测试工具，支持 HTTPS 和 SSH 两种 transport。它不会下载文件，也不会创建 `.git` 目录；它只访问远端仓库的 `git-upload-pack` refs advertisement，并打印 refs 和 capability。

## 用法

```sh
gitls [options] <url> [dns-ip]
```

示例：

```sh
gitls https://github.com/git/git.git
gitls -d 10.0.2.3 https://github.com/git/git.git
gitls --ip 140.82.114.4 https://github.com/git/git.git
gitls ssh://user@10.0.2.2/home/user/repo.git --password 123456
gitls user@10.0.2.2:repo.git --password 123456
gitls git@github.com:user/repo.git --key /home/user/.ssh/id_ed25519
```

## 参数

- `-h, --help`：显示帮助。
- `-d, --dns IP`：指定 DNS 服务器，默认是 QEMU user 网络常用的 `10.0.2.3`。
- `--dns=IP`：同上。
- `--ip IP`：跳过 DNS，直接连接指定 IPv4。TLS SNI 和 HTTP Host 仍使用 URL 里的主机名。
- `--ip=IP`：同上。
- `-p, --port PORT`：指定 TCP 端口；HTTPS 默认 `443`，SSH 默认 `22`。
- `--port=PORT`：同上。
- `-u, --user USER`：指定 SSH 用户名，当 SSH URL 里没有用户名时使用。
- `--user=USER`：同上。
- `--password PASS`：指定 SSH 密码。
- `--password=PASS`：同上。
- `-i, --key PATH`：指定 OpenSSH 格式的 Ed25519 私钥文件。当前只支持未加密的 `id_ed25519`。
- `--key=PATH`：同上。
- `-v, --verbose`：HTTPS 时打印 HTTP 请求，SSH 时打印执行的 `git-upload-pack` 命令。
- `[dns-ip]`：兼容位置参数写法，例如 `gitls https://github.com/git/git.git 10.0.2.3`。

## URL 形式

HTTPS：

```sh
gitls https://host/repo.git
```

SSH：

```sh
gitls ssh://user@host/repo.git --password PASS
gitls ssh://user:PASS@host/repo.git
gitls user@host:repo.git --password PASS
gitls git@github.com:user/repo.git --key /home/user/.ssh/id_ed25519
```

`ssh://user@host/home/user/repo.git` 会把 `/home/user/repo.git` 当作远端绝对路径；`user@host:repo.git` 会把 `repo.git` 当作 SSH 登录后的相对路径。

SSH 内部会执行：

```sh
git-upload-pack 'repo.git'
```

## 输出说明

成功时会输出类似：

```text
gitls: github.com (140.82.114.4)/git/git.git
<oid> HEAD
<oid> refs/heads/master
<oid> refs/tags/v2.45.0
refs: 1234
capabilities: multi_ack thin-pack side-band ...
```

其中：

- 第一列是对象 ID。
- 第二列是 ref 名称，例如 `HEAD`、`refs/heads/*`、`refs/tags/*`。
- `capabilities` 是远端 `git-upload-pack` 支持的协议能力。

## 当前限制

- HTTPS 支持 `https://` URL。
- SSH 支持 `ssh://user@host/repo.git` 和 `user@host:repo.git` 形式。
- SSH 支持密码认证和未加密 OpenSSH Ed25519 私钥认证。
- 暂不支持加密私钥、RSA 私钥、ECDSA 私钥、SSH agent 和 known_hosts 校验。
- 只读取 refs advertisement，不会发送 `want/done`。
- 不会下载 packfile。
- 不会写入 `.git` 目录，也不会 checkout 工作区。
- 当前 HTTPS 读取支持普通 body 和 `Transfer-Encoding: chunked`。

## 如何测试

先确认 pkt-line 基础解析通过：

```sh
gitpkt_test
```

预期末尾：

```text
git pkt-line selftest: ok
```

然后测试公网 HTTPS 仓库：

```sh
gitls https://github.com/git/git.git
```

如果 DNS 不通，显式指定 QEMU DNS：

```sh
gitls -d 10.0.2.3 https://github.com/git/git.git
```

如果想绕过 DNS，可以先在宿主机查到 IP，再用：

```sh
gitls --ip <github-ip> https://github.com/git/git.git
```

测试 SSH 仓库：

```sh
gitls ssh://kairixssh@10.0.2.2/home/kairixssh/repo.git --password 123456
gitls kairixssh@10.0.2.2:repo.git --password 123456
```

如果 SSH 服务在宿主机 QEMU 地址，通常是 `10.0.2.2:22`。

测试 GitHub/GitLab 这类只允许公钥认证的仓库：

```sh
gitls git@github.com:user/repo.git --key /home/user/.ssh/id_ed25519
```

如果 DNS 不稳定，可以配合 `--ip`：

```sh
gitls --ip <github-ip> git@github.com:user/repo.git --key /home/user/.ssh/id_ed25519
```
