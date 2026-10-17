// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

use oxp_core::{model::*, ContextUpdate, InputSink};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Sink(Mutex<Vec<ContextUpdate>>);
impl InputSink for Sink {
    fn context(&self, update: ContextUpdate) {
        self.0.lock().unwrap().push(update);
    }
    fn session(&self, _: &SessionState) {}
    fn video_format(&self, _: u32, _: Option<Arc<VideoFormat>>) {}
    fn video_frame(&self, _: VideoFrame) {}
    fn audio_format(&self, _: u32, _: Option<Arc<AudioFormat>>) {}
    fn audio_packet(&self, _: AudioPacket) {}
    fn microphone(&self, _: u32, _: Option<MicRequest>) {}
}
#[test]
fn wire_consumer_forwards_typed_context_to_input_sink() {
    let sink = Sink::default();
    let mut consumer = oxp_bin::Consumer::default();
    let updates = ContextUpdate::idle();
    for update in &updates {
        let message = oxp_wire::Message::context(update).unwrap();
        let bytes = oxp_wire::encode(&message).unwrap();
        consumer
            .receive(oxp_wire::decode(&bytes).unwrap(), &sink)
            .unwrap();
    }
    assert_eq!(*sink.0.lock().unwrap(), updates);
}
