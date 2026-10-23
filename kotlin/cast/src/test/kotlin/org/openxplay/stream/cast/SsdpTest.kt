// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import org.junit.jupiter.api.Test
import kotlin.test.*

class SsdpTest {
    @Test fun acceptsLfEitherHttpVersionAnyReasonAndFirstDuplicateHeader() {
        for (ending in listOf("\n", "\r\n")) for (version in listOf("1.0", "1.1")) for (reason in listOf("", " fine   ", "\tReady")) {
            val wire = listOf("HTTP/$version 200$reason", "LOCATION: http://127.0.0.1/first", "location: http://127.0.0.1/second",
                "USN: uuid:first", "usn: uuid:second", "ST: ${RENDERER_TYPE}1", "st: ignored", "", "").joinToString(ending)
            assertEquals(SsdpReply("http://127.0.0.1/first", "uuid:first", "${RENDERER_TYPE}1"), Ssdp.reply(wire.toByteArray()))
        }
        for (status in listOf("HTTP/1.1 2000", "HTTP/1.1 201 OK", "HTTP/2.0 200 OK")) assertNull(Ssdp.reply(
            "$status\nLOCATION: http://127.0.0.1/first\nUSN: uuid:first\nST: ${RENDERER_TYPE}1\n\n".toByteArray()))
    }
    @Test fun exactSearchBytesForBothTargets() {
        val target = InetSocketAddress(LOOPBACK, 1900)
        for (st in listOf("${RENDERER_TYPE}1", "${AVTRANSPORT_TYPE}1")) assertEquals(
            "M-SEARCH * HTTP/1.1\r\nHOST: 127.0.0.1:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nUSER-AGENT: $USER_AGENT\r\nST: $st\r\n\r\n",
            String(Ssdp.search(target, st), Charsets.US_ASCII))
    }
    @Test fun repliesAreCaseInsensitiveAndMalformedRepliesIgnored() {
        val valid = "HTTP/1.1 200 OK\r\nlOcAtIoN: http://127.0.0.1/device\r\nusn: uuid:a\r\nSt: ${RENDERER_TYPE}1\r\n\r\n"
        assertEquals(SsdpReply("http://127.0.0.1/device", "uuid:a", "${RENDERER_TYPE}1"), Ssdp.reply(valid.toByteArray()))
        for (text in listOf("garbage", valid.replace("HTTP/1.1 200 OK", "NOTIFY * HTTP/1.1"),
            valid.replace("usn: uuid:a\r\n", ""), valid.replace("St: ${RENDERER_TYPE}1\r\n", ""),
            valid.replace("http://127.0.0.1/device", "file:///tmp/device"), valid.replace("uuid:a", ""),
            valid.replace("usn: uuid:a", "nocolon"))) assertNull(Ssdp.reply(text.toByteArray()), text)
    }
    @Test fun unicastSearchDeduplicatesRepliesAndHoldsMulticastLock() {
        FakeRenderer().use { fixture ->
            var held = false; var released = false
            val network = object : CastNetwork {
                override fun localAddresses(): List<java.net.Inet4Address> { assertTrue(held); return listOf(LOOPBACK) }
                override fun multicastLock(): AutoCloseable { held = true; return AutoCloseable { held = false; released = true } }
                override fun socket(): java.net.Socket { assertTrue(held); return java.net.Socket() }
            }
            val found = CastDiscovery(network, fixture.options.discovery, SocketScope()) { true }.search(80)
            assertEquals(listOf("uuid:test"), found.map { it.udn }); assertTrue(released); assertFalse(held)
        }
    }
    @Test fun failedLocalAddressDoesNotPreventIpv4LoopbackDiscovery() {
        FakeRenderer().use { fixture ->
            val unavailable = (2..254).asSequence().map { InetAddress.getByName("127.0.0.$it") as Inet4Address }
                .first { NetworkInterface.getByInetAddress(it) == null }
            val network = object : CastNetwork {
                override fun localAddresses(): List<Inet4Address> = listOf(unavailable, LOOPBACK)
            }
            val scope = SocketScope()
            try {
                val found = CastDiscovery(network, fixture.options.discovery, scope) { true }.search(80)
                assertEquals(listOf("uuid:test"), found.map { it.udn })
            } finally { scope.close() }
        }
    }
    @Test @org.junit.jupiter.api.Timeout(5)
    fun closingScopeCancelsSelectorWaitAndReleasesMulticastLock() {
        DatagramSocket(InetSocketAddress(LOOPBACK, 0)).use { receiver ->
            receiver.soTimeout = 2000
            val scope = SocketScope(); val active = AtomicBoolean(true); val released = AtomicBoolean(false)
            val results = CopyOnWriteArrayList<List<CastRenderer>>(); val failures = CopyOnWriteArrayList<Throwable>()
            val network = object : CastNetwork {
                override fun localAddresses(): List<Inet4Address> = listOf(LOOPBACK)
                override fun multicastLock(): AutoCloseable = AutoCloseable { released.set(true) }
            }
            val discovery = CastDiscovery(network, DiscoveryOptions(InetSocketAddress(LOOPBACK, receiver.localPort)), scope) { active.get() }
            val worker = Thread({
                try { results += discovery.search(10_000) } catch (error: Throwable) { failures += error }
            }, "test-ssdp-cancel").apply { isDaemon = true }
            try {
                worker.start()
                val request = DatagramPacket(ByteArray(4096), 4096); receiver.receive(request)
                assertEquals(LOOPBACK, request.address)
                active.set(false); scope.close(); worker.join(1000)
                assertFalse(worker.isAlive); assertTrue(failures.isEmpty(), failures.joinToString())
                assertEquals(listOf(emptyList<CastRenderer>()), results); assertTrue(released.get())
            } finally { active.set(false); scope.close(); worker.join(2000) }
        }
    }
}
