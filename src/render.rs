//! Layout and drawing. Each widget is a per-pixel-alpha layered window, drawn
//! with GDI+ into a 32-bit bitmap so it blends with whatever the taskbar looks
//! like (dark, light, transparent, accent coloured).
//!
//! In "smooth" mode the frame is drawn at 2-3x size and averaged down, which
//! gives text and shapes much finer anti-aliasing than GDI+ manages directly.
#![allow(non_upper_case_globals)]

use std::ptr::null_mut;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use crate::config::{
    FONTS, ITEM_COUNT, ITEM_OUTPUT, ITEM_PLAYERS, ITEM_SEEK, ITEM_TITLE, ITEM_ART, ITEM_GEAR, ITEM_INFO, ITEM_NEXT, ITEM_PLAY, ITEM_PREV, ITEM_REPEAT,
    ITEM_SHUFFLE,
};

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
    Gear,
    Art,
    Prev,
    Play,
    Next,
    Shuffle,
    Repeat,
    Output,
    PlayerUp,
    PlayerDown,
    Title,
    Seek,
}

/// Everything the layout depends on.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct LayoutParams {
    /// Taskbar height in pixels.
    pub height: i32,
    /// DPI scale of the taskbar's monitor (1.0 = 96 dpi).
    pub scale: f32,
    /// Length of the "total time" label, which sets the label column width.
    pub time_chars: usize,
    /// 0 small, 1 medium, 2 large.
    pub text_size: u32,
    /// 0 compact, 1 normal, 2 wide.
    pub width: u32,
    pub show_times: bool,
    /// Art and buttons only, for when there is no room for the text.
    pub mini: bool,
    /// The player supports shuffle or repeat, so those buttons can show.
    pub extra: bool,
    /// More than one player is running, so the switcher can show.
    pub players: bool,
    /// The shown items left to right (`config::ITEM_*`), zero-padded.
    pub order: [u8; ITEM_COUNT],
    /// 0 small, 1 medium, 2 large.
    pub button_size: u32,
    /// 0 thin, 1 normal, 2 thick.
    pub seek_size: u32,
}

impl LayoutParams {
    fn scaled(&self, k: i32) -> Self {
        LayoutParams { height: self.height * k, scale: self.scale * k as f32, ..*self }
    }
}

/// Where everything sits, in pixels relative to the widget. From the left:
/// settings gear, album art, shuffle, transport buttons, repeat, then title
/// over the seek bar.
#[derive(Clone, Default, PartialEq)]
pub struct Layout {
    pub width: f32,
    /// Layout unit: one 96-dpi pixel, shrunk further on small taskbars.
    pub u: f32,
    /// Text size multiplier.
    pub t: f32,
    /// Button size multiplier.
    pub b: f32,
    pub gear: Rect,
    pub art: Rect,
    pub buttons: [Rect; 3],
    /// Shuffle and repeat; zero-sized when not shown.
    pub extras: [Rect; 2],
    /// The output device button; zero-sized when not shown.
    pub output: Rect,
    /// The player switcher: an up arrow above a down arrow. Zero-sized when
    /// not shown.
    pub players: Rect,
    /// The title sits above the seek bar, rather than each on its own.
    pub stacked: bool,
    pub row1: Rect,
    pub row2: Rect,
    pub elapsed: Rect,
    pub total: Rect,
    pub track: Rect,
    pub seek_hit: Rect,
}

impl Layout {
    pub fn new(p: &LayoutParams) -> Self {
        let h = p.height as f32;
        let u = p.scale.min(h / 40.0).max(0.5);
        let t = [0.92, 1.0, 1.15][p.text_size.min(2) as usize];
        let text_w = [140.0, 190.0, 260.0][p.width.min(2) as usize];

        let bs = [0.85, 1.0, 1.18][p.button_size.min(2) as usize];

        let art_size = (h - 12.0 * u).min(40.0 * u).max(12.0).round();
        let art_y = ((h - art_size) / 2.0).round();
        let gear_size = (14.0 * u).round();
        let bw = (28.0 * u * bs).round();
        let ew = (23.0 * u * bs).round();
        let bh = art_size.min(32.0 * u * bs).round();
        let by = ((h - bh) / 2.0).round();

        let mut l = Layout { u, t, b: bs, ..Default::default() };

        // Walk the items left to right, giving each its place.
        let items: Vec<u8> = p
            .order
            .iter()
            .copied()
            .filter(|item| match *item {
                0 => false,
                ITEM_SHUFFLE | ITEM_REPEAT => p.extra && !p.mini,
                ITEM_PLAYERS => p.players,
                ITEM_INFO => !p.mini,
                // The stacked block already has both; don't show them twice.
                ITEM_TITLE | ITEM_SEEK => !p.mini && !p.order.contains(&ITEM_INFO),
                _ => true,
            })
            .collect();
        let mut x = match items.first() {
            Some(&ITEM_GEAR) | Some(&ITEM_PLAYERS) => (3.0 * u).round(),
            _ => (6.0 * u).round(),
        };
        let mut end_pad = 0.0;
        for item in items {
            end_pad = 0.0;
            match item {
                // Small, level with the top of the art.
                ITEM_GEAR => {
                    l.gear = Rect { x, y: art_y, w: gear_size, h: gear_size };
                    x += gear_size + (3.0 * u).round();
                }
                // A narrow column as tall as the art.
                ITEM_PLAYERS => {
                    let w = (14.0 * u).round();
                    l.players = Rect { x, y: art_y, w, h: art_size };
                    x += w + (3.0 * u).round();
                }
                ITEM_ART => {
                    l.art = Rect { x, y: art_y, w: art_size, h: art_size };
                    x += art_size + (6.0 * u).round();
                }
                ITEM_PREV | ITEM_PLAY | ITEM_NEXT => {
                    l.buttons[(item - ITEM_PREV) as usize] = Rect { x, y: by, w: bw, h: bh };
                    x += bw;
                    end_pad = (6.0 * u).round();
                }
                ITEM_SHUFFLE | ITEM_REPEAT => {
                    l.extras[usize::from(item == ITEM_REPEAT)] = Rect { x, y: by, w: ew, h: bh };
                    x += ew;
                    end_pad = (6.0 * u).round();
                }
                ITEM_OUTPUT => {
                    l.output = Rect { x, y: by, w: ew, h: bh };
                    x += ew;
                    end_pad = (6.0 * u).round();
                }
                ITEM_INFO | ITEM_TITLE | ITEM_SEEK => {
                    x += if x > 7.0 * u { (8.0 * u).round() } else { 0.0 };
                    x = l.place_text(p, x, h, text_w, item != ITEM_SEEK, item != ITEM_TITLE);
                    end_pad = (8.0 * u).round();
                }
                _ => {}
            }
        }
        l.width = (x + end_pad).max(12.0 * u);
        l
    }

    /// Lay out the title row and / or the seek bar row starting at `tx`:
    /// stacked when both, otherwise centred on the taskbar. Returns the x
    /// just past them.
    fn place_text(&mut self, p: &LayoutParams, tx: f32, h: f32, text_w: f32, title: bool, seek: bool) -> f32 {
        let (u, t) = (self.u, self.t);
        let tw = (text_w * u).round();
        let row1_h = 17.0 * u * t;
        let row2_h = 13.0 * u * t;
        let gap = 1.0 * u;
        let stacked = title && seek;
        let ty = ((h - (row1_h + gap + row2_h)) / 2.0).floor();
        if title {
            let y = if stacked { ty } else { ((h - row1_h) / 2.0).floor() };
            self.row1 = Rect { x: tx, y, w: tw, h: row1_h };
            self.stacked = stacked;
        }
        if seek {
            let y = if stacked { ty + row1_h + gap } else { ((h - row2_h) / 2.0).floor() };
            let row2 = Rect { x: tx, y, w: tw, h: row2_h };
            let label_w = if p.show_times {
                (p.time_chars.max(4) as f32 * 5.6 + 3.0) * u * t
            } else {
                0.0
            };
            let label_gap = if p.show_times { 6.0 * u } else { 2.0 * u };
            let total = Rect { x: tx + tw - label_w, w: label_w, ..row2 };
            let track_h = ([2.0, 3.0, 5.0][p.seek_size.min(2) as usize] * u).round().max(2.0);
            let track_x = tx + label_w + label_gap;
            let track = Rect {
                x: track_x,
                y: (row2.cy() - track_h / 2.0).round(),
                w: (total.x - label_gap - track_x).max(10.0),
                h: track_h,
            };
            self.row2 = row2;
            self.elapsed = Rect { w: label_w, ..row2 };
            self.total = total;
            self.track = track;
            // Stacked: from just above the bar down to the screen edge. On
            // its own: the full height of the taskbar.
            let top = if stacked { row2.y - 3.0 * u } else { 0.0 };
            self.seek_hit = Rect { x: track.x - 5.0 * u, y: top, w: track.w + 10.0 * u, h: h - top };
        }
        tx + tw
    }

    pub fn hit(&self, x: f32, y: f32, has_timeline: bool) -> Hit {
        if has_timeline && self.seek_hit.contains(x, y) {
            return Hit::Seek;
        }
        let within = |r: &Rect, pad: f32| r.w > 0.0 && x >= r.x - pad && x < r.x + r.w + pad;
        // Generous target around the little gear.
        if within(&self.gear, 3.0 * self.u) {
            return if y < self.gear.y + self.gear.h + 4.0 * self.u { Hit::Gear } else { Hit::None };
        }
        // The switcher: top half is up, bottom half is down.
        if within(&self.players, 2.0 * self.u) {
            let middle = self.players.y + self.players.h / 2.0;
            return if y < middle { Hit::PlayerUp } else { Hit::PlayerDown };
        }
        // Art and buttons can be clicked over the full height of the taskbar.
        if within(&self.art, 2.0 * self.u) {
            return Hit::Art;
        }
        for (i, b) in self.buttons.iter().enumerate() {
            if within(b, 0.0) {
                return [Hit::Prev, Hit::Play, Hit::Next][i];
            }
        }
        for (i, b) in self.extras.iter().enumerate() {
            if within(b, 0.0) {
                return [Hit::Shuffle, Hit::Repeat][i];
            }
        }
        if within(&self.output, 0.0) {
            return Hit::Output;
        }
        if within(&self.row1, 0.0) && (!self.stacked || y < self.row2.y - 3.0 * self.u) {
            return Hit::Title;
        }
        Hit::None
    }

    pub fn seek_fraction(&self, x: f32) -> f32 {
        ((x - self.track.x) / self.track.w).clamp(0.0, 1.0)
    }
}

/// Everything that affects the picture. A widget is only redrawn when this
/// differs from its last frame.
#[derive(Clone, Default, PartialEq)]
pub struct Frame {
    pub w: i32,
    pub h: i32,
    pub params: LayoutParams,
    pub smooth: bool,
    /// The taskbar is light, so shading is done with dark tints.
    pub light: bool,
    /// Text and icon color, 0xFFRRGGBB.
    pub fg: u32,
    /// The main text of the first row.
    pub title: String,
    /// Dimmer text after it on the same row; may be empty.
    pub artist: String,
    /// Text for the second row when there is no seek bar there.
    pub below: String,
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
    /// Played fraction, 0..=1 (quantised to whole pixels by the caller).
    pub fill: f32,
    pub hover: Hit,
    pub pressed: Hit,
    pub dragging: bool,
    /// Scroll a too-long title instead of ending it with "...".
    pub scroll: bool,
    /// How far the title is currently scrolled, in pixels.
    pub scroll_px: i32,
    pub can_shuffle: bool,
    pub shuffle: bool,
    pub can_repeat: bool,
    /// 0 off, 1 track, 2 list.
    pub repeat: u8,
    /// Highlight color: 0 from the album art, 1 `accent_rgb`, 2 none.
    pub accent: u32,
    pub accent_rgb: u32,
    /// Album art corners: 0 rounded, 1 square, 2 circle.
    pub art_shape: u32,
    /// Backdrop behind the widget: 0 none, 1 subtle, 2 solid.
    pub background: u32,
    /// Index into `config::FONTS`.
    pub font: u32,
    /// Volume percentage to show briefly in place of the seek bar.
    pub volume: Option<u8>,
    /// Short message to show briefly in place of the title.
    pub notice: Option<String>,
    /// Mark the settings gear: a newer version is available.
    pub update: bool,
    /// The current output is headphones (changes the output button's icon).
    pub headphones: bool,
}

/// A widget's album art, decoded and scaled to the size it is drawn at.
pub struct ArtCache {
    art_gen: u64,
    size: i32,
    bitmap: *mut GpBitmap,
    /// The art's dominant color as (hue 0..6, saturation 0..1), if it has one.
    accent: Option<(f32, f32)>,
}

impl Drop for ArtCache {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.bitmap as *mut GpImage);
        }
    }
}

/// 32-bit premultiplied ARGB, the format layered windows expect.
const PixelFormat32bppPARGB: u32 = 0x000E_200B;

fn alpha(color: u32, a: u32) -> u32 {
    (color & 0x00FF_FFFF) | (a << 24)
}

/// Start GDI+. Call once, before any painting.
pub fn init() {
    let mut token = 0usize;
    let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
    unsafe {
        GdiplusStartup(&mut token, &input, null_mut());
    }
}

/// Supersampling factor for smooth drawing: more on low-DPI screens, where
/// it matters most.
fn supersample(smooth: bool, scale: f32) -> i32 {
    if !smooth {
        1
    } else if scale >= 1.75 {
        2
    } else {
        3
    }
}

/// Let `draw` paint a picture `k` times larger than `w` x `h`, average it
/// down, and show it in the layered window `hwnd`.
unsafe fn render_to_window<R: Default>(
    hwnd: HWND,
    w: i32,
    h: i32,
    k: i32,
    draw: impl FnOnce(*mut GpGraphics) -> R,
) -> R {
    let (bw, bh) = (w * k, h * k);
    let mut big = vec![0u32; (bw * bh) as usize];
    let mut bitmap: *mut GpBitmap = null_mut();
    GdipCreateBitmapFromScan0(
        bw,
        bh,
        bw * 4,
        PixelFormat32bppPARGB as i32,
        Some(big.as_mut_ptr() as *const u8),
        &mut bitmap,
    );
    if bitmap.is_null() {
        return R::default();
    }
    let mut result = R::default();
    let mut g: *mut GpGraphics = null_mut();
    GdipGetImageGraphicsContext(bitmap as *mut GpImage, &mut g);
    if !g.is_null() {
        result = draw(g);
        GdipFlush(g, FlushIntentionSync);
        GdipDeleteGraphics(g);
    }
    GdipDisposeImage(bitmap as *mut GpImage);

    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = null_mut();
    if let std::result::Result::Ok(dib) =
        CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
    {
        if !bits.is_null() {
            let out = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
            downsample(&big, bw as usize, out, w as usize, h as usize, k as usize);

            let old = SelectObject(mem, dib.into());
            let size = SIZE { cx: w, cy: h };
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
    result
}

/// Draw `f` and push it to the layered window. Returns how many pixels wider
/// the title line is than the space for it (0 or less when it fits).
///
/// `spotify` enables detection of the framed thumbnail some Spotify versions
/// publish (see `legacy_spotify_frame`).
pub fn paint(
    hwnd: HWND,
    f: &Frame,
    cache: &mut Option<ArtCache>,
    art: Option<&[u8]>,
    spotify: bool,
) -> f32 {
    if f.w <= 0 || f.h <= 0 {
        return 0.0;
    }
    let k = supersample(f.smooth, f.params.scale);
    let layout = Layout::new(&f.params.scaled(k));
    unsafe {
        render_to_window(hwnd, f.w, f.h, k, |g| draw(g, f, &layout, k, cache, art, spotify)) / k as f32
    }
}

/// The hover card: a larger view of the current track.
#[derive(Clone, Default, PartialEq)]
pub struct Card {
    pub w: i32,
    pub h: i32,
    /// DPI scale of the monitor it is shown on.
    pub scale: f32,
    pub smooth: bool,
    /// Light Windows theme: light card with dark text.
    pub light: bool,
    pub font: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Bottom line, for example "Spotify · Playing".
    pub status: String,
    pub art_gen: u64,
    pub has_art: bool,
}

/// Size of the hover card in pixels at a DPI scale.
pub fn card_size(scale: f32) -> (i32, i32) {
    ((320.0 * scale).round() as i32, (112.0 * scale).round() as i32)
}

/// Draw the hover card into its layered window.
pub fn paint_card(hwnd: HWND, c: &Card, art: Option<&[u8]>, spotify: bool) {
    if c.w <= 0 || c.h <= 0 {
        return;
    }
    let k = supersample(c.smooth, c.scale);
    unsafe {
        render_to_window(hwnd, c.w, c.h, k, |g| draw_card(g, c, k, art, spotify));
    }
}

unsafe fn draw_card(g: *mut GpGraphics, c: &Card, k: i32, art: Option<&[u8]>, spotify: bool) {
    let u = c.scale * k as f32;
    let (w, h) = ((c.w * k) as f32, (c.h * k) as f32);
    let fg: u32 = if c.light { 0xFF00_0000 } else { 0xFFFF_FFFF };
    let family = FONTS[(c.font as usize).min(FONTS.len() - 1)];

    GdipGraphicsClear(g, 0x0000_0000);
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
    GdipSetTextRenderingHint(
        g,
        if k > 1 { TextRenderingHintAntiAlias } else { TextRenderingHintAntiAliasGridFit },
    );

    // Panel with a hairline border.
    let panel = Rect { x: 0.5 * u, y: 0.5 * u, w: w - u, h: h - u };
    fill_round_color(g, if c.light { 0xFAF6_F6F6 } else { 0xFA26_2626 }, panel, 8.0 * u);
    let mut pen: *mut GpPen = null_mut();
    GdipCreatePen1(alpha(fg, 0x30), 1.0 * u, UnitPixel, &mut pen);
    if !pen.is_null() {
        stroke_round(g, pen, panel, 8.0 * u);
        GdipDeletePen(pen);
    }

    // Album art.
    let pad = 12.0 * u;
    let art_rect = Rect { x: pad, y: pad, w: h - 2.0 * pad, h: h - 2.0 * pad };
    let bitmap = if c.has_art { art.and_then(|bytes| scale_art(bytes, art_rect.w as i32, spotify)) } else { None };
    match bitmap {
        Some(bitmap) => {
            let mut tex: *mut GpTexture = null_mut();
            GdipCreateTexture(bitmap as *mut GpImage, WrapModeClamp, &mut tex);
            if !tex.is_null() {
                GdipTranslateTextureTransform(tex, art_rect.x, art_rect.y, MatrixOrderAppend);
                fill_round(g, tex as *mut GpBrush, art_rect, 6.0 * u);
                GdipDeleteBrush(tex as *mut GpBrush);
            }
            GdipDisposeImage(bitmap as *mut GpImage);
        }
        None => {
            fill_round_color(g, alpha(fg, 0x24), art_rect, 6.0 * u);
            let font = make_font("Segoe UI Symbol", art_rect.h * 0.5, false);
            let fmt = make_format(StringAlignmentCenter, true);
            draw_text(g, "\u{266B}", font, art_rect, fmt, alpha(fg, 0x9E));
            GdipDeleteFont(font);
            GdipDeleteStringFormat(fmt);
        }
    }

    // Title, artist, album, and a status line at the bottom.
    let x = art_rect.x + art_rect.w + 14.0 * u;
    let tw = w - x - pad;
    let near = make_format(StringAlignmentNear, true);
    let lines: [(&str, f32, bool, u32, f32, f32); 4] = [
        (if c.title.is_empty() { "Unknown track" } else { &c.title }, 15.0, true, 0xFF, pad - 1.0 * u, 22.0),
        (&c.artist, 13.0, false, 0xC8, pad + 23.0 * u, 19.0),
        (&c.album, 12.0, false, 0x90, pad + 43.0 * u, 18.0),
        (&c.status, 11.0, false, 0x90, h - pad - 15.0 * u, 16.0),
    ];
    for (text, size, bold, opacity, y, height) in lines {
        let font = make_font(family, size * u, bold);
        draw_text(g, text, font, Rect { x, y, w: tw, h: height * u }, near, alpha(fg, opacity));
        GdipDeleteFont(font);
    }
    GdipDeleteStringFormat(near);
}

/// Box-filter `src` (k times larger each way) into `dst`. Pixels are
/// premultiplied, so the channels can simply be averaged.
fn downsample(src: &[u32], src_w: usize, dst: &mut [u32], w: usize, h: usize, k: usize) {
    let n = (k * k) as u32;
    for y in 0..h {
        for x in 0..w {
            let mut px = if k == 1 {
                src[y * src_w + x]
            } else {
                let (mut a, mut r, mut g, mut b) = (0u32, 0u32, 0u32, 0u32);
                for sy in 0..k {
                    let row = (y * k + sy) * src_w + x * k;
                    for &p in &src[row..row + k] {
                        a += p >> 24;
                        r += (p >> 16) & 0xFF;
                        g += (p >> 8) & 0xFF;
                        b += p & 0xFF;
                    }
                }
                let avg = |v: u32| (v + n / 2) / n;
                (avg(a) << 24) | (avg(r) << 16) | (avg(g) << 8) | avg(b)
            };
            // Alpha 1 rather than 0: fully transparent pixels of a layered
            // window let the mouse fall through to the taskbar underneath.
            if px >> 24 == 0 {
                px = 0x0100_0000;
            }
            dst[y * w + x] = px;
        }
    }
}

unsafe fn draw(
    g: *mut GpGraphics,
    f: &Frame,
    l: &Layout,
    k: i32,
    cache: &mut Option<ArtCache>,
    art: Option<&[u8]>,
    spotify: bool,
) -> f32 {
    let supersampled = k > 1;
    let u = l.u;
    let fg: u32 = f.fg | 0xFF00_0000;
    let primary = alpha(fg, if f.light { 0xE4 } else { 0xFF });
    let dim = alpha(fg, 0x9E);
    let disabled = alpha(fg, 0x48);
    let plate = alpha(fg, if f.light { 0x12 } else { 0x1C });
    let plate_down = alpha(fg, if f.light { 0x0A } else { 0x10 });

    GdipGraphicsClear(g, 0x0000_0000);
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
    // When supersampling, leave glyphs unhinted so their shapes stay true
    // after averaging; otherwise snap them to the pixel grid for crispness.
    GdipSetTextRenderingHint(
        g,
        if supersampled { TextRenderingHintAntiAlias } else { TextRenderingHintAntiAliasGridFit },
    );

    let radius = 4.0 * u;
    let family = FONTS[(f.font as usize).min(FONTS.len() - 1)];

    // ---- backdrop ----------------------------------------------------------
    if f.background != 0 {
        let color = match (f.background, f.light) {
            (1, _) => alpha(fg, 0x16),
            (_, true) => 0xE6F3_F3F3,
            (_, false) => 0xE61C_1C1C,
        };
        let inset = 3.0 * u;
        let r = Rect {
            x: 1.0 * u,
            y: inset,
            w: (f.w * k) as f32 - 2.0 * u,
            h: (f.h * k) as f32 - 2.0 * inset,
        };
        fill_round_color(g, color, r, 7.0 * u);
    }

    // ---- settings gear ---------------------------------------------------
    if l.gear.w > 0.0 {
    if f.hover == Hit::Gear {
        let c = if f.pressed == Hit::Gear { plate_down } else { plate };
        let r = Rect { x: l.gear.x - u, y: l.gear.y - u, w: l.gear.w + 2.0 * u, h: l.gear.h + 2.0 * u };
        fill_round_color(g, c, r, 3.0 * u);
    }
    draw_gear(g, l.gear.cx(), l.gear.cy(), l.gear.w * 0.40, if f.hover == Hit::Gear { primary } else { dim });
    if f.update {
        let r = 2.2 * u;
        let brush = solid(0xFF4C_C2FF);
        GdipFillEllipse(g, brush, l.gear.x + l.gear.w - r, l.gear.y - r * 0.4, r * 2.0, r * 2.0);
        GdipDeleteBrush(brush);
    }
    }

    // ---- player switcher ----------------------------------------------------
    if l.players.w > 0.0 {
        let b = l.players;
        let half = b.h / 2.0;
        for (kind, top, dir) in [(Hit::PlayerUp, b.y, -1.0f32), (Hit::PlayerDown, b.y + half, 1.0)] {
            let zone = Rect { x: b.x - u, y: top, w: b.w + 2.0 * u, h: half };
            if f.hover == kind {
                let c = if f.pressed == kind { plate_down } else { plate };
                fill_round_color(g, c, zone, 3.0 * u);
            }
            // A small triangle pointing up or down.
            let brush = solid(if f.hover == kind { primary } else { dim });
            let (cx, cy) = (zone.cx(), zone.cy());
            polygon(
                g,
                brush,
                &[
                    (cx - 4.0 * u, cy - dir * 2.4 * u),
                    (cx + 4.0 * u, cy - dir * 2.4 * u),
                    (cx, cy + dir * 2.6 * u),
                ],
            );
            GdipDeleteBrush(brush);
        }
    }

    // ---- album art -------------------------------------------------------
    // The art is decoded even when hidden: the highlight color comes from it.
    let size = if l.art.w > 0.0 { l.art.w as i32 } else { (40.0 * u) as i32 };
    let art_radius = match f.art_shape {
        1 => 0.0,
        2 => l.art.w / 2.0,
        _ => radius,
    };
    if cache.as_ref().is_some_and(|c| c.art_gen != f.art_gen || c.size != size) {
        *cache = None;
    }
    if cache.is_none() && f.has_art {
        if let Some(bitmap) = art.and_then(|bytes| scale_art(bytes, size, spotify)) {
            let accent = art_accent(bitmap, size);
            *cache = Some(ArtCache { art_gen: f.art_gen, size, bitmap, accent });
        }
    }
    if l.art.w > 0.0 {
    match cache {
        Some(c) if f.has_art => {
            let mut tex: *mut GpTexture = null_mut();
            GdipCreateTexture(c.bitmap as *mut GpImage, WrapModeClamp, &mut tex);
            if !tex.is_null() {
                GdipTranslateTextureTransform(tex, l.art.x, l.art.y, MatrixOrderAppend);
                fill_round(g, tex as *mut GpBrush, l.art, art_radius);
                GdipDeleteBrush(tex as *mut GpBrush);
            }
        }
        _ => {
            fill_round_color(g, alpha(fg, 0x24), l.art, art_radius);
            let font = make_font("Segoe UI Symbol", l.art.h * 0.55, false);
            let fmt = make_format(StringAlignmentCenter, true);
            draw_text(g, "\u{266B}", font, l.art, fmt, dim);
            GdipDeleteFont(font);
            GdipDeleteStringFormat(fmt);
        }
    }
    if f.pressed == Hit::Art && f.hover == Hit::Art {
        fill_round_color(g, 0x4000_0000, l.art, art_radius);
    } else if f.hover == Hit::Art {
        fill_round_color(g, 0x30FF_FFFF, l.art, art_radius);
    }
    }

    // With the settings button hidden, an available update is marked on the
    // corner of the album art instead.
    if f.update && l.gear.w <= 0.0 && l.art.w > 0.0 {
        let r = 2.4 * u;
        let brush = solid(0xFF4C_C2FF);
        GdipFillEllipse(g, brush, l.art.x - r * 0.6, l.art.y - r * 0.6, r * 2.0, r * 2.0);
        GdipDeleteBrush(brush);
    }

    // ---- transport buttons -------------------------------------------------
    let kinds = [Hit::Prev, Hit::Play, Hit::Next];
    let enabled = [f.can_prev, f.can_toggle, f.can_next];
    for i in 0..3 {
        let b = l.buttons[i];
        if b.w <= 0.0 {
            continue;
        }
        // Icons scale with the chosen button size.
        let u = u * l.b;
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

    let accent = match f.accent {
        0 if f.has_art => cache.as_ref().and_then(|c| c.accent).map(|hs| accent_color(hs, f.light)),
        1 => Some(f.accent_rgb),
        _ => None,
    };

    // ---- shuffle and repeat ------------------------------------------------
    {
        let kinds = [Hit::Shuffle, Hit::Repeat];
        let enabled = [f.can_shuffle, f.can_repeat];
        let active = [f.shuffle, f.repeat != 0];
        for i in 0..2 {
            let b = l.extras[i];
            if b.w <= 0.0 {
                continue;
            }
            let u = u * l.b;
            if enabled[i] && f.hover == kinds[i] {
                let c = if f.pressed == kinds[i] { plate_down } else { plate };
                fill_round_color(g, c, b, radius);
            }
            let color = if !enabled[i] {
                disabled
            } else if active[i] {
                accent.unwrap_or(primary)
            } else {
                dim
            };
            let (cx, cy) = (b.cx(), b.cy() - 1.0 * u);
            let mut pen: *mut GpPen = null_mut();
            GdipCreatePen1(color, 1.5 * u, UnitPixel, &mut pen);
            let brush = solid(color);
            if !pen.is_null() {
                GdipSetPenLineCap197819(pen, LineCapRound, LineCapRound, DashCapRound);
                GdipSetPenLineJoin(pen, LineJoinRound);
                if i == 0 {
                    // Two crossing strands with arrowheads.
                    for dir in [1.0f32, -1.0] {
                        let (y0, y1) = (cy + 3.5 * u * dir, cy - 3.5 * u * dir);
                        GdipDrawBezier(g, pen, cx - 6.0 * u, y0, cx - 1.0 * u, y0, cx + 0.5 * u, y1, cx + 4.0 * u, y1);
                        polygon(g, brush, &[(cx + 3.6 * u, y1 - 2.6 * u), (cx + 3.6 * u, y1 + 2.6 * u), (cx + 6.8 * u, y1)]);
                    }
                } else {
                    // A loop with an arrowhead; "1" inside when repeating one track.
                    let r = Rect { x: cx - 6.0 * u, y: cy - 3.6 * u, w: 12.0 * u, h: 7.2 * u };
                    stroke_round(g, pen, r, 3.2 * u);
                    polygon(g, brush, &[(cx - 0.4 * u, r.y - 2.6 * u), (cx - 0.4 * u, r.y + 2.6 * u), (cx + 2.8 * u, r.y)]);
                    if f.repeat == 1 {
                        let font = make_font("Segoe UI", 7.5 * u, true);
                        let fmt = make_format(StringAlignmentCenter, false);
                        let box1 = Rect { x: cx - 5.0 * u, y: cy - 4.4 * u, w: 10.0 * u, h: 9.0 * u };
                        draw_text(g, "1", font, box1, fmt, color);
                        GdipDeleteFont(font);
                        GdipDeleteStringFormat(fmt);
                    }
                }
                GdipDeletePen(pen);
            }
            if enabled[i] && active[i] {
                let d = 1.3 * u;
                GdipFillEllipse(g, brush, cx - d, cy + 7.2 * u, d * 2.0, d * 2.0);
            }
            GdipDeleteBrush(brush);
        }
    }

    // ---- output device ----------------------------------------------------
    if l.output.w > 0.0 {
        let b = l.output;
        let u = u * l.b;
        if f.hover == Hit::Output {
            let c = if f.pressed == Hit::Output { plate_down } else { plate };
            fill_round_color(g, c, b, radius);
        }
        let color = if f.hover == Hit::Output { primary } else { dim };
        let (cx, cy) = (b.cx(), b.cy());
        let brush = solid(color);
        let mut pen: *mut GpPen = null_mut();
        GdipCreatePen1(color, 1.5 * u, UnitPixel, &mut pen);
        if !pen.is_null() {
            GdipSetPenLineCap197819(pen, LineCapRound, LineCapRound, DashCapRound);
            if f.headphones {
                // A band over the top with an ear cup on each side.
                GdipDrawArc(g, pen, cx - 5.5 * u, cy - 6.0 * u, 11.0 * u, 11.0 * u, 180.0, 180.0);
                for side in [-1.0f32, 1.0] {
                    let cup = Rect { x: cx + side * 5.0 * u - 1.6 * u, y: cy - 0.8 * u, w: 3.2 * u, h: 6.0 * u };
                    fill_round(g, brush, cup, 1.4 * u);
                }
            } else {
                // A speaker cone with two sound waves.
                GdipFillRectangle(g, brush, cx - 6.5 * u, cy - 2.2 * u, 3.0 * u, 4.4 * u);
                polygon(
                    g,
                    brush,
                    &[
                        (cx - 4.0 * u, cy - 2.2 * u),
                        (cx - 0.5 * u, cy - 5.5 * u),
                        (cx - 0.5 * u, cy + 5.5 * u),
                        (cx - 4.0 * u, cy + 2.2 * u),
                    ],
                );
                for r in [3.0 * u, 5.8 * u] {
                    GdipDrawArc(g, pen, cx + 0.6 * u - r, cy - r, r * 2.0, r * 2.0, -42.0, 84.0);
                }
            }
            GdipDeletePen(pen);
        }
        GdipDeleteBrush(brush);
    }

    // ---- text and seek bar -------------------------------------------------
    if l.row1.w <= 0.0 && l.row2.w <= 0.0 {
        return 0.0;
    }
    let ts = u * l.t;
    let title_font = make_font(family, 12.0 * ts, true);
    let body_font = make_font(family, 12.0 * ts, false);
    let small_font = make_font(family, 10.0 * ts, false);
    let near = make_format(StringAlignmentNear, true);
    let far = make_format(StringAlignmentFar, true);
    let free = make_format(StringAlignmentNear, false);

    // Row 1: "Title · Artist" when there is a seek bar underneath, otherwise
    // just the title (the artist then gets row 2 to itself).
    let title = match &f.notice {
        Some(notice) => notice.as_str(),
        None if f.title.is_empty() => "Unknown track",
        None => f.title.as_str(),
    };
    let artist = if f.notice.is_none() && !f.artist.is_empty() {
        format!("\u{00B7} {}", f.artist)
    } else {
        String::new()
    };
    let mut overflow = 0.0;
    if l.row1.w > 0.0 {
    let unbounded = Rect { w: 100_000.0, ..l.row1 };
    let title_full = measure(g, title, title_font, unbounded, free);
    let artist_full = measure(g, &artist, body_font, unbounded, free);
    overflow = title_full + artist_full + 2.0 * u - l.row1.w;

    if f.scroll && overflow > 0.0 {
        // Too long: draw it whole, shifted left, and clip to the row.
        let x = l.row1.x - (f.scroll_px * k) as f32;
        GdipSetClipRect(g, l.row1.x, l.row1.y, l.row1.w, l.row1.h, CombineModeReplace);
        draw_text(g, title, title_font, Rect { x, w: title_full + 8.0 * u, ..l.row1 }, free, primary);
        let r = Rect { x: x + title_full, w: artist_full + 8.0 * u, ..l.row1 };
        draw_text(g, &artist, body_font, r, free, dim);
        GdipResetClip(g);
    } else {
        let title_w = title_full.min(l.row1.w);
        draw_text(g, title, title_font, l.row1, near, primary);
        let rest = l.row1.w - title_w;
        if !artist.is_empty() && rest > 36.0 * u {
            let r = Rect { x: l.row1.x + title_w, w: rest, ..l.row1 };
            draw_text(g, &artist, body_font, r, near, dim);
        }
    }
    }

    if l.row2.w <= 0.0 {
        // No second row on this widget.
    } else if let Some(volume) = f.volume {
        // Brief volume read-out in place of the seek bar.
        let label_w = 34.0 * ts;
        let label = Rect { x: l.row2.x + l.row2.w - label_w, w: label_w, ..l.row2 };
        draw_text(g, &format!("{volume}%"), small_font, label, far, dim);
        let h = (3.0 * u).round().max(2.0);
        let t = Rect {
            x: l.row2.x + 1.0 * u,
            y: (l.row2.cy() - h / 2.0).round(),
            w: (l.row2.w - label_w - 6.0 * u).max(10.0),
            h,
        };
        fill_round_color(g, alpha(fg, 0x3A), t, h / 2.0);
        let fill = t.w * volume.min(100) as f32 / 100.0;
        if fill >= 1.0 {
            fill_round_color(g, accent.unwrap_or(primary), Rect { w: fill.max(h), ..t }, h / 2.0);
        }
    } else if f.has_timeline {
        if f.params.show_times {
            draw_text(g, &f.elapsed, small_font, l.elapsed, near, dim);
            draw_text(g, &f.total, small_font, l.total, far, dim);
        }

        let t = l.track;
        fill_round_color(g, alpha(fg, 0x3A), t, t.h / 2.0);
        let fill = (f.fill * t.w).clamp(0.0, t.w);
        if fill >= 1.0 {
            let color = if f.can_seek { accent.unwrap_or(primary) } else { dim };
            fill_round_color(g, color, Rect { w: fill.max(t.h), ..t }, t.h / 2.0);
        }
        if f.can_seek && (f.hover == Hit::Seek || f.dragging) {
            let r = if f.dragging { 5.0 * u } else { 4.0 * u };
            let brush = solid(alpha(fg, 0xFF));
            GdipFillEllipse(g, brush, t.x + fill - r, t.cy() - r, r * 2.0, r * 2.0);
            GdipDeleteBrush(brush);
        }
    } else {
        // No timeline from this player: artist on the second row.
        let artist_font = make_font(family, 11.0 * ts, false);
        draw_text(g, &f.below, artist_font, l.row2, near, dim);
        GdipDeleteFont(artist_font);
    }

    GdipDeleteFont(title_font);
    GdipDeleteFont(body_font);
    GdipDeleteFont(small_font);
    GdipDeleteStringFormat(near);
    GdipDeleteStringFormat(far);
    GdipDeleteStringFormat(free);
    overflow
}

/// An eight-toothed cog with a round hole, centred on (cx, cy).
unsafe fn draw_gear(g: *mut GpGraphics, cx: f32, cy: f32, radius: f32, color: u32) {
    let root = radius * 0.74;
    let mut pts: Vec<PointF> = Vec::with_capacity(32);
    for tooth in 0..8 {
        let a = tooth as f32 * 45.0;
        for (r, da) in [(root, -14.0f32), (radius, -8.5), (radius, 8.5), (root, 14.0)] {
            let rad = (a + da).to_radians();
            pts.push(PointF { X: cx + r * rad.cos(), Y: cy + r * rad.sin() });
        }
    }
    let mut path: *mut GpPath = null_mut();
    GdipCreatePath(FillModeAlternate, &mut path);
    if path.is_null() {
        return;
    }
    GdipAddPathPolygon(path, pts.as_ptr(), pts.len() as i32);
    let hole = radius * 0.36;
    GdipAddPathEllipse(path, cx - hole, cy - hole, hole * 2.0, hole * 2.0);
    let brush = solid(color);
    GdipFillPath(g, brush, path);
    GdipDeleteBrush(brush);
    GdipDeletePath(path);
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

unsafe fn stroke_round(g: *mut GpGraphics, pen: *mut GpPen, r: Rect, radius: f32) {
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
    GdipDrawPath(g, pen, path);
    GdipDeletePath(path);
}

/// The most prominent vivid color in the art, as (hue 0..6, saturation).
/// None for artwork that is essentially grey.
unsafe fn art_accent(bitmap: *mut GpBitmap, size: i32) -> Option<(f32, f32)> {
    const BUCKETS: usize = 12;
    let mut weight = [0f32; BUCKETS];
    let mut hue_sum = [0f32; BUCKETS];
    let mut sat_sum = [0f32; BUCKETS];
    let steps = 14;
    for iy in 0..steps {
        for ix in 0..steps {
            let (x, y) = ((ix * 2 + 1) * size / (steps * 2), (iy * 2 + 1) * size / (steps * 2));
            let mut px = 0u32;
            GdipBitmapGetPixel(bitmap, x, y, &mut px);
            let r = ((px >> 16) & 0xFF) as f32 / 255.0;
            let g = ((px >> 8) & 0xFF) as f32 / 255.0;
            let b = (px & 0xFF) as f32 / 255.0;
            let (max, min) = (r.max(g).max(b), r.min(g).min(b));
            let delta = max - min;
            if max < 0.2 || delta < 0.12 {
                continue;
            }
            let sat = delta / max;
            let hue = if max == r {
                ((g - b) / delta).rem_euclid(6.0)
            } else if max == g {
                (b - r) / delta + 2.0
            } else {
                (r - g) / delta + 4.0
            };
            let bucket = ((hue / 6.0 * BUCKETS as f32) as usize).min(BUCKETS - 1);
            let w = sat * max;
            weight[bucket] += w;
            hue_sum[bucket] += hue * w;
            sat_sum[bucket] += sat * w;
        }
    }
    let best = (0..BUCKETS).max_by(|a, b| weight[*a].total_cmp(&weight[*b]))?;
    // Needs to cover a meaningful share of the picture.
    if weight[best] < (steps * steps) as f32 * 0.03 {
        return None;
    }
    Some((hue_sum[best] / weight[best], sat_sum[best] / weight[best]))
}

/// A readable version of the art's color for the current taskbar theme.
fn accent_color((hue, sat): (f32, f32), light_taskbar: bool) -> u32 {
    let (s, v) = if light_taskbar { (sat.clamp(0.55, 1.0), 0.62) } else { (sat.clamp(0.40, 0.72), 1.0) };
    let c = v * s;
    let x = c * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hue as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    0xFF00_0000 | (to(r) << 16) | (to(g) << 8) | to(b)
}

unsafe fn fill_round_color(g: *mut GpGraphics, color: u32, r: Rect, radius: f32) {
    let brush = solid(color);
    fill_round(g, brush, r, radius);
    GdipDeleteBrush(brush);
}

/// `bold` uses Segoe UI's own semibold face, and the bold style elsewhere.
unsafe fn make_font(family: &str, px: f32, bold: bool) -> *mut GpFont {
    let (name, style) = match (family, bold) {
        ("Segoe UI", true) => ("Segoe UI Semibold", 0),
        (_, true) => (family, 1),
        _ => (family, 0),
    };
    let mut fam: *mut GpFontFamily = null_mut();
    GdipCreateFontFamilyFromName(&HSTRING::from(name), null_mut(), &mut fam);
    if fam.is_null() {
        GdipCreateFontFamilyFromName(w!("Segoe UI"), null_mut(), &mut fam);
    }
    if fam.is_null() {
        GdipGetGenericFontFamilySansSerif(&mut fam);
    }
    let mut font: *mut GpFont = null_mut();
    GdipCreateFont(fam, px.max(6.0), style, UnitPixel, &mut font);
    if font.is_null() && style != 0 {
        // Not every family has a bold face.
        GdipCreateFont(fam, px.max(6.0), 0, UnitPixel, &mut font);
    }
    GdipDeleteFontFamily(fam);
    font
}

/// `fit`: end text that does not fit with "...". Otherwise it is drawn whole,
/// even past the edge of its rectangle.
unsafe fn make_format(align: StringAlignment, fit: bool) -> *mut GpStringFormat {
    const NO_WRAP: i32 = 0x1000;
    const NO_CLIP: i32 = 0x4000;
    let mut fmt: *mut GpStringFormat = null_mut();
    GdipCreateStringFormat(if fit { NO_WRAP } else { NO_WRAP | NO_CLIP }, 0, &mut fmt);
    GdipSetStringFormatAlign(fmt, align);
    GdipSetStringFormatLineAlign(fmt, StringAlignmentCenter);
    GdipSetStringFormatTrimming(
        fmt,
        if fit { StringTrimmingEllipsisCharacter } else { StringTrimmingNone },
    );
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
