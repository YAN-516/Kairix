# Git 协议实现说明

本文记录当前 Git clone 相关协议层的实现进度。目标是让 HTTPS 和 SSH 两种 transport 共用同一套 Git 协议解析逻辑。

## 当前已完成

用户态公共库新增 `user_lib::git` 模块，当前支持：

- pkt-line 单包解析：
  - data packet，例如 `003f...`
  - flush packet：`0000`
- pkt-line 编码：
  - 写入 data packet
  - 写入 flush packet
- refs advertisement 最小解析：
  - 跳过 Smart HTTP 的 `# service=git-upload-pack`
  - 解析 `HEAD`
  - 解析 `refs/heads/*`、`refs/tags/*` 等普通 ref
  - 解析首个 ref line 上的 capability 列表
- HTTPS/SSH 版 `gitls`：
  - 解析 `https://host/repo.git` URL
  - 解析 `ssh://user@host/repo.git` 和 `user@host:repo.git`
  - DNS 解析或 `--ip` 直连
  - TCP + TLS 连接
  - 请求 Smart HTTP `info/refs?service=git-upload-pack`
  - 支持普通 HTTP body 和 `Transfer-Encoding: chunked`
  - TCP + SSH 连接、密码认证、exec `git-upload-pack`
  - 打印远端 refs 和 capability

相关代码：

```text
user/src/git.rs
user/src/bin/gitpkt_test.rs
user/src/bin/gitls.rs
```

## 自测程序

`gitpkt_test` 用于测试 pkt-line 编解码和 refs advertisement 解析：

```sh
gitpkt_test
```

预期输出包含：

```text
git pkt-line selftest: ok
```

## HTTPS refs 列表

`gitls` 用于测试 Git over HTTPS 的 refs advertisement：

```sh
gitls https://github.com/git/git.git
```

如果 DNS 不通，可以指定 QEMU user 网络 DNS：

```sh
gitls -d 10.0.2.3 https://github.com/git/git.git
```

预期输出包含若干 refs：

```text
<oid> HEAD
<oid> refs/heads/master
refs: N
capabilities: ...
```

## SSH refs 列表

`gitls` 也可通过 SSH 执行远端 `git-upload-pack`：

```sh
gitls ssh://user@host/repo.git --password PASS
gitls user@host:repo.git --password PASS
```

当前 SSH 只支持密码认证，暂不支持私钥认证。

## 当前未做

- protocol v2 的 delimiter packet：`0001`
- side-band 解包
- upload-pack `want/done` 请求生成
- packfile 校验和解析
- `.git` 目录写入
- checkout 工作区

## 下一步

实现 HTTPS 版 `gitfetch` 的最小闭环：

- 根据 refs advertisement 选择 `HEAD` 或指定 ref。
- 生成 `git-upload-pack` 请求：`want`、capability、`done`。
- 解析 side-band 数据。
- 保存 packfile，为后续 `.git` 写入和 checkout 做准备。
