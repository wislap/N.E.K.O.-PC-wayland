#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractiveRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl InteractiveRect {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InputRegion {
    rects: Vec<InteractiveRect>,
}

impl InputRegion {
    pub fn from_rects(rects: Vec<InteractiveRect>) -> Self {
        let mut region = Self { rects };
        region.normalize();
        region
    }

    pub fn rects(&self) -> &[InteractiveRect] {
        &self.rects
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    fn normalize(&mut self) {
        self.rects.retain(|rect| !rect.is_empty());
        self.rects.sort_by_key(|rect| (rect.y, rect.x, rect.width, rect.height));
        self.rects.dedup();
    }
}
