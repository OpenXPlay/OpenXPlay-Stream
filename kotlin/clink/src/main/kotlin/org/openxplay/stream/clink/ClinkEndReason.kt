// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

package org.openxplay.stream.clink

import java.io.EOFException
import java.net.SocketException
import java.net.SocketTimeoutException

internal data class ClinkEndReason(val by: String, val cause: String, val facts: String = "") {
    fun fields(): String = "by=$by cause=$cause" + if (facts.isEmpty()) "" else " $facts"
}
internal fun clinkEndReason(error: Throwable, connecting: Boolean = false): ClinkEndReason {
    val timeout = when (error.message) {
        "auth_timeout" -> "read_timeout" to "timeout=auth limitMs=20000"
        "projection_timeout" -> "record_timeout" to "timeout=projection limitMs=10000"
        "heartbeat_timeout" -> "health_timeout" to "timeout=heartbeat limitMs=15000"
        "audio_ack_timeout" -> "read_timeout" to "timeout=audio_ack limitMs=2000"
        else -> null
    }
    if (timeout != null) return ClinkEndReason("timeout", timeout.first, "${timeout.second} exception=${error.javaClass.simpleName.take(160)}")
    val exception = "exception=${error.javaClass.simpleName.take(160)}"
    return when {
        error.message in setOf("pin_cancelled", "pairing_confirmation_cancelled", "session_stopped") -> ClinkEndReason("local", "stopped")
        error.message == "rtsp_teardown" -> ClinkEndReason("peer", "teardown")
        error is SocketTimeoutException -> ClinkEndReason("timeout", if (connecting) "connect_timeout" else "read_timeout", if (connecting) "$exception timeout=connect limitMs=12000" else exception)
        error is EOFException -> ClinkEndReason("peer", "peer_eof", exception)
        error is SocketException && error.message?.contains("reset", true) == true -> ClinkEndReason("peer", "peer_reset", exception)
        connecting -> ClinkEndReason("local", "connect_failed", exception)
        error is SocketException -> ClinkEndReason("peer", "io_error", exception)
        error is IllegalArgumentException || error is IllegalStateException -> ClinkEndReason("local", "protocol_error", exception)
        else -> ClinkEndReason("local", "io_error", exception)
    }
}
