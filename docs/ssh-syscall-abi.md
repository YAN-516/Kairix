# SSH syscall ABI

The SSH syscall ABI is a small transport-level interface layered on top of an
already-connected TCP socket. It currently validates the SSH identification
exchange and exposes a per-process SSH handle for later protocol work.

## Syscall numbers

| Number | Name | Arguments | Return |
| --- | --- | --- | --- |
| 1110 | `ssh_connect` | `fd`, `ident_ptr`, `ident_len` | SSH handle |
| 1111 | `ssh_write` | `ssh_id`, `buf`, `len` | bytes written |
| 1112 | `ssh_read` | `ssh_id`, `buf`, `len` | bytes read |
| 1113 | `ssh_close` | `ssh_id` | `0` |
| 1114 | `ssh_peer_ident` | `ssh_id`, `buf`, `len` | bytes copied, or full length when `len == 0` |
| 1115 | `ssh_auth_password` | `ssh_id`, `username_ptr`, `username_len`, `password_ptr`, `password_len` | `0` |

All failures are returned as negative Linux errno values in userspace.

## Contract

`ssh_connect` requires `fd` to be an established TCP socket owned by the calling
process. `ident_ptr/ident_len` is the client identification string without the
trailing CRLF, for example `SSH-2.0-kairix-sshtest_0.1`. The kernel validates
that it starts with `SSH-`, contains no NUL/CR/LF bytes, and fits the SSH
identification line limit. On success the kernel sends `ident + "\r\n"`, reads
the peer identification line, stores it, and returns a positive SSH handle.

`ssh_peer_ident` copies the stored peer identification string without CRLF. When
called with `len == 0`, it returns the full peer identification length without
touching `buf`. When the provided buffer is smaller than the string, the result
is truncated and the copied byte count is returned.

`ssh_read` and `ssh_write` operate on raw SSH transport bytes after the
identification exchange. They are placeholders for the later Sunset-backed SSH
packet/authentication/session state machine.

`ssh_close` invalidates the SSH handle. It does not close the underlying TCP
file descriptor; userspace still owns that fd and should close it separately.

`ssh_auth_password` advances the Sunset-backed client authentication state
machine using password authentication. It returns `0` once the server accepts
the credentials.

## Expected error cases

| Case | Error |
| --- | --- |
| Invalid fd or stale SSH handle | `EBADF` |
| fd is not a TCP socket | `ENOTSOCK` |
| TCP socket is not established | `ENOTCONN` |
| Invalid client identification string | `EINVAL` |
| User pointer cannot be translated | `EFAULT` |
| Peer does not send an SSH identification line in time | `ETIMEDOUT` |
| Authentication rejected or unavailable | `EACCES` |

## Test program

`sshtest` exercises the ABI from userspace:

```sh
sshtest --selftest
sshtest 10.0.2.2 22
sshtest 10.0.2.2 22 user password
```

The first command checks local ABI error paths. The second command connects to
an SSH server and verifies the peer identification path.
