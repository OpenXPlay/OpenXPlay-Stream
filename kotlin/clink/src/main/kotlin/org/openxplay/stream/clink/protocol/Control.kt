// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.protocol

import org.openxplay.stream.core.DisplayInfo

/** CONTROL protobuf bodies. Field 10 is isDataTransMode, which must remain zero. */
public object Control {
    public fun displayBody(display: DisplayInfo): ByteArray {
        val values = mapOf(2 to display.widthPx, 3 to display.heightPx, 4 to display.densityDpi,
            5 to display.widthPx, 6 to display.heightPx, 7 to display.refreshHz.toInt(),
            8 to 0, 9 to 0, 10 to 0, 12 to 0, 13 to 0, 14 to 1, 15 to 0)
        return Fields(values.mapValues { FieldValue.Varint(it.value.toULong()) } +
            (17 to FieldValue.Bytes("openxplay-stream-1".toByteArray()))).encode()
    }
    public fun display(body: ByteArray, fallback: DisplayInfo): DisplayInfo {
        val fields = Fields.decode(body)
        require(fields.number(10) == 0uL) { "projection_mode_required" }
        fun value(id: Int, default: Int): Int = if (id in fields.numbers) {
            val n = fields.number(id); require(n in 1uL..16384uL); n.toInt()
        } else default
        return DisplayInfo(value(2, fallback.widthPx), value(3, fallback.heightPx),
            value(4, fallback.densityDpi), value(7, fallback.refreshHz.toInt()).toDouble())
    }
    public fun serverResponse(port: Int): ByteArray = Fields(mapOf(
        1 to FieldValue.Varint(if (port == 0) 2uL else 1uL),
        2 to FieldValue.Varint(port.toULong()),
    )).encode()
}
