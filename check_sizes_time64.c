#define _TIME_BITS 64
#define _FILE_OFFSET_BITS 64
#include <sys/stat.h>
#include <stdio.h>

int main() {
    printf("Time64: sizeof(struct stat) = %zu\n", sizeof(struct stat));
    return 0;
}
