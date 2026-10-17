// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink.media

import java.nio.ByteBuffer
import org.openxplay.stream.core.codec.TsMuxer

/** Thread-confined MPEG-TS RTP packetizer. The final group may contain fewer than seven packets. */
public class RtpPacketizer(public val ssrc: Int = 0x434c4144, sequence: Int = 0) {
    private var nextSequence = sequence and 0xffff
    public fun packetize(ts: List<ByteArray>, ptsUs: Long): List<ByteArray> {
        require(ts.all { it.size == 188 && it[0] == 0x47.toByte() })
        val unit = RtpUnit(Int.MAX_VALUE)
        start(unit, ptsUs)
        ts.forEach { append(unit, it) }
        check(finish(unit))
        val packets = ArrayList<ByteArray>(unit.packets)
        unit.forEachPacket { bytes, offset, length -> packets.add(bytes.copyOfRange(offset + 2, offset + length)) }
        return packets
    }
    /** Starts one muxer call in [unit]; its RTP packets share the timestamp of [ptsUs]. */
    internal fun start(unit: RtpUnit, ptsUs: Long) {
        unit.clear(TsMuxer.to90kHz(ptsUs).toInt(), nextSequence, ssrc)
    }
    /** Copies one borrowed TS packet into [unit]; no per-packet allocation. */
    internal fun append(unit: RtpUnit, ts: ByteArray) {
        require(ts.size == 188 && ts[0] == 0x47.toByte())
        unit.append(ts)
    }
    /**
     * Commits the sequence numbers of a complete unit. False when the unit exceeded its bound: it must
     * then be discarded, and the sequence stays where it was so no RTP gap is created.
     */
    internal fun finish(unit: RtpUnit): Boolean {
        if (unit.overflowed) return false
        nextSequence = (unit.firstSequence + unit.packets) and 0xffff
        return true
    }
    public companion object {
        public fun frame(packet: ByteArray): ByteArray {
            require(packet.size in 12..65535)
            return ByteBuffer.allocate(packet.size + 2).putShort(packet.size.toShort()).put(packet).array()
        }
    }
}
