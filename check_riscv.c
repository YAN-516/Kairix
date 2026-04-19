#define _GNU_SOURCE
#include <stdio.h>
#include <sys/utsname.h>
#include <sys/sysinfo.h>
#include <sys/vfs.h>
#include <sys/stat.h>
#include <time.h>
#include <termios.h>
#include <stddef.h>

char utsname_size[sizeof(struct utsname)];
char utsname_domain[offsetof(struct utsname, domainname)];
char sysinfo_size[sizeof(struct sysinfo)];
char sysinfo_memunit[offsetof(struct sysinfo, mem_unit)];
char statfs_size[sizeof(struct statfs)];
char stat_size[sizeof(struct stat)];
char stat_blocks[offsetof(struct stat, st_blocks)];
char stat_atim[offsetof(struct stat, st_atim)];
char timespec_size[sizeof(struct timespec)];
char termios_size[sizeof(struct termios)];

int main() { return 0; }
