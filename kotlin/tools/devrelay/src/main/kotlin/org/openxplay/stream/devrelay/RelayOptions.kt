// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.devrelay

import java.nio.file.Files
import java.nio.file.Path
import org.openxplay.stream.replay.ReplayAudioOptions

data class RelayOptions(
    val clip: Path,
    val fps: Double = 30.0,
    val webPort: Int? = null,
    val webBind: String = "127.0.0.1",
    val webRoot: Path = repositoryRoot().resolve("web-receiver/dist").normalize(),
    val sourceUrl: String = "https://github.com/OpenXPlay/OpenXPlay-Stream",
    val tlsKeystore: Path? = null,
    val tlsPassword: String? = null,
    val clinkHeadUnit: String? = null,
    val clinkLocal: String? = null,
    val clinkPin: String? = null,
    val audio: ReplayAudioOptions = ReplayAudioOptions(),
) {
    init {
        listOfNotNull(audio.media, audio.voice).forEach { require(Files.isRegularFile(it)) { "Audio must be a regular file: $it" } }
        require(Files.isRegularFile(clip)) { "Clip must be a regular file: $clip" }
        require(fps.isFinite() && fps > 0 && fps <= 1_000_000 && 1_000_000_000.0 / fps < Long.MAX_VALUE.toDouble()) {
            "Invalid frame rate"
        }
        require(webPort == null || webPort in 0..65535) { "Invalid web port" }
        require((tlsKeystore == null) == (tlsPassword == null)) { "TLS requires both keystore and password" }
        require(tlsKeystore == null || webPort != null) { "TLS requires --web-port" }
        require((clinkHeadUnit == null) == (clinkLocal == null)) { "Clink requires both head-unit and local IPv4 addresses" }
        require(clinkHeadUnit == null || ipv4(clinkHeadUnit) && ipv4(clinkLocal!!)) { "Clink requires numeric IPv4 addresses" }
        require(clinkPin == null || clinkPin.isNotEmpty() && clinkPin.all { it in '0'..'9' }) { "Clink PIN must contain digits" }
        require(clinkPin == null || clinkHeadUnit != null) { "Clink PIN requires Clink addresses" }
    }

    companion object {
        const val USAGE: String = """Usage: devrelay --clip <annex-b.h264> [options]
  --fps <n>                  Frame rate (default 30)
  --web-port <port>          Enable web output (0 selects an ephemeral port)
  --web-bind <address>       Bind address (default 127.0.0.1)
  --web-root <dir>           Assets (default web-receiver/dist; relative to repository root in Gradle)
  --source-url <url>         Source URL (default https://github.com/OpenXPlay/OpenXPlay-Stream)
  --tls-keystore <p12>       PKCS12 keystore for HTTPS/WSS
  --tls-password <password>  Keystore and key password
  --clink-head-unit <ipv4>   Enable Clink with the head unit's IPv4 address
  --clink-local <ipv4>       Local IPv4 address on the same network
  --clink-pin <digits>       Pairing PIN (preserves leading zeros)
  --audio-media <aac>       AAC-LC 48 kHz stereo ADTS file
  --audio-voice <wav>       PCM S16LE mono/stereo WAV at 8–48 kHz
  --audio-voice-every <s>   Play one voice turn every s seconds
  --mic-rate <hz>          Request mono PCM at hz in 20 ms frames
  --mic-out <wav>          Record received microphone packets
  --help                    Print usage
"""

        fun parse(args: Array<String>, root: Path = repositoryRoot()): RelayOptions {
            val names = setOf("clip", "fps", "web-port", "web-bind", "web-root", "source-url",
                "tls-keystore", "tls-password", "clink-head-unit", "clink-local", "clink-pin", "audio-media", "audio-voice", "audio-voice-every", "mic-rate", "mic-out")
            val values = mutableMapOf<String, String>()
            var index = 0
            while (index < args.size) {
                val flag = args[index++]
                require(flag.startsWith("--") && flag.drop(2) in names) { "Unknown option: $flag" }
                val name = flag.drop(2)
                require(name !in values) { "Duplicate option: $flag" }
                require(index < args.size && !args[index].startsWith("--")) { "Missing value for $flag" }
                values[name] = args[index++]
            }
            fun path(value: String): Path = root.resolve(value).normalize().toAbsolutePath()
            return RelayOptions(
                clip = path(requireNotNull(values["clip"]) { "--clip is required" }),
                fps = values["fps"]?.toDouble() ?: 30.0,
                webPort = values["web-port"]?.toInt(),
                webBind = values["web-bind"] ?: "127.0.0.1",
                webRoot = path(values["web-root"] ?: "web-receiver/dist"),
                sourceUrl = values["source-url"] ?: "https://github.com/OpenXPlay/OpenXPlay-Stream",
                tlsKeystore = values["tls-keystore"]?.let(::path), tlsPassword = values["tls-password"],
                audio = ReplayAudioOptions(values["audio-media"]?.let(::path), values["audio-voice"]?.let(::path),
                    values["audio-voice-every"]?.toDouble(), values["mic-rate"]?.toInt(), values["mic-out"]?.let(::path)),
                clinkHeadUnit = values["clink-head-unit"], clinkLocal = values["clink-local"], clinkPin = values["clink-pin"],
            )
        }
    }
}

internal fun repositoryRoot(): Path = Path.of(System.getProperty("devrelay.repositoryRoot", System.getProperty("user.dir")))
private fun ipv4(value: String): Boolean = value.split('.').let { parts ->
    parts.size == 4 && parts.all { it.isNotEmpty() && it.length <= 3 && it.all { c -> c in '0'..'9' } && it.toInt() in 0..255 }
}
