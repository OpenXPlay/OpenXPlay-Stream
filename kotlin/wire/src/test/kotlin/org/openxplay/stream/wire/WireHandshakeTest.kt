// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.wire

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import org.junit.jupiter.api.Test
import org.openxplay.stream.core.*
import kotlin.test.*

class WireHandshakeTest {
    @Test fun wrongRolesAndMismatchedConsumerNamesFailWithoutHostCallbacks() {
        listOf(
            WireMessage.Hello(WireRole.PRODUCER, "wire-input", "client"),
            WireMessage.Hello(WireRole.CONSUMER, "wrong-name", "client"),
            WireMessage.Client("missing-hello"),
        ).forEach { first ->
            val framed = ByteArrayOutputStream().also { StreamWireCodec.write(it, first) }.toByteArray()
            val errors = CopyOnWriteArrayList<Throwable>()
            val calls = AtomicInteger()
            val output = WireOutput(WireTransport.streams(ByteArrayInputStream(framed), ByteArrayOutputStream()),
                onError = { errors.add(it) })
            try {
                output.open(object : OutputHost {
                    override fun capabilities(capabilities: OutputCapabilities) { calls.incrementAndGet() }
                    override fun control(event: ControlEvent) { calls.incrementAndGet() }
                    override fun stats(stats: OutputStats) { calls.incrementAndGet() }
                })
                eventually { errors.size == 1 }
                assertEquals(0, calls.get())
                output.close(); output.close()
                assertEquals(1, errors.size)
            } finally { output.close() }
        }
    }
}
