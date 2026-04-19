#define _GNU_SOURCE
#include <sys/stat.h>
#include <stdio.h>

int main() {
    char buf1[((sizeof(struct stat) == 128) ? 1 : -1)];
    char buf2[((sizeof(struct stat64) == 128) ? 1 : -1)];
    return 0;
}
