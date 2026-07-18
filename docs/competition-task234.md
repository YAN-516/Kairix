# Competition Task 2/3/4 Verification

These commands verify the official `vim`, `gcc`, and `rustc` applications from
the ext4 image. They should resolve from `/usr/bin`, not from custom wrappers.

Use a simple terminal mode to avoid pager/full-screen artifacts while testing:

```sh
export TERM=dumb
export PAGER=cat
export GIT_PAGER=cat
cd /tmp
```

## Task 2: vim

```sh
which vim
vim -h | head -n 3

rm -f hello.c
vim -Nu NONE -n -es hello.c <<'EOF'
call setline(1, ['#include <stdio.h>', '', 'int main(void) {', '    printf("Hello, World!\n");', '    return 0;', '}'])
wq
EOF
cat hello.c
```

Expected:

```text
/usr/bin/vim
VIM - Vi IMproved 9.1 ...
```

`cat hello.c` should show the saved C source.

## Task 3: gcc

The official GCC accepts both `--h` and `--help` as help-style invocations in
this image. `--help` is the canonical spelling.

```sh
which gcc
gcc --h || true
gcc --help | head -n 2

cat > helloworld.c <<'EOF'
#include <stdio.h>

int main(void) {
    printf("Hello, World!\n");
    return 0;
}
EOF

gcc helloworld.c && ./a.out
```

Expected:

```text
/usr/bin/gcc
Hello, World!
```

## Task 4: rustc

```sh
which rustc
rustc -h | head -n 3

cat > helloworld.rs <<'EOF'
fn main() {
    println!("Hello, World!");
}
EOF

rustc helloworld.rs && ./helloworld
```

Expected:

```text
/usr/bin/rustc
Hello, World!
```
