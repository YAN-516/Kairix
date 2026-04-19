#define _TIME_BITS 64
#define _FILE_OFFSET_BITS 64
#include <sys/stat.h>
#include <stdio.h>

int main() {
    char buf1[((sizeof(struct stat) == 128) ? 1 : -1)];
    return 0;
}
