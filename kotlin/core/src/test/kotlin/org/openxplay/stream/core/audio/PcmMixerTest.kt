// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class PcmMixerTest {
    @Test
    fun replacementWaitsForEndedPcmToDrain() {
        val mixer = PcmMixer()
        mixer.addStream(1, 16000, 1)
        mixer.feed(1, pcm(*IntArray(640) { 1234 }))
        mixer.endStream(1)
        assertFalse(mixer.streamDrained(1))
        assertFailsWith<IllegalArgumentException> { mixer.addStream(1, 8000, 1) }
        repeat(2) { assertContentEquals(pcm(*IntArray(320) { 1234 }), mixer.readFrame()) }
        assertTrue(mixer.streamDrained(1))
        mixer.addStream(1, 8000, 1)
        assertFalse(mixer.streamDrained(1))
    }

    @Test
    fun sumsAllStreamsBeforeSaturatingAndPadsPartialFrames() {
        val mixer = PcmMixer(8000, 1, 1)
        repeat(3) { mixer.addStream(it, 8000, 1) }
        mixer.feed(0, pcm(30000, -30000, 30000))
        mixer.feed(1, pcm(30000, -30000, 30000))
        mixer.feed(2, pcm(-30000, 30000, -10000))
        repeat(3) { mixer.endStream(it) }
        assertFalse(mixer.isDrained)
        assertContentEquals(pcm(30000, -30000, 32767, 0, 0, 0, 0, 0), mixer.readFrame())
        assertTrue(mixer.isDrained)
        assertNull(mixer.readFrame())
        mixer.addStream(5, 8000, 1)
        mixer.feed(5, pcm(-32768))
        mixer.addStream(6, 8000, 1)
        mixer.feed(6, pcm(-1))
        assertEquals(-32768, values(mixer.readFrame()!!).first())
    }

    @Test
    fun liveUnderrunsAreSilenceAndAddRemoveWorksDuringPlayback() {
        val mixer = PcmMixer(16000, 1, 20)
        assertTrue(mixer.isDrained)
        mixer.addStream(1, 16000, 1)
        assertFalse(mixer.isDrained)
        assertContentEquals(ByteArray(640), mixer.readFrame())
        mixer.feed(1, pcm(*IntArray(400) { 1000 }))
        mixer.addStream(2, 16000, 2)
        mixer.feed(2, pcm(*IntArray(640) { 2000 }))
        assertTrue(values(mixer.readFrame()!!).all { it == 3000 })
        mixer.removeStream(1)
        mixer.endStream(2)
        assertTrue(mixer.isDrained)
        assertNull(mixer.readFrame())
        mixer.addStream(1, 16000, 1)
        mixer.feed(1, pcm(123))
        mixer.endStream(1)
        assertContentEquals(pcm(123) + ByteArray(638), mixer.readFrame())
    }

    @Test
    fun drainsResampledLookaheadAndQueuesAcrossFeedBoundaries() {
        val mixer = PcmMixer()
        mixer.addStream(1, 48000, 2)
        val input = pcm(*IntArray(4800 * 2) { 1000 })
        mixer.feed(1, input.copyOfRange(0, 333))
        mixer.feed(1, input.copyOfRange(333, input.size))
        mixer.endStream(1)
        var frames = 0
        while (!mixer.isDrained) {
            val output = values(mixer.readFrame()!!)
            assertEquals(320, output.size)
            if (frames in 1..3) assertTrue(output.all { it in 999..1001 })
            frames++
        }
        assertEquals(5, frames)
        assertNull(mixer.readFrame())
    }

    @Test
    fun rejectsDuplicateUnknownEndedAndFractionalFrameFormats() {
        assertFailsWith<IllegalArgumentException> { PcmMixer(11025, 1, 20) }
        val mixer = PcmMixer()
        assertFailsWith<IllegalArgumentException> { mixer.feed(0, byteArrayOf()) }
        mixer.addStream(0, 16000, 1)
        assertFailsWith<IllegalArgumentException> { mixer.addStream(0, 16000, 1) }
        mixer.endStream(0)
        mixer.endStream(0)
        assertFailsWith<IllegalStateException> { mixer.feed(0, pcm(1)) }
        mixer.addStream(0, 8000, 2) // A drained run can reuse its stream number.
        assertFalse(mixer.isDrained)
    }
}
