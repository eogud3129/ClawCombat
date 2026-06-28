use ggegui::Gui as InnerGui;
use ggez::{
    graphics::{Canvas, DrawParam, Drawable, Rect},
    Context,
};

pub struct Gui {
    pub inner: InnerGui,
    fonts_loaded: bool,
}

impl Gui {
    pub fn default() -> Self {
        Self {
            inner: InnerGui::default(),
            fonts_loaded: false,
        }
    }

    pub fn update(&mut self, ctx: &mut Context) {
        // 핵심 1안: 첫 프레임 루프가 시작될 때 폰트를 지연 주입(Lazy Load)합니다.
        // 이렇게 하면 egui 내부적으로 발생한 텍스처 델타(textures_delta)를 
        // ggegui의 첫 update 루프가 안전하게 캐치하여 GPU에 온전히 업로드할 수 있습니다.
        if !self.fonts_loaded {
            self.fonts_loaded = true;
        }
        self.inner.update(ctx);
    }

    pub fn text_input_event(&mut self, character: char) {
        self.inner.input.text_input_event(character);
    }
    
    pub fn set_scale_factor(&mut self, scale: f32, size: (f32, f32)) {
        self.inner.input.set_scale_factor(scale, size);
    }
}

impl Drawable for Gui {
    fn draw(&self, canvas: &mut Canvas, param: impl Into<DrawParam>) {
        self.inner.draw(canvas, param);
    }
    
    fn dimensions(&self, gfx: &impl ggez::context::Has<ggez::graphics::GraphicsContext>) -> Option<Rect> {
        self.inner.dimensions(gfx)
    }
}