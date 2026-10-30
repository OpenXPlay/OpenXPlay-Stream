// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.ServerSocket
import java.net.InetAddress
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(15)
class CastOutputLoopbackTest {
    @Test fun playingOrTransitioningWithoutHttpClientsRepushesAfterTimeout() {
        for (rendererState in listOf("PLAYING", "TRANSITIONING")) FakeRenderer().use { fixture ->
            val states = CopyOnWriteArrayList<CastState>(); val failures = CopyOnWriteArrayList<CastFailure>()
            val threads = TrackedThreads()
            val options = fixture.options.copy(noClientTimeoutMs = 150, stablePlayingMs = 10_000)
            val output = fixture.output(object : CastListener {
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
                override fun failure(failure: CastFailure) { failures += failure }
            }, threads, options = options)
            try {
                output.open(object : OutputHost by NO_HOST {
                    override fun control(event: ControlEvent) { if (event is ControlEvent.KeyFrameRequest) output.onVideoFrame(frame()) }
                })
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame())
                await("Initial playback missing") { states.lastOrNull() == CastState.PLAYING && fixture.packets.isNotEmpty() }
                val firstUri = fixture.uris.single()
                fixture.transportState = rendererState
                Thread.sleep(options.noClientTimeoutMs * 2)
                assertEquals(1, fixture.uris.size); assertFalse(CastState.STOPPED_BY_RENDERER in states)
                fixture.transportState = "PAUSED_PLAYBACK"; fixture.disconnectPlayers()
                await("Pause missing") { states.lastOrNull() == CastState.PAUSED }
                Thread.sleep(options.noClientTimeoutMs * 2)
                assertEquals(1, fixture.uris.size); assertFalse(CastState.STOPPED_BY_RENDERER in states)
                fixture.transportState = rendererState
                await("$rendererState without HTTP clients did not stop: $states $failures") { CastState.STOPPED_BY_RENDERER in states }
                assertEquals(1, failures.count { it.code == "renderer_stopped" })
                assertContains(failures.first { it.code == "renderer_stopped" }.detail, "without streaming HTTP clients")
                await("$rendererState did not re-push") { fixture.uris.size == 2 && states.lastOrNull() == CastState.PLAYING }
                assertNotEquals(firstUri, fixture.uris[1])
                assertTrue(fixture.errors.isEmpty(), fixture.errors.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }
    @Test fun discoversStartsPlaysRepushesWithNewGenerationAndStopsOnSessionEndAndClose() {
        FakeRenderer().use { fixture ->
            val states = CopyOnWriteArrayList<CastState>()
            val failures = CopyOnWriteArrayList<CastFailure>()
            val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun state(state: CastState, renderer: CastRenderer?) {
                    if (state == CastState.SEARCHING) assertNull(renderer) else assertEquals("uuid:test", renderer?.udn)
                    states += state
                }
                override fun failure(failure: CastFailure) { failures += failure }
            }, threads)
            try {
                output.open(object : OutputHost by NO_HOST {
                    override fun control(event: ControlEvent) { if (event is ControlEvent.KeyFrameRequest) output.onVideoFrame(frame()) }
                })
                await("Discovery did not find renderer") { CastState.WAITING_FOR_VIDEO in states }
                assertFailsWith<IllegalStateException> { output.open(NO_HOST) }
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame())
                await("Renderer did not play: $states $failures") { CastState.PLAYING in states && fixture.packets.isNotEmpty() }
                assertEquals(listOf(CastState.SEARCHING, CastState.WAITING_FOR_VIDEO, CastState.STARTING, CastState.PLAYING), states.take(4))
                assertEquals(listOf("Stop", "SetAVTransportURI", "Play"), fixture.actions.filter { it in setOf("Stop", "SetAVTransportURI", "Play") }.take(3))
                assertContains(fixture.metadata.first(), "<DIDL-Lite")
                val firstUri = fixture.uris.first()
                assertTrue(Regex("http://127\\.0\\.0\\.1:[0-9]+/cast/[a-f0-9]{32}/[0-9]+\\.ts").matches(firstUri))
                output.onVideoFormat(ScreenId.MAIN, videoFormat())
                Thread.sleep(100); assertEquals(1, fixture.uris.size, "Identical format must not restart")
                fixture.transportState = "STOPPED"
                await("STOPPED did not trigger a new generation: $states $failures") { fixture.uris.size >= 2 && states.lastOrNull() == CastState.PLAYING }
                assertNotEquals(firstUri, fixture.uris[1]); assertTrue(CastState.STOPPED_BY_RENDERER in states)
                assertEquals(1, failures.count { it.code == "renderer_stopped" })
                val beforeEnd = fixture.actions.count { it == "Stop" }
                output.onSession(SessionState(SessionId(1), SessionPhase.ENDED), SESSION_CONFIG)
                await("Session end did not stop renderer") { fixture.actions.count { it == "Stop" } > beforeEnd && states.lastOrNull() == CastState.WAITING_FOR_VIDEO }
                val beforeClose = fixture.actions.count { it == "Stop" }
                output.close(); output.close()
                await("Missing final Stop/ENDED") { fixture.actions.count { it == "Stop" } > beforeClose && states.lastOrNull() == CastState.ENDED }
                assertEquals(1, states.count { it == CastState.ENDED }); threads.awaitEnded()
                assertTrue(fixture.errors.isEmpty(), fixture.errors.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }
    @Test fun soapFaultMetadataFallbackPlay701RetriesAndRejectedEpisode() {
        FakeRenderer().use { fixture ->
            fixture.rejections["SetAVTransportURI"] = 1; fixture.rejections["Play"] = 4
            val failures = CopyOnWriteArrayList<CastFailure>(); val states = CopyOnWriteArrayList<CastState>(); val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun failure(failure: CastFailure) { failures += failure }
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
            }, threads)
            try {
                output.open(NO_HOST); output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame())
                await("SOAP fault missing") { failures.any { it.code == "renderer_rejected" } }
                assertContains(failures.first { it.code == "renderer_rejected" }.detail, "by=peer cause=soap_fault reached=starting action=Play code=701")
                await("Retry did not recover") { CastState.PLAYING in states }
                assertEquals(1, failures.count { it.code == "renderer_rejected" })
                assertEquals(5, fixture.actions.count { it == "Play" })
                assertEquals(3, fixture.actions.count { it == "SetAVTransportURI" })
                assertEquals("", fixture.metadata.first())
            } finally { output.close(); threads.awaitEnded() }
        }
    }
    @Test fun capabilitiesAudioStatisticsAndPause() {
        FakeRenderer().use { fixture ->
            val reports = CopyOnWriteArrayList<CastStatistics>(); val states = CopyOnWriteArrayList<CastState>()
            val failures = CopyOnWriteArrayList<CastFailure>(); val caps = CopyOnWriteArrayList<OutputCapabilities>(); val threads = TrackedThreads()
            val output = fixture.output(object : CastListener {
                override fun statistics(statistics: CastStatistics) { reports += statistics }
                override fun state(state: CastState, renderer: CastRenderer?) { states += state }
                override fun failure(failure: CastFailure) { failures += failure }
            }, threads)
            try {
                output.open(object : OutputHost by NO_HOST { override fun capabilities(capabilities: OutputCapabilities) { caps += capabilities } })
                assertEquals(LatencyClass.BUFFERED, caps.single().latency); assertTrue(caps.single().prefersH264)
                assertEquals(listOf(DecoderSupport("avc1.64002a", true), DecoderSupport("hvc1.1.6.L150", true)), caps.single().decoders)
                assertEquals(setOf(AudioCodec.AAC_LC), caps.single().audio); assertEquals(0, caps.single().touch)
                output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame())
                output.onAudioFormat(0, AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2, byteArrayOf(0x11, 0x90.toByte())))
                output.onAudioPacket(AudioPacket(0, SessionId(1), SILENT_AAC, 0))
                await("Missing periodic statistics") { reports.any { it.videoFrames > 0 && it.audioFrames > it.silentFrames && it.silentFrames > 0 && it.bytesSent > 0 } }
                fixture.transportState = "PAUSED_PLAYBACK"
                await("Pause missing") { states.lastOrNull() == CastState.PAUSED }
                val pushes = fixture.uris.size; Thread.sleep(100); assertEquals(pushes, fixture.uris.size)
                output.onVideoFormat(ScreenId.MAIN, null)
                await("Null format did not stop") { states.lastOrNull() == CastState.WAITING_FOR_VIDEO }
                assertTrue(failures.isEmpty(), failures.joinToString())
            } finally { output.close(); threads.awaitEnded() }
        }
    }
    @Test fun noTransportNotFoundUnreachableAndBindFailuresAreReportedOncePerEpisode() {
        FakeRenderer(transport = false).use { fixture ->
            val failures = CopyOnWriteArrayList<CastFailure>(); val threads = TrackedThreads()
            val output = fixture.output(object : CastListener { override fun failure(failure: CastFailure) { failures += failure } }, threads)
            try {
                output.open(NO_HOST)
                await("Missing discovery failures") { failures.any { it.code == "renderer_not_found" } }
                Thread.sleep(150)
                assertEquals(1, failures.count { it.code == "renderer_no_avtransport" }); assertEquals(1, failures.count { it.code == "renderer_not_found" })
            } finally { output.close(); threads.awaitEnded() }
        }
        FakeRenderer().use { fixture ->
            val failures = CopyOnWriteArrayList<CastFailure>(); val threads = TrackedThreads()
            ServerSocket(0, 1, InetAddress.getByName("0.0.0.0")).use { occupied ->
                val output = CastOutput(CastOutputConfig("uuid:test", port = occupied.localPort), LOOPBACK_NETWORK,
                    object : CastListener { override fun failure(failure: CastFailure) { failures += failure } }, threads, "cast", fixture.options)
                try {
                    output.open(NO_HOST)
                    await("Missing bind failure") { failures.any { it.code == "stream_server_failed" } }
                    Thread.sleep(100); assertEquals(1, failures.count { it.code == "stream_server_failed" })
                } finally { output.close(); threads.awaitEnded() }
            }
            failures.clear()
            val output = fixture.output(object : CastListener { override fun failure(failure: CastFailure) { failures += failure } }, threads)
            try {
                output.open(NO_HOST); output.onSession(SessionState(SessionId(1), SessionPhase.ACTIVE), SESSION_CONFIG)
                output.onVideoFormat(ScreenId.MAIN, videoFormat()); output.onVideoFrame(frame())
                await("Player missing") { fixture.packets.isNotEmpty() }
                fixture.unavailable = true
                await("Missing unreachable failure") { failures.any { it.code == "renderer_unreachable" } }
                Thread.sleep(350); assertEquals(1, failures.count { it.code == "renderer_unreachable" })
            } finally { output.close(); threads.awaitEnded() }
        }
    }
}
