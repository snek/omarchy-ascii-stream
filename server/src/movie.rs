use crate::render::Frame;

#[derive(Clone)]
pub struct Movie {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub ref_cols: u32,
    pub ref_rows: u32,
    pub frames: Vec<Frame>,
}

impl Movie {
    pub fn from_frames(
        frames: Vec<Frame>,
        fps: f64,
        src_width: u32,
        src_height: u32,
    ) -> Result<Self, String> {
        let first = frames.first().ok_or_else(|| "no frames".to_string())?;
        Ok(Self {
            fps,
            width: src_width,
            height: src_height,
            ref_cols: first.width as u32,
            ref_rows: first.height as u32,
            frames,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}
