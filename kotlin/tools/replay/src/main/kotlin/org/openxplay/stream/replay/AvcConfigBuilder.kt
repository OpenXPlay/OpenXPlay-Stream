// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.replay

import java.io.ByteArrayOutputStream
import org.openxplay.stream.core.codec.AvcConfig

/** Builds avcC from unframed SPS/PPS NAL payloads with four-byte media NAL lengths. */
public object AvcConfigBuilder {
    public fun build(sps: ByteArray, pps: ByteArray): ByteArray {
        require(sps.size in 4..65535 && (sps[0].toInt() and 0x9f) == 7) { "Expected SPS payload" }
        require(pps.size in 1..65535 && (pps[0].toInt() and 0x9f) == 8) { "Expected PPS payload" }
        val output = ByteArrayOutputStream(sps.size + pps.size + 11)
        output.write(1)
        output.write(sps, 1, 3)
        output.write(0xff)
        output.write(0xe1)
        fun parameterSet(nal: ByteArray) {
            output.write(nal.size ushr 8)
            output.write(nal.size and 0xff)
            output.write(nal)
        }
        parameterSet(sps)
        output.write(1)
        parameterSet(pps)
        return output.toByteArray().also { AvcConfig.parse(it) }
    }
}
