// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-11-07

package org.openxplay.stream.carlife

/**
 * Protocol differences between CarLife receiver generations, selected from the major version the
 * head unit announces in 98305. Both profiles share framing, ports, touch and video delivery;
 * only the session handshake and the video parameter-set policy differ.
 */
internal enum class ReceiverProfile(
    internal val majors: LongRange,
    /** A 4.x receiver challenges the sender and waits for its result; 3.x has no challenge. */
    internal val challenges: Boolean,
    /** 4.x accepts in-band parameter sets in every keyframe; 3.x stores them once per decoder. */
    internal val parameterSetsInEveryKeyframe: Boolean,
) {
    /** 3.x generation (CarLife 3.0 apps, protocol 1.0). */
    Legacy(1L..1L, challenges = false, parameterSetsInEveryKeyframe = false),

    /** 4.x generation (protocol 4.x); the majors the previous implementation accepted. */
    Current(3L..4L, challenges = true, parameterSetsInEveryKeyframe = true),
    ;

    companion object {
        fun of(major: Long): ReceiverProfile? = entries.firstOrNull { major in it.majors }
    }
}
