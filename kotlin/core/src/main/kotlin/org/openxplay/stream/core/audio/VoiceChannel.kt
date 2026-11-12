// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only

package org.openxplay.stream.core.audio

import org.openxplay.stream.core.AudioRole

/** CarPlay type 101 lands in this slot; the relay's AudioPolicy maps 101 -> 20 + role offset. */
public const val ALT_AUDIO_STREAM: Int = 2

/**
 * Whether a stream may open the head unit's voice channel.
 *
 * Only voice and phone calls may. Everything else — the navigation prompt above all — is mixed into
 * the music by the relay instead, because on the G6 the voice channel pauses media playback and does
 * not always resume when it closes: the 2026-11-11 trace shows one prompt that left the car silent
 * for the remaining 147 s while the music kept arriving at 47 access units a second. System sounds
 * such as keyboard clicks were already refused here for the same reason (2026-10-19).
 */
public fun opensVoiceChannel(role: AudioRole): Boolean = role == AudioRole.VOICE || role == AudioRole.TELEPHONY
