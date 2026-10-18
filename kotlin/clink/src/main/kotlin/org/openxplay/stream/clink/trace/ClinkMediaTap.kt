// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.clink.trace

public fun interface ClinkMediaTap {
    public fun rtp(packet: ByteArray, offset: Int, length: Int)
}
