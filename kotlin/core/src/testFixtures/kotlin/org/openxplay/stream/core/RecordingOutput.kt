// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.util.concurrent.CopyOnWriteArrayList

/** Records borrowed model objects, reports capabilities at open, and becomes inert on close. */
public open class RecordingOutput(
    override val id: String = "recording",
    public val reportedCapabilities: OutputCapabilities? = null,
) : ProjectionOutput {
    @Volatile public var host: OutputHost? = null
        private set
    @Volatile public var closed: Boolean = true
        private set
    public val events: MutableList<String> = CopyOnWriteArrayList()
    public val sessions: MutableList<Pair<SessionState, SessionConfig>> = CopyOnWriteArrayList()
    public val frames: MutableList<VideoFrame> = CopyOnWriteArrayList()
    public val packets: MutableList<AudioPacket> = CopyOnWriteArrayList()
    public val videoFormats: MutableList<Pair<ScreenId, VideoFormat?>> = CopyOnWriteArrayList()
    public val audioFormats: MutableList<Pair<Int, AudioFormat?>> = CopyOnWriteArrayList()
    public val micRequests: MutableList<Pair<Int, MicRequest?>> = CopyOnWriteArrayList()

    override fun open(host: OutputHost) {
        this.host = host
        closed = false
        reportedCapabilities?.let { host.capabilities(it) }
    }
    override fun onSession(state: SessionState, config: SessionConfig) {
        if (!closed) { events.add("session"); sessions.add(state to config) }
    }
    override fun onVideoFormat(screen: ScreenId, format: VideoFormat?) {
        if (!closed) { events.add("video-format"); videoFormats.add(screen to format) }
    }
    override fun onVideoFrame(frame: VideoFrame) {
        if (!closed) { events.add("video"); frames.add(frame) }
    }
    override fun onAudioFormat(stream: Int, format: AudioFormat?) {
        if (!closed) { events.add("audio-format"); audioFormats.add(stream to format) }
    }
    override fun onAudioPacket(packet: AudioPacket) {
        if (!closed) { events.add("audio"); packets.add(packet) }
    }
    override fun onMicrophone(stream: Int, request: MicRequest?) {
        if (!closed) { events.add("microphone"); micRequests.add(stream to request) }
    }
    override fun close() { closed = true; host = null }
}
