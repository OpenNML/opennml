//! UI layer of the host (GPU-free): images ([`UiAssets`]), fonts ([`FontBook`]) and texts ([`Texts`]) of the
//! installation, loaded according to the language file `localisation.deu` (re/ui-rendering 3.2). The renderer
//! (`opennml_render::UiRenderer`, a renderer belongs to exactly one [`UiAssets`]) lives with the host and is passed in when
//! drawing; this way loading and draw commands run in tests without a GPU.
//!
//! The fonts are created once with the width at start (O-58, `FontBook::load`). Missing atlases are created by an
//! explicitly passed generator; tests pass none or one of their own and thus never write into the user's cache.

use anyhow::Result;
use opennml_rda::Vfs;
use opennml_render::UiRenderer;
use opennml_ui::assets::{Texts, UiAssets, load_localisation};
use opennml_ui::coords::Screen;
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::DrawCmd;
use opennml_ui::fonts::{AtlasGenerator, FontBook};

/// Atlas generator of the programs: GDI with a cache outside the installation
/// (`opennml_ui::atlas_gen::default_generator`; game directory = folder of the first archive). Tests do not call it but
/// pass `None` or a generator of their own.
pub fn atlas_generator(vfs: &Vfs) -> Option<Box<dyn AtlasGenerator>> {
    let game_dir = vfs.archives().first()?.path().parent()?;
    opennml_ui::atlas_gen::default_generator(game_dir)
}

/// Screen of the UI from a pixel size (window, surface, screenshot); saturates on overflow.
pub fn screen_of(size: (u32, u32)) -> Screen {
    Screen {
        width: i32::try_from(size.0).unwrap_or(i32::MAX),
        height: i32::try_from(size.1).unwrap_or(i32::MAX),
    }
}

/// Images, fonts and texts of the UI.
pub struct UiLayer {
    pub assets: UiAssets,
    pub fonts: FontBook,
    pub texts: Texts,
}

impl UiLayer {
    /// Loads language file, texts and all fonts (width `screen_width` for relative sizes, only now: O-58). Missing
    /// atlases are created by `generator` (`None`: fallback).
    pub fn new(
        vfs: &Vfs,
        screen_width: u32,
        generator: Option<&dyn AtlasGenerator>,
    ) -> Result<Self> {
        let loc = load_localisation(vfs).map_err(anyhow::Error::msg)?;
        let texts = Texts::load(vfs, &loc);
        let mut assets = UiAssets::new();
        let width = i32::try_from(screen_width).unwrap_or(i32::MAX);
        let fonts = FontBook::load(vfs, &loc, width, &mut assets, generator);
        Ok(Self {
            assets,
            fonts,
            texts,
        })
    }

    /// Context for building and draw commands of the dialogs (`opennml_ui::dialog`) on a screen `screen`.
    pub fn context<'a>(&'a mut self, vfs: &'a Vfs, screen: Screen) -> UiContext<'a> {
        UiContext {
            vfs,
            assets: &mut self.assets,
            fonts: &self.fonts,
            texts: &self.texts,
            screen,
        }
    }

    /// Draws `cmds` with `renderer` over the existing image (`LoadOp::Load`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        renderer: &mut UiRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        cmds: &[DrawCmd],
    ) {
        renderer.draw(device, queue, encoder, target, size, &self.assets, cmds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{LOCALISATION, fnt, install, tga, utf16};

    #[test]
    fn layer_loads_texts_and_fonts_with_the_start_width_without_gpu() {
        let (_dir, vfs) = install(&[
            (
                "data/txt/localisation.deu",
                LOCALISATION.as_bytes().to_vec(),
            ),
            ("data/txt/t.txt", utf16("1=AB\r\n")),
            ("data/fonts/Test_21_0_0_0_0.fnt", fnt()),
            ("data/fonts/Test_21_0_0_0_0.tga", tga(256, 256)),
        ]);
        let mut layer = UiLayer::new(&vfs, 1024, None).unwrap();
        assert_eq!(layer.texts.get(1), Some("AB"));
        let button = layer.fonts.by_name("MenuButton");
        assert_eq!(layer.fonts.font(button).px, 21, "relative 27 at width 1024");
        let screen = Screen {
            width: 1024,
            height: 768,
        };
        let ctx = layer.context(&vfs, screen);
        assert_eq!(ctx.screen, screen);
        // Without a language file: error instead of a crash.
        let (_empty, vfs) = install(&[]);
        assert!(UiLayer::new(&vfs, 1024, None).is_err());
    }
}
