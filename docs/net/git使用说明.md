# Git 命令使用说明

本文档总结当前用户态 `git` 命令的使用方式、典型测试流程和已知限制。当前实现目标是支持比赛题目中的本地 Git 操作和远程 clone/push/pull 流程。

## 总览

统一入口：

```sh
git <command> [args]
git -h
git help
```

当前支持的主要命令：

```text
git init
git add
git commit
git config
git log
git status
git clone
git fetch
git pull
git push
git remote add
git branch
git checkout
git switch
git ls-remote
```

内部还提供调试工具：

```text
git pack
git checkout-pack
git pkt-test
```

## 默认配置

SSH 默认私钥路径：

```text
/musl/id_ed25519
```

因此访问 GitHub SSH 仓库时通常不需要手动写 `--key`：

```sh
git clone git@github.com:StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_test
git push origin
git pull origin
```

如果需要覆盖默认私钥，可以显式指定：

```sh
git clone git@github.com:user/repo.git /tmp/repo --key /path/to/id_ed25519
git push origin --key /path/to/id_ed25519
```

当前 SSH key 支持未加密 OpenSSH Ed25519 私钥。

HTTPS push 默认 token 路径：

```text
/musl/github_token
```

GitHub HTTPS push 不能使用账号密码，需要 Personal Access Token。推荐把 token 写入默认文件：

```sh
echo 'github_pat_xxx' > /musl/github_token
```

测试时不要 `cat /musl/github_token`，避免 token 出现在日志里。只检查文件是否存在和大小：

```sh
ls -l /musl/github_token
```

也可以显式指定 token 文件：

```sh
git push origin --user StarryNight0630 --token-file /musl/github_token
```

如果使用默认 `/musl/github_token`，只需要：

```sh
git push origin --user StarryNight0630
```

## 本地仓库操作

### 初始化仓库

```sh
mkdir -p /tmp/proj
cd /tmp/proj
git init
```

预期输出：

```text
Initialized empty Git repository in ./.git
```

### 添加文件

```sh
echo "# Test Project" > README.md
git status
git add .
git status
```

也可以添加单个文件：

```sh
git add README.md
```

在非仓库当前目录中添加文件时，可以指定仓库目录：

```sh
git add --repo /tmp/proj README.md
```

### 提交

```sh
git commit -m "add README.md"
```

指定提交时间：

```sh
git commit -m "add README.md" --date "2026-07-08 13:00:00"
```

默认作者来自全局配置；如果没有配置，会使用内置默认值。

### 查看日志

```sh
git log
git log /tmp/proj
git log -n 3
```

### 查看状态

```sh
git status
git status /tmp/proj
```

常见输出：

```text
nothing to commit, working tree clean
modified: README.md
staged: modified README.md
untracked: tmp.txt
```

## 用户配置

设置全局用户名和邮箱：

```sh
git config --global user.name "StarryNight0630"
git config --global user.email "alice@example.com"
```

配置写入后不需要每次重复执行。

## 远程仓库 clone

### SSH clone

```sh
git clone git@github.com:StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_ssh
cd /tmp/os_net_ssh
cat README.md
git branch -a
```

也支持 `ssh://` URL：

```sh
git clone ssh://kairixssh@10.0.2.2/home/kairixssh/repo.git /tmp/repo
```

### HTTPS clone

```sh
git clone https://github.com/StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_https
cd /tmp/os_net_https
cat README.md
git branch -r
```

HTTPS 会通过 Smart HTTP `git-upload-pack` 获取 refs 和 packfile。当前已经支持 clone、pull 和 push。

## fetch 和 pull

### fetch

`git fetch` 会从远端下载 packfile，并保存元数据。一般用户更常用 `git pull`。

```sh
git fetch git@github.com:StarryNight0630/OS_Competition_Net_Test.git --repo /tmp/os_net_ssh -o /tmp/inc.pack
```

### SSH pull

```sh
cd /tmp/os_net_ssh
git pull origin
cat README.md
git status
```

### HTTPS pull

```sh
cd /tmp/os_net_https
git pull origin
cat README.md
git status
```

当前 `git pull` 的语义接近：

```text
fetch remote refs + 下载 pack + checkout 到当前分支
```

它不是完整 Git 的三方 merge/rebase。建议在工作区干净时执行 pull。

## push

当前 push 支持 SSH 和 HTTPS 远端。

```sh
cd /tmp/os_net_ssh
echo "# push test" >> README.md
git add README.md
git commit -m "push test"
git push origin
```

成功时会看到类似：

```text
pack objects: N
pack bytes: N
remote: unpack ok
remote: ok refs/heads/main
gitpush complete
```

### SSH push

SSH push 默认使用 `/musl/id_ed25519`：

```sh
git push origin
```

也可以显式指定私钥：

```sh
git push origin --key /musl/id_ed25519
```

### HTTPS push

HTTPS push 需要 GitHub PAT。默认读取：

```text
/musl/github_token
```

测试流程：

```sh
git clone https://github.com/StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_https_push
cd /tmp/os_net_https_push

echo "# https push test" >> README.md
git add README.md
git commit -m "https push test"
git push origin --user StarryNight0630

git status
cat README.md
cat .git/refs/heads/main
cat .git/refs/remotes/origin/main
```

成功时会看到：

```text
gitpush https: github.com (...) /StarryNight0630/OS_Competition_Net_Test.git
push <old> -> refs/heads/main
pack objects: 3
pack bytes: ...
remote: unpack ok
remote: ok refs/heads/main
updated remote ref: refs/remotes/origin/main
gitpush complete
```

如果远端分支已经变化，会拒绝 push：

```text
push rejected: remote branch changed
run git fetch or git pull before pushing
```

此时先执行：

```sh
git pull origin
git push origin
```

## remote

### 添加远端

```sh
git remote add me git@github.com:StarryNight0630/OS_Competition_Net_Test.git
cat .git/config
```

写入内容类似：

```ini
[remote "me"]
        url = git@github.com:StarryNight0630/OS_Competition_Net_Test.git
        fetch = +refs/heads/*:refs/remotes/me/*
```

然后可以：

```sh
git push me
git pull me
```

如果 `me` 和 `origin` 指向同一个 URL，首次 `git push me` 可以复用 `origin/main` 做安全检查；push 成功后会写入：

```text
.git/refs/remotes/me/main
```

## 分支

### 查看分支

```sh
git branch
git branch -r
git branch -a
git branch -vv
```

示例：

```text
* main
  remotes/origin/main
  remotes/origin/test_branch
```

### 切换分支

```sh
git checkout test_branch
cat README.md
```

如果本地没有 `test_branch`，但存在 `origin/test_branch`，会从远端分支创建本地分支并切换。

也可以使用：

```sh
git switch test_branch
```

### 创建本地分支

```sh
git checkout -b local_test
git branch
```

从指定起点创建：

```sh
git checkout -b from_remote origin/test_branch
git branch new_branch origin/main
```

### 删除本地分支

```sh
git branch -d local_test
git branch -D local_test
```

不能删除当前所在分支，需要先切到其他分支。

## ls-remote

列出远端 refs，不下载 packfile：

```sh
git ls-remote https://github.com/StarryNight0630/OS_Competition_Net_Test.git
git ls-remote git@github.com:StarryNight0630/OS_Competition_Net_Test.git
```

也可以直接使用底层命令：

```sh
gitls https://github.com/StarryNight0630/OS_Competition_Net_Test.git
gitls git@github.com:StarryNight0630/OS_Competition_Net_Test.git
```

## pack 调试命令

查看 packfile：

```sh
git pack /musl/gitfetch.pack
gitpack /musl/gitfetch.pack
```

从 packfile checkout：

```sh
git checkout-pack /musl/gitfetch.pack /tmp/repo --git --meta /musl/gitclone.meta
gitcheckout /musl/gitfetch.pack /tmp/repo --git --meta /musl/gitclone.meta
```

pkt-line 自测：

```sh
git pkt-test
gitpkt_test
```

预期：

```text
git pkt-line selftest: ok
```

## 完整测试流程

### 比赛题目验收流程

Task0：加载运行和帮助命令。

```sh
git -h
git help
```

Task1：文件系统相关功能。

```sh
mkdir -p /tmp/proj
cd /tmp/proj
git init
echo "# Test Project" > README.md
git add .
git commit -m "add README.md"
git log
git status
```

Task2：网络相关功能。下面以 GitHub 和 `xv6-riscv` 为例。

```sh
git config --global user.name "StarryNight0630"
git config --global user.email "alice@example.com"

git clone git@github.com:oscomp/xv6-riscv.git /tmp/xv6-riscv
cd /tmp/xv6-riscv
echo "# push from kairix git" >> README
git add README
git commit -m "update README"
```

在 GitHub 上提前创建自己的远程仓库，例如：

```text
git@github.com:StarryNight0630/xv6_clone_test.git
```

添加远端并 push：

```sh
git remote add me git@github.com:StarryNight0630/xv6_clone_test.git
git push me
```

在 GitHub 网页上修改自己的远程仓库 README 后，拉取更新：

```sh
git pull me
git status
cat README
```

如果要用 HTTPS push，则先准备 PAT：

```sh
echo 'github_pat_xxx' > /musl/github_token
git remote add mehttps https://github.com/StarryNight0630/xv6_clone_test.git
git push mehttps --user StarryNight0630
```

### 1. 本地 Git 测试

```sh
mkdir -p /tmp/proj
cd /tmp/proj
git init
echo "# Test Project" > README.md
git status
git add .
git status
git commit -m "add README.md"
git log
git status
```

### 2. SSH clone 和分支测试

```sh
git clone git@github.com:StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_ssh
cd /tmp/os_net_ssh
cat README.md
git branch -a
git checkout test_branch
cat README.md
cat test.md
git checkout main
cat README.md
```

### 3. SSH push 和 pull 测试

```sh
cd /tmp/os_net_ssh
echo "# ssh push test" >> README.md
git add README.md
git commit -m "ssh push test"
git push origin
```

在 GitHub 网页修改 `README.md` 后：

```sh
git pull origin
cat README.md
git status
```

### 4. remote alias 测试

```sh
cd /tmp/os_net_ssh
git remote add me git@github.com:StarryNight0630/OS_Competition_Net_Test.git
echo "# remote alias test" >> README.md
git add README.md
git commit -m "remote alias test"
git push me
cat .git/refs/heads/main
cat .git/refs/remotes/me/main
```

### 5. HTTPS clone 和 pull 测试

```sh
git clone https://github.com/StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_https
cd /tmp/os_net_https
cat README.md
git branch -r
```

在 GitHub 网页修改 `README.md` 后：

```sh
git pull origin
cat README.md
git status
```

### 6. HTTPS push 测试

先准备 GitHub PAT：

```sh
echo 'github_pat_xxx' > /musl/github_token
ls -l /musl/github_token
```

执行 HTTPS push：

```sh
git clone https://github.com/StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_https_push
cd /tmp/os_net_https_push

echo "# https push test" >> README.md
git add README.md
git commit -m "https push test"
git push origin --user StarryNight0630

git status
cat README.md
cat .git/refs/heads/main
cat .git/refs/remotes/origin/main
```

再回归 HTTPS pull：

```sh
git pull origin
git status
cat README.md
```

预期：

```text
nothing to commit, working tree clean
```

### 7. xv6 大仓库测试

```sh
git clone git@github.com:oscomp/xv6-riscv.git /tmp/xv6-riscv
cd /tmp/xv6-riscv
ls
cat README
git status
```

## 当前支持的对象和协议能力

对象支持：

- commit
- tree
- blob
- loose object 写入
- `.git/index` 写入
- `.git/HEAD` 写入
- `.git/refs/heads/*` 写入
- `.git/refs/remotes/<remote>/*` 写入

pack 支持：

- pack v2/v3 基础解析
- ofs-delta
- ref-delta
- thin pack：delta base 可从本地 `.git/objects` 查找
- 大对象流式落盘，避免大仓库 clone 时堆内存 panic

网络支持：

- Git over SSH：clone/fetch/pull/push/ls-remote
- Git over HTTPS：clone/fetch/pull/push/ls-remote
- GitHub HTTPS 连接带 DNS 多服务器查询和 fallback IP 尝试

## 当前限制

- `git pull` 不是完整 merge/rebase；建议只在工作区干净且线性更新时使用。
- 暂不支持冲突处理。
- 暂不支持 pack index `.idx`，对象主要以 loose object 写入。
- SSH 暂不校验 known_hosts。
- SSH 暂不支持加密私钥、RSA/ECDSA 私钥和 SSH agent。
- HTTPS push 需要 PAT，不支持浏览器 OAuth、credential helper 和 GitHub CLI 登录态。
- `git remote` 当前主要支持 `remote add`。
- 输出中仍保留部分调试信息，例如 pack、checkout、remote 进度信息。

## 常见问题

### 为什么 clone 后在 `/musl` 下 `cat README.md` 找不到？

clone 的目标目录是你指定的路径，例如：

```sh
git clone ... /tmp/os_net_https
```

需要进入仓库目录：

```sh
cd /tmp/os_net_https
cat README.md
```

### 为什么 `git push` 被拒绝？

如果远端分支已经变化，会看到：

```text
push rejected: remote branch changed
```

先同步远端：

```sh
git pull origin
git push origin
```

### HTTPS 偶尔 DNS 超时怎么办？

当前 `gitfetch/gitls/gitpush` 会尝试多个 DNS 和 GitHub fallback IP。若仍失败，可以重试，或者临时使用 SSH URL：

```sh
git clone git@github.com:StarryNight0630/OS_Competition_Net_Test.git /tmp/os_net_ssh
```

### HTTPS push 为什么需要 token？

GitHub 不允许用账号密码进行 HTTPS push。普通 Linux 上之所以经常不用手动输入，是因为系统 Git 使用了 credential helper、Git Credential Manager、系统 keychain 或 `gh auth login` 保存了 token。

当前用户态 `git` 没有这些外部组件，所以需要显式提供 PAT：

```sh
echo 'github_pat_xxx' > /musl/github_token
git push origin --user StarryNight0630
```

如果看到：

```text
https auth failed: http status 401
```

说明 token 错误或已失效。

如果看到：

```text
https auth failed: http status 403
```

说明 token 没有仓库写权限。Fine-grained token 至少需要目标仓库的 `Contents: Read and write` 权限。

### `git checkout test_branch` 后会发生什么？

如果本地还没有 `test_branch`，但远端有 `origin/test_branch`，会：

1. 创建本地 `test_branch`
2. 更新 `.git/HEAD`
3. 更新 `.git/refs/heads/test_branch`
4. 按该分支的 tree 重写工作区文件
5. 重写 `.git/index`
