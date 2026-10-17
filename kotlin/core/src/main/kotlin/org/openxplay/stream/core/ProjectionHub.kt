// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Live fan-out runs on the caller thread. Opening/replay uses an ordered caller-driven queue.
 * State locks never cover external calls; close follows any in-flight callbacks.
 * Outputs and inputs must honor their non-blocking contracts. Stop detaches and closes all outputs.
 */
public class ProjectionHub(
    private val input: ProjectionInput,
    private val policy: SessionPolicy = DefaultSessionPolicy,
    private val clock: MonotonicClock = MonotonicClock.system,
    private val listener: HubListener = HubListener.NONE,
) : InputSink {
    private val vehicleListeners = CopyOnWriteArrayList<VehicleRegistration>()

    /**
     * Registers an asynchronous observer. Each listener has its own daemon worker and a queue of
     * 64 reports; overflow drops the oldest queued report. Failures are isolated from media.
     * Close the returned registration to remove it; stop also removes all registrations.
     * No vehicle report is forwarded to the input; future inputs could use GPS for CarPlay location.
     */
    public fun addVehicleListener(listener: VehicleListener): AutoCloseable =
        VehicleRegistration(listener).also { vehicleListeners.add(it) }

    private inner class VehicleRegistration(val listener: VehicleListener) : AutoCloseable {
        private val active = AtomicBoolean(true)
        private val executor = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS,
            ArrayBlockingQueue<Runnable>(64),
            java.util.concurrent.ThreadFactory { task -> Thread(task, "projection-vehicle").apply { isDaemon = true } },
            ThreadPoolExecutor.DiscardOldestPolicy())
        fun offer(destination: Destination, origin: Origin, data: VehicleData) {
            if (!active.get()) return
            executor.execute {
                if (active.get() && destination.active) report { listener.vehicle(origin, data) }
            }
        }
        override fun close() {
            if (!active.compareAndSet(true, false)) return
            vehicleListeners.remove(this)
            executor.shutdownNow()
        }
    }

    private fun vehicle(destination: Destination, origin: Origin, data: VehicleData) {
        if (!destination.active || origin.outputId != destination.id) return
        vehicleListeners.forEach { it.offer(destination, origin, data) }
    }

    private val lock = Any()
    private val outputs = linkedMapOf<String, Destination>()
    private val context = ContextUpdate.idle().associateByTo(linkedMapOf()) { it.topic }
    private val video = linkedMapOf<ScreenId, VideoFormat>()
    private val audio = linkedMapOf<Int, AudioFormat>()
    private val microphones = linkedMapOf<Int, MicRequest>()
    private val microphoneOwners = mutableMapOf<Int, MicrophoneOwner>()
    private val inputCalls = Calls()
    private val outbound = ThreadLocal.withInitial { Calls() }
    private val limiter = KeyframeLimiter()
    private var inputCapabilities = input.capabilities
    private var config = policy.derive(inputCapabilities, emptyList())
    private var attempted: SessionConfig? = null
    private var pending: SessionConfig? = null
    private var state: SessionState? = null
    private var running = false
    private var generation = 0L
    private var timerVersion = 0L
    private var timer: ClockTask? = null
    private var lastTouch: Origin? = null
    private val pressed = mutableSetOf<Pair<ScreenId, Int>>()

    /** Latest applied configuration, excluding a configuration awaiting restart. */
    public val sessionConfig: SessionConfig get() = synchronized(lock) { config }
    public val pendingConfiguration: SessionConfig? get() = synchronized(lock) { pending }

    public fun start() {
        synchronized(lock) {
            if (running) return
            running = true
            generation++
            config = derive()
            attempted = config
            val initial = config
            val sink = fencedSink(generation)
            inputCalls.add { input.start(initial, sink) }
        }
        drain()
    }

    public fun stop() {
        vehicleListeners.forEach { it.close() }
        val destinations: List<Destination>
        val cancelled: ClockTask?
        synchronized(lock) {
            if (!running && outputs.isEmpty()) return
            if (running) inputCalls.add { input.stop() }
            running = false
            generation++
            cancelled = timer
            timer = null
            timerVersion++
            destinations = outputs.values.toList()
            outputs.clear()
            destinations.forEach { detach(it) }
            resetSession()
            pending = null
            attempted = null
        }
        cancelled?.cancel()
        inputCalls.drain()
        destinations.forEach { it.calls.drain() }
    }

    /** IDs must be unique among attached outputs. Open precedes replay and all live callbacks. */
    public fun addOutput(output: ProjectionOutput) {
        val destination = Destination(output)
        synchronized(lock) {
            require(destination.id !in outputs) { "Duplicate output id: ${destination.id}" }
            outputs[destination.id] = destination
            destination.calls.add {
                invoke(destination) { output.open(destination.host) }
                synchronized(lock) { destination.opened = true; destination.contextDelivery?.schedule() }
            }
            state?.let { current ->
                val applied = config
                deliver(destination) { it.onSession(current, applied) }
            }
            context.values.forEach { destination.contextDelivery?.offer(it) }
            video.forEach { (screen, format) -> deliver(destination) { it.onVideoFormat(screen, format) } }
            audio.forEach { (stream, format) -> deliver(destination) { it.onAudioFormat(stream, format) } }
            microphones.forEach { (stream, request) -> deliver(destination) { it.onMicrophone(stream, request) } }
            inputCalls.add { negotiate() }
        }
        destination.calls.drain()
        drain()
    }

    public fun removeOutput(id: String): Unit = removeOutput(id, null)

    private fun removeOutput(id: String, expected: Destination?) {
        val destination = synchronized(lock) {
            if (expected != null && outputs[id] !== expected) return
            val removed = outputs.remove(id) ?: return
            detach(removed)
            microphoneOwners.values.forEach { if (it.origin?.outputId == id) it.origin = null }
            if (lastTouch?.outputId == id) { lastTouch = null; pressed.clear() }
            inputCalls.add { negotiate() }
            removed
        }
        destination.calls.drain()
        drain()
    }

    /** Applies the latest pending configuration through a stop/start cycle. */
    public fun requestRestart() {
        synchronized(lock) { inputCalls.add { restart() } }
        drain()
    }

    override fun inputCapabilities(capabilities: InputCapabilities): Unit = inputCapabilities(capabilities, null)

    private fun inputCapabilities(capabilities: InputCapabilities, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch)) return
            inputCapabilities = capabilities
            inputCalls.add { negotiate() }
        }
        drain()
    }

    override fun context(update: ContextUpdate): Unit = context(update, null)

    private fun context(update: ContextUpdate, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch)) return
            setContext(update)
        }
    }

    private fun setContext(update: ContextUpdate) {
        if (context[update.topic] == update) return
        context[update.topic] = update
        outputs.values.forEach { it.contextDelivery?.offer(update) }
    }

    override fun session(state: SessionState): Unit = acceptSession(state, null)

    private fun acceptSession(value: SessionState, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch) || value.id.value < (state?.id?.value ?: Long.MIN_VALUE)) return
            if (state?.id != value.id) resetSession()
            if (value.phase == SessionPhase.ENDED) ContextUpdate.idle().forEach(::setContext)
            state = value
            val applied = config
            broadcast { it.onSession(value, applied) }
            if (value.phase == SessionPhase.ENDED && pending != null) inputCalls.add { restart(requireIdle = true) }
        }
        drain()
        updateTimer()
    }

    override fun videoFormat(screen: ScreenId, format: VideoFormat?): Unit = format(screen, format, null)
    private fun format(screen: ScreenId, format: VideoFormat?, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch)) return
            require(format == null || format.screen == screen)
            if (format == null) video.remove(screen) else video[screen] = format
            broadcast { it.onVideoFormat(screen, format) }
        }
        drain()
    }

    override fun videoFrame(frame: VideoFrame): Unit = frame(frame, null)
    private fun frame(frame: VideoFrame, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch) || frame.session.value < (state?.id?.value ?: Long.MIN_VALUE)) return
            broadcast { it.onVideoFrame(frame) }
        }
        drain()
    }

    override fun audioFormat(stream: Int, format: AudioFormat?): Unit = audioFormat(stream, format, null)
    private fun audioFormat(stream: Int, format: AudioFormat?, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch)) return
            require(format == null || format.stream == stream)
            if (format == null) audio.remove(stream) else audio[stream] = format
            broadcast { it.onAudioFormat(stream, format) }
        }
        drain()
    }

    override fun audioPacket(packet: AudioPacket): Unit = packet(packet, null)
    private fun packet(packet: AudioPacket, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch) || packet.session.value < (state?.id?.value ?: Long.MIN_VALUE)) return
            broadcast { it.onAudioPacket(packet) }
        }
        drain()
    }

    override fun microphone(stream: Int, request: MicRequest?): Unit = microphone(stream, request, null)
    private fun microphone(stream: Int, request: MicRequest?, epoch: Long?) {
        synchronized(lock) {
            if (!accept(epoch)) return
            require(request == null || request.stream == stream)
            microphoneOwners.remove(stream)
            if (request != null) microphoneOwners[stream] = MicrophoneOwner()
            if (request == null) microphones.remove(stream) else microphones[stream] = request
            broadcast { it.onMicrophone(stream, request) }
        }
        drain()
    }

    private fun control(destination: Destination, event: ControlEvent) {
        synchronized(lock) {
            if (!running || !destination.active || event.origin.outputId != destination.id) return
            when (event) {
                is ControlEvent.KeyFrameRequest -> {
                    if (!inputCapabilities.keyFrameRequests) return
                    limiter.request(event, clock.nowNanos()).forEach { forward(it) }
                }
                is ControlEvent.Touch -> {
                    if (!inputCapabilities.touch || pressed.isNotEmpty() && lastTouch != event.origin) return
                    lastTouch = event.origin
                    // Each touch event is the origin's complete contact set for the screen.
                    pressed.removeAll { it.first == event.screen }
                    event.contacts.filter { it.pressed }.forEach { pressed.add(event.screen to it.id) }
                    forward(event)
                }
                is ControlEvent.Key -> if ((lastTouch == null || lastTouch == event.origin) && event.code in inputCapabilities.keys) forward(event)
                is ControlEvent.MicPacket -> {
                    if (!inputCapabilities.microphone) return
                    val owner = microphoneOwners[event.stream] ?: return
                    // A fallback sender can start immediately; the last touch takes priority when it sends.
                    if (owner.origin == null || event.origin == lastTouch) owner.origin = event.origin
                    if (owner.origin == event.origin) {
                        val epoch = generation
                        inputCalls.add {
                            val accepted = synchronized(lock) {
                                accept(epoch) && destination.active && inputCapabilities.microphone &&
                                    microphoneOwners[event.stream] === owner && owner.origin == event.origin
                            }
                            if (accepted) input.control(event)
                        }
                    }
                }
            }
        }
        drain()
        updateTimer()
    }

    private fun forward(event: ControlEvent) {
        val epoch = generation
        inputCalls.add {
            if (synchronized(lock) { accept(epoch) }) input.control(event)
        }
    }

    private fun derive(): SessionConfig = policy.derive(inputCapabilities, outputs.values.mapNotNull { it.capabilities })

    // Called only by the input command drain, so start/reconfigure/stop cannot overlap.
    private fun negotiate() {
        val desired: SessionConfig
        val epoch: Long
        synchronized(lock) {
            if (!running) return
            desired = derive()
            if (desired == config) { pending = null; attempted = desired; return }
            if (desired == attempted) return
            attempted = desired
            epoch = generation
        }
        val result = input.reconfigure(desired)
        synchronized(lock) {
            if (!accept(epoch)) return
            when (result) {
                ReconfigureResult.APPLIED -> {
                    config = desired
                    pending = null
                    state?.let { current -> broadcast { it.onSession(current, desired) } }
                }
                ReconfigureResult.RESTART_SESSION -> {
                    pending = desired
                    if (state == null || state?.phase == SessionPhase.ENDED) inputCalls.add { restart(requireIdle = true) }
                }
                ReconfigureResult.UNSUPPORTED -> pending = null
            }
        }
    }

    private fun restart(authorized: Boolean = false, requireIdle: Boolean = false) {
        val next: SessionConfig
        val sink: InputSink
        synchronized(lock) {
            if (!running) return
            if (requireIdle && state != null && state?.phase != SessionPhase.ENDED) return
            next = pending ?: return
            pending = null
            config = next
            attempted = next
            generation++
            resetSession()
            sink = fencedSink(generation)
        }
        updateTimer()
        input.restart(next, sink, authorized)
    }

    private fun updateTimer() {
        val version: Long
        val deadline: Long?
        val cancelled: ClockTask?
        synchronized(lock) {
            version = ++timerVersion
            cancelled = timer
            timer = null
            deadline = if (running) limiter.nextDeadlineNanos else null
        }
        cancelled?.cancel()
        if (deadline == null) return
        val scheduled = clock.schedule(maxOf(0, deadline - clock.nowNanos())) {
            synchronized(lock) {
                if (!running || version != timerVersion) return@schedule
                limiter.poll(clock.nowNanos()).forEach { forward(it) }
            }
            drain()
            updateTimer()
        }
        val keep = synchronized(lock) {
            if (version == timerVersion) { timer = scheduled; true } else false
        }
        if (!keep) scheduled.cancel()
    }

    private fun accept(epoch: Long?): Boolean = running && (epoch == null || epoch == generation)
    private fun resetSession() {
        ContextUpdate.idle().forEach(::setContext)
        state = null; video.clear(); audio.clear(); microphones.clear()
        microphoneOwners.clear()
        lastTouch = null; pressed.clear(); limiter.clear()
    }

    private fun fencedSink(epoch: Long): InputSink = object : InputSink {
        override fun inputCapabilities(capabilities: InputCapabilities): Unit = this@ProjectionHub.inputCapabilities(capabilities, epoch)
        override fun context(update: ContextUpdate): Unit = this@ProjectionHub.context(update, epoch)
        override fun session(state: SessionState): Unit = acceptSession(state, epoch)
        override fun videoFormat(screen: ScreenId, format: VideoFormat?): Unit = this@ProjectionHub.format(screen, format, epoch)
        override fun videoFrame(frame: VideoFrame): Unit = this@ProjectionHub.frame(frame, epoch)
        override fun audioFormat(stream: Int, format: AudioFormat?): Unit = this@ProjectionHub.audioFormat(stream, format, epoch)
        override fun audioPacket(packet: AudioPacket): Unit = this@ProjectionHub.packet(packet, epoch)
        override fun microphone(stream: Int, request: MicRequest?): Unit = this@ProjectionHub.microphone(stream, request, epoch)
    }

    private fun broadcast(call: (ProjectionOutput) -> Unit) { outputs.values.forEach { deliver(it, call) } }
    // Called with the state lock held. Live callbacks belong to this caller's drain.
    private fun deliver(destination: Destination, call: (ProjectionOutput) -> Unit) {
        if (!destination.ready) {
            destination.calls.add { invoke(destination) { call(destination.output) } }
        } else {
            destination.inFlight++
            outbound.get().add {
                try { invoke(destination) { call(destination.output) } }
                finally {
                    synchronized(lock) {
                        destination.inFlight--
                        if (!destination.active && destination.inFlight == 0) enqueueClose(destination)
                    }
                    if (!destination.active) destination.calls.drain()
                }
            }
        }
    }

    private fun detach(destination: Destination) {
        destination.active = false
        destination.contextDelivery?.stop()
        if (destination.inFlight == 0) enqueueClose(destination)
    }

    private fun enqueueClose(destination: Destination) {
        if (destination.closeQueued) return
        destination.closeQueued = true
        destination.calls.add { close(destination) }
    }

    private fun drainOpening(destination: Destination) {
        destination.calls.drain()
        synchronized(lock) {
            if (destination.active && destination.opened && destination.calls.isIdle) destination.ready = true
        }
    }

    private fun invoke(destination: Destination, call: () -> Unit) {
        if (!destination.active) return
        try { call() } catch (failure: Exception) {
            report { listener.outputFailed(destination.id, failure) }
            removeOutput(destination.id, destination)
        }
    }

    private fun close(destination: Destination) {
        try { destination.output.close() } catch (failure: Exception) {
            report { listener.outputFailed(destination.id, failure) }
        }
    }

    private fun report(call: () -> Unit) { try { call() } catch (_: Exception) { /* Listener isolation. */ } }
    private fun drain() {
        val snapshot = synchronized(lock) { outputs.values.toList() }
        snapshot.forEach { drainOpening(it) }
        outbound.get().drain()
        inputCalls.drain()
        // Reconfigure can enqueue session notifications without an input callback.
        snapshot.forEach { drainOpening(it) }
        val caller = outbound.get()
        caller.drain()
        if (caller.isIdle) outbound.remove()
    }

    private inner class Destination(val output: ProjectionOutput) {
        val id = output.id
        @Volatile var active = true
        var opened = false
        var ready = false
        var inFlight = 0
        var closeQueued = false
        var capabilities: OutputCapabilities? = null
        val contextDelivery = if (output.wantsContext()) ContextDelivery() else null

        // Allocated only for opted-in outputs; four latest-only slots bound pending work.
        inner class ContextDelivery {
            private val pending = linkedMapOf<ContextTopic, ContextUpdate>()
            private var scheduled = false
            private var stopped = false
            private val executor = java.util.concurrent.Executors.newSingleThreadExecutor { task ->
                Thread(task, "projection-context-$id").apply { isDaemon = true }
            }
            fun stop() {
                stopped = true
                pending.clear()
                executor.shutdownNow()
            }
            fun offer(update: ContextUpdate) {
                if (stopped) return
                pending[update.topic] = update
                schedule()
            }
            fun schedule() {
                if (!active || stopped || !opened || scheduled || pending.isEmpty()) return
                scheduled = true
                executor.execute {
                    while (true) {
                        val next = synchronized(lock) {
                            if (!active || stopped || pending.isEmpty()) { scheduled = false; null }
                            else pending.entries.first().let { inFlight++; pending.remove(it.key) }
                        } ?: break
                        try {
                            output.onContext(next)
                        } catch (failure: Throwable) {
                            synchronized(lock) { stop() }
                            // Context failure disables only this delivery path, never the output.
                            report { listener.outputFailed(id, failure) }
                        } finally {
                            synchronized(lock) {
                                inFlight--
                                if (!active && inFlight == 0) enqueueClose(this@Destination)
                            }
                            if (!active) calls.drain()
                        }
                    }
                }
            }
        }
        val calls = Calls()
        val host = object : OutputHost {
            override fun capabilities(capabilities: OutputCapabilities) {
                synchronized(lock) {
                    if (!active) return
                    this@Destination.capabilities = capabilities
                    inputCalls.add { negotiate() }
                }
                drain()
            }
            override fun control(event: ControlEvent): Unit = control(this@Destination, event)
            override fun requestRestart(origin: Origin) {
                synchronized(lock) {
                    if (!active || origin.outputId != id) return
                    inputCalls.add {
                        if (synchronized(lock) { active }) restart(authorized = true)
                    }
                }
                drain()
            }
            override fun vehicle(data: VehicleData): Unit = vehicle(data, Origin(id, ""))
            override fun vehicle(data: VehicleData, origin: Origin): Unit = this@ProjectionHub.vehicle(this@Destination, origin, data)
            override fun receiverDiagnostics(origin: Origin, type: String, json: String) {
                if (active && origin.outputId == id) report { listener.receiverDiagnostics(id, origin, type, json) }
            }
            override fun stats(stats: OutputStats) {
                if (active && stats.origin.outputId == id) report { listener.stats(id, stats) }
            }
        }
    }

    private class MicrophoneOwner(var origin: Origin? = null)

    /** A reentrant caller-driven drain; contending callers enqueue and return without waiting. */
    private inner class Calls {
        private val queue = ConcurrentLinkedQueue<() -> Unit>()
        private val draining = AtomicBoolean(false)
        val isIdle: Boolean get() = !draining.get() && queue.isEmpty()
        fun add(call: () -> Unit) { queue.add(call) }
        fun drain() {
            do {
                if (!draining.compareAndSet(false, true)) return
                try {
                    while (true) {
                        val call = queue.poll() ?: break
                        try { call() } catch (failure: Exception) { report { listener.inputFailed(failure) } }
                    }
                } finally { draining.set(false) }
            } while (queue.isNotEmpty())
        }
    }
}
