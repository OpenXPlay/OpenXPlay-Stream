// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.web

import org.junit.jupiter.api.Test
import java.util.concurrent.TimeUnit
import kotlin.test.*

class HelloRateLimitTest {
    private val second = TimeUnit.SECONDS.toNanos(1)
    @Test fun rollingWindowCooldownAndAddressIsolation() {
        val limit = HelloRateLimit()
        repeat(4) { assertFalse(limit.allow("a", it * second) { false }) }
        assertTrue(limit.allow("a", 4 * second) { true })
        assertFalse(limit.allow("a", 10 * second) { false })
        assertFalse(limit.allow("a", 69 * second) { error("Blocked requests must not verify") })
        assertTrue(limit.allow("b", 69 * second) { true })
        assertTrue(limit.allow("a", 70 * second) { true })
        repeat(4) { assertFalse(limit.allow("a", (71 + it) * second) { false }) }
        assertTrue(limit.allow("a", 75 * second) { true })
    }
    @Test fun failuresExpireAtSixtySecondsAndBlockedAttemptsDoNotExtendCooldown() {
        val limit = HelloRateLimit()
        repeat(4) { assertFalse(limit.allow("a", 0) { false }) }
        assertFalse(limit.allow("a", 60 * second) { false })
        assertTrue(limit.allow("a", 60 * second) { true })
        repeat(4) { assertFalse(limit.allow("a", 60 * second) { false }) }
        assertFalse(limit.allow("a", 119 * second) { true })
        assertTrue(limit.allow("a", 120 * second) { true })
    }
    @Test fun constantTimeVerifierAcceptsOnlyTheExactToken() {
        val verifier = TokenVerifier.constantTime("00123456")
        assertTrue(verifier.verify("00123456"))
        for (token in listOf("", "0012345", "001234560", "10123456", "00123457")) assertFalse(verifier.verify(token))
    }
}
