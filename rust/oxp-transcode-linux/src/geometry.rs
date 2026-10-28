// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-27

//! Carlink sends the active area stretched to the encoded frame; the receiver stretches that
//! frame to its window. Touch coordinates therefore map back through the active area only.

use oxp_core::model::{TouchContact, ViewArea};

pub fn crop(width: u32, height: u32, requested: Option<ViewArea>) -> ViewArea {
    requested
        .filter(|a| {
            a.validate().is_ok()
                && a.width >= 2
                && a.height >= 2
                && a.canvas_width == width
                && a.canvas_height == height
        })
        .unwrap_or(ViewArea {
            x: 0,
            y: 0,
            width,
            height,
            canvas_width: width,
            canvas_height: height,
        })
}

pub fn map(area: ViewArea, contact: &mut TouchContact) {
    contact.x = (area.x as f64 + contact.x * area.width as f64) / area.canvas_width as f64;
    contact.y = (area.y as f64 + contact.y * area.height as f64) / area.canvas_height as f64;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn touch_corners_and_releases_map_through_the_crop() {
        let area = ViewArea {
            x: 100,
            y: 200,
            width: 1200,
            height: 800,
            canvas_width: 1600,
            canvas_height: 1200,
        };
        for (x, y) in [(0., 0.), (1., 0.), (0., 1.), (1., 1.), (0.5, 0.5)] {
            let mut c = TouchContact::new(2, x, y, false).unwrap();
            map(area, &mut c);
            assert_eq!(c.x, (100. + x * 1200.) / 1600.);
            assert_eq!(c.y, (200. + y * 800.) / 1200.);
            assert_eq!(c.id, 2);
            assert!(!c.pressed);
        }
        assert_eq!(crop(1280, 720, Some(area)), crop(1280, 720, None));
    }
}
