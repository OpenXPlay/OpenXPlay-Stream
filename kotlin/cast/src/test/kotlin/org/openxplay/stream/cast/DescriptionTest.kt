// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.URI
import org.junit.jupiter.api.Test
import kotlin.test.*

class DescriptionTest {
    private val location = URI("http://127.0.0.1:1234/device/description.xml")
    @Test fun rootEmbeddedRelativeUrlsAndExactVersions() {
        val child = device("uuid:child", service("${AVTRANSPORT_TYPE}3", "control"))
        val root = device(embedded = "<deviceList>$child</deviceList>", services = service() + service("${CONNECTION_TYPE}2", "/connection"))
        val result = parseDescription(location, description(root)).renderers
        assertEquals(2, result.size)
        assertEquals("Test & Renderer", result.first().renderer.friendlyName)
        assertEquals(URI("http://127.0.0.1:1234/device/control"), result.last().transport.control)
        assertEquals("${AVTRANSPORT_TYPE}3", result.last().transport.type)
        assertEquals("${CONNECTION_TYPE}2", result.first().connection!!.type)
        assertEquals("127.0.0.1", result.first().renderer.address)
    }
    @Test fun urlBaseAndAbsentOptionalFields() {
        val xml = description(device(services = service(control = "play")).replace("<manufacturer>OXP</manufacturer>", "")
            .replace("<modelName>Loopback</modelName>", ""), "<URLBase>http://127.0.0.1:5678/base/</URLBase>")
        val renderer = parseDescription(location, xml).renderers.single()
        assertEquals(URI("http://127.0.0.1:5678/base/play"), renderer.transport.control)
        assertEquals("", renderer.renderer.manufacturer); assertEquals("", renderer.renderer.modelName)
    }
    @Test fun foreignHostsAndNonHttpRejectedAndMissingTransportExcluded() {
        for (control in listOf("http://127.0.0.2/control", "https://127.0.0.1/control", "//127.0.0.2/control"))
            assertFailsWith<IllegalArgumentException> { parseDescription(location, description(device(services = service(control = control)))) }
        assertFailsWith<IllegalArgumentException> { parseDescription(location, description(device(), "<URLBase>http://127.0.0.2/</URLBase>")) }
        val missing = parseDescription(location, description(device(services = "")))
        assertTrue(missing.renderers.isEmpty()); assertEquals(setOf("uuid:test"), missing.withoutTransport)
    }
}
