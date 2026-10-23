// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.devrelay

import java.util.concurrent.CountDownLatch
import kotlin.system.exitProcess
import org.openxplay.stream.cast.CastDiscovery

object DevRelayMain {
    @JvmStatic
    fun main(args: Array<String>) {
        if ("--help" in args) {
            print(RelayOptions.USAGE)
            System.out.flush()
            return
        }
        val options = try { RelayOptions.parse(args) } catch (failure: Exception) {
            EventLog(System.out).use { it.error("arguments", failure) }
            System.err.print(RelayOptions.USAGE)
            exitProcess(2)
        }
        if (options.castSearch) {
            CastDiscovery().search().forEach { renderer ->
                println(listOf(renderer.udn, renderer.friendlyName, renderer.modelName, renderer.address)
                    .joinToString("\t") { it.replace('\t', ' ').replace('\r', ' ').replace('\n', ' ') })
            }
            return
        }
        val stopped = CountDownLatch(1)
        // Register before starting so SIGINT also cleans up a partially started shell.
        val lifecycle = Any()
        var relay: DevRelay? = null
        var shuttingDown = false
        Runtime.getRuntime().addShutdownHook(Thread({
            synchronized(lifecycle) {
                shuttingDown = true
                relay?.close()
            }
            stopped.countDown()
        }, "devrelay-shutdown"))
        try {
            synchronized(lifecycle) {
                if (!shuttingDown) relay = DevRelay.start(options)
            }
            stopped.await()
        } catch (_: InterruptedException) {
            synchronized(lifecycle) { relay?.close() }
            Thread.currentThread().interrupt()
        } catch (_: Exception) {
            // DevRelay.start records the failure and closes all components.
            exitProcess(1)
        }
    }
}
