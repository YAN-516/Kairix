#define _GNU_SOURCE
#include <sys/stat.h>
#include <stdio.h>

int main() {
    printf("Default: sizeof(struct stat) = %zu\n", sizeof(struct stat));
    printf("Default: sizeof(struct stat64) = %zu\n", sizeof(struct stat64));
    return 0;
}
