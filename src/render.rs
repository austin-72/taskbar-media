//! Layout and drawing. The widget is a per-pixel-alpha layered window, drawn
//! with GDI+ into a 32-bit bitmap so it blends with whatever the taskbar looks
//! like (dark, light, transparent, accent coloured).
#![allow(non_upper_case_globals)]

use std::ptr::null_mut;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    fn f(&self) -> RectF {
        RectF { X: self.x, Y: self.y, Width: self.w, Height: self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Hit {
    #[default]
    None,
    Art,
    Prev,
    Play,
    Next,
    Seek,
}

/// Where everything sits, in physical pixels relative to the widget.
#[derive(Clone, Default, PartialEq)]
pub struct Layout {
    pub width: f32,
    /// Layout unit: one 96-dpi pixel, shrunk further on small taskbars.
    pub u: f32,
    pub art: Rect,
    pub row1: Rect,
    pub row2: Rect,
    pub elapsed: Rect,
    pub total: Rect,
    pub track: Rect,
    pub seek_hit: Rect,
    pub buttons: [Rect; 3],
}

impl Layout {
    pub fn new(height: i32, scale: f32, time_chars: usize) -> Self {
        let h = height as f32;
        let u = scale.min(h / 40.0).max(0.5);

        let pad = (8.0 * u).round();
        let art_size = (h - 12.0 * u).min(40.0 * u).max(12.0).round();
        let art = Rect { x: pad, y: ((h - art_size) / 2.0).round(), w: art_size, h: art_size };

        let tx = art.x + art.w + (9.0 * u).round();
        let tw = (190.0 * u).round();
        let row1_h = 17.0 * u;
        let row2_h = 13.0 * u;
        let gap = 1.0 * u;
        let ty = ((h - (row1_h + gap + row2_h)) / 2.0).floor();
        let row1 = Rect { x: tx, y: ty, w: tw, h: row1_h };
        let row2 = Rect { x: tx, y: ty + row1_h + gap, w: tw, h: row2_h };

        let label_w = (time_chars.max(4) as f32 * 5.6 + 3.0) * u;
        let elapsed = Rect { w: label_w, ..row2 };
        let total = Rect { x: tx + tw - label_w, w: label_w, ..row2 };
        let track_h = (3.0 * u).round().max(2.0);
        let track_x = tx + label_w + 6.0 * u;
        let track = Rect {
            x: track_x,
            y: (row2.cy() - track_h / 2.0).round(),
            w: (total.x - 6.0 * u - track_x).max(10.0),
            h: track_h,
        };
        let seek_hit = Rect {
            x: track.x - 5.0 * u,
            y: row2.y - 3.0 * u,
            w: track.w + 10.0 * u,
            h: h - (row2.y - 3.0 * u),
        };

        let bw = (30.0 * u).round();
        let bh = art_size.min(32.0 * u).round();
        let bx = tx + tw + (6.0 * u).round();
        let by = ((h - bh) / 2.0).round();
        let buttons = [0.0, 1.0, 2.0].map(|i| Rect { x: bx + i * bw, y: by, w: bw, h: bh });

        Layout {
            width: bx + 3.0 * bw + (6.0 * u).round(),
            u,
            art,
            row1,
            row2,
            elapsed,
            total,
            track,
            seek_hit,
            buttons,
        }
    }

    pub fn hit(&self, x: f32, y: f32, has_timeline: bool) -> Hit {
        if has_timeline && self.seek_hit.contains(x, y) {
            return Hit::Seek;
        }
        // The art's click target is the full height of the taskbar.
        if x >= 0.0 && x < self.art.x + self.art.w + 3.0 * self.u {
            return Hit::Art;
        }
        for (i, b) in self.buttons.iter().enumerate() {
            if x >= b.x && x < b.x + b.w {
                return [Hit::Prev, Hit::Play, Hit::Next][i];
            }
        }
        Hit::None
    }

    pub fn seek_fraction(&self, x: f32) -> f32 {
        ((x - self.track.x) / self.track.w).clamp(0.0, 1.0)
    }
}

/// Everything that affects the picture. The window is only redrawn when this
/// differs from the last frame.
#[derive(Clone, Default, PartialEq)]
pub struct Frame {
    pub w: i32,
    pub h: i32,
    pub light: bool,
    pub title: String,
    pub artist: String,
    pub art_gen: u64,
    pub has_art: bool,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
    pub can_toggle: bool,
    pub can_seek: bool,
    pub has_timeline: bool,
    pub elapsed: String,
    pub total: String,
    pub fill_px: i32,
    pub hover: Hit,
    pub pressed: Hit,
    pub dragging: bool,
}

struct ArtCache {
    gen: u64,
    size: i32,
    bitmap: *mut GpBitmap,
}

pub struct Renderer {
    _token: usize,
    art: Option<ArtCache>,
}

/// 32-bit premultiplied ARGB, the format layered windows expect.
const PixelFormat32bppPARGB: u32 = 0x000E_200B;

fn alpha(color: u32, a: u32) -> u32 {
    (color & 0x00FF_FFFF) | (a << 24)
}

impl Renderer {
    pub fn new() -> Self {
        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        unsafe {
            GdiplusStartup(&mut token, &input, null_mut());
        }
        Renderer { _token: token, art: None }
    }

    /// `spotify` enables detection of the framed thumbnail some Spotify
    /// versions publish (see `legacy_spotify_crop`).
    pub fn paint(&mut self, hwnd: HWND, f: &Frame, l: &Layout, art: Option<&[u8]>, spotify: bool) {
        if f.w <= 0 || f.h <= 0 {
            return;
        }
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: f.w,
                    biHeight: -f.h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = null_mut();
            if let std::result::Result::Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                if !bits.is_null() {
                    let old = SelectObject(mem, dib.into());

                    let mut bitmap: *mut GpBitmap = null_mut();
                    GdipCreateBitmapFromScan0(
                        f.w,
                        f.h,
                        f.w * 4,
                        PixelFormat32bppPARGB as i32,
                        Some(bits as *const u8),
                        &mut bitmap,
                    );
                    if !bitmap.is_null() {
                        let mut g: *mut GpGraphics = null_mut();
                        GdipGetImageGraphicsContext(bitmap as *mut GpImage, &mut g);
                        if !g.is_null() {
                            self.draw(g, f, l, art, spotify);
                            GdipFlush(g, FlushIntentionSync);
                            GdipDeleteGraphics(g);
                        }
                        GdipDisposeImage(bitmap as *mut GpImage);
                    }

                    let size = SIZE { cx: f.w, cy: f.h };
                    let origin = POINT::default();
                    let blend = BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: 255,
                        AlphaFormat: AC_SRC_ALPHA as u8,
                    };
                    let _ = UpdateLayeredWindow(
                        hwnd,
                        Some(screen),
                        None,
                        Some(&size),
                        Some(mem),
                        Some(&origin),
                        COLORREF(0),
                        Some(&blend),
                        ULW_ALPHA,
                    );
                    SelectObject(mem, old);
                }
                let _ = DeleteObject(dib.into());
            }
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
        }
    }

    unsafe fn draw(
        &mut self,
        g: *mut GpGraphics,
        f: &Frame,
        l: &Layout,
        art: Option<&[u8]>,
        spotify: bool,
    ) {
        let u = l.u;
        let fg: u32 = if f.light { 0xFF00_0000 } else { 0xFFFF_FFFF };
        let primary = alpha(fg, if f.light { 0xE4 } else { 0xFF });
        let dim = alpha(fg, 0x9E);
        let disabled = alpha(fg, 0x48);
        let plate = alpha(fg, if f.light { 0x12 } else { 0x1C });
        let plate_down = alpha(fg, if f.light { 0x0A } else { 0x10 });

        // Alpha 1 rather than 0: fully transparent pixels of a layered window
        // let the mouse fall through to the taskbar underneath.
        GdipGraphicsClear(g, 0x0100_0000);
        GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        GdipSetTextRenderingHint(g, TextRenderingHintAntiAliasGridFit);

        let radius = 4.0 * u;

        // ---- album art -------------------------------------------------
        let size = l.art.w as i32;
        if self.art.as_ref().is_some_and(|c| c.gen != f.art_gen || c.size != size) {
            if let Some(c) = self.art.take() {
                GdipDisposeImage(c.bitmap as *mut GpImage);
            }
        }
        if self.art.is_none() && f.has_art {
            if let Some(bitmap) = art.and_then(|bytes| scale_art(bytes, size, spotify)) {
                self.art = Some(ArtCache { gen: f.art_gen, size, bitmap });
            }
        }
        match &self.art {
            Some(c) if f.has_art => {
                let mut tex: *mut GpTexture = null_mut();
                GdipCreateTexture(c.bitmap as *mut GpImage, WrapModeClamp, &mut tex);
                if !tex.is_null() {
                    GdipTranslateTextureTransform(tex, l.art.x, l.art.y, MatrixOrderAppend);
                    fill_round(g, tex as *mut GpBrush, l.art, radius);
                    GdipDeleteBrush(tex as *mut GpBrush);
                }
            }
            _ => {
                fill_round_color(g, alpha(fg, 0x24), l.art, radius);
                let font = make_font(w!("Segoe UI Symbol"), l.art.h * 0.55);
                let fmt = make_format(StringAlignmentCenter);
                draw_text(g, "\u{266B}", font, l.art, fmt, dim);
                GdipDeleteFont(font);
                GdipDeleteStringFormat(fmt);
            }
        }
        if f.pressed == Hit::Art && f.hover == Hit::Art {
            fill_round_color(g, 0x4000_0000, l.art, radius);
        } else if f.hover == Hit::Art {
            fill_round_color(g, 0x30FF_FFFF, l.art, radius);
        }

        // ---- text ------------------------------------------------------
        let title_font = make_font(w!("Segoe UI Semibold"), 12.0 * u);
        let body_font = make_font(w!("Segoe UI"), 12.0 * u);
        let small_font = make_font(w!("Segoe UI"), 10.0 * u);
        let near = make_format(StringAlignmentNear);
        let far = make_format(StringAlignmentFar);

        let title = if f.title.is_empty() { "Unknown track" } else { f.title.as_str() };
        if f.has_timeline {
            // Row 1: "Title · Artist", row 2: the seek bar.
            let title_w = measure(g, title, title_font, l.row1, near).min(l.row1.w);
            draw_text(g, title, title_font, l.row1, near, primary);
            let rest = l.row1.w - title_w;
            if !f.artist.is_empty() && rest > 36.0 * u {
                let r = Rect { x: l.row1.x + title_w, w: rest, ..l.row1 };
                draw_text(g, &format!("\u{00B7} {}", f.artist), body_font, r, near, dim);
            }

            draw_text(g, &f.elapsed, small_font, l.elapsed, near, dim);
            draw_text(g, &f.total, small_font, l.total, far, dim);

            let t = l.track;
            fill_round_color(g, alpha(fg, 0x3A), t, t.h / 2.0);
            let fill = (f.fill_px as f32).clamp(0.0, t.w);
            if fill >= 1.0 {
                let color = if f.can_seek { primary } else { dim };
                fill_round_color(g, color, Rect { w: fill.max(t.h), ..t }, t.h / 2.0);
            }
            if f.can_seek && (f.hover == Hit::Seek || f.dragging) {
                let r = if f.dragging { 5.0 * u } else { 4.0 * u };
                let brush = solid(alpha(fg, 0xFF));
                GdipFillEllipse(g, brush, t.x + fill - r, t.cy() - r, r * 2.0, r * 2.0);
                GdipDeleteBrush(brush);
            }
        } else {
            // No timeline from this player: title above, artist below.
            draw_text(g, title, title_font, l.row1, near, primary);
            let artist_font = make_font(w!("Segoe UI"), 11.0 * u);
            draw_text(g, &f.artist, artist_font, l.row2, near, dim);
            GdipDeleteFont(artist_font);
        }

        GdipDeleteFont(title_font);
        GdipDeleteFont(body_font);
        GdipDeleteFont(small_font);
        GdipDeleteStringFormat(near);
        GdipDeleteStringFormat(far);

        // ---- transport buttons ------------------------------------------
        let kinds = [Hit::Prev, Hit::Play, Hit::Next];
        let enabled = [f.can_prev, f.can_toggle, f.can_next];
        for i in 0..3 {
            let b = l.buttons[i];
            if enabled[i] && f.hover == kinds[i] {
                let inner = Rect { x: b.x + u, w: b.w - 2.0 * u, ..b };
                let c = if f.pressed == kinds[i] { plate_down } else { plate };
                fill_round_color(g, c, inner, radius);
            }
            let brush = solid(if enabled[i] { primary } else { disabled });
            let (cx, cy) = (b.cx(), b.cy());
            match kinds[i] {
                Hit::Play if f.playing => {
                    GdipFillRectangle(g, brush, cx - 5.0 * u, cy - 6.0 * u, 3.5 * u, 12.0 * u);
                    GdipFillRectangle(g, brush, cx + 1.5 * u, cy - 6.0 * u, 3.5 * u, 12.0 * u);
                }
                Hit::Play => {
                    polygon(g, brush, &[(cx - 4.0 * u, cy - 6.5 * u), (cx - 4.0 * u, cy + 6.5 * u), (cx + 6.5 * u, cy)]);
                }
                Hit::Next => {
                    polygon(g, brush, &[(cx - 5.5 * u, cy - 5.0 * u), (cx - 5.5 * u, cy + 5.0 * u), (cx + 3.0 * u, cy)]);
                    GdipFillRectangle(g, brush, cx + 3.5 * u, cy - 5.0 * u, 2.0 * u, 10.0 * u);
                }
                _ => {
                    polygon(g, brush, &[(cx + 5.5 * u, cy - 5.0 * u), (cx + 5.5 * u, cy + 5.0 * u), (cx - 3.0 * u, cy)]);
                    GdipFillRectangle(g, brush, cx - 5.5 * u, cy - 5.0 * u, 2.0 * u, 10.0 * u);
                }
            }
            GdipDeleteBrush(brush);
        }
    }
}

unsafe fn solid(color: u32) -> *mut GpBrush {
    let mut brush: *mut GpSolidFill = null_mut();
    GdipCreateSolidFill(color, &mut brush);
    brush as *mut GpBrush
}

unsafe fn polygon(g: *mut GpGraphics, brush: *mut GpBrush, pts: &[(f32, f32)]) {
    let pts: Vec<PointF> = pts.iter().map(|&(x, y)| PointF { X: x, Y: y }).collect();
    GdipFillPolygon(g, brush, pts.as_ptr(), pts.len() as i32, FillModeAlternate);
}

unsafe fn fill_round(g: *mut GpGraphics, brush: *mut GpBrush, r: Rect, radius: f32) {
    let d = (radius * 2.0).min(r.w).min(r.h).max(0.1);
    let mut path: *mut GpPath = null_mut();
    GdipCreatePath(FillModeAlternate, &mut path);
    if path.is_null() {
        return;
    }
    GdipAddPathArc(path, r.x, r.y, d, d, 180.0, 90.0);
    GdipAddPathArc(path, r.x + r.w - d, r.y, d, d, 270.0, 90.0);
    GdipAddPathArc(path, r.x + r.w - d, r.y + r.h - d, d, d, 0.0, 90.0);
    GdipAddPathArc(path, r.x, r.y + r.h - d, d, d, 90.0, 90.0);
    GdipClosePathFigure(path);
    GdipFillPath(g, brush, path);
    GdipDeletePath(path);
}

unsafe fn fill_round_color(g: *mut GpGraphics, color: u32, r: Rect, radius: f32) {
    let brush = solid(color);
    fill_round(g, brush, r, radius);
    GdipDeleteBrush(brush);
}

unsafe fn make_font(family: PCWSTR, px: f32) -> *mut GpFont {
    let mut fam: *mut GpFontFamily = null_mut();
    GdipCreateFontFamilyFromName(family, null_mut(), &mut fam);
    if fam.is_null() {
        GdipCreateFontFamilyFromName(w!("Segoe UI"), null_mut(), &mut fam);
    }
    if fam.is_null() {
        GdipGetGenericFontFamilySansSerif(&mut fam);
    }
    let mut font: *mut GpFont = null_mut();
    GdipCreateFont(fam, px.max(6.0), 0, UnitPixel, &mut font);
    GdipDeleteFontFamily(fam);
    font
}

unsafe fn make_format(align: StringAlignment) -> *mut GpStringFormat {
    const NO_WRAP: i32 = 0x1000;
    let mut fmt: *mut GpStringFormat = null_mut();
    GdipCreateStringFormat(NO_WRAP, 0, &mut fmt);
    GdipSetStringFormatAlign(fmt, align);
    GdipSetStringFormatLineAlign(fmt, StringAlignmentCenter);
    GdipSetStringFormatTrimming(fmt, StringTrimmingEllipsisCharacter);
    fmt
}

unsafe fn draw_text(
    g: *mut GpGraphics,
    text: &str,
    font: *mut GpFont,
    r: Rect,
    fmt: *mut GpStringFormat,
    color: u32,
) {
    if text.is_empty() || font.is_null() {
        return;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let brush = solid(color);
    GdipDrawString(g, PCWSTR(wide.as_ptr()), wide.len() as i32, font, &r.f(), fmt, brush);
    GdipDeleteBrush(brush);
}

unsafe fn measure(
    g: *mut GpGraphics,
    text: &str,
    font: *mut GpFont,
    r: Rect,
    fmt: *mut GpStringFormat,
) -> f32 {
    if text.is_empty() || font.is_null() {
        return 0.0;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut out = RectF::default();
    GdipMeasureString(
        g,
        PCWSTR(wide.as_ptr()),
        wide.len() as i32,
        font,
        &r.f(),
        fmt,
        &mut out,
        null_mut(),
        null_mut(),
    );
    out.Width.ceil()
}

/// Some Spotify desktop builds publish a 300x300 thumbnail in which the cover
/// is a 234 px square at (33, 0), with blank side bars and a logo strip
/// underneath. Recognise that by its perfectly uniform side bars.
unsafe fn legacy_spotify_frame(bitmap: *mut GpBitmap, w: u32, h: u32) -> bool {
    if w != 300 || h != 300 {
        return false;
    }
    let mut first = 0u32;
    GdipBitmapGetPixel(bitmap, 0, 0, &mut first);
    let close = |a: u32, b: u32| {
        (0..4).all(|i| (((a >> (i * 8)) & 0xFF) as i32 - ((b >> (i * 8)) & 0xFF) as i32).abs() <= 6)
    };
    for y in (0..234).step_by(6) {
        for x in [0, 6, 12, 18, 24, 30, 270, 276, 282, 288, 294, 299] {
            let mut px = 0u32;
            GdipBitmapGetPixel(bitmap, x, y, &mut px);
            if !close(px, first) {
                return false;
            }
        }
    }
    // A cover that is one flat colour all over is not the framed layout.
    let mut centre = 0u32;
    GdipBitmapGetPixel(bitmap, 150, 117, &mut centre);
    let mut inner = 0u32;
    GdipBitmapGetPixel(bitmap, 60, 40, &mut inner);
    !(close(centre, first) && close(inner, first))
}

/// Decode the artwork and scale a centred square crop of it to `size` pixels.
unsafe fn scale_art(bytes: &[u8], size: i32, spotify: bool) -> Option<*mut GpBitmap> {
    if size <= 0 {
        return None;
    }
    // GDI+ reads from the stream lazily, so it has to outlive `src`.
    let stream = SHCreateMemStream(Some(bytes))?;
    let mut src: *mut GpBitmap = null_mut();
    GdipCreateBitmapFromStream(&stream, &mut src);
    if src.is_null() {
        return None;
    }
    let (mut iw, mut ih) = (0u32, 0u32);
    GdipGetImageWidth(src as *mut GpImage, &mut iw);
    GdipGetImageHeight(src as *mut GpImage, &mut ih);

    let mut dst: *mut GpBitmap = null_mut();
    if iw > 0 && ih > 0 {
        GdipCreateBitmapFromScan0(size, size, 0, PixelFormat32bppPARGB as i32, None, &mut dst);
    }
    if !dst.is_null() {
        let mut side = iw.min(ih) as i32;
        let mut sx = (iw as i32 - side) / 2;
        let mut sy = (ih as i32 - side) / 2;
        if spotify && legacy_spotify_frame(src, iw, ih) {
            (sx, sy, side) = (33, 0, 234);
        }
        let mut g: *mut GpGraphics = null_mut();
        GdipGetImageGraphicsContext(dst as *mut GpImage, &mut g);
        if !g.is_null() {
            GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
            GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
            // Mirror at the edges so bicubic sampling doesn't fade the border.
            let mut attrs: *mut GpImageAttributes = null_mut();
            GdipCreateImageAttributes(&mut attrs);
            if !attrs.is_null() {
                GdipSetImageAttributesWrapMode(attrs, WrapModeTileFlipXY, 0, false);
            }
            GdipDrawImageRectRectI(
                g,
                src as *mut GpImage,
                0,
                0,
                size,
                size,
                sx,
                sy,
                side,
                side,
                UnitPixel,
                attrs,
                0,
                null_mut(),
            );
            if !attrs.is_null() {
                GdipDisposeImageAttributes(attrs);
            }
            GdipDeleteGraphics(g);
        }
    }
    GdipDisposeImage(src as *mut GpImage);
    drop(stream);
    if dst.is_null() {
        None
    } else {
        Some(dst)
    }
}
