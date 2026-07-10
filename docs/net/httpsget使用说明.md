# httpsget 使用说明

`httpsget` 是用户态 HTTPS 调试程序，用于通过 TCP + TLS 发起简单 HTTP 请求，支持域名解析、指定 IP、请求头、HEAD 请求、预览长度控制和状态码检查。

## 基本用法

```sh
httpsget [options] <host-or-url> [path] [dns-ip]
```

兼容旧用法：

```sh
httpsget example.com /
httpsget example.com / 10.0.2.3
httpsget https://example.com/index.html
```

不传参数时会回退到：

```sh
httpsget example.com /
```

## 常用示例

获取首页并预览前 1024 字节响应：

```sh
httpsget example.com /
```

只请求并显示响应头：

```sh
httpsget https://example.com/ -I
```

指定 DNS 服务器：

```sh
httpsget -d 10.0.2.3 example.com /
```

跳过 DNS，直接连接指定 IPv4，但仍使用域名作为 Host 和 TLS SNI：

```sh
httpsget --ip 93.184.216.34 --host example.com example.com /
```

添加自定义请求头：

```sh
httpsget example.com / -H "Accept: text/html" -H "Cache-Control: no-cache"
```

指定端口和预览长度：

```sh
httpsget --port 8443 --max-preview 4096 example.com /
```

HTTP 状态码大于等于 400 时返回错误：

```sh
httpsget --fail example.com /not-found
```

打印实际发送的请求报文，便于调试：

```sh
httpsget -v example.com /
```

## 参数说明

| 参数 | 说明 |
| --- | --- |
| `-h`, `--help` | 显示帮助信息。 |
| `-X`, `--method METHOD` | 指定请求方法，默认 `GET`。例如 `GET`、`HEAD`、`OPTIONS`。 |
| `-H`, `--header "K: V"` | 添加请求头，最多 8 个。必须包含冒号，不能包含换行。 |
| `-I`, `--head`, `--headers-only` | 使用 `HEAD` 方法并只打印响应头。如果已通过 `-X` 指定方法，则保留指定方法，但读取到头部结束后停止。 |
| `-d`, `--dns IP` | 指定 DNS 服务器 IPv4，默认 `10.0.2.3`。 |
| `--ip IP` | 跳过 DNS，直接连接指定 IPv4。 |
| `-p`, `--port PORT` | 指定 TCP 端口，默认 `443`。URL 中的 `host:port` 也会被识别，显式 `--port` 优先。 |
| `--path PATH` | 覆盖 URL 或位置参数中的路径。 |
| `--host HOST` | 覆盖 HTTP `Host` 头，同时作为默认 TLS SNI。常用于 `--ip` 直连测试。 |
| `--sni HOST` | 仅覆盖 TLS SNI，不改变 HTTP `Host` 头。 |
| `-n`, `--max-preview N` | 最多打印 N 字节响应内容，默认 `1024`。设置为 `0` 时不打印响应体，只输出统计信息。 |
| `--http10` | 使用 `HTTP/1.0`。 |
| `--http11` | 使用 `HTTP/1.1`，默认值。 |
| `-q`, `--quiet` | 静默模式，抑制普通输出。错误仍会打印。 |
| `-v`, `--verbose` | 发送前打印请求报文。 |
| `-f`, `--fail` | 如果解析到 HTTP 状态码 `>= 400`，程序返回错误。 |

## 输出说明

普通请求会打印：

- DNS 解析和连接目标；
- 响应预览；
- 总读取字节数；
- HTTP 状态码；
- 耗时毫秒数。

`-I` 模式只打印响应头，遇到 `\r\n\r\n` 后停止读取。

## 限制

- 只支持 IPv4 和 DNS A 记录，不支持 IPv6。
- 不支持请求体上传，`POST` 等方法只能发送空 body。
- 请求缓冲区固定为 2048 字节；路径和自定义请求头过长会报 `request too long`。
- 单个命令行参数最长 512 字节。
- TLS 能力依赖内核侧 `tls_connect/tls_read/tls_write` 系统调用实现。
