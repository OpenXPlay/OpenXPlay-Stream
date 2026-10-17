// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.clink

import org.openxplay.stream.clink.protocol.AuthIdentity

/** Platform network bootstrap. Implementations document callback delivery and serialize lifecycle calls. */
public interface ClinkLink {
    /** May block for discovery, GATT and network join. Never call on media threads.
     * onLinkLost may run on a platform thread; callers must dispatch it to their session owner.
     */
    public fun connect(onLinkLost: (Throwable?) -> Unit): ClinkAddresses
    /** May block; call on the lifecycle worker, never on a media thread. */
    public fun disconnect()
}
public data class ClinkAddresses(
    public val headUnitIpv4: String,
    public val localIpv4: String,
    public val authPort: Int = 57209,
    public val headUnitId: String = headUnitIpv4,
)
/** UI adapter. May block awaiting user input; call on a session worker, never a media thread. */
public fun interface PinProvider {
    /** Return the current displayed PIN, preserving leading zeros, or null on cancellation. */
    public fun requestPin(headUnitId: String): String?
    /** Explicit approval when the head unit requires confirmation of a different saved peer. */
    public fun confirmPairing(headUnitId: String): Boolean = false
}
/** Persistent identity and peer pins. Calls may block; serialize on a lifecycle worker.
 * Store private keys securely. Do not persist a peer until the caller's trust checks succeed.
 */
public interface TrustStore {
    public fun loadIdentity(): AuthIdentity?
    public fun saveIdentity(identity: AuthIdentity)
    /** Raw P-256 X || Y, 64 bytes. Return a copy. */
    public fun loadPeerKey(headUnitId: String): ByteArray?
    public fun savePeerKey(headUnitId: String, publicKey: ByteArray)
    public fun forgetPeer(headUnitId: String)
}
