// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

package org.openxplay.stream.core

class FakeInputContractTest : ProjectionInputContract() {
    override fun createInput(): ProjectionInput = FakeInput()
    override fun stimulate(input: ProjectionInput) {
        (input as FakeInput).emit { it.videoFrame(VideoFrame(ScreenId.MAIN, SessionId(1), byteArrayOf(1), 0, true, 0)) }
    }
}

class RecordingOutputContractTest : ProjectionOutputContract() {
    override fun createOutput(): ProjectionOutput = RecordingOutput()
    override fun videoFormat(): VideoFormat = VideoFormat.create(ScreenId.MAIN, VideoCodec.H264, vector("avc-baseline-640x480"))
    override fun stimulate(output: ProjectionOutput) {
        (output as RecordingOutput).host?.control(ControlEvent.KeyFrameRequest(ScreenId.MAIN, Origin(output.id, "client")))
    }
}
