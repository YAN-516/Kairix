#include <sys/stat.h>
int main() {
    char buf[((sizeof(struct stat) == 999) ? 1 : -1)];
    return 0;
}
