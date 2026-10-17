// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.openxplay.stream.core.*
import kotlin.test.assertTrue

internal fun clipPath(): Path = Path.of(checkNotNull(System.getProperty("replay.testClip")))
internal fun clipBytes(): ByteArray = Files.readAllBytes(clipPath())
internal fun configuration(): SessionConfig = SessionConfig(
    mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H264, 640, 360, 30.0)),
)
internal val origin: Origin = Origin("test", "client")
internal fun CountDownLatch.awaitResult(): Unit = assertTrue(await(5, TimeUnit.SECONDS), "Replay timed out")

internal open class TestSink : InputSink {
    override fun session(state: SessionState): Unit = Unit
    override fun videoFormat(screen: ScreenId, format: VideoFormat?): Unit = Unit
    override fun videoFrame(frame: VideoFrame): Unit = Unit
    override fun audioFormat(stream: Int, format: AudioFormat?): Unit = Unit
    override fun audioPacket(packet: AudioPacket): Unit = Unit
    override fun microphone(stream: Int, request: MicRequest?): Unit = Unit
}
