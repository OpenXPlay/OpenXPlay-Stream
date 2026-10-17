// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import kotlin.test.Test
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertSame

class ModelTest {
    @Test
    fun packetsRetainSharedBytesWithIdentityEquality() {
        val bytes = hex("65 88")
        val session = SessionId(1)
        val frame = VideoFrame(ScreenId.MAIN, session, bytes, 0, true, 0)
        assertSame(bytes, frame.data)
        assertNotEquals(frame, VideoFrame(ScreenId.MAIN, session, bytes, 0, true, 0))
        assertSame(bytes, AudioPacket(0, session, bytes, 0).data)
        assertSame(bytes, AudioFormat(0, AudioRole.MEDIA, AudioCodec.AAC_LC, 48000, 2, bytes).config)
        assertSame(bytes, ControlEvent.MicPacket(0, bytes, 0, Origin("web", "tab")).data)
    }

    @Test
    fun touchRequiresFiniteNormalizedCoordinatesAndValidIds() {
        assertFailsWith<IllegalArgumentException> { TouchContact(10, 0.0, 0.0, true) }
        assertFailsWith<IllegalArgumentException> { TouchContact(-1, 0.0, 0.0, true) }
        assertFailsWith<IllegalArgumentException> { TouchContact(0, Double.NaN, 0.0, true) }
        assertFailsWith<IllegalArgumentException> { TouchContact(0, 0.0, Double.POSITIVE_INFINITY, true) }
        assertFailsWith<IllegalArgumentException> { TouchContact(0, 1.01, 0.0, true) }
        assertFailsWith<IllegalArgumentException> { TouchContact(0, 0.0, -0.01, true) }
    }
}
