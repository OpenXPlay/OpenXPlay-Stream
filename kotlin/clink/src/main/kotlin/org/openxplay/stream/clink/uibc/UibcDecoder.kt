// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.uibc

import kotlinx.serialization.json.*
import org.openxplay.stream.clink.trace.*
import org.openxplay.stream.core.*

/** Stateful Android pointer mapper. Wire track IDs are assigned stable core slots 0 and 1. */
public class UibcDecoder(private val origin: Origin, private val videoSize: () -> Pair<Int, Int>,
    private val trace: ClinkTrace = ClinkTrace.NONE, private val session: Long = 0) {
    public constructor(origin: Origin, videoSize: () -> Pair<Int, Int>) : this(origin, videoSize, ClinkTrace.NONE, 0)
    private data class Point(val x: Double, val y: Double)
    private val active = linkedMapOf<Long, Pair<Int, Point>>()
    public fun release(): ControlEvent.Touch {
        active.clear()
        return ControlEvent.Touch(ScreenId.MAIN, emptyList(), origin)
    }
    public fun decode(bytes: ByteArray): ControlEvent? {
        require(bytes.size <= 65536)
        val obj = org.openxplay.stream.clink.protocol.jsonObject(bytes)
        require(obj.size <= 128)
        fun number(key: String, default: Long? = null): Long {
            val element = obj[key] ?: return default ?: error("missing_$key")
            val value = element as? JsonPrimitive ?: error("invalid_$key")
            require(!value.isString)
            return value.longOrNull ?: error("invalid_$key")
        }
        when (number("type")) {
            2L -> {
                val action = number("action", 0); val meta = number("metaState", 0)
                val code = number("keycode"); require(code in 0..65535 && action in 0..2 && meta in 0..0xffffffffL)
                if (trace !== ClinkTrace.NONE) trace.emit(ClinkTraceEvent.Input(System.nanoTime(), session,
                    when (action) { 0L -> "key_down"; 1L -> "key_up"; else -> "key_multiple" }, emptyList()))
                return key(code.toInt(), action.toInt(), origin)
            }
            1L -> Unit
            else -> return null
        }
        val rawCount = number("count"); require(rawCount in 0..16)
        val count = rawCount.toInt(); val action = number("action")
        require(count in 0..16 && action in 0..65535)
        val masked = (action and 255).toInt(); val index = ((action shr 8) and 255).toInt()
        if (masked == 3) {
            if (trace !== ClinkTrace.NONE) trace.emit(ClinkTraceEvent.Input(System.nanoTime(), session, "touch_cancel", emptyList()))
            return release()
        }
        require(count > 0 && index < count && masked in listOf(0, 1, 2, 5, 6))
        val (width, height) = videoSize(); require(width > 0 && height > 0)
        val points = linkedMapOf<Long, Point>()
        repeat(count) { n ->
            val track = number("trackID$n"); require(track in 0..0xffffffffL && track !in points)
            points[track] = Point((number("x$n").toDouble() / width).coerceIn(0.0, 1.0),
                (number("y$n").toDouble() / height).coerceIn(0.0, 1.0))
        }
        val lifted = if (masked in listOf(1, 6)) points.keys.elementAt(index) else null
        val contacts = mutableListOf<TouchContact>()
        val occupied = active.values.map { it.first }.toMutableSet()
        for ((track, slotPoint) in active.toMap()) {
            val p = points[track] ?: slotPoint.second
            val pressed = track != lifted && track in points && masked != 1
            contacts += TouchContact(slotPoint.first, p.x, p.y, pressed)
            if (pressed) active[track] = slotPoint.first to p else active.remove(track)
        }
        for ((track, p) in points) {
            if (track != lifted && masked != 1 && track !in active) {
                val slot = (0..1).firstOrNull { it !in occupied } ?: continue
                occupied += slot; active[track] = slot to p
                contacts += TouchContact(slot, p.x, p.y, true)
            }
        }
        if (trace !== ClinkTrace.NONE) trace.emit(ClinkTraceEvent.Input(System.nanoTime(), session,
            when (masked) { 0 -> "touch_down"; 1 -> "touch_up"; 2 -> "touch_move"; 5 -> "pointer_down"; else -> "pointer_up" },
            points.values.map { it.x to it.y }))
        return ControlEvent.Touch(ScreenId.MAIN, contacts, origin)
    }
    public companion object {
        public val keys: Set<KeyCode> = KeyCode.entries.toSet()
        public fun key(androidCode: Int, action: Int, origin: Origin): ControlEvent.Key? {
            require(action in 0..2)
            val code = when (androidCode) {
                3 -> KeyCode.HOME; 4 -> KeyCode.BACK; 23, 66 -> KeyCode.SELECT
                19 -> KeyCode.KNOB_UP; 20 -> KeyCode.KNOB_DOWN; 21 -> KeyCode.KNOB_LEFT; 22 -> KeyCode.KNOB_RIGHT
                85 -> KeyCode.PLAY_PAUSE; 87 -> KeyCode.NEXT; 88 -> KeyCode.PREVIOUS; 231 -> KeyCode.VOICE
                else -> return null
            }
            if (action == 2) return null // ACTION_MULTIPLE has no down/up equivalent.
            return ControlEvent.Key(code, action == 0, origin)
        }
    }
}
