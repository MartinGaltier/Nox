//! Executes a [`Scene`] in the current AppKit graphics context (flipped view).

use super::util::{ns_color, ns_rect};
use crate::geom::{Point, Rect};
use crate::geometry::Seg;
use crate::scene::{Align, Op, Scene, TextStyle, Weight};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBezierPath, NSCompositingOperation, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSGraphicsContext, NSImage, NSImageSymbolConfiguration,
    NSLineBreakMode, NSLineCapStyle, NSLineJoinStyle, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSStringDrawingOptions, NSStringNSExtendedStringDrawing,
    NSTextAlignment,
};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSPoint, NSRect, NSSize, NSString};
use std::collections::HashMap;

fn font_weight(w: Weight) -> f64 {
    // AppKit's NSFontWeight values.
    match w {
        Weight::Regular => 0.0,
        Weight::Medium => 0.23,
        Weight::Semibold => 0.3,
        Weight::Bold => 0.4,
    }
}

/// Symbol cache key: name, size×4, RGBA bytes, weight.
type SymbolKey = (String, u32, [u8; 4], u8);

#[derive(Default)]
pub struct Painter {
    symbols: HashMap<SymbolKey, Option<Retained<NSImage>>>,
    artwork: HashMap<u64, Option<Retained<NSImage>>>,
    /// Downscaled thumbnails of image files, by path (full-size screenshots
    /// are never kept decoded).
    thumbs: HashMap<String, Option<Retained<NSImage>>>,
}

/// Source of decoded artwork bytes for an id.
pub type ArtworkSource<'a> = &'a dyn Fn(u64) -> Option<std::sync::Arc<Vec<u8>>>;

pub fn path(segs: &[Seg], dx: f64, dy: f64) -> Retained<NSBezierPath> {
    let p = NSBezierPath::bezierPath();
    let pt = |q: &Point| NSPoint::new(q.x + dx, q.y + dy);
    for s in segs {
        match s {
            Seg::Move(a) => p.moveToPoint(pt(a)),
            Seg::Line(a) => p.lineToPoint(pt(a)),
            Seg::Curve(c1, c2, e) => {
                p.curveToPoint_controlPoint1_controlPoint2(pt(e), pt(c1), pt(c2))
            }
            Seg::Close => p.closePath(),
        }
    }
    p
}

fn rounded(r: Rect, radius: f64) -> Retained<NSBezierPath> {
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(r), rad, rad)
}

fn cg() -> Option<Retained<CGContext>> {
    NSGraphicsContext::currentContext().map(|c| c.CGContext())
}

impl Painter {
    /// Drops cached artwork no longer referenced.
    pub fn retain_artwork(&mut self, keep: &[u64]) {
        self.artwork.retain(|k, _| keep.contains(k));
        if self.symbols.len() > 400 {
            self.symbols.clear();
        }
    }

    fn symbol(
        &mut self,
        name: &str,
        size: f64,
        c: crate::geom::Color,
        w: Weight,
    ) -> Option<Retained<NSImage>> {
        let key = (
            name.to_string(),
            (size * 4.0) as u32,
            [
                (c.r * 255.0) as u8,
                (c.g * 255.0) as u8,
                (c.b * 255.0) as u8,
                (c.a * 255.0) as u8,
            ],
            w as u8,
        );
        self.symbols
            .entry(key)
            .or_insert_with(|| {
                let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(name),
                    None,
                )?;
                let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    size,
                    font_weight(w),
                );
                let colors = NSArray::from_retained_slice(&[ns_color(c)]);
                let cfg = cfg.configurationByApplyingConfiguration(
                    &NSImageSymbolConfiguration::configurationWithPaletteColors(&colors),
                );
                img.imageWithSymbolConfiguration(&cfg)
            })
            .clone()
    }

    /// Keeps only the thumbnails of these paths.
    pub fn retain_thumbnails(&mut self, keep: &[&str]) {
        self.thumbs.retain(|k, _| keep.contains(&k.as_str()));
    }

    fn thumbnail(&mut self, path: &str, rect: Rect) -> Option<Retained<NSImage>> {
        self.thumbs
            .entry(path.to_string())
            .or_insert_with(|| {
                let src = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path))?;
                let sz = src.size();
                // Twice the tile size for Retina, aspect preserved.
                let k = ((rect.w * 2.0) / sz.width.max(1.0)).max((rect.h * 2.0) / sz.height.max(1.0)).min(1.0);
                let tw = (sz.width * k).max(1.0);
                let th = (sz.height * k).max(1.0);
                let thumb = NSImage::initWithSize(NSImage::alloc(), NSSize::new(tw, th));
                #[allow(deprecated)]
                thumb.lockFocus();
                src.drawInRect_fromRect_operation_fraction(
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(tw, th)),
                    NSRect::ZERO,
                    NSCompositingOperation::Copy,
                    1.0,
                );
                #[allow(deprecated)]
                thumb.unlockFocus();
                Some(thumb)
            })
            .clone()
    }

    fn artwork_image(&mut self, id: u64, src: ArtworkSource) -> Option<Retained<NSImage>> {
        self.artwork
            .entry(id)
            .or_insert_with(|| {
                let bytes = src(id)?;
                let data = NSData::with_bytes(&bytes);
                NSImage::initWithData(NSImage::alloc(), &data)
            })
            .clone()
    }

    fn text(&self, r: Rect, text: &str, st: &TextStyle) {
        let font = if st.mono {
            NSFont::monospacedDigitSystemFontOfSize_weight(st.size, font_weight(st.weight))
        } else {
            NSFont::systemFontOfSize_weight(st.size, font_weight(st.weight))
        };
        let para = NSMutableParagraphStyle::new();
        para.setAlignment(match st.align {
            Align::Left => NSTextAlignment::Left,
            Align::Center => NSTextAlignment::Center,
            Align::Right => NSTextAlignment::Right,
        });
        para.setLineBreakMode(if st.lines > 1 {
            NSLineBreakMode::ByWordWrapping
        } else {
            NSLineBreakMode::ByTruncatingTail
        });
        let color = ns_color(st.color);
        // SAFETY: the attribute statics are valid NSString keys provided by
        // AppKit; values have the types AppKit expects for each key.
        let attrs = unsafe {
            NSDictionary::<objc2_foundation::NSString, AnyObject>::from_slices(
                &[
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ],
                &[
                    font.as_ref() as &AnyObject,
                    color.as_ref() as &AnyObject,
                    para.as_ref() as &AnyObject,
                ],
            )
        };
        let line_h = (st.size * 1.25).ceil();
        let h = r.h.max(line_h).min(line_h * st.lines as f64);
        let s = NSString::from_str(text);
        let rr = NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w.max(1.0), h));
        let opts = NSStringDrawingOptions::UsesLineFragmentOrigin
            | NSStringDrawingOptions::TruncatesLastVisibleLine;
        // SAFETY: attributes dictionary has string keys and AppKit value types.
        unsafe { s.drawWithRect_options_attributes_context(rr, opts, Some(&attrs), None) };
    }

    pub fn paint(&mut self, scene: &Scene, artwork: ArtworkSource) {
        let Some(gc) = NSGraphicsContext::currentContext() else {
            return;
        };
        let cgc = cg();
        let mut depth = 0;
        for op in &scene.ops {
            match op {
                Op::Fill {
                    rect,
                    radius,
                    color,
                } => {
                    if color.a <= 0.0 {
                        continue;
                    }
                    ns_color(*color).setFill();
                    rounded(*rect, *radius).fill();
                }
                Op::Stroke {
                    rect,
                    radius,
                    color,
                    width,
                } => {
                    ns_color(*color).setStroke();
                    let p = rounded(rect.inset(width / 2.0, width / 2.0), *radius);
                    p.setLineWidth(*width);
                    p.stroke();
                }
                Op::Ellipse { rect, color } => {
                    ns_color(*color).setFill();
                    NSBezierPath::bezierPathWithOvalInRect(ns_rect(*rect)).fill();
                }
                Op::Text { rect, text, style } => self.text(*rect, text, style),
                Op::Symbol {
                    rect,
                    name,
                    size,
                    color,
                    weight,
                } => {
                    if let Some(img) = self.symbol(name, *size, *color, *weight) {
                        let sz = img.size();
                        let r = rect.centered(sz.width, sz.height);
                        // SAFETY: drawing an image into the current context.
                        unsafe {
                            img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                                ns_rect(r),
                                NSRect::ZERO,
                                NSCompositingOperation::SourceOver,
                                1.0,
                                true,
                                None,
                            )
                        };
                    }
                }
                Op::Artwork { rect, id, radius } => {
                    gc.saveGraphicsState();
                    rounded(*rect, *radius).addClip();
                    if let Some(img) = self.artwork_image(*id, artwork) {
                        // Aspect fill.
                        let sz = img.size();
                        let k = (rect.w / sz.width.max(1.0)).max(rect.h / sz.height.max(1.0));
                        let r = rect.centered(sz.width * k, sz.height * k);
                        // SAFETY: drawing an image into the current context.
                        unsafe {
                            img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                                ns_rect(r),
                                NSRect::ZERO,
                                NSCompositingOperation::SourceOver,
                                1.0,
                                true,
                                None,
                            )
                        };
                    } else {
                        ns_color(crate::geom::Color::CARD_HOVER).setFill();
                        rounded(*rect, *radius).fill();
                    }
                    gc.restoreGraphicsState();
                }
                Op::Line {
                    points,
                    color,
                    width,
                } => {
                    if points.len() < 2 {
                        continue;
                    }
                    let p = NSBezierPath::bezierPath();
                    p.moveToPoint(NSPoint::new(points[0].x, points[0].y));
                    for q in &points[1..] {
                        p.lineToPoint(NSPoint::new(q.x, q.y));
                    }
                    p.setLineWidth(*width);
                    p.setLineCapStyle(NSLineCapStyle::Round);
                    p.setLineJoinStyle(NSLineJoinStyle::Round);
                    ns_color(*color).setStroke();
                    p.stroke();
                }
                Op::Area { points, color } => {
                    if points.len() < 3 {
                        continue;
                    }
                    let p = NSBezierPath::bezierPath();
                    p.moveToPoint(NSPoint::new(points[0].x, points[0].y));
                    for q in &points[1..] {
                        p.lineToPoint(NSPoint::new(q.x, q.y));
                    }
                    p.closePath();
                    ns_color(*color).setFill();
                    p.fill();
                }
                Op::Ring {
                    center,
                    radius,
                    width,
                    fraction,
                    color,
                    track,
                } => {
                    let c = NSPoint::new(center.x, center.y);
                    let t = NSBezierPath::bezierPathWithOvalInRect(ns_rect(Rect::new(
                        center.x - radius,
                        center.y - radius,
                        radius * 2.0,
                        radius * 2.0,
                    )));
                    t.setLineWidth(*width);
                    ns_color(*track).setStroke();
                    t.stroke();
                    if *fraction > 0.0 {
                        let p = NSBezierPath::bezierPath();
                        // Flipped view: angles grow clockwise on screen.
                        p.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                            c,
                            *radius,
                            -90.0,
                            -90.0 + 360.0 * fraction.clamp(0.0, 1.0),
                            false,
                        );
                        p.setLineWidth(*width);
                        p.setLineCapStyle(NSLineCapStyle::Round);
                        ns_color(*color).setStroke();
                        p.stroke();
                    }
                }
                Op::PushClip { rect, radius } => {
                    gc.saveGraphicsState();
                    depth += 1;
                    rounded(*rect, *radius).addClip();
                }
                Op::PopClip => {
                    if depth > 0 {
                        gc.restoreGraphicsState();
                        depth -= 1;
                    }
                }
                Op::Alpha(a) => CGContext::set_alpha(cgc.as_deref(), a.clamp(0.0, 1.0)),
                Op::FileImage { rect, path, radius } => {
                    gc.saveGraphicsState();
                    rounded(*rect, *radius).addClip();
                    match self.thumbnail(path, *rect) {
                        Some(img) => {
                            let sz = img.size();
                            let k = (rect.w / sz.width.max(1.0)).max(rect.h / sz.height.max(1.0));
                            let r = rect.centered(sz.width * k, sz.height * k);
                            // SAFETY: drawing an image into the current context.
                            unsafe {
                                img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                                    ns_rect(r),
                                    NSRect::ZERO,
                                    NSCompositingOperation::SourceOver,
                                    1.0,
                                    true,
                                    None,
                                )
                            };
                        }
                        None => {
                            ns_color(crate::geom::Color::CARD_HOVER).setFill();
                            rounded(*rect, *radius).fill();
                        }
                    }
                    gc.restoreGraphicsState();
                }
                Op::CameraHost { .. } | Op::TextEditor { .. } => {}
            }
        }
        while depth > 0 {
            gc.restoreGraphicsState();
            depth -= 1;
        }
        CGContext::set_alpha(cgc.as_deref(), 1.0);
    }
}
