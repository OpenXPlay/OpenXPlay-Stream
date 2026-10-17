// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-16

//! Complete plugin context snapshots. No dependency on a plugin runtime.
use crate::{require, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ContextTopic {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "nowPlaying")]
    NowPlaying,
    #[serde(rename = "call")]
    Call,
    #[serde(rename = "activity")]
    Activity,
    #[serde(rename = "route")]
    Route,
    #[serde(rename = "audioSource")]
    AudioSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CarPlayState {
    Connected,
    Disconnected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallState {
    Idle,
    Ringing,
    Dialing,
    Active,
    Held,
    Ending,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionContext {
    pub carplay: Option<CarPlayState>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallContext {
    pub state: Option<CallState>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityContext {
    pub siri: Option<bool>,
    pub phone_call: Option<bool>,
    pub navigation: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artwork {
    pub url: Option<String>,
    pub r#type: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NowPlayingContext {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub playing: Option<bool>,
    pub rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<Artwork>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteContext {
    pub active: bool,
    pub destination: Option<String>,
    pub eta_ms: Option<f64>,
    pub distance_remaining_m: Option<f64>,
    pub maneuver: Option<RouteManeuver>,
    pub app_name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteManeuver {
    pub r#type: Option<u8>,
    pub road: Option<String>,
    pub distance_m: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSourceContext {
    pub app_name: Option<String>,
}
/// The variant fixes the topic/value pairing; None explicitly clears that topic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "topic", content = "value")]
pub enum ContextUpdate {
    #[serde(rename = "session")]
    Session(Option<SessionContext>),
    #[serde(rename = "nowPlaying")]
    NowPlaying(Option<NowPlayingContext>),
    #[serde(rename = "call")]
    Call(Option<CallContext>),
    #[serde(rename = "activity")]
    Activity(Option<ActivityContext>),
    #[serde(rename = "route")]
    Route(Option<RouteContext>),
    #[serde(rename = "audioSource")]
    AudioSource(Option<AudioSourceContext>),
}
impl ContextUpdate {
    pub fn topic(&self) -> ContextTopic {
        match self {
            Self::Session(_) => ContextTopic::Session,
            Self::NowPlaying(_) => ContextTopic::NowPlaying,
            Self::Call(_) => ContextTopic::Call,
            Self::Activity(_) => ContextTopic::Activity,
            Self::Route(_) => ContextTopic::Route,
            Self::AudioSource(_) => ContextTopic::AudioSource,
        }
    }
    pub fn from_json(v: &Value) -> Result<Self> {
        let o = v.as_object().ok_or("Expected context update")?;
        require(
            o.contains_key("topic") && o.contains_key("value"),
            "Missing context field",
        )?;
        let topic: ContextTopic = serde_json::from_value(v["topic"].clone())?;
        let value = &v["value"];
        if !value.is_null() {
            let fields: &[&str] = match topic {
                ContextTopic::Route => &[
                    "active",
                    "destination",
                    "etaMs",
                    "distanceRemainingM",
                    "maneuver",
                    "appName",
                ],
                ContextTopic::AudioSource => &["appName"],
                ContextTopic::Session => &["carplay"],
                ContextTopic::Call => &["state"],
                ContextTopic::Activity => &["siri", "phoneCall", "navigation"],
                ContextTopic::NowPlaying => &[
                    "title",
                    "artist",
                    "album",
                    "durationMs",
                    "elapsedMs",
                    "playing",
                    "rate",
                ],
            };
            let object = value.as_object().ok_or("Expected topic object")?;
            require(
                fields.iter().all(|k| object.contains_key(*k)),
                "Missing topic field",
            )?;
            if topic == ContextTopic::Route {
                if let Some(m) = object.get("maneuver").filter(|v| !v.is_null()) {
                    let m = m.as_object().ok_or("Expected maneuver object")?;
                    require(
                        ["type", "road", "distanceM"]
                            .iter()
                            .all(|k| m.contains_key(*k)),
                        "Missing maneuver field",
                    )?;
                }
            }
            for key in ["durationMs", "elapsedMs"] {
                if let Some(n) = object
                    .get(key)
                    .filter(|n| topic == ContextTopic::NowPlaying && !n.is_null())
                {
                    require(
                        n.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991),
                        "Invalid milliseconds",
                    )?;
                }
            }
        }
        if let Some(artwork) = value
            .get("artwork")
            .filter(|v| topic == ContextTopic::NowPlaying && !v.is_null())
        {
            let a = artwork.as_object().ok_or("Expected artwork object")?;
            require(
                a.contains_key("url") && a.contains_key("type"),
                "Missing artwork field",
            )?;
        }
        let update: Self = serde_json::from_value(v.clone())?;
        if let Self::NowPlaying(Some(n)) = &update {
            require(
                n.rate.is_none_or(|r| r.is_finite()),
                "Invalid playback rate",
            )?;
        }
        update.validate_route()?;
        Ok(update)
    }
    pub fn to_json(&self) -> Result<Value> {
        self.validate_route()?;
        let v = serde_json::to_value(self)?;
        Self::from_json(&v)?;
        if let Self::NowPlaying(Some(n)) = self {
            require(
                n.rate.is_none_or(|r| r.is_finite()),
                "Invalid playback rate",
            )?;
        }
        Ok(v)
    }
    fn validate_route(&self) -> Result<()> {
        if let Self::Route(Some(r)) = self {
            for n in [
                r.eta_ms,
                r.distance_remaining_m,
                r.maneuver.as_ref().and_then(|m| m.distance_m),
            ]
            .into_iter()
            .flatten()
            {
                require(n.is_finite() && n >= 0.0, "Invalid route number")?;
            }
        }
        Ok(())
    }
    pub fn idle() -> [Self; 6] {
        [
            Self::Session(Some(SessionContext {
                carplay: Some(CarPlayState::Disconnected),
            })),
            Self::NowPlaying(None),
            Self::Call(Some(CallContext {
                state: Some(CallState::Idle),
            })),
            Self::Activity(Some(ActivityContext {
                siri: Some(false),
                phone_call: Some(false),
                navigation: Some(false),
            })),
            Self::Route(None),
            Self::AudioSource(None),
        ]
    }
}
