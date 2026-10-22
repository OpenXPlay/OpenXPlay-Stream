// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-21

package org.openxplay.stream.clink

import java.util.concurrent.CopyOnWriteArrayList
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Timeout
import org.openxplay.stream.core.*
import kotlin.test.*

@Timeout(20)
class ClinkDisplayTest {
    @Test fun controlWindowReachesCapabilitiesWithoutChangingThePlaceholder() {
        val display = DisplayInfo(1532, 1013, 240, 30.0)
        for (pictureSize in ClinkPictureSize.entries) {
            val ports = freePorts()
            val reported = CopyOnWriteArrayList<DisplayInfo>()
            val errors = CopyOnWriteArrayList<Throwable>()
            SimulatedHeadUnit(ports, display = display).use { peer ->
                val output = ClinkOutput(StaticClinkLink(ClinkAddresses("127.0.0.1", "127.0.0.1", ports.auth)),
                    PinProvider { "012345" }, MemoryTrustStore(), ports = ports, pictureSize = pictureSize,
                    listener = object : ClinkListener { override fun error(error: Throwable) { errors += error } })
                output.open(object : OutputHost {
                    override fun capabilities(capabilities: OutputCapabilities) { reported += capabilities.displays.getValue(ScreenId.MAIN) }
                    override fun control(event: ControlEvent) {}
                    override fun stats(stats: OutputStats) {}
                })
                try {
                    peer.await("window capabilities and placeholder") {
                        peer.configurationAnswered && peer.accessUnits.isNotEmpty() && reported.any { it.widthPx == 1532 }
                    }
                    val expected = display.copy(viewport = Viewport(1532, 1013), fullscreen = true,
                        frameSize = if (pictureSize == ClinkPictureSize.MODE) Viewport(1280, 720) else null)
                    assertEquals(expected, reported.last())
                    assertEquals(1280 to 720, peer.accessUnits.first().size)
                    assertTrue(errors.isEmpty(), errors.joinToString())
                    peer.assertHealthy()
                } finally { peer.expectRelayClose(); output.close() }
            }
        }
    }
}
