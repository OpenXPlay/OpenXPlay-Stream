// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-30

package org.openxplay.stream.cast

import java.io.EOFException
import java.net.SocketException
import java.net.SocketTimeoutException

internal fun castEndReason(error: Throwable, connecting: Boolean = false): String = when {
    error is SocketTimeoutException -> "by=timeout cause=${if (connecting) "connect_timeout" else "read_timeout"}"
    error is EOFException -> "by=peer cause=peer_eof"
    error is SocketException && error.message?.contains("reset", true) == true -> "by=peer cause=peer_reset"
    connecting -> "by=local cause=connect_failed"
    error is SocketException -> "by=peer cause=io_error"
    error is IllegalArgumentException -> "by=local cause=protocol_error"
    else -> "by=local cause=io_error"
} + " exception=${error.javaClass.simpleName.take(160)}"
