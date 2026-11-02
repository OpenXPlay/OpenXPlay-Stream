// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.nio.ByteBuffer
import org.openxplay.stream.core.*
import kotlin.test.*
import org.junit.jupiter.api.Test

class CarlifeInputTest {
    private val origin = Origin("carlife", "input-test")
    private fun touch(action: Int, x: Int? = null, y: Int? = null, previous: List<TouchContact> = emptyList()): ControlEvent.Touch {
        val bytes = ProtoWriter().number(1, action.toLong()).apply {
            x?.let { number(2, it.toLong()) }; y?.let { number(3, it.toLong()) }
        }.build()
        return CarlifeInput.touch(bytes, false, 1920, 864, origin, previous)
    }

    @Test fun observedHomeTapRetainsEncodedCoordinatesOnDownAndUp() {
        val down = touch(0, 95, 768)
        val up = touch(1, 95, 768, down.contacts)
        assertEquals(95.0 / 1920, down.contacts.single().x)
        assertEquals(768.0 / 864, down.contacts.single().y)
        assertTrue(down.contacts.single().pressed)
        assertEquals(down.contacts.single().copy(pressed = false), up.contacts.single())
    }

    @Test fun releaseUsesWirePositionAndFallsBackToLastPositionWhenOmitted() {
        val down = touch(0, 253, 79)
        val movedRelease = touch(1, 260, 80, down.contacts)
        assertEquals(260.0 / 1920, movedRelease.contacts.single().x)
        assertFalse(movedRelease.contacts.single().pressed)
        val missingRelease = touch(1, previous = down.contacts)
        assertEquals(down.contacts.single().copy(pressed = false), missingRelease.contacts.single())
    }

    @Test fun cancelExplicitlyReleasesEveryRetainedContact() {
        val previous = listOf(TouchContact(0, .1, .2, true), TouchContact(1, .8, .9, true))
        val cancel = touch(3, previous = previous)
        assertEquals(previous.map { it.copy(pressed = false) }, cancel.contacts)
    }

    @Test fun rawPointerUpRetainsReleasedPointerAndOtherActivePointers() {
        val bytes = ByteBuffer.allocate(14).putInt((1 shl 8) or 6)
            .put(3).putShort(192).putShort(216).put(7).putShort(1728).putShort(648).array()
        val up = CarlifeInput.touch(bytes, true, 1920, 864, origin)
        assertEquals(listOf(3, 7), up.contacts.map { it.id })
        assertEquals(listOf(true, false), up.contacts.map { it.pressed })
        assertEquals(.9, up.contacts[1].x)
        assertEquals(.75, up.contacts[1].y)
    }

    @Test fun pointerUpWithoutSecondCoordinatesUsesItsLastPosition() {
        val previous = listOf(TouchContact(0, .1, .2, true), TouchContact(1, .8, .9, true))
        val up = touch((1 shl 8) or 6, 192, 173, previous)
        assertEquals(2, up.contacts.size)
        assertTrue(up.contacts[0].pressed)
        assertEquals(previous[1].copy(pressed = false), up.contacts[1])
    }
}
