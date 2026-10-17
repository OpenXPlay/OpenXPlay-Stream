// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.simheadunit

import kotlinx.serialization.json.*
import org.openxplay.stream.clink.*

/** Loopback receiver. Commands: touch ACTION TRACK:X:Y [TRACK:X:Y ...], key CODE ACTION, idr, vehicle, daynight MODE, disconnect, quit. */
public object SimHeadUnitMain {
    @JvmStatic public fun main(args: Array<String>): Unit {
        val lock = Any()
        fun emit(value: JsonObject) { synchronized(lock) { println(value); System.out.flush() } }
        try {
            if (args.contentEquals(arrayOf("--help"))) {
                emit(buildJsonObject {
                    put("type", "help")
                    put("options", "--port auth=N,control=N,media=N,sensor=N,cert=N,rtsp=N,uibc=N,rtp=N --pin DIGITS")
                    put("commands", "touch ACTION TRACK:X:Y [TRACK:X:Y ...]; key CODE ACTION; idr; vehicle; daynight MODE; disconnect; quit")
                })
                return
            }
            require(args.size % 2 == 0) { "Options: --port auth=N,control=N,media=N,sensor=N,cert=N,rtsp=N,uibc=N,rtp=N --pin DIGITS" }
            val options = args.toList().chunked(2).associate { it[0] to it[1] }
            require(options.size == args.size / 2 && options.keys.all { it in setOf("--port", "--pin") })
            val defaults = ClinkPorts()
            val ports = mutableMapOf("auth" to defaults.auth, "control" to defaults.control, "media" to defaults.media,
                "sensor" to defaults.sensor, "cert" to defaults.cert, "rtsp" to defaults.rtsp, "uibc" to defaults.uibc, "rtp" to defaults.rtp)
            val seen = mutableSetOf<String>()
            options["--port"]?.split(',')?.forEach {
                val pair = it.split('='); require(pair.size == 2 && pair[0] in ports && seen.add(pair[0]))
                ports[pair[0]] = pair[1].toInt()
            }
            val pin = options["--pin"] ?: "012345"
            require(pin.isNotEmpty() && pin.all { it in '0'..'9' })
            var authenticated = false
            var units = 0
            var tables = Triple(0, 0, 0)
            var failed = false
            SimulatedHeadUnit(ClinkPorts(ports.getValue("auth"), ports.getValue("control"), ports.getValue("media"),
                ports.getValue("cert"), ports.getValue("rtsp"), ports.getValue("uibc"), ports.getValue("rtp"), ports.getValue("sensor")), pin = pin,
                onProgress = { hu -> synchronized(lock) {
                    if (hu.authenticated && !authenticated) {
                        authenticated = true
                        emit(buildJsonObject { put("type", "authentication"); put("success", true) })
                    }
                    val received = hu.accessUnits
                    while (units < received.size) {
                        val au = received[units++]
                        emit(buildJsonObject {
                            put("type", "access-unit"); put("width", au.size.first); put("height", au.size.second)
                            put("key", au.keyFrame); put("bytes", au.data.size)
                        })
                    }
                    val counts = Triple(hu.patCount, hu.pmtCount, hu.pcrCount)
                    if (counts != tables) {
                        tables = counts
                        emit(buildJsonObject { put("type", "tables"); put("pat", counts.first); put("pmt", counts.second); put("pcr", counts.third) })
                    }
                    try { hu.assertHealthy() } catch (e: Throwable) {
                        if (!failed) { failed = true; emit(buildJsonObject { put("type", "error"); put("message", e.cause?.message ?: e.toString()) }) }
                    }
                } }).use { hu ->
                emit(buildJsonObject { put("type", "ready") })
                for (line in System.`in`.bufferedReader().lineSequence()) {
                    try {
                        val command = line.trim().split(Regex("\\s+"))
                        when (command[0]) {
                            "touch" -> {
                                require(command.size >= 2)
                                hu.touch(command[1].toInt(), command.drop(2).map {
                                    val p = it.split(':'); require(p.size == 3)
                                    PixelContact(p[0].toLong(), p[1].toInt(), p[2].toInt())
                                })
                            }
                            "key" -> { require(command.size == 3); hu.key(command[1].toInt(), command[2].toInt()) }
                            "idr" -> hu.requestKeyframe()
                            "vehicle" -> hu.vehicleSamples()
                            "daynight" -> { require(command.size == 2); hu.dayNight(command[1].toInt()) }
                            "disconnect" -> hu.disconnect()
                            "quit" -> break
                            else -> error("Unknown command: ${command[0]}")
                        }
                    } catch (e: Exception) { emit(buildJsonObject { put("type", "error"); put("message", e.toString()) }) }
                }
                hu.expectRelayClose()
            }
        } catch (e: Throwable) {
            emit(buildJsonObject { put("type", "error"); put("message", e.toString()) })
            throw e
        }
    }
}
