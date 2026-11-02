// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.devrelay

import org.junit.jupiter.api.Test
import kotlin.test.*

class CarlifeOptionsTest {
    private val clip = repositoryRoot().resolve("core/src/test/resources/media/baseline-640x360.h264")

    @Test fun legacyReplayKeepsCarlifeDisabled() {
        assertFalse(RelayOptions.parse(arrayOf("--clip", clip.toString())).carlife)
    }

    @Test fun explicitCarlifeTargetDoesNotEnableOtherOutputs() {
        val options = RelayOptions.parse(arrayOf("--clip", clip.toString(), "--carlife",
            "--carlife-head-unit", "192.168.5.220", "--carlife-bind", "192.168.5.2"))
        assertTrue(options.carlife)
        assertEquals("192.168.5.220", options.carlifeHeadUnit)
        assertEquals("192.168.5.2", options.carlifeBind)
        assertNull(options.webPort)
        assertNull(options.clinkHeadUnit)
        assertNull(options.castRenderer)
    }

    @Test fun invalidAndImplicitAddressesAreRejected() {
        for (args in listOf(arrayOf("--carlife-head-unit", "192.168.5.220"),
            arrayOf("--carlife", "--carlife-head-unit", "300.1.2.3"),
            arrayOf("--carlife", "--carlife-bind", "example.com"))) {
            assertFailsWith<IllegalArgumentException> { RelayOptions.parse(arrayOf("--clip", clip.toString()) + args) }
        }
    }
}
