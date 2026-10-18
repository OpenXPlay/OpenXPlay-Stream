<p align="center"><b>English</b> · <a href="README-zh.md">简体中文</a></p>

# OpenXPlay-Stream

The output side of an [OpenXPlay](https://github.com/OpenXPlay/OpenXPlay) relay. Stream takes a projection session (video, audio, touch and keys) from the relay and delivers it to software a head unit already has, so nothing is installed on the head unit:

| Output | Head unit uses | Touch | Status |
| --- | --- | --- | --- |
| Web | Its browser: H.264 over a secure WebSocket, decoded with WebCodecs | Yes | In development |
| Carlink | Its Carlink app, over Wi-Fi | Yes | Experimental |
| Cast | Screen-casting receivers (DLNA) | No | Planned |

Every output talks only to the OXP-Stream model, either in process or over the Stream wire protocol. Inputs and outputs never depend on each other, so the same outputs serve every relay. The [OpenXPlay Relay](https://github.com/OpenXPlay/OpenXPlay-Relay) provides the CarPlay input.

## Layout

The model and the outputs are implemented three times, one per runtime. Shared protocol test vectors keep the three implementations in step.

| Directory | Language | Contents | Used by |
| --- | --- | --- | --- |
| [`kotlin/`](kotlin) | Kotlin/JVM (Gradle) | `core` (session model and hub), `wire` (wire codec), `web-server`, `clink` (Carlink output), `tools/` (replay, a development relay, a simulated head unit) | The Android relay app |
| [`rust/`](rust) | Rust (Cargo) | `oxp-core`, `oxp-wire`, `oxp-web`, `oxp-clink`, `oxp-media`, `oxp-link-linux` (Carlink Wi-Fi link through BlueZ and NetworkManager), `oxp-bin` (the `oxp-relay` output process) | The Linux relay |
| [`web/`](web) | TypeScript (Vite) | `receiver/` (the page the head-unit browser opens), `protocol/`, `shared/`, `probe/` (a decoder capability probe) | Served by both relays |
| [`spec/vectors/`](spec/vectors) | Data | Wire and web protocol test vectors | Tests in all three implementations |

## Build and test

| Part | Requirements | Commands |
| --- | --- | --- |
| Kotlin | JDK 17 | `cd kotlin && ./gradlew test` |
| Rust | Rust stable | `cd rust && cargo test --workspace` |
| Rust for aarch64 Linux | `rustup target add aarch64-unknown-linux-musl`, `llvm-tools`, clang | `rust/scripts/build-aarch64.sh` |
| Web receivers | Node.js 20.19+ or 22.12+ | `npm ci && npm test && npm run build` in `kotlin/web-receiver` and in `rust/web-receiver` (output in each `dist`) |

The Android relay packages `kotlin/web-receiver/dist` and the Linux relay `rust/web-receiver/dist`; build that receiver before building the relay. The two receivers are separate code.

## Contributing

Contributions are accepted under the [Contributor License Agreement](./CLA.md). The CLA Assistant
check on each pull request records your acceptance.

## License

This project is licensed under the [GNU Affero General Public License v3.0 (AGPLv3)](./LICENSE),
version 3 only.

Additional terms under AGPLv3 Section 7 apply. Modified versions must preserve the attribution
notice `Powered by OpenXPlay.` in the appropriate legal notices and in any prominent about, legal,
footer, or attribution location presented by the user interface, together with a visible link to
the original project: <https://github.com/OpenXPlay/OpenXPlay-Stream>. See [NOTICE](./NOTICE).

If your organization's policies do not permit the use of AGPLv3-licensed software, or if you wish
to avoid the open-source obligations of AGPLv3, please contact us at:
[support@openxplay.org](mailto:support@openxplay.org)

CarPlay is a trademark of Apple Inc. OpenXPlay is an independent project and is not affiliated with,
endorsed or certified by Apple.
