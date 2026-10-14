# Linux 公网部署与恢复

以下命令供管理员在选定的 Linux x86_64 主机上运行。账号服务不需要 hbbs/hbbr 的数据库目录或私钥。登录后连接还需要 hbbs 的 [独立握手兼容模块](../../rustdesk-server/docs/account-handshake.md)，hbbr 不需要修改。请同时查看 [验证记录](VALIDATION.md) 中的实测范围。

## 本次公网部署：原生 systemd

2026-10-14 已部署 `https://47.108.237.195`，客户端 API 地址不加 `/api`。原有 ID/中继地址仍为 `47.108.237.195:22116`、`47.108.237.195:22117`，Key 保持原值。由于服务器没有 Docker，本次使用单个 API 进程和 Caddy 原生进程，减少额外运行组件；下文 Compose 仍作为另一种安装方式。

| 项目 | 已部署位置 |
|---|---|
| API / Caddy 二进制 | `/opt/rustdesk-account-server/bin/` |
| API 配置 | `/etc/rustdesk-account/account.toml` |
| Caddy 配置 / 地址参数 | `/etc/rustdesk-account/Caddyfile`、`caddy.env` |
| SQLite 数据 | `/var/lib/rustdesk-account/accounts.sqlite3` |
| 独立主密钥 | `/etc/rustdesk-account/secrets/master.key` |
| Caddy 证书数据 | `/var/lib/rustdesk-account-caddy/data/` |
| systemd 单元 | `rustdesk-account.service`、`rustdesk-account-caddy.service` |

可复用配置见 `deploy/systemd/`。API 以 `rustdesk-account` 用户运行，只监听 `127.0.0.1:21114`；只信任 `127.0.0.1/32` 代理。Caddy 以 `rustdesk-account-caddy` 用户运行，只有低端口绑定能力，覆盖转发 IP 头。两个服务启用只读系统目录和指定可写目录，并已设置开机启动。主密钥和数据库为 0600，配置无明文账号密码。

公网仅新增 80/443：80 用于 ACME 验证及 HTTPS 跳转，443 提供 API。无域名时 Caddy 2.11.7 使用 Let's Encrypt 的 `shortlived` ACME profile 申请受信任 IP 证书；`default_sni` 兼容省略 IP SNI 的客户端。`caddy.env` 中的 `ACCOUNT_ADDRESS` 可以改为自己的公网 IP，不能使用示例保留地址。短期证书依赖 Caddy 持续自动续期，应保留证书目录和 ACME 入站路径。参见 [Let's Encrypt IP 证书说明](https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability) 和 [Caddy 自动 HTTPS](https://caddyserver.com/docs/automatic-https)。

管理入口仍是 SSH，本次没有开放网页后台。以下函数只缩短命令，不保存密码：

```sh
ras() { sudo -u rustdesk-account /opt/rustdesk-account-server/bin/rustdesk-account-server --config /etc/rustdesk-account/account.toml "$@"; }
ras user list
ras user create another-user
ras user password myname
ras session list --username myname
ras session revoke-all myname
ras device list
ras logs --limit 100
ras backup /var/lib/rustdesk-account/backup-NEW-TIMESTAMP.sqlite3
sudo systemctl status rustdesk-account.service rustdesk-account-caddy.service
```

密码命令需要交互终端；通过 SSH 使用 `ssh -t`，不要把密码写进参数、环境文件或脚本。备份目标必须是新文件，数据库和主密钥分开备份。原生恢复也遵循下文“离线恢复”：用 `systemctl stop rustdesk-account` 停止 API，恢复到新数据库路径，核验后更新配置，最后 `systemctl start rustdesk-account`。Caddy无需随数据库恢复重启。

本次 hbbs 使用 `rustdesk-native-p2p-signal.service`，工作目录仍为 `/home/admin/rustdesk-native-p2p-v1`，执行 `hbbs-account-20261014` 并设置 `HBBS_ACCOUNT_HANDSHAKE=Y`。原 `hbbs` 二进制保留。未改 21116 上的另一套 hbbs、两个 hbbr、客户端二进制和 P2P 协议；不涉及 hbbs 数据迁移。若需回滚，在 systemd 单元中把 ExecStart 文件改回原 `hbbs`、停用握手变量，执行 `daemon-reload` 和重启该单元。回滚会恢复已登录客户端握手失败的旧行为，须先退出账号再连接。

## 另一种方式：首次使用 Compose 部署

1. 为 API 准备独立域名，例如 `rustdesk-api.example.com`，DNS 指向本机。开放 TCP 80/443 用于 Caddy 的 HTTPS 和证书申请；API 21114 不映射到宿主机。现有 RustDesk ID/中继端口按原方案保留。若机器上已有 80/443 反向代理，应将此 API 接入现有 HTTPS 代理，并相应调整信任来源，不要同时抢占端口。
2. 复制项目到服务器，进入 `deploy/`。检查 `172.30.45.0/24` 与现有 Docker/局域网不冲突；如需修改，同时改 Compose 的网络和 `account.toml` 的 Caddy 精确地址。不能把 `trusted_proxies` 配成所有公网地址。
3. 建立配置和目录（下面 `sudo install` 只处理当前项目的新目录）：

```sh
cd /opt/rustdesk-account-server/deploy
cp .env.example .env
# 编辑 .env 的 ACCOUNT_DOMAIN 和 ACME_EMAIL；其中不需要密码或密钥。
sudo install -d -m 0700 -o 10001 -g 10001 data secrets caddy-data caddy-config
docker compose build account
docker compose config --quiet
docker compose run --rm --no-deps \
  --volume "$PWD/secrets:/secrets:rw" \
  account --config /etc/account/account.toml init
docker compose run --rm --no-deps \
  account --config /etc/account/account.toml user create myname --generate
docker compose up -d
```

`init` 是唯一创建主密钥的步骤；服务运行只读挂载密钥。新文件默认 0600，目录应为 0700，由 UID/GID 10001 拥有。不要使用 `chmod 777`。Compose 中 API 和 Caddy 均以非 root 用户运行、只读根文件系统、移除全部 capabilities、禁止新增权限，分别限制内存及 PID。数据和证书目录是独立的可写挂载。

上面的生成密码命令须在终端执行，并在显示后保存密码。初始化的空数据库可以复用预先准备的 32 字节主密钥；已有数据库绝不能通过 `init` 重建密钥。主密钥没有在配置里内嵌，也没有编进二进制或镜像。

4. 检查本机 `docker compose ps`、Caddy 日志和外部 `https://域名/healthz`。确认宿主机没有对外监听/发布 21114。API 容器只连接内部网络，Caddy 同时连接入口网络用于 ACME。默认 Caddy 不信任额外 CDN/LB 头；如果以后加 Cloudflare 或负载均衡，需要重新设计可信代理链，不能直接信任来访的 X-Forwarded-For。
5. 在原版客户端 ID/中继设置中保留原 ID、Relay、Key，将 **API 服务器** 填为 `https://域名`，不要包含 `/api`。登录新建账号，检查原生设备列表及地址簿。另一设备按同样配置登录后可见。集中首次登录时注意默认每账号每分钟 8 次的限制。

Caddy 使用 [自动 HTTPS](https://caddyserver.com/docs/automatic-https)，容器内部监听 8080/8443 映射到公网 80/443，因此不需要以 root 绑定低端口。发布前应完成公网证书、外部连通性和真实客户端验证。

## 管理与日志

```sh
docker compose exec account rustdesk-account-server --config /etc/account/account.toml user list
docker compose exec account rustdesk-account-server --config /etc/account/account.toml user password myname
docker compose exec account rustdesk-account-server --config /etc/account/account.toml session revoke-all myname
docker compose exec account rustdesk-account-server --config /etc/account/account.toml logs --limit 100
```

密码命令保留 TTY，不能加 `-T` 或把输出重定向到日志。其他只读命令可以使用 `exec -T`。管理员通过 SSH 进入服务器后执行 CLI，没有公网管理 API。API 进程的 stdout/stderr 只含启动状态和不带敏感上下文的故障提示。详细账号操作用 CLI 查数据库，客户端上报通过 `logs --client-reports` 单独查。

加密主密钥丢失时，现有连接凭据不可恢复；应用拒绝错误密钥而不是清空凭据。不要通过修改校验字段绕过错误。SQLite 数据库、WAL、备份都应只允许服务账号及管理员访问，密钥单独存储和备份。容器内存上限 128 MiB 只用于 API，Caddy、hbbs、hbbr 资源另外计算。

## 一致性备份

```sh
docker compose exec -T account rustdesk-account-server \
  --config /etc/account/account.toml backup /data/backup-2026-10-14.sqlite3
```

备份命令调用 SQLite Backup API，可在 API 运行时执行，目标必须不存在。不要仅复制运行中的 `.sqlite3` 而遗漏 WAL。将备份搬到受保护的独立存储；另外用受权限控制/加密的备份通道保存 `secrets/master.key`。本程序不会把密钥打包进数据库备份。备份包含账号密码哈希和仍有效的令牌哈希，整份备份仍属于敏感数据。

备份时记录程序版本、镜像 ID 和 `PRAGMA user_version`（当前为 1）。回滚可能复活备份时尚有效、之后才撤销的令牌；恢复后如怀疑泄露，应对各账号执行 `session revoke-all`，必要时重置密码和被控端远控密码。

## 离线恢复与升级回滚

恢复始终写入**新数据库路径**，不覆盖已有库或 WAL，避免错覆盖、半恢复以及运行时竞争。操作步骤：

1. 停止 API：`docker compose stop account`，停止其他修改数据库的管理命令。保留旧程序、旧数据库及其伴随文件。
2. 准备 `restore.toml`，复制原配置，把 `database` 改成尚不存在的 `/data/restored.sqlite3`，保持 `master_key` 指向备份对应的原密钥。
3. 运行同版本程序执行恢复（下面复用镜像，挂载新的恢复配置）：

```sh
docker compose run --rm --no-deps \
  --volume "$PWD/restore.toml:/etc/account/restore.toml:ro" \
  account --config /etc/account/restore.toml restore /data/backup-2026-10-14.sqlite3
```

4. 命令会验证密钥、模式版本、SQLite 完整性与外键，再使用 Backup API 写入新文件。先用 `user list` 验证新库；将正式 `account.toml` 的 `database` 指向已验证的新文件，再启动 API，并用隔离账号验证登录与凭据解密。旧库保留到确认恢复成功。

版本 0.1.0 的模式版本是 1；遇到未知/新版本库会拒绝启动，不擅自迁移或降级。未来升级若包含迁移，必须先用当前版本进行一致性备份，检查新版本迁移说明，再升级。回滚应同时恢复对应的旧程序/镜像与升级前数据库备份，不把旧程序直接接到新模式数据库。密钥保持与备份匹配。

## 镜像与二进制

```sh
docker build --pull --tag rustdesk-account-server:0.1.0 .
docker build --target binary --output type=local,dest=dist .
docker save --output dist/rustdesk-account-server-0.1.0-image.tar rustdesk-account-server:0.1.0
```

镜像构建在 Debian bookworm 上完成，运行时仅需要 glibc 基础环境，不需要 OpenSSL 或外部 SQLite。`Cargo.lock` 固定 Rust 依赖。部署保留验证过的镜像 ID，升级基础镜像和依赖时重新跑兼容测试与安全检查。源码为独立目录，后续合并原 RustDesk 仓库上游无需修改此服务；客户端 API 变化则按协议核对和回归测试处理。

## 验证脚本

```sh
cargo test --release --locked
cargo build --release --locked --examples
python3 scripts/check-client-contract.py --local-client ../rustdesk --output test-results/client-source-contract.json
python3 scripts/measure.py --binary target/release/rustdesk-account-server \
  --fixture-binary target/release/examples/test_fixture \
  --seconds 180 --output test-results/resources-linux.json
```

资源脚本只绑定随机回环端口，在临时目录创建测试库，运行结束关闭服务并清理测试数据。使用 15 个已登记设备、每 3 秒心跳及地址簿写入/另一会话读取，检测日志不含测试凭据。基准为测量登录峰值把测试环境登录额度调到 60，生产配置保持默认额度。它不连接 hbbs，不进行真实远控，不会改已有客户端配置。
