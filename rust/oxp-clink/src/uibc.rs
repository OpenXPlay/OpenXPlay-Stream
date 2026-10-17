// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use crate::{model::*, protocol::json_object, require, Result};
/// Android track IDs retain stable slots 0 and 1; insertion order is significant.
pub struct UibcDecoder {
    origin: Origin,
    active: Vec<(u32, u8, (f64, f64))>,
}
impl UibcDecoder {
    pub fn new(origin: Origin) -> Self {
        Self {
            origin,
            active: Vec::new(),
        }
    }
    pub fn release(&mut self) -> ControlEvent {
        self.active.clear();
        ControlEvent::Touch {
            screen: 0,
            contacts: Vec::new(),
            origin: self.origin.clone(),
        }
    }
    pub fn decode(&mut self, bytes: &[u8], video_size: (u32, u32)) -> Result<Option<ControlEvent>> {
        require(bytes.len() <= 65536, "invalid_uibc")?;
        let obj = json_object(bytes)?;
        require(obj.len() <= 128, "invalid_uibc")?;
        let number = |k: &str, default: Option<i64>| -> Result<i64> {
            match obj.get(k) {
                None => default.ok_or_else(|| format!("missing_{k}").into()),
                Some(v) => v.as_i64().ok_or_else(|| format!("invalid_{k}").into()),
            }
        };
        match number("type", None)? {
            2 => {
                let action = number("action", Some(0))?;
                let meta = number("metaState", Some(0))?;
                let code = number("keycode", None)?;
                require(
                    (0..=65535).contains(&code)
                        && (0..=2).contains(&action)
                        && (0..=0xffffffff).contains(&meta),
                    "invalid_uibc",
                )?;
                return key(code as u16, action as u8, self.origin.clone());
            }
            1 => {}
            _ => return Ok(None),
        }
        let count = number("count", None)?;
        let action = number("action", None)?;
        require(
            (0..=16).contains(&count) && (0..=65535).contains(&action),
            "invalid_uibc",
        )?;
        let masked = action & 255;
        let index = ((action >> 8) & 255) as usize;
        if masked == 3 {
            return Ok(Some(self.release()));
        }
        require(
            count > 0 && index < count as usize && [0, 1, 2, 5, 6].contains(&masked),
            "invalid_uibc",
        )?;
        let (w, h) = video_size;
        require(w > 0 && h > 0, "invalid_video_size")?;
        let mut points = Vec::new();
        for n in 0..count {
            let track = number(&format!("trackID{n}"), None)?;
            require(
                (0..=0xffffffff).contains(&track)
                    && !points.iter().any(|(t, _)| *t == track as u32),
                "invalid_uibc_track",
            )?;
            let x = (number(&format!("x{n}"), None)? as f64 / w as f64).clamp(0.0, 1.0);
            let y = (number(&format!("y{n}"), None)? as f64 / h as f64).clamp(0.0, 1.0);
            points.push((track as u32, (x, y)));
        }
        let lifted = if masked == 1 || masked == 6 {
            Some(points[index].0)
        } else {
            None
        };
        let mut contacts = Vec::new();
        let mut occupied: Vec<u8> = self.active.iter().map(|(_, s, _)| *s).collect();
        let old = std::mem::take(&mut self.active);
        for (track, slot, previous) in old {
            let p = points.iter().find(|(t, _)| *t == track).map(|(_, p)| *p);
            let pressed = Some(track) != lifted && p.is_some() && masked != 1;
            let p = p.unwrap_or(previous);
            contacts.push(TouchContact::new(slot, p.0, p.1, pressed)?);
            if pressed {
                self.active.push((track, slot, p));
            }
        }
        for (track, p) in points {
            if Some(track) != lifted
                && masked != 1
                && !self.active.iter().any(|(t, _, _)| *t == track)
            {
                if let Some(slot) = (0..=1).find(|s| !occupied.contains(s)) {
                    occupied.push(slot);
                    self.active.push((track, slot, p));
                    contacts.push(TouchContact::new(slot, p.0, p.1, true)?);
                }
            }
        }
        Ok(Some(ControlEvent::Touch {
            screen: 0,
            contacts,
            origin: self.origin.clone(),
        }))
    }
}
pub fn key(android_code: u16, action: u8, origin: Origin) -> Result<Option<ControlEvent>> {
    require(action <= 2, "invalid_key_action")?;
    let code = match android_code {
        3 => KeyCode::Home,
        4 => KeyCode::Back,
        23 | 66 => KeyCode::Select,
        19 => KeyCode::KnobUp,
        20 => KeyCode::KnobDown,
        21 => KeyCode::KnobLeft,
        22 => KeyCode::KnobRight,
        85 => KeyCode::PlayPause,
        87 => KeyCode::Next,
        88 => KeyCode::Previous,
        231 => KeyCode::Voice,
        _ => return Ok(None),
    };
    Ok(if action == 2 {
        None
    } else {
        Some(ControlEvent::Key {
            code,
            down: action == 0,
            origin,
        })
    })
}
