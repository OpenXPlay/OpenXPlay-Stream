// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-18

use super::*;

#[derive(Default)]
struct Host(Mutex<Vec<OutputCapabilities>>);
impl OutputHost for Host {
    fn capabilities(&self, caps: OutputCapabilities) {
        self.0.lock().unwrap().push(caps);
    }
    fn control(&self, _: ControlEvent) {}
    fn stats(&self, _: OutputStats) {}
}
#[test]
fn hello_preference_is_strict() {
    for text in [
        include_str!("../../web-receiver/spec/vectors/web/hello-video-codec-h264.json"),
        include_str!("../../web-receiver/spec/vectors/web/hello-video-codec-auto.json"),
        include_str!("../../web-receiver/spec/vectors/web/hello.json"),
    ] {
        let mut hello: Value = serde_json::from_str(text).unwrap();
        json::validate(&hello, None).unwrap();
        for bad in [json!("H265"), json!(null), json!(1), json!(true), json!("")] {
            hello["videoCodec"] = bad;
            assert!(json::validate(&hello, None).is_err());
        }
    }
}
#[test]
fn repeated_hello_updates_preference_without_changing_hardware() {
    let config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        PathBuf::new(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    let web = WebOutput::new(config.clone()).unwrap();
    let host = Arc::new(Host::default());
    web.state.lock().unwrap().host = Some(host.clone());
    for id in ["a", "b"] {
        web.state
            .lock()
            .unwrap()
            .clients
            .insert(id.into(), Client::new(config.queue_duration_us));
    }
    let mut hello: Value = serde_json::from_str(include_str!(
        "../../web-receiver/spec/vectors/web/hello-video-codec-h264.json"
    ))
    .unwrap();
    hello.as_object_mut().unwrap().remove("videoCodec");
    handle(hello.clone(), "a", &config, &web.state, 0).unwrap();
    assert!(!host.0.lock().unwrap().last().unwrap().prefers_h264);
    hello["videoCodec"] = json!("H264");
    handle(hello.clone(), "b", &config, &web.state, 0).unwrap();
    {
        let caps = host.0.lock().unwrap();
        let caps = caps.last().unwrap();
        assert!(caps.prefers_h264);
        assert!(caps
            .decoders
            .iter()
            .any(|d| d.hardware && oxp_core::video_codec_policy::is_hevc(&d.codec_string)));
    }
    hello["displays"][0]["screen"] = json!(1);
    handle(hello.clone(), "b", &config, &web.state, 0).unwrap();
    assert!(!host.0.lock().unwrap().last().unwrap().prefers_h264);
    hello["displays"][0]["screen"] = json!(0);
    handle(hello.clone(), "b", &config, &web.state, 0).unwrap();
    assert!(host.0.lock().unwrap().last().unwrap().prefers_h264);
    hello["videoCodec"] = json!("AUTO");
    handle(hello.clone(), "b", &config, &web.state, 0).unwrap();
    assert!(!host.0.lock().unwrap().last().unwrap().prefers_h264);
    hello.as_object_mut().unwrap().remove("videoCodec");
    handle(hello, "b", &config, &web.state, 0).unwrap();
    assert!(!host.0.lock().unwrap().last().unwrap().prefers_h264);
}

#[test]
fn transcoding_capabilities_follow_every_main_screen_browser() {
    let mut config = WebOutputConfig::new(
        "127.0.0.1:0".parse().unwrap(),
        PathBuf::new(),
        Pairing::Disabled,
        Tls::Disabled,
    );
    config.common_video_decoders = true;
    let web = WebOutput::new(config.clone()).unwrap();
    let host = Arc::new(Host::default());
    web.state.lock().unwrap().host = Some(host.clone());
    let mut hello: Value = serde_json::from_str(include_str!(
        "../../web-receiver/spec/vectors/web/hello-video-codec-auto.json"
    ))
    .unwrap();
    for id in ["hevc", "avc"] {
        web.state
            .lock()
            .unwrap()
            .clients
            .insert(id.into(), Client::new(config.queue_duration_us));
        if id == "avc" {
            hello["decoders"].as_array_mut().unwrap().retain(|d| {
                !oxp_core::video_codec_policy::is_hevc(d["codecString"].as_str().unwrap())
            });
        }
        handle(hello.clone(), id, &config, &web.state, 0).unwrap();
    }
    assert!(!host
        .0
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .decoders
        .iter()
        .any(|d| oxp_core::video_codec_policy::is_hevc(&d.codec_string)));
    assert!(!host.0.lock().unwrap().last().unwrap().decoders.is_empty());
    web.state.lock().unwrap().clients.remove("avc");
    report_caps(&web.state);
    assert!(host
        .0
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .decoders
        .iter()
        .any(|d| oxp_core::video_codec_policy::is_hevc(&d.codec_string)));
}
