// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core.audio

/** AAC Main/LC/SSR/LTP with 1024-sample frames and a channel configuration (no PCE or extensions). */
public data class AudioSpecificConfig(
    public val objectType: Int,
    public val sampleRate: Int,
    public val channelConfiguration: Int,
    public val samplingFrequencyIndex: Int = frequencyIndex(sampleRate),
) {
    init {
        require(objectType in 1..4) { "Unsupported AAC object type" }
        require(sampleRate in 1..0xffffff) { "Invalid AAC sample rate" }
        require(channelConfiguration in 1..7) { "Unsupported AAC channel configuration" }
        require(samplingFrequencyIndex in 0..12 || samplingFrequencyIndex == 15) { "Reserved AAC frequency index" }
        require(samplingFrequencyIndex == 15 || rates[samplingFrequencyIndex] == sampleRate) {
            "AAC frequency index does not match rate"
        }
    }

    public fun build(): ByteArray {
        val bytes = ByteArray(if (samplingFrequencyIndex == 15) 5 else 2)
        var position = 0
        fun write(value: Int, count: Int) {
            for (shift in count - 1 downTo 0) {
                val bit = (value ushr shift) and 1
                bytes[position / 8] = (bytes[position / 8].toInt() or (bit shl (7 - position % 8))).toByte()
                position++
            }
        }
        write(objectType, 5)
        write(samplingFrequencyIndex, 4)
        if (samplingFrequencyIndex == 15) write(sampleRate, 24)
        write(channelConfiguration, 4)
        write(0, 3) // GASpecificConfig: frameLengthFlag, dependsOnCoreCoder, extensionFlag.
        return bytes
    }

    public companion object {
        private val rates: IntArray = intArrayOf(96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350)

        public fun frequencyIndex(sampleRate: Int): Int = rates.indexOf(sampleRate).let { if (it < 0) 15 else it }

        public fun rateForIndex(index: Int): Int {
            require(index in rates.indices) { "Reserved or explicit AAC frequency index" }
            return rates[index]
        }

        /** Rejects unsupported codec features rather than silently discarding their bits. */
        public fun parse(bytes: ByteArray): AudioSpecificConfig {
            var position = 0
            fun read(count: Int): Int {
                require(position.toLong() + count <= bytes.size.toLong() * 8) { "Truncated AudioSpecificConfig" }
                var value = 0
                repeat(count) {
                    value = (value shl 1) or ((bytes[position / 8].toInt() ushr (7 - position % 8)) and 1)
                    position++
                }
                return value
            }
            val type = read(5)
            require(type in 1..4) { "Unsupported AAC object type" }
            val index = read(4)
            val rate = if (index == 15) read(24) else rateForIndex(index)
            val channels = read(4)
            require(read(3) == 0) { "Unsupported AAC GASpecificConfig flags" }
            require(position == bytes.size * 8) { "Unexpected AudioSpecificConfig bytes" }
            return AudioSpecificConfig(type, rate, channels, index)
        }
    }
}
