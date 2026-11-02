// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-02

package org.openxplay.stream.carlife

/** A fixed diagnostic code; no peer-controlled payload is included in the exception text. */
public class CarlifeProtocolException(
    public val code: String,
    public val messageId: Int? = null,
    public val channel: Int? = null,
) : IllegalArgumentException(code)

internal fun protocolRequire(condition: Boolean, code: String, messageId: Int? = null, channel: Int? = null) {
    if (!condition) throw CarlifeProtocolException(code, messageId, channel)
}
