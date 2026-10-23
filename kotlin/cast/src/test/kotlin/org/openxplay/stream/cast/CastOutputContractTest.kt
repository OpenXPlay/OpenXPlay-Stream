// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.assertTrue

@Timeout(10)
class CastOutputContractTest : ProjectionOutputContract() {
    private val fixture = FakeRenderer()
    private val threads = TrackedThreads()
    private val ended = CountDownLatch(1)
    private var stimulations = 0
    override fun createOutput(): ProjectionOutput = fixture.output(object : CastListener {
        override fun state(state: CastState, renderer: CastRenderer?) { if (state == CastState.ENDED) ended.countDown() }
    }, threads)
    override fun videoFormat(): VideoFormat = org.openxplay.stream.cast.videoFormat()
    override fun stimulate(output: ProjectionOutput) {
        stimulations++
        output.onSession(SessionState(SessionId(2), SessionPhase.ACTIVE), SESSION_CONFIG)
        output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame(session = 2))
    }
    override fun awaitIdle() {
        if (stimulations == 1) await("Contract player not started") { fixture.packets.isNotEmpty() }
        else { assertTrue(ended.await(3, TimeUnit.SECONDS)); threads.awaitEnded() }
    }
    @AfterEach fun closePeer() { fixture.close() }
}
