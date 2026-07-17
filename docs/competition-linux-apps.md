# Competition Linux App Compatibility

The competition rule means the final filesystem may be self-made, but the
Linux application files for `git`, `vim`, `gcc`, and `rustc` must match the
official filesystem image / `soft-info.txt` requirement. In practice, the
program reached by normal shell lookup must be the official Linux binary, not
our custom user program or a self-built replacement.

Expected versions from the official LoongArch64 image currently used for
comparison:

```text
git   2.47.3
gcc   14.2.0 (Alpine 14.2.0)
rustc 1.83.0-r0
vim   9.1, patches 1-1105
```

## Required project policy

Use the official ext4 image as the base root filesystem when possible. Patch
only our kernel/test helper files into it.

Keep `/usr/bin` before `/bin` in `PATH`, so `git`, `vim`, `gcc`, and `rustc`
resolve to the official files under `/usr/bin`.

Do not install custom compatibility programs as `/bin/git`, `/bin/gcc`, or
`/bin/rustc`. Custom implementations are kept under `k*` names, such as
`kgit`, `kgcc`, and `krustc`, for manual debugging only.

The current embedded GCC payload is Alpine GCC 14.2.0 and matches the expected
major version. The current embedded Rust payload is Alpine Rust 1.87.0-r1 and
does not match the expected `rustc 1.83.0-r0`; for final submission, use the
official image's Rust files or replace the payload with exact 1.83.0-r0 files.

## Verification

Inside the guest:

```sh
which git
git -v
which gcc
gcc -v
which rustc
rustc --version
which vim
vim --version
```

The `which` output for these four commands should point to `/usr/bin/...`.

On the host, compare the final image against the official image with
`sha256sum` or `cmp` after mounting both images. At minimum compare:

```text
/usr/bin/git
/usr/bin/vim
/usr/bin/gcc
/usr/bin/rustc
```

If the official files have dynamic dependencies, keep their matching libraries
from the official image as well.
