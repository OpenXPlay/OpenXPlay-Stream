// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

package org.openxplay.stream.web

import kotlinx.serialization.json.*

public data class ViewArea(public val x: Int, public val y: Int, public val width: Int, public val height: Int,
    public val canvasWidth: Int, public val canvasHeight: Int) {
    init { require(x >= 0 && y >= 0 && width > 0 && height > 0 &&
        x.toLong() + width <= canvasWidth && y.toLong() + height <= canvasHeight) }
    public fun message(): JsonObject = buildJsonObject {
        put("type", "view-area"); put("x", x); put("y", y); put("width", width); put("height", height)
        put("canvasWidth", canvasWidth); put("canvasHeight", canvasHeight)
    }
}
/** Callbacks run after publishing the current area; closing removes the listener. */
public interface ViewAreaPort {
    public fun current(): ViewArea?
    public fun listen(callback: (ViewArea) -> Unit): AutoCloseable
}
