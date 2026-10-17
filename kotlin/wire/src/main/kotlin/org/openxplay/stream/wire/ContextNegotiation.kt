// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

package org.openxplay.stream.wire

/** Legacy v0 peers reject extra hello fields and unknown types; advertise in its opaque client id. */
public object ContextNegotiation {
    public const val CLIENT_SUFFIX: String = ";oxp-context=0"
    public fun client(client: String): String = client + CLIENT_SUFFIX
    public fun supports(peer: WireMessage.Hello): Boolean = peer.role == WireRole.CONSUMER && peer.client.endsWith(CLIENT_SUFFIX)
    public fun clientId(peer: WireMessage.Hello): String = if (supports(peer)) peer.client.removeSuffix(CLIENT_SUFFIX) else peer.client
}
