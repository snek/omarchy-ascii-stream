use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{Mutex, OnceCell};
use tokio::task::JoinHandle;

use crate::movie::Movie;
use crate::render::render_frame;

pub fn nearest(existing: &[(u32, u32)], cols: u32, rows: u32) -> (u32, u32) {
    existing.iter().copied().min_by(|a, b| {
        let rank = |s: (u32, u32)| {
            let dcols = (s.0 as i32 - cols as i32).abs();
            let drows = (s.1 as i32 - rows as i32).abs();
            let cells = (s.0 as i64) * (s.1 as i64);
            (dcols, drows, -cells)
        };
        rank(*a).cmp(&rank(*b))
    }).unwrap_or((cols, rows))
}

struct Slot {
    gate: Mutex<()>,
    value: OnceCell<Bytes>,
}

impl Slot {
    fn new() -> Self {
        Self { gate: Mutex::new(()), value: OnceCell::new() }
    }

    async fn get(&self, render: impl FnOnce() -> Vec<u8> + Send + 'static) -> (Bytes, bool) {
        if let Some(v) = self.value.get() {
            return (v.clone(), false);
        }
        let _guard = self.gate.lock().await;
        if let Some(v) = self.value.get() {
            return (v.clone(), false);
        }
        let rendered = tokio::task::spawn_blocking(render).await.expect("render");
        let bytes = Bytes::from(rendered);
        let _ = self.value.set(bytes.clone());
        (bytes, true)
    }
}

struct SizeState {
    slots: Vec<Slot>,
    fill: Mutex<Option<JoinHandle<()>>>,
}

struct Inner {
    order: Vec<(u32, u32)>,
    sizes: HashMap<(u32, u32), Arc<SizeState>>,
    retained: usize,
}

pub struct AnsiCache {
    budget: usize,
    inner: Mutex<Inner>,
}

impl AnsiCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            budget: budget_bytes,
            inner: Mutex::new(Inner {
                order: Vec::new(),
                sizes: HashMap::new(),
                retained: 0,
            }),
        }
    }

    pub async fn resolve(&self, cols: u32, rows: u32, frame_count: usize) -> (u32, u32) {
        let mut inner = self.inner.lock().await;
        if inner.sizes.contains_key(&(cols, rows)) {
            return (cols, rows);
        }
        let admit = inner.order.is_empty() || inner.retained < self.budget;
        if !admit {
            return nearest(&inner.order, cols, rows);
        }
        let slots = (0..frame_count).map(|_| Slot::new()).collect();
        inner.order.push((cols, rows));
        inner.sizes.insert((cols, rows), Arc::new(SizeState {
            slots,
            fill: Mutex::new(None),
        }));
        (cols, rows)
    }

    pub async fn frame(&self, movie: &Movie, cols: u32, rows: u32, index: usize) -> Bytes {
        let index = index % movie.frame_count();
        let size = {
            let inner = self.inner.lock().await;
            inner.sizes.get(&(cols, rows)).cloned().expect("resolve before frame")
        };
        let frame = movie.frames[index].clone();
        let cols_us = cols as usize;
        let rows_us = rows as usize;
        let (bytes, stored) = size.slots[index].get(move || render_frame(&frame, cols_us, rows_us)).await;
        if stored {
            let mut inner = self.inner.lock().await;
            inner.retained += bytes.len();
        }
        bytes
    }

    pub async fn spawn_fill(self: &Arc<Self>, movie: Arc<Movie>, cols: u32, rows: u32) {
        let size = {
            let inner = self.inner.lock().await;
            inner.sizes.get(&(cols, rows)).cloned()
        };
        let Some(size) = size else { return };
        let mut fill = size.fill.lock().await;
        if fill.is_some() {
            return;
        }
        let cache = Arc::clone(self);
        *fill = Some(tokio::spawn(async move {
            for index in 0..movie.frame_count() {
                let _ = cache.frame(&movie, cols, rows, index).await;
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movie::Movie;
    use crate::render::Frame;
    fn movie() -> Movie {
        let rgb = vec![1u8, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3];
        Movie::from_frames(
            vec![
                Frame { width: 2, height: 2, rgb: rgb.clone() },
                Frame { width: 2, height: 2, rgb },
            ],
            8.0,
            20,
            10,
        ).unwrap()
    }

    #[test]
    fn nearest_breaks_ties_by_cols_then_rows_then_cells_then_age() {
        assert_eq!(nearest(&[(10, 4), (40, 10)], 38, 8), (40, 10));
        assert_eq!(nearest(&[(10, 4), (10, 16)], 10, 10), (10, 16));
        assert_eq!(nearest(&[(8, 4), (8, 4)], 8, 4), (8, 4));
    }

    #[tokio::test]
    async fn second_call_reuses_the_stored_buffer() {
        let movie = movie();
        let cache = AnsiCache::new(10_000);
        let (cols, rows) = cache.resolve(2, 1, movie.frame_count()).await;
        let a = cache.frame(&movie, cols, rows, 0).await;
        let b = cache.frame(&movie, cols, rows, 0).await;
        assert_eq!(a, b);
        assert_eq!(a.as_ptr(), b.as_ptr());
    }

    #[tokio::test]
    async fn concurrent_waiters_share_one_buffer() {
        let movie = movie();
        let cache = std::sync::Arc::new(AnsiCache::new(10_000));
        let (cols, rows) = cache.resolve(2, 1, movie.frame_count()).await;
        let c1 = cache.clone();
        let c2 = cache.clone();
        let m1 = movie.clone();
        let m2 = movie.clone();
        let (a, b) = tokio::join!(
            tokio::spawn(async move { c1.frame(&m1, cols, rows, 0).await }),
            tokio::spawn(async move { c2.frame(&m2, cols, rows, 0).await }),
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(a.as_ptr(), b.as_ptr());
    }

    #[tokio::test]
    async fn second_size_snaps_once_budget_is_spent() {
        let movie = movie();
        let cache = AnsiCache::new(1);
        let first = cache.resolve(8, 4, movie.frame_count()).await;
        assert_eq!(first, (8, 4));
        let _ = cache.frame(&movie, first.0, first.1, 0).await;
        let second = cache.resolve(20, 6, movie.frame_count()).await;
        assert_eq!(second, first);
    }
}
