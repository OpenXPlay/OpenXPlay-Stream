// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

import java.nio.ByteBuffer
import org.openxplay.stream.core.ControlEvent
import org.openxplay.stream.core.KeyCode
import org.openxplay.stream.core.Origin
import org.openxplay.stream.core.ScreenId
import org.openxplay.stream.core.TouchContact

internal object CarlifeInput {
    fun touch(payload: ByteArray, raw: Boolean, width: Int, height: Int, origin: Origin,
        previous: List<TouchContact> = emptyList()): ControlEvent.Touch {
        require(width > 0 && height > 0)
        val contacts = mutableListOf<Triple<Int, Int, Int>>()
        val action: Int
        if (raw) {
            require(payload.size >= 4 && (payload.size - 4) % 5 == 0 && (payload.size - 4) / 5 <= 10)
            val data = ByteBuffer.wrap(payload)
            action = data.int
            while (data.hasRemaining()) contacts += Triple(data.get().toInt() and 255,
                data.short.toInt() and 65535, data.short.toInt() and 65535)
        } else {
            val proto = Proto.parse(payload)
            action = requireNotNull(proto.number(1)).toInt()
            val x = proto.number(2)?.toInt()
            val y = proto.number(3)?.toInt()
            if (x != null && y != null) contacts += Triple(0, x, y)
            else if ((action and 255) !in setOf(1, 3, 6)) {
                requireNotNull(x); requireNotNull(y)
            }
            val pointerX = proto.number(4)?.toInt() ?: 0
            val pointerY = proto.number(5)?.toInt() ?: 0
            if (pointerX != 0 || pointerY != 0) contacts += Triple(1, pointerX, pointerY)
        }
        val masked = action and 255
        require(masked in setOf(0, 1, 2, 3, 5, 6)) { "Unsupported touch action" }
        val releasedIndex = if (masked == 6) (action ushr 8) and 255 else -1
        val normalized = contacts.mapIndexedNotNull { index, (id, x, y) ->
            if (id !in 0..9) null else TouchContact(id,
                (x.toDouble() / width).coerceIn(0.0, 1.0), (y.toDouble() / height).coerceIn(0.0, 1.0),
                masked != 1 && masked != 3 && index != releasedIndex)
        }.distinctBy { it.id }
        val result = when (masked) {
            1, 3 -> {
                // Preserve release positions: a downstream HID implementation can otherwise release
                // at a default position outside the control where the contact originally went down.
                val retained = previous.associateByTo(linkedMapOf()) { it.id }
                normalized.forEach { retained[it.id] = it }
                retained.values.map { it.copy(pressed = false) }
            }
            6 -> {
                val retained = previous.associateByTo(linkedMapOf()) { it.id }
                normalized.forEach { retained[it.id] = it }
                val releasedId = contacts.getOrNull(releasedIndex)?.first ?: previous.getOrNull(releasedIndex)?.id
                retained.values.map { it.copy(pressed = it.id != releasedId) }
            }
            else -> normalized
        }
        return ControlEvent.Touch(ScreenId.MAIN, result, origin)
    }

    // Android key values are the only mapping supplied by this transport. Unknown keys stay local.
    fun key(payload: ByteArray): KeyCode? = when (Proto.parse(payload).number(1)?.toInt()) {
        3 -> KeyCode.HOME
        4 -> KeyCode.BACK
        19 -> KeyCode.KNOB_UP
        20 -> KeyCode.KNOB_DOWN
        21 -> KeyCode.KNOB_LEFT
        22 -> KeyCode.KNOB_RIGHT
        23, 66 -> KeyCode.SELECT
        79, 85 -> KeyCode.PLAY_PAUSE
        87 -> KeyCode.NEXT
        88 -> KeyCode.PREVIOUS
        219, 231 -> KeyCode.VOICE
        else -> null
    }
}
