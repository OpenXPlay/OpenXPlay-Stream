// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-22

package org.openxplay.stream.cast

import java.net.Inet4Address
import java.net.NetworkInterface
import java.net.Socket

/** A UPnP AV MediaRenderer with an AVTransport service. */
public data class CastRenderer(
    public val udn: String,
    public val friendlyName: String,
    public val manufacturer: String,
    public val modelName: String,
    public val address: String,
    public val location: String,
)

/** Platform network seams; Android supplies its reachable interfaces and multicast lock. */
public interface CastNetwork {
    public fun localAddresses(): List<Inet4Address>
    public fun multicastLock(): AutoCloseable = AutoCloseable { }
    public fun socket(): Socket = Socket()
    public companion object {
        public val SYSTEM: CastNetwork = object : CastNetwork {
            override fun localAddresses(): List<Inet4Address> = NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
                .filter { it.isUp && !it.isLoopback }.flatMap { it.inetAddresses.toList() }
                .filterIsInstance<Inet4Address>().filter { !it.isLoopbackAddress && !it.isAnyLocalAddress }
        }
    }
}

public data class CastOutputConfig(
    public val rendererUdn: String,
    public val width: Int = 1280,
    public val height: Int = 720,
    public val fps: Int = 30,
    public val audio: Boolean = true,
    public val port: Int = 0,
    public val title: String = "CarPlay",
) {
    init {
        require(rendererUdn.isNotBlank())
        require(width > 0 && height > 0 && fps in setOf(30, 60))
        require(port in 0..65535)
    }
}

public enum class CastState { SEARCHING, WAITING_FOR_VIDEO, STARTING, PLAYING, PAUSED, STOPPED_BY_RENDERER, ENDED }
public data class CastFailure(public val code: String, public val detail: String)
public data class CastStatistics(
    public val clients: Int, public val videoFrames: Long, public val audioFrames: Long, public val silentFrames: Long,
    public val bytesSent: Long, public val droppedFrames: Long, public val overflows: Long, public val rendererState: String?,
)
/** Called on worker threads, never under output locks; implementations must not block. */
public interface CastListener {
    public fun state(state: CastState, renderer: CastRenderer?): Unit = Unit
    public fun failure(failure: CastFailure): Unit = Unit
    public fun diagnostic(message: String): Unit = Unit
    public fun statistics(statistics: CastStatistics): Unit = Unit
}
