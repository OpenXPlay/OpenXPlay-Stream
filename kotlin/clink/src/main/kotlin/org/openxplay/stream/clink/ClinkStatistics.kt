// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-20
package org.openxplay.stream.clink

import org.openxplay.stream.core.VideoCodec

/** Cumulative media counters for the current Carlink session, independent of tracing. */
public data class ClinkStatistics(
    public val liveFrames: Long,
    public val placeholderFrames: Long,
    public val droppedFrames: Long,
    public val liveBytes: Long,
    public val sendStalls: Long,
    public val codec: VideoCodec?,
    public val width: Int,
    public val height: Int,
)
