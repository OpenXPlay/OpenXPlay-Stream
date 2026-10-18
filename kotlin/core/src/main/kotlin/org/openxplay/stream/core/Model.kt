// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

import org.openxplay.stream.core.codec.AvcConfig
import org.openxplay.stream.core.codec.HevcConfig

/** An increasing identifier assigned by the input to fence stale session callbacks. */
@JvmInline
public value class SessionId(public val value: Long)

public enum class SessionPhase { CONNECTING, ACTIVE, RECONNECTING, ENDED }

public data class SessionState(
    public val id: SessionId,
    public val phase: SessionPhase,
    public val reason: String? = null,
)

@JvmInline
public value class ScreenId(public val value: Int) {
    public companion object {
        public val MAIN: ScreenId = ScreenId(0)
        public val CLUSTER: ScreenId = ScreenId(1)
    }
}

public enum class VideoCodec { H264, H265 }

/**
 * Parsed decoder configuration. [config] is shared and must not be mutated after hand-off.
 * Instances use identity equality, including for their byte arrays.
 */
public class VideoFormat private constructor(
    public val screen: ScreenId,
    public val codec: VideoCodec,
    public val config: ByteArray,
    public val codecString: String,
    public val width: Int,
    public val height: Int,
    public val fpsHint: Double?,
) {
    public companion object {
        /** Parses avcC/hvcC; malformed or non-four-byte-length configurations are rejected. */
        public fun create(
            screen: ScreenId,
            codec: VideoCodec,
            config: ByteArray,
            fpsHint: Double? = null,
        ): VideoFormat {
            require(fpsHint == null || (fpsHint.isFinite() && fpsHint > 0.0))
            return when (codec) {
                VideoCodec.H264 -> {
                    val parsed = AvcConfig.parse(config)
                    require(parsed.nalLengthSize == 4) { "Video frames require four-byte NAL lengths" }
                    VideoFormat(screen, codec, config, parsed.codecString, parsed.width, parsed.height, fpsHint)
                }
                VideoCodec.H265 -> {
                    val parsed = HevcConfig.parse(config)
                    require(parsed.nalLengthSize == 4) { "Video frames require four-byte NAL lengths" }
                    VideoFormat(screen, codec, config, parsed.codecString, parsed.width, parsed.height, fpsHint)
                }
            }
        }
    }
}

/** One AVCC access unit with four-byte lengths. [data] must not be mutated after hand-off; no copy is made. */
public class VideoFrame(
    public val screen: ScreenId,
    public val session: SessionId,
    public val data: ByteArray,
    public val ptsUs: Long,
    public val keyFrame: Boolean,
    public val receivedAtNanos: Long,
)

public enum class AudioRole { MEDIA, ALERT, VOICE, TELEPHONY, OTHER }
public enum class AudioCodec { AAC_LC, OPUS, PCM_S16LE }

/** [config], when present, must not be mutated after hand-off. Uses identity equality. */
public class AudioFormat(
    public val stream: Int,
    public val role: AudioRole,
    public val codec: AudioCodec,
    public val sampleRate: Int,
    public val channels: Int,
    public val config: ByteArray? = null,
)

/** [data] must not be mutated after hand-off. Uses identity equality and retains the supplied array. */
public class AudioPacket(
    public val stream: Int,
    public val session: SessionId,
    public val data: ByteArray,
    public val ptsUs: Long,
)

public data class MicRequest(
    public val stream: Int,
    public val codec: AudioCodec,
    public val sampleRate: Int,
    public val channels: Int,
    public val frameMs: Int,
)

/** Stable OXP-Web v0 numbers; these are independent of enum ordinals. */
public enum class KeyCode(public val wireCode: Int) {
    HOME(0), BACK(1), SELECT(2), KNOB_LEFT(3), KNOB_RIGHT(4), KNOB_UP(5), KNOB_DOWN(6),
    PLAY_PAUSE(7), NEXT(8), PREVIOUS(9), VOICE(10),
}

public data class Origin(public val outputId: String, public val clientId: String)

public data class TouchContact(
    public val id: Int,
    public val x: Double,
    public val y: Double,
    public val pressed: Boolean,
) {
    init {
        require(id in 0..9) { "Contact id must be in 0..9" }
        require(x.isFinite() && x in 0.0..1.0 && y.isFinite() && y in 0.0..1.0) {
            "Touch coordinates must be finite and normalized"
        }
    }
}

public sealed interface ControlEvent {
    public val origin: Origin

    /**
     * The origin's complete contact set for [screen]: contacts that are not listed, or listed with
     * `pressed = false`, are released. An empty list releases all contacts.
     */
    public data class Touch(
        public val screen: ScreenId,
        public val contacts: List<TouchContact>,
        override val origin: Origin,
    ) : ControlEvent

    public data class Key(
        public val code: KeyCode,
        public val down: Boolean,
        override val origin: Origin,
    ) : ControlEvent

    public data class KeyFrameRequest(public val screen: ScreenId, override val origin: Origin) : ControlEvent

    /** [data] must not be mutated after hand-off. Uses identity equality. */
    public class MicPacket(
        public val stream: Int,
        public val data: ByteArray,
        public val ptsUs: Long,
        override val origin: Origin,
    ) : ControlEvent
}

public enum class LatencyClass { INTERACTIVE, BUFFERED }
public enum class ReconfigureCost { NONE, RESTART_SESSION, UNSUPPORTED }

/** RESTART_SESSION reports that applying the configuration requires an explicitly authorized restart. */
public enum class ReconfigureResult { APPLIED, RESTART_SESSION, UNSUPPORTED }

public data class DecoderSupport(public val codecString: String, public val hardware: Boolean)

/** Insets in physical pixels, measured inward from each display edge. */
public data class SafeArea(
    public val left: Int = 0,
    public val top: Int = 0,
    public val right: Int = 0,
    public val bottom: Int = 0,
)

public enum class FrameRatePreference { AUTO, FPS_30, FPS_35, FPS_40, FPS_45, FPS_50, FPS_55, FPS_60 }

public data class Viewport(public val width: Int, public val height: Int)

public data class DisplayInfo(
    public val widthPx: Int,
    public val heightPx: Int,
    public val densityDpi: Int,
    public val refreshHz: Double,
    public val safeArea: SafeArea = SafeArea(),
    public val frameRate: FrameRatePreference = FrameRatePreference.AUTO,
    public val sustainedFps: Double? = null,
    public val viewport: Viewport? = null,
    public val fullscreen: Boolean? = null,
    public val fullscreenPreferred: Boolean? = null,
)

public data class InputCapabilities(
    public val codecs: Set<VideoCodec>,
    public val screens: Set<ScreenId>,
    public val touch: Boolean,
    public val keys: Set<KeyCode>,
    public val microphone: Boolean,
    public val keyFrameRequests: Boolean,
    public val reconfigure: ReconfigureCost,
)

public data class OutputCapabilities(
    public val latency: LatencyClass,
    public val decoders: List<DecoderSupport>,
    public val displays: Map<ScreenId, DisplayInfo>,
    public val touch: Int,
    public val keys: Set<KeyCode>,
    public val audio: Set<AudioCodec>,
    public val microphone: Boolean,
    public val prefersH264: Boolean = false,
)

public data class ScreenConfig(
    public val codec: VideoCodec,
    public val width: Int,
    public val height: Int,
    public val fps: Double,
)

/** One negotiated codec, display size and frame rate per screen. */
public data class SessionConfig(public val screens: Map<ScreenId, ScreenConfig>)

/** Per-client, per-screen counters; time measurements are in microseconds and may be unavailable. */
public data class OutputStats(
    public val origin: Origin,
    public val screen: ScreenId,
    public val decodeTimeUs: Long?,
    public val decodeQueue: Int,
    public val dropped: Long,
    public val rendered: Long,
    public val endToEndLatencyUs: Long?,
    public val audioBufferedUs: Long? = null,
    public val audioUnderruns: Long = 0,
)
