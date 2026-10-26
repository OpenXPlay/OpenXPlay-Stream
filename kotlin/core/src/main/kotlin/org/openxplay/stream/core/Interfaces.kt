// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

/**
 * Produces one projection session at a time.
 * All methods are non-blocking, callable from any thread, and perform no caller-thread I/O.
 * The input serializes sink callbacks per stream and dispatches blocking control work to its own executor.
 */
public interface ProjectionInput {
    public val capabilities: InputCapabilities
    public fun start(config: SessionConfig, sink: InputSink)
    /** Restarts with a new configuration; [authorized] is true only for an output's request. */
    public fun restart(config: SessionConfig, sink: InputSink, authorized: Boolean) {
        stop()
        start(config, sink)
    }
    public fun reconfigure(config: SessionConfig): ReconfigureResult
    /** Returns false when the input cannot accept the event now. */
    public fun control(event: ControlEvent): Boolean
    public fun stop()
}

/**
 * Implemented by the hub. Non-blocking, any thread, with no caller-thread I/O.
 * Calls for one stream are serialized by the input; different streams may call concurrently.
 */
public interface InputSink {
    /**
     * Replaces the input's capabilities when they become known or change.
     * Non-blocking, callable from any thread, with no caller-thread I/O, like all sink callbacks.
     */
    public fun inputCapabilities(capabilities: InputCapabilities): Unit = Unit
    public fun context(update: ContextUpdate): Unit = Unit
    /** One-shot request to show the receiver's own UI; never retained or replayed. */
    public fun hostUiRequested(): Unit = Unit
    public fun session(state: SessionState)
    /** A null format ends the stream. */
    public fun videoFormat(screen: ScreenId, format: VideoFormat?)
    public fun videoFrame(frame: VideoFrame)
    /** A null format ends the stream. */
    public fun audioFormat(stream: Int, format: AudioFormat?)
    public fun audioPacket(packet: AudioPacket)
    /** A null request stops microphone capture. */
    public fun microphone(stream: Int, request: MicRequest?)
}

/**
 * A projection destination. All methods are non-blocking, any thread, with no caller-thread I/O.
 * Per-stream callbacks follow the input's serialization; outputs enqueue work without reordering.
 */
public interface ProjectionOutput {
    public val id: String
    public fun open(host: OutputHost)
    /** Opts into asynchronous context replay and updates when attached to a hub. */
    public fun wantsContext(): Boolean = false
    public fun onContext(update: ContextUpdate): Unit = Unit
    /** Shows the receiver's own UI for a one-shot request; never replayed to later outputs. */
    public fun onHostUiRequested(): Unit = Unit
    public fun onSession(state: SessionState, config: SessionConfig)
    public fun onVideoFormat(screen: ScreenId, format: VideoFormat?)
    public fun onVideoFrame(frame: VideoFrame)
    public fun onAudioFormat(stream: Int, format: AudioFormat?)
    public fun onAudioPacket(packet: AudioPacket)
    public fun onMicrophone(stream: Int, request: MicRequest?)
    public fun close()
}

/**
 * The hub as seen by an output. Non-blocking, any thread, with no caller-thread I/O.
 * Control and reports may be concurrent; input media callbacks remain serialized per stream by the input.
 */
public interface OutputHost {
    public fun capabilities(capabilities: OutputCapabilities)
    /** Requests application of a pending configuration through an authorized restart. */
    public fun requestRestart(origin: Origin): Unit = Unit
    public fun control(event: ControlEvent)
    /** Optional technical browser diagnostics as JSON, including unknown extension fields. */
    public fun receiverDiagnostics(origin: Origin, type: String, json: String): Unit = Unit
    public fun stats(stats: OutputStats)
    /** Optional vehicle report, independent of media and control arbitration. */
    public fun vehicle(data: VehicleData): Unit = Unit
    /** Client-scoped report; hosts without vehicle support retain the default no-op behavior. */
    public fun vehicle(data: VehicleData, origin: Origin): Unit = vehicle(data)
}
