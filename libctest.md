* musl static
[×] argv
[×] basename
[×] clocale_mbfuncs
[×] clock_gettime
[×] dirname
[×] env
[×] fdopen
[×] fnmatch
[×] fscanf
[×] fwscanf
[×] iconv_open
[×] inet_pton
[×] mbc
[×] memstream
[ ] pthread_cancel-points
[ ] pthread_cancel
[ ] pthread_cond
[ ] pthread_tsd
[×] qsort
[×] random
[×] search_hsearch
[×] search_insque
[×] search_lsearch
[×] search_tsearch
[×] setjmp
[×] snprintf
[ ] socket
[×] sscanf
[×] sscanf_long
[×] stat
[×] strftime
[×] string
[×] string_memcpy
[×] string_memmem
[×] string_memset
[×] string_strchr
[×] string_strcspn
[×] string_strstr
[×] strptime
[×] strtod
[×] strtod_simple
[×] strtof
[×] strtol
[×] strtold
[×] swprintf
[×] tgmath
[×] time
[×] tls_align
[×] udiv
[×] ungetc
[×] utime
[×] wcsstr
[×] wcstol
src/regression/
[×] daemon-failure
[×] dn_expand-empty
[×] dn_expand-ptr-0
[×] fflush-exit
[×] fgets-eof
[×] fgetwc-buffering
[×] fpclassify-invalid-ld80
[×] ftello-unflushed-append
[×] getpwnam_r-crash
[×] getpwnam_r-errno
[×] iconv-roundtrips
[×] inet_ntop-v4mapped
[×] inet_pton-empty-last-field
[×] iswspace-null
[×] lrand48-signextend
[×] lseek-large
[×] malloc-0
[×] mbsrtowcs-overflow
[×] memmem-oob-read
[×] memmem-oob
[×] mkdtemp-failure
[×] mkstemp-failure
[×] printf-1e9-oob
[×] printf-fmt-g-round
[×] printf-fmt-g-zeros
[×] printf-fmt-n
[ ] pthread-robust-detach
[ ] pthread_cancel-sem_wait
[ ] pthread_cond-smasher
[ ] pthread_condattr_setclock
[ ] pthread_exit-cancel
[ ] pthread_once-deadlock
[ ] pthread_rwlock-ebusy
[×] putenv-doublefree
[×] regex-backref-0
[×] regex-bracket-icase
[×] regex-ere-backref
[×] regex-escaped-high-byte
[×] regex-negated-range
[×] regexec-nosub
[×] rewind-clear-error
[ ] rlimit-open-files
[×] scanf-bytes-consumed
[×] scanf-match-literal-eof
[×] scanf-nullbyte-char
[×] setvbuf-unget
[×] sigprocmask-internal
[×] sscanf-eof
[×] statvfs
[×] strverscmp
[ ] syscall-sign-extend
[×] uselocale-0
[×] wcsncpy-read-overflow
[×] wcsstr-false-negative

11个pthread，1个socket
* musl dynamic
[×] argv
[×] basename
[×] clocale_mbfuncs
[×] clock_gettime
[×] dirname
[×] env
[×] fdopen
[×] fnmatch
[×] fscanf
[×] fwscanf
[×] iconv_open
[×] inet_pton
[×] mbc
[×] memstream
[ ] pthread_cancel-points
[ ] pthread_cancel
[ ] pthread_cond
[ ] pthread_tsd
[×] qsort
[×] random
[×] search_hsearch
[×] search_insque
[×] search_lsearch
[×] search_tsearch
[×] setjmp
[×] snprintf
[ ] socket
[×] sscanf
[×] sscanf_long
[×] stat
[×] strftime
[×] string
[×] string_memcpy
[×] string_memmem
[×] string_memset
[×] string_strchr
[×] string_strcspn
[×] string_strstr
[×] strptime
[×] strtod
[×] strtod_simple
[×] strtof
[×] strtol
[×] strtold
[×] swprintf
[×] tgmath
[×] time
[] tls_align
[×] udiv
[×] ungetc
[×] utime
[×] wcsstr
[×] wcstol
src/regression/
[×] daemon-failure
[×] dn_expand-empty
[×] dn_expand-ptr-0
[×] fflush-exit
[×] fgets-eof
[×] fgetwc-buffering
[×] fpclassify-invalid-ld80
[×] ftello-unflushed-append
[×] getpwnam_r-crash
[×] getpwnam_r-errno
[×] iconv-roundtrips
[×] inet_ntop-v4mapped
[×] inet_pton-empty-last-field
[×] iswspace-null
[×] lrand48-signextend
[×] lseek-large
[×] malloc-0
[×] mbsrtowcs-overflow
[×] memmem-oob-read
[×] memmem-oob
[×] mkdtemp-failure
[×] mkstemp-failure
[×] printf-1e9-oob
[×] printf-fmt-g-round
[×] printf-fmt-g-zeros
[×] printf-fmt-n
[ ] pthread-robust-detach
[ ] pthread_cancel-sem_wait
[ ] pthread_cond-smasher
[ ] pthread_condattr_setclock
[ ] pthread_exit-cancel
[ ] pthread_once-deadlock
[ ] pthread_rwlock-ebusy
[×] putenv-doublefree
[×] regex-backref-0
[×] regex-bracket-icase
[×] regex-ere-backref
[×] regex-escaped-high-byte
[×] regex-negated-range
[×] regexec-nosub
[×] rewind-clear-error
[ ] rlimit-open-files
[×] scanf-bytes-consumed
[×] scanf-match-literal-eof
[×] scanf-nullbyte-char
[×] setvbuf-unget
[×] sigprocmask-internal
[×] sscanf-eof
[×] statvfs
[×] strverscmp
[ ] syscall-sign-extend
[×] uselocale-0
[×] wcsncpy-read-overflow
[×] wcsstr-false-negative