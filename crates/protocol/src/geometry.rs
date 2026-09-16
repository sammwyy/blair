#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x as f64
            && point.x < (self.x + self.width) as f64
            && point.y >= self.y as f64
            && point.y < (self.y + self.height) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_treats_the_far_edge_as_exclusive() {
        let rect = Rect {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
        };
        assert!(rect.contains(Point { x: 10.0, y: 10.0 }));
        assert!(rect.contains(Point { x: 29.9, y: 29.9 }));
        assert!(!rect.contains(Point { x: 30.0, y: 10.0 }));
        assert!(!rect.contains(Point { x: 9.9, y: 10.0 }));
    }
}
