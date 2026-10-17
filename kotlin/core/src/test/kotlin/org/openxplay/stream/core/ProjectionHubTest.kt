// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class ProjectionHubTest {
    private val main = ScreenId.MAIN
    private fun capabilities(
        width: Int = 800,
        hardware: Boolean = true,
        latency: LatencyClass = LatencyClass.INTERACTIVE,
        displays: Map<ScreenId, DisplayInfo> = mapOf(main to DisplayInfo(width, 480, 160, 30.0)),
    ): OutputCapabilities = OutputCapabilities(latency, listOf(DecoderSupport("hvc1.1.6.L93", hardware)), displays, 10, KeyCode.entries.toSet(), emptySet(), true)
    private fun frame(session: Long = 1): VideoFrame = VideoFrame(main, SessionId(session), byteArrayOf(1, 2), 0, true, 0)
    private fun active(hub: ProjectionHub, id: Long = 1) { hub.session(SessionState(SessionId(id), SessionPhase.ACTIVE)) }

    @Test
    fun lateOpenReplaysOnlyCurrentStateFormatsAndMicrophonesInOrder() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        hub.start()
        active(hub)
        val format = VideoFormat.create(main, VideoCodec.H264, vector("avc-baseline-640x480"))
        val audio = AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2)
        val mic = MicRequest(0, AudioCodec.PCM_S16LE, 48000, 1, 20)
        hub.videoFormat(main, format)
        hub.audioFormat(0, audio)
        hub.microphone(0, mic)
        hub.audioFormat(1, audio.let { AudioFormat(1, it.role, it.codec, it.sampleRate, it.channels) })
        hub.audioFormat(1, null)
        val output = RecordingOutput()
        hub.addOutput(output)
        assertEquals(listOf("session", "video-format", "audio-format", "microphone"), output.events)
        assertSame(format, output.videoFormats.single().second)
        assertSame(audio, output.audioFormats.single().second)
        assertSame(mic, output.micRequests.single().second)
        hub.stop()
    }

    @Test
    fun newerSessionsClearReplayAndFenceOldMediaAndSessionCallbacks() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        hub.start()
        active(hub, 10)
        hub.microphone(0, MicRequest(0, AudioCodec.OPUS, 48000, 1, 20))
        active(hub, 11)
        val output = RecordingOutput()
        hub.addOutput(output)
        hub.videoFrame(frame(10))
        hub.audioPacket(AudioPacket(0, SessionId(10), byteArrayOf(1), 0))
        active(hub, 9)
        val current = frame(11)
        hub.videoFrame(current)
        val packet = AudioPacket(0, SessionId(11), byteArrayOf(2), 0)
        hub.audioPacket(packet)
        assertSame(current, output.frames.single())
        assertSame(packet, output.packets.single())
        assertEquals(11L, output.sessions.last().first.id.value)
        assertTrue(output.micRequests.isEmpty())
        hub.stop()
    }

    @Test
    fun keyframesAreCoalescedPerScreenAndTimerRunsWithoutMedia() {
        val clock = ManualClock()
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = clock)
        hub.start()
        val a = RecordingOutput("a")
        val b = RecordingOutput("b")
        hub.addOutput(a); hub.addOutput(b)
        fun request(output: RecordingOutput, screen: ScreenId = main) {
            output.host!!.control(ControlEvent.KeyFrameRequest(screen, Origin(output.id, "client")))
        }
        request(a)
        clock.advanceNanos(100_000_000)
        request(b); request(a); request(b, ScreenId.CLUSTER)
        assertEquals(2, input.controls.size)
        clock.advanceNanos(149_999_999)
        assertEquals(2, input.controls.size)
        clock.advanceNanos(1)
        assertEquals(3, input.controls.size)
        clock.advanceNanos(500_000_000)
        assertEquals(3, input.controls.size)
        request(a)
        assertEquals(4, input.controls.size)
        request(b)
        hub.stop()
        clock.advanceNanos(1_000_000_000)
        assertEquals(4, input.controls.size)
    }

    @Test
    fun touchOwnershipTracksMultipleContactsAndKeysFollowLastTouch() {
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start()
        val a = RecordingOutput("a"); val b = RecordingOutput("b")
        hub.addOutput(a); hub.addOutput(b)
        val oa = Origin("a", "one"); val ob = Origin("b", "two")
        fun touch(output: RecordingOutput, origin: Origin, vararg contacts: TouchContact) {
            output.host!!.control(ControlEvent.Touch(main, contacts.toList(), origin))
        }
        val down = TouchContact(0, 0.5, 0.5, true)
        val keyB = ControlEvent.Key(KeyCode.HOME, true, ob)
        b.host!!.control(keyB)
        touch(a, oa, down, down.copy(id = 1))
        touch(a, oa, down.copy(pressed = false), down.copy(id = 1))
        touch(b, ob, down) // A still owns contact 1.
        b.host!!.control(keyB)
        assertEquals(3, input.controls.size)
        touch(a, oa)
        b.host!!.control(keyB) // Last touch is still A.
        assertEquals(4, input.controls.size)
        touch(b, ob, down)
        b.host!!.control(keyB)
        assertEquals(6, input.controls.size)
        a.host!!.control(ControlEvent.Key(KeyCode.HOME, true, ob)) // Forged output ID.
        assertEquals(6, input.controls.size)
        hub.stop()
    }

    @Test
    fun touchEventsAreCompleteSnapshotsSoOmittedContactsAreReleased() {
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start()
        val a = RecordingOutput("a"); val b = RecordingOutput("b")
        hub.addOutput(a); hub.addOutput(b)
        val oa = Origin("a", "one"); val ob = Origin("b", "two")
        val down = TouchContact(0, 0.5, 0.5, true)
        a.host!!.control(ControlEvent.Touch(main, listOf(down, down.copy(id = 1)), oa))
        a.host!!.control(ControlEvent.Touch(main, listOf(down.copy(id = 1, pressed = false)), oa))
        b.host!!.control(ControlEvent.Touch(main, listOf(down), ob)) // Contact 0 was omitted, so A holds nothing.
        assertEquals(3, input.controls.size)
        hub.stop()
    }

    @Test
    fun inputCapabilityUpdatesImmediatelyChangeControlAcceptanceAndRederiveConfig() {
        val initial = FakeInput().capabilities.copy(
            codecs = setOf(VideoCodec.H264), touch = false, keys = emptySet(), microphone = false,
        )
        val input = FakeInput(initial)
        val hub = ProjectionHub(input, clock = ManualClock())
        val output = RecordingOutput("primary", capabilities())
        hub.addOutput(output); hub.start(); active(hub)
        val origin = Origin(output.id, "client")
        hub.microphone(0, MicRequest(0, AudioCodec.PCM_S16LE, 16000, 1, 20))
        val events = listOf(
            ControlEvent.Touch(main, listOf(TouchContact(0, 0.5, 0.5, true)), origin),
            ControlEvent.Key(KeyCode.HOME, true, origin),
            ControlEvent.MicPacket(0, byteArrayOf(1), 0, origin),
        )
        events.forEach { output.host!!.control(it) }
        assertTrue(input.controls.isEmpty())
        assertEquals(VideoCodec.H264, hub.sessionConfig.screens[main]!!.codec)

        val updated = initial.copy(
            codecs = setOf(VideoCodec.H264, VideoCodec.H265), screens = setOf(main, ScreenId.CLUSTER),
            touch = true, keys = setOf(KeyCode.HOME), microphone = true,
        )
        input.emit { it.inputCapabilities(updated) }
        val desired = DefaultSessionPolicy.derive(updated, listOf(output.reportedCapabilities!!))
        assertEquals(desired, hub.sessionConfig)
        assertEquals(listOf(desired), input.reconfigurations)
        assertEquals(desired, output.sessions.last().second)
        assertNull(hub.pendingConfiguration)
        events.forEach { output.host!!.control(it) }
        assertEquals(events, input.controls)

        input.emit { it.inputCapabilities(updated) }
        assertEquals(1, input.reconfigurations.size)
        input.emit { it.inputCapabilities(initial) }
        events.forEach { output.host!!.control(it) }
        assertEquals(events, input.controls)
        assertEquals(VideoCodec.H264, hub.sessionConfig.screens[main]!!.codec)
        hub.stop()
    }

    @Test
    fun inputCapabilityUpdatesUsePendingRestartRulesAndFenceOldSinks() {
        val initial = FakeInput().capabilities.copy(codecs = setOf(VideoCodec.H264), touch = false)
        val input = FakeInput(initial).apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        val output = RecordingOutput("primary", capabilities())
        hub.addOutput(output); hub.start(); active(hub)
        val oldSink = input.sink!!
        val applied = hub.sessionConfig
        val updated = initial.copy(codecs = setOf(VideoCodec.H264, VideoCodec.H265), touch = true)
        val desired = DefaultSessionPolicy.derive(updated, listOf(output.reportedCapabilities!!))
        input.emit { it.inputCapabilities(updated) }
        assertEquals(applied, hub.sessionConfig)
        assertEquals(desired, hub.pendingConfiguration)
        assertEquals(1, input.starts.size)
        val touch = ControlEvent.Touch(main, emptyList(), Origin(output.id, "client"))
        output.host!!.control(touch)
        assertEquals<List<ControlEvent>>(listOf(touch), input.controls)

        input.emit { it.inputCapabilities(initial) }
        assertNull(hub.pendingConfiguration)
        input.emit { it.inputCapabilities(updated) }
        input.emit { it.session(SessionState(SessionId(1), SessionPhase.ENDED)) }
        assertEquals(listOf(applied, desired), input.starts)
        assertEquals(desired, hub.sessionConfig)
        assertNull(hub.pendingConfiguration)
        oldSink.inputCapabilities(initial)
        output.host!!.control(touch)
        assertEquals<List<ControlEvent>>(listOf(touch, touch), input.controls)
        assertEquals(desired, hub.sessionConfig)
        hub.stop()
        oldSink.inputCapabilities(initial)
        assertEquals(desired, hub.sessionConfig)
    }

    @Test
    fun inputCapabilityUpdatesWaitForEndedAndKeepUnsupportedConfigUnapplied() {
        val initial = FakeInput().capabilities.copy(codecs = setOf(VideoCodec.H264), touch = false)
        val input = FakeInput(initial).apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        val output = RecordingOutput("primary", capabilities())
        hub.addOutput(output); hub.start()
        val updated = initial.copy(codecs = setOf(VideoCodec.H264, VideoCodec.H265), touch = true)
        input.emit { it.inputCapabilities(updated) }
        assertEquals(1, input.starts.size)
        assertEquals(VideoCodec.H264, hub.sessionConfig.screens[main]!!.codec)
        input.emit { it.session(SessionState(SessionId(1), SessionPhase.ENDED)) }
        assertEquals(listOf(false), input.restartAuthorizations)
        assertEquals(2, input.starts.size)
        assertEquals(VideoCodec.H265, hub.sessionConfig.screens[main]!!.codec)
        assertNull(hub.pendingConfiguration)

        input.reconfigureResult = ReconfigureResult.UNSUPPORTED
        input.emit { it.inputCapabilities(initial) }
        assertEquals(VideoCodec.H265, hub.sessionConfig.screens[main]!!.codec)
        assertNull(hub.pendingConfiguration)
        output.host!!.control(ControlEvent.Touch(main, emptyList(), Origin(output.id, "client")))
        assertTrue(input.controls.isEmpty())
        hub.stop()
    }

    @Test
    fun primaryInteractiveDisplayDecidesSizeAndAllViewersMustHaveHardwareHevc() {
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock())
        val buffered = RecordingOutput("buffered", capabilities(1920, latency = LatencyClass.BUFFERED))
        val primary = RecordingOutput("primary", capabilities(800))
        val secondary = RecordingOutput("secondary", capabilities(1024))
        hub.addOutput(buffered); hub.addOutput(primary); hub.addOutput(secondary)
        hub.start()
        assertEquals(ScreenConfig(VideoCodec.H265, 800, 480, 30.0), hub.sessionConfig.screens[main])
        secondary.host!!.capabilities(capabilities(1024, hardware = false))
        assertEquals(VideoCodec.H264, hub.sessionConfig.screens[main]!!.codec)
        assertEquals(800, hub.sessionConfig.screens[main]!!.width)
        hub.removeOutput("secondary")
        assertEquals(VideoCodec.H265, hub.sessionConfig.screens[main]!!.codec)
        hub.removeOutput("primary")
        assertEquals(1920, hub.sessionConfig.screens[main]!!.width)
        hub.stop()
    }

    @Test
    fun invisibleOutputsDoNotPreventHevcAndCodecIsNegotiatedPerScreen() {
        val policy = DefaultSessionPolicy
        val input = FakeInput().capabilities.copy(screens = setOf(main, ScreenId.CLUSTER))
        val primary = capabilities(displays = mapOf(main to DisplayInfo(800, 480, 160, 60.0)))
        val cluster = capabilities(hardware = false, displays = mapOf(ScreenId.CLUSTER to DisplayInfo(400, 200, 160, 30.0)))
        val invisible = capabilities(hardware = false, displays = emptyMap())
        val config = policy.derive(input, listOf(primary, cluster, invisible))
        assertEquals(VideoCodec.H265, config.screens[main]!!.codec)
        assertEquals(VideoCodec.H264, config.screens[ScreenId.CLUSTER]!!.codec)
    }

    @Test
    fun restartIsDeferredWhileActiveAndExplicitRestartFencesOldSink() {
        val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start(); active(hub)
        val oldSink = input.sink!!
        val oldConfig = hub.sessionConfig
        hub.addOutput(RecordingOutput("primary", capabilities()))
        val desired = hub.pendingConfiguration!!
        assertEquals(oldConfig, hub.sessionConfig)
        assertEquals(1, input.starts.size)
        hub.requestRestart()
        assertEquals(2, input.starts.size)
        assertEquals(listOf(false), input.restartAuthorizations)
        assertEquals(1, input.stopCount)
        assertEquals(desired, hub.sessionConfig)
        assertNull(hub.pendingConfiguration)
        val output = RecordingOutput("late")
        hub.addOutput(output)
        oldSink.microphone(0, MicRequest(0, AudioCodec.OPUS, 48000, 1, 20))
        oldSink.videoFrame(frame(999))
        assertTrue(output.micRequests.isEmpty())
        assertTrue(output.frames.isEmpty())
        hub.requestRestart()
        assertEquals(2, input.starts.size)
        hub.stop()
    }

    @Test
    fun pendingRestartAppliesWhenSessionEndsAndSupersededPendingIsCleared() {
        val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start(); active(hub)
        val output = RecordingOutput("primary", capabilities())
        hub.addOutput(output)
        assertTrue(hub.pendingConfiguration != null)
        hub.removeOutput(output.id)
        assertNull(hub.pendingConfiguration)
        hub.addOutput(RecordingOutput("next", capabilities(1024)))
        hub.session(SessionState(SessionId(1), SessionPhase.ENDED))
        assertEquals(2, input.starts.size)
        assertEquals(1024, hub.sessionConfig.screens[main]!!.width)
        hub.stop()
    }

    @Test
    fun restartAppliesAfterEndedAndUnsupportedChangesRemainUnapplied() {
        val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start()
        hub.addOutput(RecordingOutput("primary", capabilities()))
        assertEquals(1, input.starts.size)
        hub.session(SessionState(SessionId(1), SessionPhase.ENDED))
        assertEquals(2, input.starts.size)
        assertEquals(listOf(false), input.restartAuthorizations)
        assertNull(hub.pendingConfiguration)
        active(hub, 2)
        val current = hub.sessionConfig
        input.reconfigureResult = ReconfigureResult.UNSUPPORTED
        val other = RecordingOutput("other", capabilities(hardware = false))
        hub.addOutput(other)
        assertEquals(current, hub.sessionConfig)
        assertNull(hub.pendingConfiguration)
        val count = input.reconfigurations.size
        other.host!!.capabilities(capabilities(hardware = false))
        assertEquals(count, input.reconfigurations.size)
        hub.stop()
    }

    @Test
    fun outputRequestedRestartAppliesPendingConfigAndFencesOldSink() {
        val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start(); active(hub)
        val oldSink = input.sink!!
        val output = RecordingOutput("primary", capabilities())
        hub.addOutput(output)
        val desired = hub.pendingConfiguration!!
        val host = output.host!!
        host.requestRestart(Origin("forged", "client"))
        assertTrue(input.restartAuthorizations.isEmpty())
        host.requestRestart(Origin(output.id, "client"))
        assertEquals(listOf(true), input.restartAuthorizations)
        assertEquals(desired, input.starts.last())
        assertEquals(desired, hub.sessionConfig)
        assertEquals(1, input.stopCount)
        assertNull(hub.pendingConfiguration)
        oldSink.videoFrame(frame(999))
        assertTrue(output.frames.isEmpty())
        host.requestRestart(Origin(output.id, "client"))
        assertEquals(listOf(true), input.restartAuthorizations)

        hub.removeOutput(output.id)
        val pending = hub.pendingConfiguration
        assertTrue(pending != null)
        host.requestRestart(Origin(output.id, "client"))
        assertEquals(pending, hub.pendingConfiguration)
        assertEquals(listOf(true), input.restartAuthorizations)
        hub.stop()
    }

    @Test
    fun connectingReconnectingAndActiveNeverAutomaticallyApplyPendingConfig() {
        for (phase in listOf(SessionPhase.CONNECTING, SessionPhase.RECONNECTING, SessionPhase.ACTIVE)) {
            val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
            val hub = ProjectionHub(input, clock = ManualClock())
            hub.start()
            hub.session(SessionState(SessionId(1), phase))
            val applied = hub.sessionConfig
            hub.addOutput(RecordingOutput("primary", capabilities()))
            val desired = hub.pendingConfiguration!!
            assertEquals(applied, hub.sessionConfig, "$phase during negotiation")
            assertEquals(1, input.starts.size)
            for (next in listOf(SessionPhase.CONNECTING, SessionPhase.RECONNECTING, SessionPhase.ACTIVE)) {
                hub.session(SessionState(SessionId(1), next))
                assertEquals(applied, hub.sessionConfig, "$phase to $next")
                assertEquals(desired, hub.pendingConfiguration)
                assertTrue(input.restartAuthorizations.isEmpty())
            }
            hub.session(SessionState(SessionId(1), SessionPhase.ENDED))
            assertEquals(desired, hub.sessionConfig)
            assertNull(hub.pendingConfiguration)
            assertEquals(listOf(false), input.restartAuthorizations)
            assertEquals(2, input.starts.size)
            hub.stop()
        }
    }

    @Test
    fun pendingConfigAutomaticallyAppliesWithoutSessionOrWhenAlreadyEnded() {
        for (ended in listOf(false, true)) {
            val input = FakeInput().apply { reconfigureResult = ReconfigureResult.RESTART_SESSION }
            val silentInput = object : ProjectionInput by input {
                override fun start(config: SessionConfig, sink: InputSink) {
                    input.start(config, object : InputSink by sink {
                        override fun session(state: SessionState) = Unit
                    })
                }
            }
            val hub = ProjectionHub(silentInput, clock = ManualClock())
            hub.start()
            if (ended) hub.session(SessionState(SessionId(1), SessionPhase.ENDED))
            hub.addOutput(RecordingOutput("primary", capabilities()))
            assertEquals(2, input.starts.size)
            assertEquals(800, hub.sessionConfig.screens[main]!!.width)
            assertEquals(listOf(false), input.restartAuthorizations)
            assertNull(hub.pendingConfiguration)
            hub.stop()
        }
    }

    @Test
    fun statsReachListenerOnlyForAttachedOutputAndMatchingOrigin() {
        val diagnostics = mutableListOf<String>()
        val reported = mutableListOf<OutputStats>()
        val hub = ProjectionHub(FakeInput(), clock = ManualClock(), listener = object : HubListener {
            override fun receiverDiagnostics(outputId: String, origin: Origin, type: String, json: String) { diagnostics.add(json) }
            override fun stats(outputId: String, stats: OutputStats) { reported.add(stats) }
        })
        val output = RecordingOutput("output")
        hub.addOutput(output)
        val host = output.host!!
        val stats = OutputStats(Origin("output", "client"), main, null, 0, 0, 0, null)
        host.receiverDiagnostics(stats.origin, "receiver-info", "{}")
        host.receiverDiagnostics(Origin("other", "client"), "receiver-info", "ignored")
        host.stats(stats)
        host.stats(stats.copy(origin = Origin("other", "client")))
        hub.removeOutput(output.id)
        host.stats(stats)
        host.receiverDiagnostics(stats.origin, "receiver-info", "detached")
        assertEquals(listOf("{}"), diagnostics)
        assertEquals(listOf(stats), reported)
        hub.stop()
    }

    @Test
    fun outputFailuresAreReportedClosedAndIsolated() {
        val failures = mutableListOf<String>()
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock(), listener = object : HubListener {
            override fun outputFailed(outputId: String, failure: Throwable) { failures.add(outputId) }
        })
        hub.start()
        val bad = object : RecordingOutput("bad") {
            override fun onVideoFrame(frame: VideoFrame) { error("broken output") }
        }
        val good = RecordingOutput("good")
        hub.addOutput(bad); hub.addOutput(good)
        val retainedHost = bad.host!!
        val frame = frame()
        hub.videoFrame(frame)
        hub.videoFrame(frame)
        assertEquals(listOf("bad"), failures)
        assertTrue(bad.closed)
        assertEquals(2, good.frames.size)
        assertSame(frame, good.frames.first())
        retainedHost.control(ControlEvent.Key(KeyCode.HOME, true, Origin("bad", "client")))
        assertTrue(input.controls.isEmpty())
        hub.stop()
    }

    @Test
    fun openFailureDoesNotPreventOtherOutputsAndCloseFailureIsReported() {
        val failures = mutableListOf<String>()
        val hub = ProjectionHub(FakeInput(), clock = ManualClock(), listener = object : HubListener {
            override fun outputFailed(outputId: String, failure: Throwable) { failures.add(outputId) }
        })
        hub.start()
        hub.addOutput(object : RecordingOutput("bad") {
            override fun open(host: OutputHost) { error("open") }
            override fun close() { error("close") }
        })
        val output = RecordingOutput("good")
        hub.addOutput(output)
        hub.videoFrame(frame())
        assertEquals(listOf("bad", "bad"), failures)
        assertEquals(1, output.frames.size)
        hub.stop()
    }

    @Test
    fun callbacksCanReadHubFromAnotherThreadAndRemoveThemselvesReentrantly() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        hub.start()
        val output = object : RecordingOutput("self") {
            override fun onVideoFrame(frame: VideoFrame) {
                val done = CountDownLatch(1)
                val thread = Thread { hub.sessionConfig; done.countDown() }
                thread.start()
                assertTrue(done.await(1, TimeUnit.SECONDS), "Hub lock must not cover callbacks")
                hub.removeOutput(id)
                assertFalse(closed, "Close must follow the current callback")
            }
        }
        hub.addOutput(output)
        hub.videoFrame(frame())
        assertTrue(output.closed)
        hub.stop()
    }

    @Test
    fun liveMediaStaysOnItsCallerThreadAndCloseWaitsForInFlightCallbacks() {
        val hub = ProjectionHub(FakeInput(), clock = ManualClock())
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val videoCaller = Thread.currentThread()
        var actualVideoCaller: Thread? = null
        val output = object : RecordingOutput("concurrent") {
            override fun onAudioPacket(packet: AudioPacket) {
                entered.countDown()
                release.await(2, TimeUnit.SECONDS)
            }
            override fun onVideoFrame(frame: VideoFrame) { actualVideoCaller = Thread.currentThread() }
        }
        hub.start(); hub.addOutput(output)
        val audioCaller = Thread { hub.audioPacket(AudioPacket(0, SessionId(1), byteArrayOf(1), 0)) }
        audioCaller.start()
        try {
            assertTrue(entered.await(1, TimeUnit.SECONDS))
            hub.videoFrame(frame())
            assertSame(videoCaller, actualVideoCaller)
            hub.removeOutput(output.id)
            assertFalse(output.closed)
        } finally {
            release.countDown()
            audioCaller.join(1000)
            hub.stop()
        }
        assertFalse(audioCaller.isAlive)
        assertTrue(output.closed)
    }

    @Test
    fun repeatedStartStopDoesNotDuplicateInputLifecycle() {
        val input = FakeInput()
        val hub = ProjectionHub(input, clock = ManualClock())
        hub.start(); hub.start(); hub.stop(); hub.stop(); hub.start(); hub.stop()
        assertEquals(2, input.starts.size)
        assertEquals(2, input.stopCount)
    }
}
