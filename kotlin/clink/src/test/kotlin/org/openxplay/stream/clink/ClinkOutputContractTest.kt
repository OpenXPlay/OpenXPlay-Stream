// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.assertTrue

/** Runs core's output contract with a real authenticated socket peer. */
@Timeout(10)
class ClinkOutputContractTest : ProjectionOutputContract() {
    private var vehicle: SimulatedHeadUnit? = null
    private val ended = CountDownLatch(1)
    private val errors = CopyOnWriteArrayList<Throwable>()
    private var stimulations = 0

    override fun createOutput(): ProjectionOutput {
        val ports = freePorts()
        vehicle = SimulatedHeadUnit(ports)
        return ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth)),
            PinProvider { "012345" }, MemoryTrustStore(), ports = ports, listener = object : ClinkListener {
                override fun state(state: ClinkState) { if (state == ClinkState.ENDED) ended.countDown() }
                override fun error(error: Throwable) { errors += error }
            })
    }
    override fun videoFormat(): VideoFormat = BundledPlaceholderVideo().format
    override fun stimulate(output: ProjectionOutput) {
        stimulations++
        if (stimulations == 1) {
            checkNotNull(vehicle).await("contract peer PLAY and placeholder") {
                checkNotNull(vehicle).playing && checkNotNull(vehicle).accessUnits.isNotEmpty()
            }
        }
        // Also exercise callbacks after close; they must not revive the lifecycle or host.
        output.onSession(SessionState(SessionId(2), SessionPhase.ACTIVE),
            SessionConfig(mapOf(ScreenId.MAIN to ScreenConfig(VideoCodec.H264, 1280, 720, 30.0))))
        output.onVideoFormat(ScreenId.MAIN, null)
    }
    override fun awaitIdle() {
        if (stimulations > 1) assertTrue(ended.await(3, TimeUnit.SECONDS), "Output did not finish closing")
        checkNotNull(vehicle).assertHealthy()
        assertTrue(errors.isEmpty(), errors.joinToString())
        if (stimulations == 1) checkNotNull(vehicle).expectRelayClose()
    }
    @AfterEach
    fun closePeer() { vehicle?.close() }
}
