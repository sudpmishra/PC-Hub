//! Recent readings drawn as small graphs behind the Monitor page's cards. A graph is SVG
//! path commands in a box SAMPLES - 1 wide and 100 tall that the UI stretches to fit,
//! with the newest reading on the right.

use std::collections::VecDeque;

/// How many readings a graph shows: a minute at the default one-second refresh.
pub const SAMPLES: usize = 60;

#[derive(Default)]
pub struct History {
    values: VecDeque<f32>,
}

impl History {
    pub fn push(&mut self, value: f32) {
        if self.values.len() == SAMPLES {
            self.values.pop_front();
        }
        self.values.push_back(value.max(0.0));
    }

    pub fn max(&self) -> f32 {
        self.values.iter().copied().fold(0.0, f32::max)
    }

    /// The line along the readings and the filled area under it, scaled so `top` reaches
    /// the top of the box. Both are empty until there are two readings to join.
    pub fn graph(&self, top: f32) -> (String, String) {
        if self.values.len() < 2 || top <= 0.0 {
            return (String::new(), String::new());
        }
        let start = SAMPLES - self.values.len();
        let points: Vec<String> = self
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| format!("{} {:.1}", start + i, 100.0 - (v / top).min(1.0) * 100.0))
            .collect();
        let line = format!("M{}", points.join(" L"));
        let area = format!("M{start} 100 L{} L{} 100 Z", points.join(" L"), SAMPLES - 1);
        (line, area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_grows_from_the_right() {
        let mut history = History::default();
        assert_eq!(history.graph(1.0), (String::new(), String::new()));
        history.push(0.0);
        history.push(0.5);
        history.push(2.0);
        let (line, area) = history.graph(1.0);
        assert_eq!(line, "M57 100.0 L58 50.0 L59 0.0");
        assert_eq!(area, "M57 100 L57 100.0 L58 50.0 L59 0.0 L59 100 Z");
        assert_eq!(history.max(), 2.0);
    }

    #[test]
    fn keeps_only_the_latest_samples() {
        let mut history = History::default();
        for i in 0..SAMPLES + 10 {
            history.push(i as f32);
        }
        assert_eq!(history.values.len(), SAMPLES);
        assert_eq!(history.values[0], 10.0);
        assert!(history.graph(100.0).0.starts_with("M0 "));
    }
}
