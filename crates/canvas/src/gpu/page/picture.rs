//! The page's own pictures, decoded off the frame thread at the size they show and let go
//! under a budget.

use super::{SETTLE, Slot, density, queue};
use draw::RasterImage;
use onestore::ExGuid;
use std::{collections::BTreeMap, sync::Arc, task::Waker, time::Duration};
use web_time::Instant;

/// Decoded bytes a scene's pictures keep at once. Pictures in view shrink to fit it, and
/// the rest of the renderer's frame budget stays for template art and the interface.
pub(super) const BUDGET: u64 = draw::MAX_IMAGE_BYTES / 2;

pub(super) struct Picture {
    /// The stored picture, which saving keeps as it is.
    bytes: Arc<[u8]>,
    /// The stored picture's size in pixels.
    native: [u32; 2],
    shown: Option<RasterImage>,
    /// The one decode under way and the size it makes; replacing it abandons it.
    pending: Option<(Slot<Option<RasterImage>>, [u32; 2])>,
    /// The stored bytes did not decode, so the picture paints as a placeholder.
    failed: bool,
    /// When the picture was last near the view; the longest unseen are let go first.
    seen: Instant,
}

impl Picture {
    /// Whether both show the same stored bytes.
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }

    /// Reads the size from the picture's header; None when the renderer cannot decode it.
    pub fn new(bytes: &Arc<[u8]>) -> Option<Self> {
        Some(Self {
            native: RasterImage::measure(bytes).ok()?,
            bytes: Arc::clone(bytes),
            shown: None,
            pending: None,
            failed: false,
            seen: Instant::now(),
        })
    }

    /// The latest raster, once one has landed.
    pub(crate) fn image(&self) -> Option<&RasterImage> {
        self.shown.as_ref()
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed
    }
}

fn bytes([width, height]: [u32; 2]) -> u64 {
    u64::from(width) * u64::from(height) * 4
}

/// Takes finished decodes and asks for the pictures `rects` places near `view` at `scale`
/// device pixels per point; `waker` hears when one lands. Pictures in view come first and
/// shrink together to fit the budget, the half view around it decodes ahead with what
/// room is left, and those unseen longest are let go past it. True once every picture in
/// view shows the raster it asked for.
pub(super) fn update(
    pictures: &mut BTreeMap<ExGuid, Picture>,
    rects: impl IntoIterator<Item = (ExGuid, [f32; 4])>,
    view: [f32; 4],
    scale: f32,
    waker: &Waker,
) -> bool {
    let now = Instant::now();
    for picture in pictures.values_mut() {
        let landed = picture
            .pending
            .as_ref()
            .and_then(|(slot, _)| slot.lock().unwrap().take());
        if let Some(image) = landed {
            picture.failed = image.is_none();
            picture.shown = image;
            picture.pending = None;
        }
    }
    let density = density(scale);
    let [x0, y0, x1, y1] = view;
    let [ahead_x, ahead_y] = [(x1 - x0) / 2.0, (y1 - y0) / 2.0];
    let near = [x0 - ahead_x, y0 - ahead_y, x1 + ahead_x, y1 + ahead_y];
    let overlaps = |rect: [f32; 4], area: [f32; 4]| {
        rect[0] <= area[2] && rect[2] >= area[0] && rect[1] <= area[3] && rect[3] >= area[1]
    };
    let mut wanted: Vec<_> = rects
        .into_iter()
        .filter_map(|(id, rect)| {
            let picture = pictures.get(&id)?;
            if picture.failed || !overlaps(rect, near) {
                return None;
            }
            let shown = [rect[2] - rect[0], rect[3] - rect[1]];
            let size = [0, 1]
                .map(|axis| ((shown[axis] * density).ceil() as u32).clamp(1, picture.native[axis]));
            Some((id, size, overlaps(rect, view)))
        })
        .collect();
    let visible: u64 = wanted
        .iter()
        .filter(|(.., visible)| *visible)
        .map(|(_, size, _)| bytes(*size))
        .sum();
    let shrink = (BUDGET as f64 / visible as f64).sqrt();
    let mut room = BUDGET.saturating_sub(visible);
    wanted.retain_mut(|(_, size, visible)| {
        if *visible && shrink < 1.0 {
            *size = size.map(|side| ((f64::from(side) * shrink) as u32).max(1));
        } else if !*visible {
            if bytes(*size) > room {
                return false;
            }
            room -= bytes(*size);
        }
        true
    });
    let wanted: BTreeMap<_, _> = wanted
        .into_iter()
        .map(|(id, size, visible)| (id, (size, visible)))
        .collect();
    let mut settled = true;
    for (id, picture) in pictures.iter_mut() {
        let Some(&(size, visible)) = wanted.get(id) else {
            picture.pending = None;
            continue;
        };
        picture.seen = now;
        let shown = picture.shown.as_ref().map(RasterImage::size);
        if shown != Some(size) && picture.pending.as_ref().map(|(_, made)| *made) != Some(size) {
            let encoded = Arc::clone(&picture.bytes);
            let start = now
                + if shown.is_some() {
                    SETTLE
                } else {
                    Duration::ZERO
                };
            let slot = queue(start, waker, move || {
                RasterImage::decode(&encoded, size).ok()
            });
            picture.pending = Some((slot, size));
        }
        settled &= !visible || shown == Some(size);
    }
    let mut kept: u64 = pictures
        .values()
        .filter_map(Picture::image)
        .map(|image| bytes(image.size()))
        .sum();
    if kept > BUDGET {
        let mut spare: Vec<_> = pictures
            .iter_mut()
            .filter(|(id, picture)| {
                picture.shown.as_ref().is_some_and(|image| {
                    wanted
                        .get(*id)
                        .is_none_or(|(size, _)| image.size() != *size)
                })
            })
            .collect();
        spare.sort_by_key(|(id, picture)| (wanted.contains_key(*id), picture.seen));
        for (_, picture) in spare {
            if kept <= BUDGET {
                break;
            }
            kept -= picture.shown.take().map_or(0, |image| bytes(image.size()));
        }
    }
    settled
}
