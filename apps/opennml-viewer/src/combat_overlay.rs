//! Painter of the combat display with egui: health bars, rank icons and the counter-upgrade icon from the
//! calculation in [`opennml_client::game::combat_calc`] (`GameObject_DrawHealthBar` `0x0046C0F0`, re/combat-host 4.3;
//! `FUN_0046BBE0`, re/combat-damage 4.1; `FUN_00469F60`). Rendering only; the simulation stays untouched.

use egui::{Color32, Pos2, Rect};
use opennml_rda::Vfs;

use opennml_client::game::combat_calc::{COUNTER_UPGRADE_ICON, CombatOverlay, RANK_ICONS};

/// The five rank textures and the counter-upgrade icon as egui textures, loaded in the first frame with an
/// icon. A missing file is logged once; the icon is then omitted.
#[derive(Default)]
pub struct RankIcons {
    textures: [Option<egui::TextureHandle>; 5],
    counter: Option<egui::TextureHandle>,
    loaded: bool,
}

/// Texture `path` as an egui texture; errors are logged.
fn load_icon(ctx: &egui::Context, vfs: &Vfs, path: &str) -> Option<egui::TextureHandle> {
    match opennml_world::assets::load_texture(vfs, path) {
        Ok(image) => {
            let size = [image.width() as usize, image.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            Some(ctx.load_texture(path, color, egui::TextureOptions::LINEAR))
        }
        Err(e) => {
            log::error!("Icon {path}: {e:#}");
            None
        }
    }
}

impl RankIcons {
    fn load(&mut self, ctx: &egui::Context, vfs: &Vfs) {
        self.loaded = true;
        for (slot, path) in self.textures.iter_mut().zip(RANK_ICONS) {
            *slot = load_icon(ctx, vfs, path);
        }
        self.counter = load_icon(ctx, vfs, COUNTER_UPGRADE_ICON);
    }
}

/// Draws bars and rank icons with the egui background painter (always on top, below the windows).
pub fn paint(ctx: &egui::Context, overlay: &CombatOverlay, icons: &mut RankIcons, vfs: &Vfs) {
    if overlay.bars.is_empty() && overlay.icons.is_empty() && overlay.counter_icons.is_empty() {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let point = |p: [f32; 2]| Pos2::new(p[0] / ppp, p[1] / ppp);
    let rect = |r: [[f32; 2]; 2]| Rect::from_min_max(point(r[0]), point(r[1]));
    let painter = ctx.layer_painter(egui::LayerId::background());
    let argb = |c: u32| {
        Color32::from_rgba_unmultiplied((c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8)
    };
    for bar in &overlay.bars {
        painter.rect_filled(rect(bar.background), 0.0, argb(bar.red));
        painter.rect_filled(rect(bar.fill), 0.0, argb(bar.green));
    }
    let any_icon = !overlay.icons.is_empty() || !overlay.counter_icons.is_empty();
    if any_icon && !icons.loaded {
        icons.load(ctx, vfs);
    }
    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    for icon in &overlay.icons {
        if let Some(Some(texture)) = icons.textures.get(usize::from(icon.rank) - 1) {
            painter.image(texture.id(), rect(icon.rect), uv, Color32::WHITE);
        }
    }
    if let Some(texture) = &icons.counter {
        for &r in &overlay.counter_icons {
            painter.image(texture.id(), rect(r), uv, Color32::WHITE);
        }
    }
}
