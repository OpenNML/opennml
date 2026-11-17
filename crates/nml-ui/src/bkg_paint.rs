//! Image quads, `.bkg` backgrounds, desktop image and frame decoration (re/menu-system 8.4.1-8.4.3, 8.4.10,
//! 8.4.12; `.bkg` format `docs/formats/dlg.md`).
//!
//! All drawing paths end in [`image_quad`] (`ImageList_DrawRect` `FUN_004028C0`) or [`fill_rect`]
//! (`FUN_00459BD0`). Positions and sizes are screen pixels as `i32`, UV `f32` in the calculation order of the original.

use nml_formats::bkg::{BkgDesc, Slot, TileRef, parse_bkg};
use nml_formats::dlg::FrameDesc;
use nml_formats::dlg::desc::frame_flags;
use nml_rda::Vfs;

use crate::assets::UiAssets;
use crate::coords::{Screen, normalize_rect, to_screen_rect, to_screen_x, to_screen_y};
use crate::draw::{Blend, DrawCmd, Filter, TextureId};
use crate::fonts::{FontBook, FontRef};
use crate::geom::intersect;
use crate::text::{draw_string, measure};

/// Diffuse color of all frame and decoration parts (`0xFFFFFFFF`, re/menu-system 8.4.2/8.4.3).
const WHITE: u32 = 0xFFFF_FFFF;

/// Tile size of the extra frame images `zusatz_85x85.tga` / `…_gespiegelt.tga` (loader `0x00461BE2`...`0x00461C28`).
pub const ZUSATZ_TILE: [i32; 2] = [85, 85];

/// Shared tile calculation of [`image_quad`] and the `[ICON]` runs of the text set (`text::draw_icon`):
/// `FUN_004028C0` with the grid `FUN_00401DF0`, the texture size comes from the caller.
///
/// - Grid: columns = `TexB / kb`, rows = `TexH / kh` (integer), tile `index` row by row; index outside (also
///   negative) or tile size <= 0 (division by 0 in the original) -> nothing.
/// - Cell origin `zu = (kb·sp)·(1/TexB)`, `zv = (kh·ze)·(1/TexH)`, cell size `cu = (kb−1)·(1/TexB)`,
///   `cv = (kh−1)·(1/TexH)`, half texel `h = 0,5/TexB` for u **and** v (`+0x28`/`+0x2C` both from the width, O-71).
/// - `dst ∩ clip` empty -> nothing. If the intersection is not `dst` itself, the edges are moved to the intersection
///   and the UV shortened proportionally, **before** the half texel: `u0 = (cl−x1)/(x2−x1)·cu + zu`, `u1 = (zu+cu) −
///   (x2−cr)/(x2−x1)·cu`, v likewise; afterwards `u0 += h`, `v0 += h`, `u1 −= h`, `v1 −= h`.
/// - Fill level 1.0 (all menu calls): `v0 = (1 − 1,0)·cv + zv = zv`, no height shortening.
///
/// Calculation order in `f32` (x87 with 24-bit precision, like `FUN_00401DF0`/`FUN_004028C0`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn tile_quad(
    texture: TextureId,
    tex_size: [u32; 2],
    tile: [i32; 2],
    index: i32,
    dst: [i32; 4],
    clip: [i32; 4],
    color: u32,
    filter: Filter,
    out: &mut Vec<DrawCmd>,
) {
    let [kb, kh] = tile;
    if kb <= 0 || kh <= 0 {
        return;
    }
    let (Ok(tex_w), Ok(tex_h)) = (i32::try_from(tex_size[0]), i32::try_from(tex_size[1])) else {
        return;
    };
    let columns = tex_w / kb;
    let rows = tex_h / kh;
    if index < 0 || index >= columns.saturating_mul(rows) {
        return;
    }
    let Some(cut) = intersect(dst, clip) else {
        return;
    };
    let (column, row) = (index % columns, index / columns);
    let inv_w = 1.0f32 / tex_w as f32;
    let inv_h = 1.0f32 / tex_h as f32;
    let half = 0.5f32 / tex_w as f32;
    let zu = (kb * column) as f32 * inv_w;
    let zv = (kh * row) as f32 * inv_h;
    let cu = (kb - 1) as f32 * inv_w;
    let cv = (kh - 1) as f32 * inv_h;
    let (mut u0, mut v0, mut u1, mut v1) = (zu, zv, zu + cu, zv + cv);
    if cut != dst {
        let [x1, y1, x2, y2] = dst;
        let w = x2.wrapping_sub(x1) as f32;
        let h = y2.wrapping_sub(y1) as f32;
        if x1 < cut[0] {
            u0 += (cut[0].wrapping_sub(x1) as f32 / w) * cu;
        }
        if cut[2] < x2 {
            u1 -= (x2.wrapping_sub(cut[2]) as f32 / w) * cu;
        }
        if y1 < cut[1] {
            v0 += (cut[1].wrapping_sub(y1) as f32 / h) * cv;
        }
        if cut[3] < y2 {
            v1 -= (y2.wrapping_sub(cut[3]) as f32 / h) * cv;
        }
    }
    out.push(DrawCmd::Quad {
        texture: Some(texture),
        dst: cut,
        uv: [u0 + half, v0 + half, u1 - half, v1 - half],
        color,
        filter,
        blend: Blend::Alpha,
    });
}

/// Image quad `ImageList_DrawRect` `FUN_004028C0` (re/menu-system 8.4.1): tile `index` of the grid
/// `tile = [kb, kh]` of `tex` to `dst`, intersected with `clip` (both screen pixels). For the calculation see
/// [`tile_quad`]; corners without the -0.5 offset. The menus pass `Filter::Linear` (state block `3dicons-cubic`).
#[allow(clippy::too_many_arguments)]
pub fn image_quad(
    assets: &UiAssets,
    tex: TextureId,
    tile: [i32; 2],
    index: i32,
    dst: [i32; 4],
    clip: [i32; 4],
    color: u32,
    filter: Filter,
    out: &mut Vec<DrawCmd>,
) {
    let (w, h) = assets.image(tex).dimensions();
    tile_quad(tex, [w, h], tile, index, dst, clip, color, filter, out);
}

/// Area fill `FUN_00459BD0(l, t, r, b, color)`: untextured quad in screen pixels
/// (state block `console`, color as diffuse - hypothesis, re/menu-system 8.4.1). UV and filter are meaningless
/// without a texture (`[0; 4]`, POINT).
pub fn fill_rect(rect: [i32; 4], color: u32, out: &mut Vec<DrawCmd>) {
    out.push(DrawCmd::Quad {
        texture: None,
        dst: rect,
        uv: [0.0; 4],
        color,
        filter: Filter::Point,
        blend: Blend::Alpha,
    });
}

/// A loaded `.bkg` (state after `FUN_0045BB00`).
///
/// `textures[i]` belongs to the `FILE` line `desc.files[i]`; `None` if the image is missing or corrupted. In the
/// original `FUN_00445F40` creates an image list entry for every `FILE` line (path and tile size,
/// `FUN_00402DF0` always succeeds); the texture is not loaded until drawing (`FUN_00401DF0`), and if that fails,
/// only the tiles of this file draw nothing. The file indexes of the others therefore stay as they are.
#[derive(Debug, Clone)]
pub struct LoadedBkg {
    pub desc: BkgDesc,
    pub textures: Vec<Option<TextureId>>,
}

impl LoadedBkg {
    /// Tile size and texture of the file `file` (`None` for an index outside the `FILE` lines).
    fn file(&self, file: i32) -> Option<([i32; 2], Option<TextureId>)> {
        let i = usize::try_from(file).ok()?;
        let f = self.desc.files.get(i)?;
        Some((
            [f.tile_w, f.tile_h],
            self.textures.get(i).copied().flatten(),
        ))
    }
}

/// Reads a `.bkg` (`FUN_0045BB00`) and loads its images into `assets`. If the file is missing: `None` without a
/// message, the widget stays without a background (as in the original).
pub fn load_bkg(vfs: &Vfs, path: &str, assets: &mut UiAssets) -> Option<LoadedBkg> {
    let bytes = match vfs.read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::debug!("Frame file \"{path}\" cannot be read: {e}");
            return None;
        }
    };
    let desc = parse_bkg(&bytes).desc();
    let textures = desc
        .files
        .iter()
        .map(|f| assets.texture(vfs, &f.path))
        .collect();
    Some(LoadedBkg { desc, textures })
}

/// Draws a `.bkg` like `DrawBackground` `FUN_0045A570` (re/menu-system 8.4.2). `abs` is the absolute
/// grid rectangle of the widget (swapped corners are ordered by [`normalize_rect`] like the constructor
/// `FUN_0045A270`), `clip` the visible rectangle in screen pixels.
///
/// 1. Tile sequence in screen pixels: tile size of the file converted as a point (`sw, sh`), from `S(abs).lt`
///    per tile `(l+ax, t+ay, l+ax+sw, t+ay+sh)`, `ax += width`; `NEWLINE`: `ay += sh` of the most recently placed
///    tile, `ax = 0`. `STRECHTOBG`: every tile onto `S(abs)`. Clip = `clip`.
/// 2. 9-slice **in the grid** (table 8.4.2), `clip` stays unused; tile size of all parts that of the
///    `TOPLEFT` file (O-70), the grid of each tile that of its own file; each tile and each clip
///    individually to S.
///
/// LINEAR, color `0xFFFFFFFF`. Missing parts, file indexes outside the `FILE` lines, missing images and
/// tile indexes outside the grid are omitted.
pub fn paint_bkg(
    bkg: &LoadedBkg,
    abs: [i32; 4],
    clip: [i32; 4],
    screen: Screen,
    assets: &UiAssets,
    out: &mut Vec<DrawCmd>,
) {
    if bkg.desc.files.is_empty() {
        return;
    }
    let abs = normalize_rect(abs);
    paint_sequence(bkg, abs, clip, screen, assets, out);
    paint_nine_slice(bkg, abs, screen, assets, out);
}

/// Part 1 of `FUN_0045A570`: the tile sequence (list `+0x74`).
fn paint_sequence(
    bkg: &LoadedBkg,
    abs: [i32; 4],
    clip: [i32; 4],
    screen: Screen,
    assets: &UiAssets,
    out: &mut Vec<DrawCmd>,
) {
    let s = to_screen_rect(screen, abs);
    let (mut ax, mut ay, mut last_h) = (0i32, 0i32, 0i32);
    for (i, row) in bkg.desc.sequence.iter().enumerate() {
        if i > 0 {
            // A new row of the sequence stands for a `NEWLINE`.
            ay = ay.wrapping_add(last_h);
            ax = 0;
        }
        for &TileRef { file, tile } in row {
            // The original packs `(k << 16) | (f & 0xFFFF)` (`0x0045BDB2`...`0x0045BDBB`) and reads when drawing
            // `k = value >> 16`, `f = value & 0xFFFF`; `k = 0xFFFF` is the mark of `NEWLINE`, so `TILE f 65535`
            // or `TILE f -1` also acts as a line break.
            let k = (tile as u32) & 0xFFFF;
            let f = (file as u32) & 0xFFFF;
            if k == 0xFFFF {
                ay = ay.wrapping_add(last_h);
                ax = 0;
                continue;
            }
            let Some(([kb, kh], texture)) = bkg.file(f as i32) else {
                continue;
            };
            let (sw, sh) = (to_screen_x(screen, kb), to_screen_y(screen, kh));
            let dst = if bkg.desc.stretch {
                s
            } else {
                let (l, t) = (s[0].wrapping_add(ax), s[1].wrapping_add(ay));
                [l, t, l.wrapping_add(sw), t.wrapping_add(sh)]
            };
            if let Some(tex) = texture {
                image_quad(
                    assets,
                    tex,
                    [kb, kh],
                    k as i32,
                    dst,
                    clip,
                    WHITE,
                    Filter::Linear,
                    out,
                );
            }
            ax = ax.wrapping_add(dst[2].wrapping_sub(dst[0]));
            last_h = sh;
        }
    }
}

/// Part 2 of `FUN_0045A570`: the 9-slice table `+0x78`, table 8.4.2.
///
/// Deviation: if `TOPLEFT` is missing or its file lies outside the `FILE` lines, the original aborts in
/// `FUN_0040F6F0` (range check -> `FUN_005E86BF`); with tile size <= 0 it would divide by 0 or loop forever.
/// The reimplementation then omits the 9 parts (not present in the shipped data).
fn paint_nine_slice(
    bkg: &LoadedBkg,
    abs: [i32; 4],
    screen: Screen,
    assets: &UiAssets,
    out: &mut Vec<DrawCmd>,
) {
    let slots = &bkg.desc.slots;
    if slots.iter().all(Option::is_none) {
        return; // no table `+0x78`
    }
    let Some(top_left) = slots[Slot::TopLeft.index()] else {
        return;
    };
    let Some(([kb, kh], _)) = bkg.file(top_left.file) else {
        return;
    };
    if kb <= 0 || kh <= 0 {
        return;
    }
    let [l, t, r, b] = abs;
    let il = l.wrapping_add(kb);
    let it = t.wrapping_add(kh);
    let ir = r.wrapping_sub(kb).max(il);
    let ib = b.wrapping_sub(kh).max(it);
    let piece = |slot: Slot, rect: [i32; 4], clip: [i32; 4], out: &mut Vec<DrawCmd>| {
        let Some(TileRef { file, tile }) = slots[slot.index()] else {
            return;
        };
        let Some((grid, Some(tex))) = bkg.file(file) else {
            return;
        };
        image_quad(
            assets,
            tex,
            grid,
            tile,
            to_screen_rect(screen, rect),
            to_screen_rect(screen, clip),
            WHITE,
            Filter::Linear,
            out,
        );
    };
    // Steps with saturation: the loops also end at the limit of `i32`.
    let steps = |from: i32, to: i32, step: i32| {
        std::iter::successors(Some(from), move |&v| {
            let next = v.saturating_add(step);
            (next > v).then_some(next)
        })
        .take_while(move |&v| v < to)
    };
    piece(
        Slot::BottomLeft,
        [l, b.wrapping_sub(kh), l.wrapping_add(kh), b],
        [l, t, il, b],
        out,
    );
    piece(
        Slot::BottomRight,
        [r.wrapping_sub(kb), b.wrapping_sub(kh), r, b],
        [ir, t, r, b],
        out,
    );
    piece(Slot::TopLeft, [l, t, il, it], [l, t, il, b], out);
    piece(
        Slot::TopRight,
        [r.wrapping_sub(kb), t, r, t.wrapping_add(kh)],
        [ir, t, r, b],
        out,
    );
    let fill = [il, it, ir, ib];
    if il < ir && it < ib {
        for y in steps(it, ib, kh) {
            for x in steps(il, ir, kb) {
                piece(
                    Slot::Fill,
                    [x, y, x.wrapping_add(kb), y.wrapping_add(kh)],
                    fill,
                    out,
                );
            }
        }
    }
    for y in steps(it, ib, kh) {
        piece(
            Slot::LeftMid,
            [l, y, l.wrapping_add(kb), y.wrapping_add(kh)],
            [l, it, il, ib],
            out,
        );
    }
    for y in steps(it, ib, kh) {
        piece(
            Slot::RightMid,
            [r.wrapping_sub(kb), y, r, y.wrapping_add(kh)],
            [ir, it, r, ib],
            out,
        );
    }
    // TOPMID: height `kb` instead of `kh` (O-70, `0x0045AF3F`...`0x0045AF89`).
    for x in steps(il, ir, kb) {
        piece(
            Slot::TopMid,
            [x, t, x.wrapping_add(kb), t.wrapping_add(kb)],
            [il, t, ir, it],
            out,
        );
    }
    for x in steps(il, ir, kb) {
        piece(
            Slot::BottomMid,
            [x, b.wrapping_sub(kh), x.wrapping_add(kb), b],
            [il, ib, ir, b],
            out,
        );
    }
}

/// Desktop image `MenuDesktop::Draw` `FUN_0045CC80` (re/menu-system 8.4.10): `(0, 0)…(B, H)`, UV `(0, 0)…(1, 1)`
/// **without** half texel, state block `3dicons-linear` = POINT (O-76: the name is misleading), color
/// `(α << 24) | 0xFFFFFF`. The 1024^2 image is stretched to B x H, not preserving the aspect ratio.
pub fn paint_fullscreen(tex: TextureId, screen: Screen, alpha: u8, out: &mut Vec<DrawCmd>) {
    out.push(DrawCmd::Quad {
        texture: Some(tex),
        dst: [0, 0, screen.width, screen.height],
        uv: [0.0, 0.0, 1.0, 1.0],
        color: (u32::from(alpha) << 24) | 0x00FF_FFFF,
        filter: Filter::Point,
        blend: Blend::Alpha,
    });
}

/// Frame decoration `Draw` `FUN_00461520` (re/menu-system 8.4.3).
///
/// `flags` are the flags of the widget after the constructor `FUN_00461AD0` (with `0x100` for a title without
/// bar); `rel_extent` the relative rectangle `+0x10` (slot 23, moves along during the fly-in); `offset` the offset
/// `+0x130/+0x134` of bars and titles (fly-in 12.2, in the end position `(0, 0)`, after closing `(0, −500)`;
/// the title without bar and the ornaments do not take it); `zusatz` = `[zusatz_85x85.tga (+0xF4),
/// zusatz_85x85_gespiegelt.tga (+0xF8)]`, `None` = image missing (tiles from it are omitted); `title` = text
/// `TITLE` as atlas codes ([`crate::text::atlas_codes`]) with the font `MenuTitle`, `None` = no text.
///
/// Order (8.4.12): bar L (`0x4`), bar R (`0x10`), ornaments (`0x2`), title L (`0x8`), title R
/// (`0x20`), title without bar (`0x100`); each title first the tiles, then the text. With `HIDE` nothing.
/// Everything with its own clip, LINEAR, color `0xFFFFFFFF`.
#[allow(clippy::too_many_arguments)]
pub fn paint_frame_decor(
    frame: &FrameDesc,
    flags: u32,
    rel_extent: [i32; 4],
    offset: [i32; 2],
    screen: Screen,
    zusatz: [Option<TextureId>; 2],
    title: Option<(&[u8], FontRef)>,
    fonts: &FontBook,
    assets: &UiAssets,
    out: &mut Vec<DrawCmd>,
) {
    if flags & frame_flags::HIDE != 0 {
        return;
    }
    let [plain, mirrored] = zusatz;
    let moved = |r: [i32; 4]| {
        [
            r[0].wrapping_add(offset[0]),
            r[1].wrapping_add(offset[1]),
            r[2].wrapping_add(offset[0]),
            r[3].wrapping_add(offset[1]),
        ]
    };
    let quad = |tex: Option<TextureId>, index: i32, dst: [i32; 4], clip: [i32; 4], out: &mut _| {
        if let Some(tex) = tex {
            image_quad(
                assets,
                tex,
                ZUSATZ_TILE,
                index,
                dst,
                clip,
                WHITE,
                Filter::Linear,
                out,
            );
        }
    };
    if flags & frame_flags::DRAWBARL != 0 {
        // `FUN_004605D0`
        paint_bar(
            to_screen_rect(screen, moved(frame.bar_extent)),
            plain,
            &quad,
            out,
        );
    }
    if flags & frame_flags::DRAWBARR != 0 {
        // `FUN_00460D20`: same calculation from the mirrored texture.
        paint_bar(
            to_screen_rect(screen, moved(frame.bar_extent)),
            mirrored,
            &quad,
            out,
        );
    }
    if flags & frame_flags::DRAWTAGS != 0 {
        // `FUN_00460770`: two ornaments 55 x 85 grid from cell 8 of the 85 grid (O-72), clip = the rectangle itself,
        // both from the unmirrored texture.
        let [x1, y1, x2, _] = rel_extent;
        for r in [
            [
                x1.wrapping_add(55),
                y1.wrapping_sub(60),
                x1.wrapping_add(110),
                y1.wrapping_add(25),
            ],
            [
                x2.wrapping_sub(110),
                y1.wrapping_sub(60),
                x2.wrapping_sub(55),
                y1.wrapping_add(25),
            ],
        ] {
            let s = to_screen_rect(screen, r);
            quad(plain, 8, s, s, out);
        }
    }
    let title_text = |s: [i32; 4], out: &mut Vec<DrawCmd>| {
        // `0x00460AD7`...`0x00460B29`: measured, centered (C division), directly `FUN_00428390`, without shadow.
        if let Some((text, font)) = title {
            let [tw, th] = measure(text, font, fonts);
            let x = s[0].wrapping_add(s[2].wrapping_sub(s[0]).wrapping_sub(tw) / 2);
            let y = s[1].wrapping_add(s[3].wrapping_sub(s[1]).wrapping_sub(th) / 2);
            draw_string(text, [x, y], frame.title_color, font, fonts, out);
        }
    };
    if flags & frame_flags::DRAWTITLEL != 0 {
        // `FUN_004608C0`
        let s = to_screen_rect(screen, moved(frame.title_extent));
        paint_title(s, [plain; 3], [5, 3, 4, 6, 7], &quad, out);
        title_text(s, out);
    }
    if flags & frame_flags::DRAWTITLER != 0 {
        // `FUN_00460EC0`
        let s = to_screen_rect(screen, moved(frame.title_extent));
        paint_title(s, [mirrored; 3], [3, 6, 7, 4, 5], &quad, out);
        title_text(s, out);
    }
    if flags & frame_flags::TITLE_WITHOUT_BAR != 0 {
        // `FUN_004612B0`: `TITLEEXTENT` without offset (O-73); center `+0xF4`, left `+0xF8`, right `+0xF4`.
        let s = to_screen_rect(screen, frame.title_extent);
        paint_title(s, [plain, mirrored, plain], [5, 6, 7, 6, 7], &quad, out);
        title_text(s, out);
    }
}

/// Bar (`FUN_004605D0`/`FUN_00460D20`): `h` = pixel height of `S`, `n = W / h`; middle tile 1 for
/// `i = 0 … n−1` with clip `(S.l+h, S.t, S.r−h, S.b)`, then tile 0 on the left and tile 2 on the right with clip `S`.
/// Height <= 0: nothing (division by 0 or empty intersections in the original).
fn paint_bar(
    s: [i32; 4],
    tex: Option<TextureId>,
    quad: &impl Fn(Option<TextureId>, i32, [i32; 4], [i32; 4], &mut Vec<DrawCmd>),
    out: &mut Vec<DrawCmd>,
) {
    let [l, t, r, b] = s;
    let h = b.wrapping_sub(t);
    if h <= 0 {
        return;
    }
    let n = r.wrapping_sub(l) / h;
    let mid_clip = [l.wrapping_add(h), t, r.wrapping_sub(h), b];
    for i in 0..n.max(0) {
        let x = l.wrapping_add(i.wrapping_mul(h));
        quad(tex, 1, [x, t, x.wrapping_add(h), b], mid_clip, out);
    }
    quad(tex, 0, [l, t, l.wrapping_add(h), b], s, out);
    quad(tex, 2, [r.wrapping_sub(h), t, r, b], s, out);
}

/// Title strip (`FUN_004608C0`, `FUN_00460EC0`, `FUN_004612B0`): `W`, `h` in pixels, `n = W / h`.
/// Middle only for `n − 1 > 1`: `k = 1 … n−2` at `(S.l + k·h …)` with clip `(S.l + 2h − 2, S.t, S.r − 2h + 2, S.b)`
/// (2 px under the end pieces, O-73); then two end pieces on the left `(S.l …)`, `(S.l+h …)` and on the right
/// `(S.r−2h …)`, `(S.r−h …)` with clip `S`. `tex = [middle, left, right]`, `tiles = [middle, left 1, left 2, right 1,
/// right 2]`. Height <= 0: nothing (division by 0 in the original).
fn paint_title(
    s: [i32; 4],
    tex: [Option<TextureId>; 3],
    tiles: [i32; 5],
    quad: &impl Fn(Option<TextureId>, i32, [i32; 4], [i32; 4], &mut Vec<DrawCmd>),
    out: &mut Vec<DrawCmd>,
) {
    let [l, t, r, b] = s;
    let h = b.wrapping_sub(t);
    if h <= 0 {
        return;
    }
    let n = r.wrapping_sub(l) / h;
    let h2 = h.wrapping_mul(2);
    if n.saturating_sub(1) > 1 {
        let clip = [
            l.wrapping_add(h2).wrapping_sub(2),
            t,
            r.wrapping_sub(h2).wrapping_add(2),
            b,
        ];
        for k in 1..n.saturating_sub(1) {
            let x = l.wrapping_add(k.wrapping_mul(h));
            quad(tex[0], tiles[0], [x, t, x.wrapping_add(h), b], clip, out);
        }
    }
    quad(tex[1], tiles[1], [l, t, l.wrapping_add(h), b], s, out);
    quad(
        tex[1],
        tiles[2],
        [l.wrapping_add(h), t, l.wrapping_add(h2), b],
        s,
        out,
    );
    quad(
        tex[2],
        tiles[3],
        [r.wrapping_sub(h2), t, r.wrapping_sub(h), b],
        s,
        out,
    );
    quad(tex[2], tiles[4], [r.wrapping_sub(h), t, r, b], s, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TestVfs, fnt_text, test_vfs, tga_bottom_up};
    use crate::text::atlas_codes;
    use image::RgbaImage;
    use nml_formats::localisation::parse_localisation;

    const S1280: Screen = Screen {
        width: 1280,
        height: 1024,
    };
    const S1024: Screen = Screen {
        width: 1024,
        height: 768,
    };
    const FULL: [i32; 4] = [0, 0, 1280, 1024];
    /// Half texel of a width of 64.
    const H64: f32 = 0.5 / 64.0;

    fn white(w: u16, h: u16) -> Vec<u8> {
        tga_bottom_up(w, h, |_, _| [255, 255, 255, 255])
    }

    /// Expectation via `image_quad` (whose calculation the first tests check with numbers).
    fn expect(
        assets: &UiAssets,
        tex: TextureId,
        tile: [i32; 2],
        index: i32,
        dst: [i32; 4],
        clip: [i32; 4],
        out: &mut Vec<DrawCmd>,
    ) {
        image_quad(
            assets,
            tex,
            tile,
            index,
            dst,
            clip,
            WHITE,
            Filter::Linear,
            out,
        );
    }

    fn dsts(cmds: &[DrawCmd]) -> Vec<[i32; 4]> {
        cmds.iter()
            .map(|c| match *c {
                DrawCmd::Quad { dst, .. } => dst,
                DrawCmd::Quad4 { .. } => unreachable!(),
            })
            .collect()
    }

    fn uv(cmd: &DrawCmd) -> [f32; 4] {
        match *cmd {
            DrawCmd::Quad { uv, .. } => uv,
            DrawCmd::Quad4 { .. } => unreachable!(),
        }
    }

    fn texture_of(cmd: &DrawCmd) -> Option<TextureId> {
        match *cmd {
            DrawCmd::Quad { texture, .. } => texture,
            DrawCmd::Quad4 { .. } => unreachable!(),
        }
    }

    fn quad_on(
        w: u32,
        h: u32,
        tile: [i32; 2],
        index: i32,
        dst: [i32; 4],
        clip: [i32; 4],
    ) -> Vec<DrawCmd> {
        let mut assets = UiAssets::new();
        let tex = assets.insert_image("data/t.tga", RgbaImage::new(w, h));
        let mut out = Vec::new();
        image_quad(
            &assets,
            tex,
            tile,
            index,
            dst,
            clip,
            0xFF11_2233,
            Filter::Linear,
            &mut out,
        );
        out
    }

    // QUIRK O-71: kept
    #[test]
    fn tile_uv_skips_the_last_texel() {
        // 8.4.1/O-71: 64^2 texture, 21-pixel tiles (3 x 3), tile 1: cell origin 21/64, cell size 20/64, half texel
        // 0.5/64 inside - the texel column 41 of the tile is never hit.
        let out = quad_on(64, 64, [21, 21], 1, [10, 20, 31, 41], FULL);
        assert_eq!(
            out,
            [DrawCmd::Quad {
                texture: Some(TextureId(0)),
                dst: [10, 20, 31, 41],
                uv: [
                    21.0 / 64.0 + H64,
                    H64,
                    21.0 / 64.0 + 20.0 / 64.0 - H64,
                    20.0 / 64.0 - H64
                ],
                color: 0xFF11_2233,
                filter: Filter::Linear,
                blend: Blend::Alpha,
            }]
        );
        // Half texel for v from the **width** (`+0x2C = 0,5/TexB`): 64 x 128, tile 3 = column 0, row 1.
        let out = quad_on(64, 128, [21, 21], 3, [0, 0, 21, 21], FULL);
        assert_eq!(
            uv(&out[0]),
            [
                H64,
                21.0 / 128.0 + H64,
                20.0 / 64.0 - H64,
                21.0 / 128.0 + 20.0 / 128.0 - H64
            ]
        );
    }

    // QUIRK O-71: kept
    #[test]
    fn clip_cuts_uv_proportionally() {
        // Tile 0 (cell 20/64) on 42 x 42 pixels; cut of 10 pixels on the left and 21 at the bottom: UV proportionally
        // in f32 before the half texel (`FUN_004028C0`), edges onto the intersection.
        let cu = 20.0f32 / 64.0;
        let out = quad_on(64, 64, [21, 21], 0, [10, 10, 52, 52], [20, 0, 100, 31]);
        assert_eq!(dsts(&out), [[20, 10, 52, 31]]);
        assert_eq!(
            uv(&out[0]),
            [
                (10.0f32 / 42.0) * cu + H64,
                H64,
                cu - H64,
                (cu - (21.0f32 / 42.0) * cu) - H64
            ]
        );
        // Cut of 12 pixels on the right and 20 at the top.
        let out = quad_on(64, 64, [21, 21], 0, [10, 10, 52, 52], [0, 30, 40, 100]);
        assert_eq!(dsts(&out), [[10, 30, 40, 52]]);
        assert_eq!(
            uv(&out[0]),
            [
                H64,
                (20.0f32 / 42.0) * cu + H64,
                (cu - (12.0f32 / 42.0) * cu) - H64,
                cu - H64
            ]
        );
        // Clip equal to the destination or larger: not shortened.
        let whole = [H64, H64, cu - H64, cu - H64];
        for clip in [[10, 10, 52, 52], [-5, -5, 60, 60]] {
            let out = quad_on(64, 64, [21, 21], 0, [10, 10, 52, 52], clip);
            assert_eq!(dsts(&out), [[10, 10, 52, 52]]);
            assert_eq!(uv(&out[0]), whole);
        }
    }

    // QUIRK O-71: kept
    #[test]
    fn empty_clip_draws_nothing() {
        let dst = [10, 10, 52, 52];
        for clip in [
            [52, 0, 60, 60],
            [0, 52, 60, 60],
            [0, 0, 0, 0],
            [60, 60, 0, 0],
            [30, 30, 30, 40],
        ] {
            assert!(
                quad_on(64, 64, [21, 21], 0, dst, clip).is_empty(),
                "{clip:?}"
            );
        }
        // Empty or swapped destination.
        for dst in [[10, 10, 10, 52], [52, 52, 10, 10]] {
            assert!(
                quad_on(64, 64, [21, 21], 0, dst, FULL).is_empty(),
                "{dst:?}"
            );
        }
    }

    // QUIRK O-71: kept
    // QUIRK O-88: deviation
    #[test]
    fn index_outside_the_grid_draws_nothing() {
        let dst = [0, 0, 21, 21];
        assert_eq!(
            quad_on(64, 64, [21, 21], 8, dst, FULL).len(),
            1,
            "last tile"
        );
        for index in [9, -1, i32::MAX, i32::MIN] {
            assert!(
                quad_on(64, 64, [21, 21], index, dst, FULL).is_empty(),
                "{index}"
            );
        }
        // Tile larger than the image (no grid) or tile size <= 0 (original: division by 0).
        for tile in [[65, 21], [21, 65], [0, 21], [21, 0], [-21, 21]] {
            assert!(quad_on(64, 64, tile, 0, dst, FULL).is_empty(), "{tile:?}");
        }
        assert!(
            quad_on(0, 0, [21, 21], 0, dst, FULL).is_empty(),
            "empty image"
        );
    }

    #[test]
    fn fill_rect_is_an_untextured_quad() {
        let mut out = Vec::new();
        fill_rect([1, 2, 3, 4], 0x8012_3456, &mut out);
        assert_eq!(
            out,
            [DrawCmd::Quad {
                texture: None,
                dst: [1, 2, 3, 4],
                uv: [0.0; 4],
                color: 0x8012_3456,
                filter: Filter::Point,
                blend: Blend::Alpha,
            }]
        );
    }

    /// `.bkg` under `data/test.bkg` with white images `(path, width, height)`.
    fn bkg_vfs(bkg: &str, images: &[(&str, u16, u16)]) -> TestVfs {
        let mut files = vec![("data/test.bkg", bkg.as_bytes().to_vec())];
        for &(path, w, h) in images {
            files.push((path, white(w, h)));
        }
        test_vfs(&files)
    }

    fn load(bkg: &str, images: &[(&str, u16, u16)]) -> (LoadedBkg, UiAssets) {
        let vfs = bkg_vfs(bkg, images);
        let mut assets = UiAssets::new();
        let loaded = load_bkg(&vfs, "data/test.bkg", &mut assets).expect("frame file loaded");
        (loaded, assets)
    }

    fn paint(
        bkg: &LoadedBkg,
        abs: [i32; 4],
        clip: [i32; 4],
        screen: Screen,
        assets: &UiAssets,
    ) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        paint_bkg(bkg, abs, clip, screen, assets, &mut out);
        out
    }

    /// Like `menuframe.bkg`: 21-pixel tiles 0...8 in reading order.
    const FRAME_BKG: &str = "FILE data/r.tga 21 21\r\nTOPLEFT 0 0\r\nTOPMID 0 1\r\nTOPRIGHT 0 2\r\nLEFTMID 0 3\r\n\
RIGHTMID 0 5\r\nBOTTOMLEFT 0 6\r\nBOTTOMMID 0 7\r\nBOTTOMRIGHT 0 8\r\nFILLTILE 0 4\r\nCLIENTAREA 15 30 15 30\r\n";

    // QUIRK O-12, O-70: kept
    #[test]
    fn nine_slice_follows_8_4_2_order() {
        let (bkg, assets) = load(FRAME_BKG, &[("data/r.tga", 64, 64)]);
        let tex = bkg.textures[0].unwrap();
        // 100 x 100 grid at 1280 x 1024 (grid = pixel): iL = iT = 121, iR = iB = 179. The clip of the call
        // does not affect the 9 parts.
        let out = paint(&bkg, [100, 100, 200, 200], [0, 0, 1, 1], S1280, &assets);
        let mut want = Vec::new();
        let t = [21, 21];
        let mut e = |index, dst, clip| expect(&assets, tex, t, index, dst, clip, &mut want);
        e(6, [100, 179, 121, 200], [100, 100, 121, 200]); // BOTTOMLEFT
        e(8, [179, 179, 200, 200], [179, 100, 200, 200]); // BOTTOMRIGHT
        e(0, [100, 100, 121, 121], [100, 100, 121, 200]); // TOPLEFT
        e(2, [179, 100, 200, 121], [179, 100, 200, 200]); // TOPRIGHT
        for y in [121, 142, 163] {
            for x in [121, 142, 163] {
                e(4, [x, y, x + 21, y + 21], [121, 121, 179, 179]); // FILLTILE
            }
        }
        for y in [121, 142, 163] {
            e(3, [100, y, 121, y + 21], [100, 121, 121, 179]); // LEFTMID
        }
        for y in [121, 142, 163] {
            e(5, [179, y, 200, y + 21], [179, 121, 200, 179]); // RIGHTMID
        }
        for x in [121, 142, 163] {
            e(1, [x, 100, x + 21, 121], [121, 100, 179, 121]); // TOPMID
        }
        for x in [121, 142, 163] {
            e(7, [x, 179, x + 21, 200], [121, 179, 179, 200]); // BOTTOMMID
        }
        assert_eq!(out.len(), 25);
        assert_eq!(out, want);
        // Last fill tile (163...184) shortened to 179: 5 of 21 pixels on the right and at the bottom via UV.
        let last_fill = &out[4 + 8];
        assert_eq!(
            dsts(std::slice::from_ref(last_fill)),
            [[163, 163, 179, 179]]
        );
        let (z, cu) = (21.0f32 / 64.0, 20.0f32 / 64.0);
        let cut = ((z + cu) - (5.0f32 / 21.0) * cu) - H64;
        assert_eq!(uv(last_fill), [z + H64, z + H64, cut, cut]);
        // Each tile individually in the grid to S: at 1024 x 768 shared edges fall onto the same pixels.
        let out = paint(&bkg, [100, 100, 200, 200], FULL, S1024, &assets);
        assert_eq!(out.len(), 25);
        assert_eq!(dsts(&out)[2], to_screen_rect(S1024, [100, 100, 121, 121]));
        assert_eq!(dsts(&out)[2][2], dsts(&out)[4][0], "TL right = fill left");
    }

    // QUIRK O-70: kept
    #[test]
    fn small_rect_keeps_inner_edges_at_max() {
        // 30 x 30 with 21-pixel tiles: iR = max(9, 21) = 21, iB = 21 - left/top win, no fill, no
        // borders; the right/bottom corners are shortened by the clip from iR/iB.
        let (bkg, assets) = load(FRAME_BKG, &[("data/r.tga", 64, 64)]);
        let out = paint(&bkg, [0, 0, 30, 30], FULL, S1280, &assets);
        assert_eq!(
            dsts(&out),
            [
                [0, 9, 21, 30],
                [21, 9, 30, 30],
                [0, 0, 21, 21],
                [21, 0, 30, 21]
            ]
        );
    }

    // QUIRK O-12: kept
    #[test]
    fn sequence_tiles_in_screen_pixels() {
        // Like `logo.bkg`: tile size 64 as a point at 1024 x 768 -> 51 x 48 pixels, from S(abs).lt = (80, 75).
        let images = [("data/a.tga", 64, 64), ("data/b.tga", 64, 64)];
        let (bkg, assets) = load(
            "FILE data/a.tga 64 64\nFILE data/b.tga 64 64\nTILE 0 0\nTILE 1 0\nNEWLINE\nTILE 1 0\n",
            &images,
        );
        let [a, b] = [bkg.textures[0].unwrap(), bkg.textures[1].unwrap()];
        let clip = [0, 0, 1024, 768];
        let out = paint(&bkg, [100, 100, 700, 600], clip, S1024, &assets);
        let mut want = Vec::new();
        expect(&assets, a, [64, 64], 0, [80, 75, 131, 123], clip, &mut want);
        expect(
            &assets,
            b,
            [64, 64],
            0,
            [131, 75, 182, 123],
            clip,
            &mut want,
        );
        expect(
            &assets,
            b,
            [64, 64],
            0,
            [80, 123, 131, 171],
            clip,
            &mut want,
        );
        assert_eq!(out, want);
        assert_eq!(
            uv(&out[0]),
            [H64, H64, 63.0 / 64.0 - H64, 63.0 / 64.0 - H64]
        );
        // The clip of the call cuts the sequence.
        let out = paint(&bkg, [100, 100, 700, 600], [0, 0, 100, 768], S1024, &assets);
        assert_eq!(dsts(&out), [[80, 75, 100, 123], [80, 123, 100, 171]]);

        // Two `NEWLINE`s advance twice by the height of the last tile; `TILE f 65535` and `TILE f -1` carry the
        // `NEWLINE` mark `k = 0xFFFF`; a tile outside the grid (7) still advances.
        let (bkg, assets) = load(
            "FILE data/a.tga 64 64\nTILE 0 0\nNEWLINE\nNEWLINE\nTILE 0 0\nTILE 0 65535\nTILE 0 0\nTILE 0 -1\n\
TILE 0 7\nTILE 0 0\n",
            &images[..1],
        );
        let out = paint(&bkg, [100, 100, 700, 600], clip, S1024, &assets);
        assert_eq!(
            dsts(&out),
            [
                [80, 75, 131, 123],
                [80, 171, 131, 219],
                [80, 219, 131, 267],
                [131, 267, 182, 315]
            ]
        );
    }

    #[test]
    fn stretch_to_bg_fills_the_widget() {
        // Like `levelbackground.bkg`: `STRECHTOBG` puts every tile onto the whole S(abs), clip = argument.
        let (bkg, assets) = load(
            "FILE data/a.tga 64 64\nTILE 0 0\nTILE 0 0\nSTRECHTOBG\n",
            &[("data/a.tga", 64, 64)],
        );
        let a = bkg.textures[0].unwrap();
        let clip = [0, 0, 1024, 768];
        let out = paint(&bkg, [100, 100, 700, 600], clip, S1024, &assets);
        let s = [80, 75, 560, 450];
        let mut want = Vec::new();
        expect(&assets, a, [64, 64], 0, s, clip, &mut want);
        expect(&assets, a, [64, 64], 0, s, clip, &mut want);
        assert_eq!(out, want);
        assert_eq!(
            uv(&out[0]),
            [H64, H64, 63.0 / 64.0 - H64, 63.0 / 64.0 - H64]
        );
    }

    // QUIRK O-12: kept
    // QUIRK O-88: deviation
    #[test]
    fn paint_skips_missing_tiles() {
        // File 0 is missing, file 1 is there. Missing slots (-1), file index outside (5, -1), image missing (file
        // 0), tile outside the grid (99, -5) - everything is omitted without a panic.
        let (bkg, assets) = load(
            "FILE data/missing.tga 21 21\nFILE data/r.tga 21 21\nTOPLEFT 1 0\nTOPRIGHT 0 2\nBOTTOMLEFT 5 6\n\
BOTTOMRIGHT -1 8\nTOPMID 1 99\nFILLTILE 1 4\nTILE 0 0\nTILE 7 0\nTILE 1 -5\n",
            &[("data/r.tga", 64, 64)],
        );
        assert_eq!(bkg.textures, [None, Some(TextureId(0))]);
        assert_eq!(assets.missing(), ["data/missing.tga"]);
        let r = bkg.textures[1].unwrap();
        let out = paint(&bkg, [0, 0, 63, 63], FULL, S1280, &assets);
        let mut want = Vec::new();
        expect(
            &assets,
            r,
            [21, 21],
            0,
            [0, 0, 21, 21],
            [0, 0, 21, 63],
            &mut want,
        );
        expect(
            &assets,
            r,
            [21, 21],
            4,
            [21, 21, 42, 42],
            [21, 21, 42, 42],
            &mut want,
        );
        assert_eq!(out, want);

        // Without `TOPLEFT` (the original would abort) or with `TOPLEFT` outside the files: no 9 parts.
        for text in [
            "FILE data/r.tga 21 21\nFILLTILE 0 4\n",
            "FILE data/r.tga 21 21\nTOPLEFT 3 0\nFILLTILE 0 4\n",
        ] {
            let (bkg, assets) = load(text, &[("data/r.tga", 64, 64)]);
            assert!(
                paint(&bkg, [0, 0, 63, 63], FULL, S1280, &assets).is_empty(),
                "{text}"
            );
        }
        // Without a `FILE` line: nothing.
        let (bkg, assets) = load("TILE 0 0\nTOPLEFT 0 0\n", &[]);
        assert!(paint(&bkg, [0, 0, 63, 63], FULL, S1280, &assets).is_empty());
        // Missing file: `None` like `FUN_0045BB00` (return value 0).
        let vfs = bkg_vfs("", &[]);
        let mut assets = UiAssets::new();
        assert!(load_bkg(&vfs, "data/nonexistent.bkg", &mut assets).is_none());
    }

    // QUIRK O-12, O-70: kept
    #[test]
    fn missing_texture_keeps_the_file_indices() {
        // `FUN_00445F40` creates an image list entry for every `FILE` line (`FUN_00402DF0` always succeeds);
        // the texture is not loaded until `FUN_00401DF0` when drawing. A missing image does not shift the indexes:
        // `TOPLEFT 1 0` draws from `r.tga`.
        let (bkg, assets) = load(
            "FILE data/missing.tga 30 30\nFILE data/r.tga 21 21\nTOPLEFT 1 0\n",
            &[("data/r.tga", 64, 64)],
        );
        let out = paint(&bkg, [0, 0, 100, 100], FULL, S1280, &assets);
        assert_eq!(dsts(&out), [[0, 0, 21, 21]]);
        assert_eq!(texture_of(&out[0]), bkg.textures[1]);
        // The tile size of the (imageless) `TOPLEFT` file (30) applies to the position, the grid (21) is that of its
        // own file.
        let (bkg, assets) = load(
            "FILE data/missing.tga 30 30\nFILE data/r.tga 21 21\nTOPLEFT 0 0\nTOPRIGHT 1 2\n",
            &[("data/r.tga", 64, 64)],
        );
        let out = paint(&bkg, [0, 0, 100, 100], FULL, S1280, &assets);
        assert_eq!(dsts(&out), [[70, 0, 100, 30]]);
    }

    #[test]
    fn swapped_extent_is_normalized() {
        // The constructor `FUN_0045A270` orders swapped `EXTENT` corners (`coords::normalize_rect`); a
        // swapped grid rectangle draws like the ordered one (sequence and 9 parts).
        let (bkg, assets) = load(
            &format!("{FRAME_BKG}FILE data/a.tga 64 64\nTILE 1 0\n"),
            &[("data/r.tga", 64, 64), ("data/a.tga", 64, 64)],
        );
        let ordered = paint(&bkg, [100, 100, 200, 200], FULL, S1024, &assets);
        assert_eq!(ordered.len(), 26);
        for swapped in [
            [200, 200, 100, 100],
            [200, 100, 100, 200],
            [100, 200, 200, 100],
        ] {
            assert_eq!(
                paint(&bkg, swapped, FULL, S1024, &assets),
                ordered,
                "{swapped:?}"
            );
        }
    }

    // QUIRK O-76: kept
    #[test]
    fn fullscreen_background_is_point_without_half_texel() {
        let mut assets = UiAssets::new();
        let tex = assets.insert_image("data/bg.tga", RgbaImage::new(8, 8));
        let mut out = Vec::new();
        paint_fullscreen(tex, S1024, 255, &mut out);
        paint_fullscreen(
            tex,
            Screen {
                width: 1600,
                height: 900,
            },
            0x80,
            &mut out,
        );
        assert_eq!(
            out,
            [
                DrawCmd::Quad {
                    texture: Some(tex),
                    dst: [0, 0, 1024, 768],
                    uv: [0.0, 0.0, 1.0, 1.0],
                    color: 0xFFFF_FFFF,
                    filter: Filter::Point,
                    blend: Blend::Alpha,
                },
                DrawCmd::Quad {
                    texture: Some(tex),
                    dst: [0, 0, 1600, 900],
                    uv: [0.0, 0.0, 1.0, 1.0],
                    color: 0x80FF_FFFF,
                    filter: Filter::Point,
                    blend: Blend::Alpha,
                }
            ]
        );
    }

    /// Font `MenuTitle` (15 px, advance 10, cell 16 x 16) and the two extra images (256^2, 85 grid).
    struct Decor {
        assets: UiAssets,
        fonts: FontBook,
        title_font: FontRef,
        plain: TextureId,
        mirrored: TextureId,
    }

    fn decor() -> Decor {
        let vfs = test_vfs(&[
            (
                "data/fonts/Test_15_0_0_0_0.fnt",
                fnt_text(|_| 10, |_| 15).into_bytes(),
            ),
            ("data/fonts/Test_15_0_0_0_0.tga", white(256, 256)),
        ]);
        let mut assets = UiAssets::new();
        let loc = parse_localisation("Font MenuTitle\nFontSize 15\nTextureFont\nFontName Test\n");
        let fonts = FontBook::load(&vfs, &loc, 1024, &mut assets, None);
        let title_font = fonts.by_name("MenuTitle");
        let plain = assets.insert_image("data/menü/zusatz_85x85.tga", RgbaImage::new(256, 256));
        let mirrored = assets.insert_image(
            "data/menü/zusatz_85x85_gespiegelt.tga",
            RgbaImage::new(256, 256),
        );
        Decor {
            assets,
            fonts,
            title_font,
            plain,
            mirrored,
        }
    }

    fn start_frame() -> FrameDesc {
        // `start.dlg` of the shipped data.
        FrameDesc {
            flags: frame_flags::DRAWBARL | frame_flags::DRAWTAGS | frame_flags::DRAWTITLEL,
            extent: [-440, 212, 0, 912],
            target: [40, 212],
            bar_extent: [0, 140, 550, 225],
            title_color: 0xDDFF_F5BF,
            title_extent: [-490, 50, -50, 185],
            title: Some(15099),
            ..FrameDesc::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn decorate(
        d: &Decor,
        frame: &FrameDesc,
        flags: u32,
        rel: [i32; 4],
        offset: [i32; 2],
        screen: Screen,
        zusatz: [Option<TextureId>; 2],
        title: Option<&[u8]>,
    ) -> Vec<DrawCmd> {
        let mut out = Vec::new();
        paint_frame_decor(
            frame,
            flags,
            rel,
            offset,
            screen,
            zusatz,
            title.map(|t| (t, d.title_font)),
            &d.fonts,
            &d.assets,
            &mut out,
        );
        out
    }

    // QUIRK O-72: kept
    #[test]
    fn frame_decor_matches_8_4_3() {
        let d = decor();
        let frame = start_frame();
        let text = atlas_codes("Hauptmenü");
        let zusatz = [Some(d.plain), Some(d.mirrored)];
        // End position: `ZIELPOSITION 40 212` with the size from `EXTENT −440 212 0 912` -> relative (40, 212, 480,
        // 912).
        let rel = [40, 212, 480, 912];
        let out = decorate(
            &d,
            &frame,
            frame.flags,
            rel,
            [0, 0],
            S1024,
            zusatz,
            Some(&text),
        );
        let p = d.plain;
        let mut want = Vec::new();
        let mut e =
            |index, dst, clip| expect(&d.assets, p, ZUSATZ_TILE, index, dst, clip, &mut want);
        // Bar L: S(BAREXTENT 0 140 550 225) = (0, 105, 440, 169), h = 64, n = 6; middle tile 1 with clip
        // (64 ... 376): i = 0 falls out completely, i = 5 is shortened to 376; then tile 0 and 2 with clip S.
        let s = [0, 105, 440, 169];
        for i in 0..6 {
            e(1, [64 * i, 105, 64 * i + 64, 169], [64, 105, 376, 169]);
        }
        e(0, [0, 105, 64, 169], s);
        e(2, [376, 105, 440, 169], s);
        // Ornaments: (95, 152, 150, 237) and (370, 152, 425, 237) in the grid -> S, tile 8, clip = rectangle.
        e(8, [76, 114, 120, 178], [76, 114, 120, 178]);
        e(8, [296, 114, 340, 178], [296, 114, 340, 178]);
        // Title L: S(TITLEEXTENT) = (-391, 38, -39, 139), W = 352, h = 101, n = 3: middle (k = 1) with empty clip
        // (-191 ... -239), end pieces 3, 4, 6, 7.
        let t = [-391, 38, -39, 139];
        e(5, [-290, 38, -189, 139], [-191, 38, -239, 139]);
        e(3, [-391, 38, -290, 139], t);
        e(4, [-290, 38, -189, 139], t);
        e(6, [-241, 38, -140, 139], t);
        e(7, [-140, 38, -39, 139], t);
        let tiles = want.len();
        assert_eq!(tiles, 13);
        // Title text: tw = 90, th = 15 -> (-391 + 131, 38 + 43).
        draw_string(
            &text,
            [-260, 81],
            0xDDFF_F5BF,
            d.title_font,
            &d.fonts,
            &mut want,
        );
        assert_eq!(out, want);
        assert_eq!(
            dsts(&out[..tiles]),
            [
                [64, 105, 128, 169],
                [128, 105, 192, 169],
                [192, 105, 256, 169],
                [256, 105, 320, 169],
                [320, 105, 376, 169],
                [0, 105, 64, 169],
                [376, 105, 440, 169],
                [76, 114, 120, 178],
                [296, 114, 340, 178],
                [-391, 38, -290, 139],
                [-290, 38, -189, 139],
                [-241, 38, -140, 139],
                [-140, 38, -39, 139],
            ]
        );
        // The title lies completely off the left edge of the screen (O-8): tiles and text.
        assert_eq!(out.len(), 13 + 9);
        assert!(
            dsts(&out[9..]).iter().all(|r| r[2] <= 0),
            "{:?}",
            dsts(&out[9..])
        );
        // Ornaments: 55 x 85 grid from cell 8 of the 85 grid (compressed, O-72).
        let (z, cu, h) = (170.0f32 / 256.0, 84.0f32 / 256.0, 0.5f32 / 256.0);
        assert_eq!(uv(&out[7]), [z + h, z + h, (z + cu) - h, (z + cu) - h]);
        // `HIDE`: nothing.
        let hidden = decorate(
            &d,
            &frame,
            frame.flags | frame_flags::HIDE,
            rel,
            [0, 0],
            S1024,
            zusatz,
            Some(&text),
        );
        assert!(hidden.is_empty());
    }

    #[test]
    fn decor_offset_moves_bars_and_titles_only() {
        // 8.4.3, 12.2: the offset `+0x130/+0x134` enters BAREXTENT and TITLEEXTENT of bar and title L/R, not the
        // ornaments (slot 23) and not the title without bar (`FUN_004612B0`). At 1280 x 1024 grid = pixel for
        // non-negative values: all quads move by exactly the offset.
        let d = decor();
        let frame = FrameDesc {
            title_extent: [100, 50, 640, 185],
            ..start_frame()
        };
        let text = atlas_codes("AB");
        let zusatz = [Some(d.plain), Some(d.mirrored)];
        let rel = [40, 212, 480, 912];
        let run =
            |flags, offset| decorate(&d, &frame, flags, rel, offset, S1280, zusatz, Some(&text));
        let shifted = |cmds: Vec<DrawCmd>, dx: i32, dy: i32| -> Vec<DrawCmd> {
            cmds.into_iter()
                .map(|c| match c {
                    DrawCmd::Quad {
                        texture,
                        dst,
                        uv,
                        color,
                        filter,
                        blend,
                    } => DrawCmd::Quad {
                        texture,
                        dst: [dst[0] + dx, dst[1] + dy, dst[2] + dx, dst[3] + dy],
                        uv,
                        color,
                        filter,
                        blend,
                    },
                    DrawCmd::Quad4 { .. } => unreachable!(),
                })
                .collect()
        };
        let moving = frame_flags::DRAWBARL
            | frame_flags::DRAWBARR
            | frame_flags::DRAWTITLEL
            | frame_flags::DRAWTITLER;
        let base = run(moving, [0, 0]);
        assert!(!base.is_empty());
        assert_eq!(run(moving, [240, 0]), shifted(base.clone(), 240, 0));
        assert_eq!(run(moving, [0, 500]), shifted(base, 0, 500));
        for still in [frame_flags::DRAWTAGS, frame_flags::TITLE_WITHOUT_BAR] {
            assert_eq!(run(still, [-240, 0]), run(still, [0, 0]), "{still:#x}");
        }
    }

    // QUIRK O-73: kept
    #[test]
    fn title_text_is_centered_without_shadow() {
        let d = decor();
        let frame = FrameDesc {
            title_color: 0xC012_3456,
            title_extent: [100, 50, 500, 150],
            ..FrameDesc::default()
        };
        let text = atlas_codes("AB");
        let title = |frame: &FrameDesc, text: Option<&[u8]>| {
            decorate(
                &d,
                frame,
                frame_flags::DRAWTITLEL,
                [0; 4],
                [0, 0],
                S1280,
                [None, None],
                text,
            )
        };
        // Without extra images only the text remains: tw = 20, th = 15 -> (100 + 190, 50 + 42), one quad per
        // character in TITLECOLOR (no shadow).
        let out = title(&frame, Some(&text));
        let mut want = Vec::new();
        draw_string(
            &text,
            [290, 92],
            0xC012_3456,
            d.title_font,
            &d.fonts,
            &mut want,
        );
        assert_eq!(out, want);
        assert_eq!(dsts(&out), [[290, 92, 306, 108], [300, 92, 316, 108]]);
        // Wider than the title: C division towards 0, (10 - 20) / 2 = -5.
        let narrow = FrameDesc {
            title_extent: [100, 50, 110, 150],
            ..frame.clone()
        };
        assert_eq!(dsts(&title(&narrow, Some(&text)))[0], [95, 92, 111, 108]);
        // Without text no glyphs.
        assert!(title(&frame, None).is_empty());
    }

    // QUIRK O-73: kept
    // QUIRK O-88: deviation
    #[test]
    fn mirrored_and_barless_titles_use_their_tiles() {
        let d = decor();
        let frame = FrameDesc {
            title_extent: [0, 0, 500, 100],
            ..FrameDesc::default()
        };
        let zusatz = [Some(d.plain), Some(d.mirrored)];
        let (a, b) = (d.plain, d.mirrored);
        let run = |frame: &FrameDesc, flags| {
            decorate(&d, frame, flags, [0; 4], [0, 0], S1280, zusatz, None)
        };
        // h = 100, n = 5: middle k = 1 ... 3 with clip (198, 0, 302, 100), then left (0 ...), (100 ...), right
        // (300 ...), (400 ...).
        let clip = [198, 0, 302, 100];
        let s = [0, 0, 500, 100];
        let mid = [[100, 0, 200, 100], [200, 0, 300, 100], [300, 0, 400, 100]];
        let ends = [
            [0, 0, 100, 100],
            [100, 0, 200, 100],
            [300, 0, 400, 100],
            [400, 0, 500, 100],
        ];
        let build = |mid_tex, mid_tile, end: [(TextureId, i32); 4]| {
            let mut want = Vec::new();
            for r in mid {
                expect(
                    &d.assets,
                    mid_tex,
                    ZUSATZ_TILE,
                    mid_tile,
                    r,
                    clip,
                    &mut want,
                );
            }
            for (r, (tex, tile)) in ends.into_iter().zip(end) {
                expect(&d.assets, tex, ZUSATZ_TILE, tile, r, s, &mut want);
            }
            want
        };
        // `DRAWTITLER` (`FUN_00460EC0`): everything mirrored, middle 3, left 6/7, right 4/5.
        let out = run(&frame, frame_flags::DRAWTITLER);
        assert_eq!(out, build(b, 3, [(b, 6), (b, 7), (b, 4), (b, 5)]));
        assert_eq!(
            dsts(&out[..3]),
            [[198, 0, 200, 100], [200, 0, 300, 100], [300, 0, 302, 100]],
            "middle 2 px under the end pieces (O-73)"
        );
        // Title without bar `0x100` (`FUN_004612B0`): middle unmirrored 5, left mirrored 6/7, right
        // unmirrored 6/7.
        let out = run(&frame, frame_flags::TITLE_WITHOUT_BAR);
        assert_eq!(out, build(a, 5, [(b, 6), (b, 7), (a, 6), (a, 7)]));
        // `DRAWTITLEL` for comparison: everything unmirrored, middle 5, left 3/4, right 6/7.
        let out = run(&frame, frame_flags::DRAWTITLEL);
        assert_eq!(out, build(a, 5, [(a, 3), (a, 4), (a, 6), (a, 7)]));
        // Too short for a middle (n - 1 <= 1): only the end pieces.
        let short = FrameDesc {
            title_extent: [0, 0, 299, 100],
            ..frame.clone()
        };
        assert_eq!(run(&short, frame_flags::DRAWTITLEL).len(), 4);
        // Bar R from the mirrored texture (n = 2: middle i = 0 and 1 fall into the empty clip
        // (100 ... 100), tiles 0 and 2 remain).
        let bar = FrameDesc {
            bar_extent: [0, 0, 200, 100],
            ..frame.clone()
        };
        let out = run(&bar, frame_flags::DRAWBARR);
        assert_eq!(dsts(&out), [[0, 0, 100, 100], [100, 0, 200, 100]]);
        assert!(out.iter().all(|c| texture_of(c) == Some(b)));
        // Height 0 (division by 0 in the original): nothing.
        let flat = FrameDesc {
            bar_extent: [0, 0, 200, 0],
            title_extent: [0, 0, 200, 0],
            ..frame.clone()
        };
        let all = frame_flags::DRAWBARL
            | frame_flags::DRAWBARR
            | frame_flags::DRAWTITLEL
            | frame_flags::DRAWTITLER
            | frame_flags::TITLE_WITHOUT_BAR;
        assert!(run(&flat, all).is_empty());
    }
}
