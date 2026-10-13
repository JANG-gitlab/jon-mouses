use crate::{screen::Screen, store::Side};

/// Maps a point on our edge to the same physical spot on the neighbour's edge.
/// `offset_mm` is where the neighbour's edge starts relative to ours, so a
/// point outside the overlap has nowhere to land and the cursor stays put.
pub fn crossing(own: &Screen, peer: &Screen, side: Side, offset_mm: f32, fraction: f32) -> Option<f32> {
    let (own_length, peer_length) = match side {
        Side::Left | Side::Right => (own.height_mm, peer.height_mm),
        Side::Top | Side::Bottom => (own.width_mm, peer.width_mm),
    };
    let landing = (fraction * own_length - offset_mm) / peer_length;
    (0.0..=1.0).contains(&landing).then_some(landing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Screen = Screen { width_px: 1470.0, height_px: 956.0, width_mm: 300.0, height_mm: 195.0 };
    const MONITOR: Screen = Screen { width_px: 2560.0, height_px: 1440.0, width_mm: 597.0, height_mm: 336.0 };

    #[test]
    fn keeps_the_physical_height() {
        let landing = crossing(&LAPTOP, &MONITOR, Side::Right, -100.0, 0.5).unwrap();
        assert!((landing * MONITOR.height_mm - (97.5 + 100.0)).abs() < 0.01);
    }

    #[test]
    fn blocks_outside_the_overlap() {
        assert_eq!(crossing(&MONITOR, &LAPTOP, Side::Left, 200.0, 0.1), None);
        assert!(crossing(&MONITOR, &LAPTOP, Side::Left, 200.0, 0.9).is_some());
    }

    #[test]
    fn round_trip_lands_where_it_left() {
        let there = crossing(&LAPTOP, &MONITOR, Side::Right, -40.0, 0.3).unwrap();
        let back = crossing(&MONITOR, &LAPTOP, Side::Left, 40.0, there).unwrap();
        assert!((back - 0.3).abs() < 1e-5);
    }
}
