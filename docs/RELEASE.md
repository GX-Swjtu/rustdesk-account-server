# 账号服务 CI 与发布

客户端联合发布直接编译锁定的本仓库提交，不要求这里先发布 Release，也不会替本仓库创建 tag 或 Release。需要单独验证时，在 Actions 运行 **Account server build (artifacts only)**；只生成五个平台和容器的构建附件。需要独立发布时继续使用下述 `v...` tag 流程；两种入口复用 `.github/workflows/build.yml`，都不会自动部署。也可以完全不运行独立构建/发布，只使用客户端仓库的联合 Release。

也可推送专用分支启动只构建流程：`git push origin HEAD:refs/heads/build/account/check-001`。这种方式不要求默认分支先存在手动工作流，不创建 tag 或 Release。

代码仓库：`GX-Swjtu/rustdesk-account-server`。账号服务独立于客户端版本发布，使用 `v0.1.0` 一类 tag，与 Cargo.toml 一致。

提交 main 或创建 PR 后运行格式检查、完整 Rust 测试、Clippy 和 Docker 构建检查。手动推送 `v*` tag 后，构建 Linux x64/ARM64、Windows x64、macOS Intel/Apple Silicon 原生包及 Linux x64 容器导出包。全部通过后生成带中英文防诈骗提醒和 SHA256SUMS 的 Release 草稿；维护者审核后手动发布。

原生包包含二进制、许可证、说明和 Git 中的部署示例。Linux 原生包在 Ubuntu 24.04 构建，较旧主机请使用容器包。容器包可以用 `docker load` 导入，不需要额外容器仓库凭据。

发行包内的 `deploy/compose.yaml` 在打包时写入本次镜像版本，并移除源码构建配置；账号服务设为 `pull_policy: never`，只使用已导入的本地镜像。源码仓库的 Compose 仍用于本地构建，两种配置不要混用。

在 Linux x86_64 主机使用预构建容器时，同时下载同一 Release 的 Linux x86_64 原生包（取得部署示例）和容器归档。核对 `SHA256SUMS` 后执行，版本号替换为所下载的版本：

```sh
version=0.1.1
mkdir account-release
tar -xzf "rustdesk-account-server-${version}-x86_64-unknown-linux-gnu.tar.gz" -C account-release
docker load -i "rustdesk-account-server-${version}-docker-linux-x86_64.tar.gz"
cd account-release/deploy
cp .env.example .env
# 编辑 .env 的 ACCOUNT_DOMAIN 和 ACME_EMAIL。
docker compose config --images
```

确认输出包含 `rustdesk-account-server:<本次版本>` 后，按 [部署文档](DEPLOYMENT.md) 创建目录、初始化新数据库和账号并启动服务，**跳过 `docker compose build account`**。已有部署升级时先备份数据库和主密钥，再导入新镜像、修改现有 Compose 的账号服务镜像版本并重建该服务；不要重新执行 `init` 或覆盖现有配置、数据目录。这里的容器归档仅支持 Linux x86_64，ARM64 主机使用对应原生包或自行从源码构建。

发布脚本不读取、不打包本地数据库、主密钥、备份、真实 account.toml、deploy/.env 或测试结果。任何发布都不自动连接生产服务器，不重启服务，不迁移数据库。上线仍按 [部署文档](DEPLOYMENT.md) 先备份数据库和主密钥，再由管理员执行升级。

账号登录后的远控仍需要支持账号加密握手的 hbbs；本仓库不把客户端 release 作为服务端部署触发器。原生客户端、hbbs/hbbr 和账号 API 的版本与生命周期分别管理。
