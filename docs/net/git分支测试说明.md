# Git 分支功能测试说明

本文档用于测试当前用户态 `git` 的分支相关功能，包括本地分支、远端分支、创建分支、切换分支以及 tracking 配置。

## 前置条件

已经有一个带两个远端分支的仓库，例如：

```sh
git@github.com:StarryNight0630/OS_Competition_Net_Test.git
```

假设远端包含：

- `main`
- `test_branch`

并且两个分支的 `README.md` 内容不同。

## 重新克隆测试仓库

建议放在 `/tmp` 下测试，避免污染 `/musl`：

```sh
rm -rf /tmp/test_net
git clone git@github.com:StarryNight0630/OS_Competition_Net_Test.git /tmp/test_net --key /musl/id_ed25519
cd /tmp/test_net
```

## 查看分支

查看本地分支：

```sh
git branch
```

预期类似：

```text
* main
```

查看远端分支：

```sh
git branch -r
```

预期类似：

```text
  origin/main
  origin/test_branch
```

查看全部分支：

```sh
git branch -a
```

预期类似：

```text
* main
  remotes/origin/main
  remotes/origin/test_branch
```

查看带短 commit 的分支：

```sh
git branch -vv
git branch -a -vv
```

## 从远端分支切换

切换到远端存在但本地还没有的分支：

```sh
git checkout test_branch
cat README.md
```

预期：

```text
# Test Branch
```

再次查看分支：

```sh
git branch -a
```

预期本地已创建 `test_branch`：

```text
  main
* test_branch
  remotes/origin/main
  remotes/origin/test_branch
```

切回 `main`：

```sh
git checkout main
cat README.md
```

预期看到 `main` 分支的 README 内容。

## 创建本地分支

从当前 HEAD 创建并切换：

```sh
git checkout -b local_test
git branch
```

预期：

```text
* local_test
  main
  test_branch
```

从远端分支创建本地分支：

```sh
git checkout main
git checkout -b from_remote origin/test_branch
cat README.md
```

预期 README 内容来自 `origin/test_branch`。

也可以只创建不切换：

```sh
git branch only_create origin/test_branch
git branch
```

## 检查 tracking 配置

当从 `origin/test_branch` 创建本地分支时，`.git/config` 会追加类似内容：

```ini
[branch "test_branch"]
        remote = origin
        merge = refs/heads/test_branch
```

检查命令：

```sh
cat .git/config
```

## 删除本地分支

不能删除当前分支，先切到其他分支：

```sh
git checkout main
git branch -d local_test
git branch -D only_create
git branch
```

## 完整快速测试

```sh
cd /tmp/test_net
git branch -a
git checkout test_branch
cat README.md
git checkout main
cat README.md
git checkout -b local_test
git branch
git checkout main
git branch -d local_test
git checkout -b from_remote origin/test_branch
cat README.md
cat .git/config
```

如果 `git checkout test_branch` 后 `cat README.md` 仍显示 `main` 内容，请确认是在仓库根目录执行 checkout：

```sh
cd /tmp/test_net
git checkout test_branch
```

