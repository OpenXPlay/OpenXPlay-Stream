<p align="center"><a href="README.md">English</a> · <b>简体中文</b></p>

# OpenXPlay-Stream

[OpenXPlay](https://github.com/OpenXPlay/OpenXPlay) 中继的输出端。Stream 从中继接收投屏会话（视频、音频、触控和按键），交给车机上已有的软件，车机无需安装任何软件：

| 输出 | 车机使用 | 触控 | 状态 |
| --- | --- | --- | --- |
| Web | 车机浏览器：H.264 经安全 WebSocket 传输，由 WebCodecs 解码 | 支持 | 开发中 |
| Carlink | 车机的 Carlink 应用，经 Wi-Fi 连接 | 支持 | 实验性 |
| Cast | 投屏接收端（DLNA） | 不支持 | 计划中 |

每个输出只与 OXP-Stream 模型交互，或在进程内，或通过 Stream 传输协议。输入与输出互不依赖，因此同一套输出可服务所有中继。CarPlay 输入由 [OpenXPlay Relay](https://github.com/OpenXPlay/OpenXPlay-Relay) 提供。

## 目录结构

模型和各输出按运行时分别实现，共三套；共享的协议测试向量让三套实现保持一致。

| 目录 | 语言 | 内容 | 使用方 |
| --- | --- | --- | --- |
| [`kotlin/`](kotlin) | Kotlin/JVM（Gradle） | `core`（会话模型和 hub）、`wire`（传输协议编解码）、`web-server`、`clink`（Carlink 输出）、`tools/`（回放、开发用中继、模拟车机） | Android 中继应用 |
| [`rust/`](rust) | Rust（Cargo） | `oxp-core`、`oxp-wire`、`oxp-web`、`oxp-clink`、`oxp-media`、`oxp-link-linux`（通过 BlueZ 和 NetworkManager 建立 Carlink Wi-Fi 连接）、`oxp-bin`（`oxp-relay` 输出进程） | Linux 中继 |
| [`web/`](web) | TypeScript（Vite） | `receiver/`（车机浏览器打开的页面）、`protocol/`、`shared/`、`probe/`（解码器能力探测） | 由两种中继提供 |
| [`spec/vectors/`](spec/vectors) | 数据 | 传输协议和 Web 协议的测试向量 | 三套实现的测试 |

## 构建与测试

| 部分 | 要求 | 命令 |
| --- | --- | --- |
| Kotlin | JDK 17 | `cd kotlin && ./gradlew test` |
| Rust | Rust 稳定版 | `cd rust && cargo test --workspace` |
| 面向 aarch64 Linux 的 Rust | `rustup target add aarch64-unknown-linux-musl`、`llvm-tools`、clang | `rust/scripts/build-aarch64.sh` |
| Web 接收端 | Node.js 20.19+ 或 22.12+ | `cd web && npm ci && npm test && npm run build`（输出在 `web/dist`） |

中继会打包 `web/dist`，因此请先构建 Web 接收端，再构建中继。

## 参与贡献

贡献须遵循[贡献者许可协议](./CLA.md)（协议文本为英文）。每个 pull request 上的 CLA Assistant 检查会记录你对协议的接受。

## 许可证

本项目采用 [GNU Affero General Public License v3.0 (AGPLv3)](./LICENSE) 授权，仅限第 3 版。

本项目适用依据 AGPLv3 第 7 条设定的附加条款。修改版本必须在相应的法律声明中，以及用户界面中任何显著的“关于”、法律、页脚或署名位置，保留署名声明 `Powered by OpenXPlay.`，并附上指向原项目的可见链接：<https://github.com/OpenXPlay/OpenXPlay-Stream>。详见 [NOTICE](./NOTICE)。

如果贵组织的政策不允许使用 AGPLv3 许可的软件，或你希望免于履行 AGPLv3 的开源义务，请联系：[support@openxplay.org](mailto:support@openxplay.org)

CarPlay 为 Apple Inc. 的商标。OpenXPlay 为独立项目，与 Apple 不存在隶属、认可或认证关系。
