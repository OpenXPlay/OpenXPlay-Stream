// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.clink.trace

public fun interface ClinkTrace {
    public fun record(event: ClinkTraceEvent)
    public companion object { public val NONE: ClinkTrace = ClinkTrace { } }
}

internal fun ClinkTrace.emit(event: ClinkTraceEvent) { try { record(event) } catch (_: Exception) { } }
