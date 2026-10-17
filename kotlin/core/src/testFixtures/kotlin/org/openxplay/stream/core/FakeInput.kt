// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.util.concurrent.CopyOnWriteArrayList

/** Synchronous producer with explicit test-driven callbacks and configurable reconfigure results. */
public class FakeInput(
    override val capabilities: InputCapabilities = InputCapabilities(
        setOf(VideoCodec.H264, VideoCodec.H265), setOf(ScreenId.MAIN), true,
        KeyCode.entries.toSet(), true, true, ReconfigureCost.RESTART_SESSION,
    ),
) : ProjectionInput {
    public var reconfigureResult: ReconfigureResult = ReconfigureResult.APPLIED
    public val starts: MutableList<SessionConfig> = CopyOnWriteArrayList()
    public val reconfigurations: MutableList<SessionConfig> = CopyOnWriteArrayList()
    public val restartAuthorizations: MutableList<Boolean> = CopyOnWriteArrayList()
    public val controls: MutableList<ControlEvent> = CopyOnWriteArrayList()
    public var stopCount: Int = 0
        private set
    public var sink: InputSink? = null
        private set
    private var nextSession = 0L

    @Synchronized
    override fun start(config: SessionConfig, sink: InputSink) {
        if (this.sink != null) return
        starts.add(config)
        this.sink = sink
        sink.session(SessionState(SessionId(++nextSession), SessionPhase.CONNECTING))
    }
    @Synchronized
    override fun restart(config: SessionConfig, sink: InputSink, authorized: Boolean) {
        restartAuthorizations.add(authorized)
        super<ProjectionInput>.restart(config, sink, authorized)
    }
    @Synchronized
    override fun reconfigure(config: SessionConfig): ReconfigureResult {
        reconfigurations.add(config)
        return reconfigureResult
    }
    @Synchronized
    override fun control(event: ControlEvent): Boolean {
        if (sink == null) return false
        controls.add(event)
        return true
    }
    @Synchronized
    override fun stop() {
        if (sink != null) stopCount++
        sink = null
    }

    /** Emits nothing after stop. Tests can retain an old sink to simulate a broken producer. */
    @Synchronized
    public fun emit(action: (InputSink) -> Unit) { sink?.let(action) }
}
