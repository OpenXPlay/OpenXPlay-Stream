// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.util.concurrent.CountDownLatch
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import kotlin.test.*

public class ReplayHubTest {
    @Test
    public fun hubFansOutClipToRecordingFixtureAndReplaysLateFormat(): Unit {
        val input = AnnexBFileInput(clipPath(), frameRate = 100.0)
        val ready = CountDownLatch(3)
        val output = object : RecordingOutput("first") {
            override fun onVideoFrame(frame: VideoFrame) { super.onVideoFrame(frame); ready.countDown() }
        }
        val late = RecordingOutput("late")
        val hub = ProjectionHub(input)
        try {
            hub.addOutput(output)
            hub.start()
            ready.awaitResult()
            hub.addOutput(late)
            assertEquals(SessionPhase.ACTIVE, output.sessions.first().first.phase)
            assertEquals("avc1.42C01F", output.videoFormats.first().second?.codecString)
            assertEquals(640 to 360, output.videoFormats.first().second!!.let { it.width to it.height })
            assertEquals(listOf("session", "video-format", "video"), output.events.take(3))
            assertTrue(output.frames.first().keyFrame)
            assertEquals(output.sessions.first().first.id, output.frames.first().session)
            assertSame(output.videoFormats.first().second, late.videoFormats.first().second)
            assertEquals(SessionPhase.ACTIVE, late.sessions.first().first.phase)
            hub.stop()
            assertTrue(output.closed && late.closed)
            val count = output.frames.size
            Thread.sleep(50)
            assertEquals(count, output.frames.size)
        } finally { hub.stop() }
    }
}
