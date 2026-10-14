# 账号服务 CI 与发布

代码仓库：`GX-Swjtu/rustdesk-account-server`。账号服务独立于客户端版本发布，使用 `v0.1.0` 一类 tag，与 Cargo.toml 一致。

提交 main 或创建 PR 后运行格式检查、完整 Rust 测试、Clippy 和 Docker 构建检查。手动推送 `v*` tag 后，构建 Linux x64/ARM64、Windows x64、macOS Intel/Apple Silicon 原生包及 Linux x64 容器导出包。全部通过后生成带中英文防诈骗提醒和 SHA256SUMS 的 Release 草稿；维护者审核后手动发布。

原生包包含二进制、许可证、说明和 Git 中的部署示例。Linux 原生包在 Ubuntu 24.04 构建，较旧主机请使用容器包。容器包可以用 `docker load` 导入，不需要额外容器仓库凭据。

发布脚本不读取、不打包本地数据库、主密钥、备份、真实 account.toml、deploy/.env 或测试结果。任何发布都不自动连接生产服务器，不重启服务，不迁移数据库。上线仍按 [部署文档](DEPLOYMENT.md) 先备份数据库和主密钥，再由管理员执行升级。

账号登录后的远控仍需要支持账号加密握手的 hbbs；本仓库不把客户端 release 作为服务端部署触发器。原生客户端、hbbs/hbbr 和账号 API 的版本与生命周期分别管理。
