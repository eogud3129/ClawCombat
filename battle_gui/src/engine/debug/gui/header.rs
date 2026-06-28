use battle_core::{
    config::ChangeConfigMessage,
    state::battle::{message::BattleStateMessage, phase::Phase},
};
use ggegui::egui::{Context as EguiContext, Ui};
use ggez::Context;

use crate::{
    debug::DebugPhysics,
    engine::{
        message::{EngineMessage, GuiStateMessage},
        Engine,
    },
};
use strum::IntoEnumIterator;

impl Engine {
    pub fn debug_gui_header(
        &mut self,
        ctx: &mut Context,
        _egui_ctx: &EguiContext,
        ui: &mut Ui,
    ) -> Vec<EngineMessage> {
        let mut messages = vec![];

        ui.horizontal(|ui| {
            let side_text = format!("Side {}", self.gui_state.side());
            if ui.button(&side_text).clicked() {
                messages.push(EngineMessage::GuiState(GuiStateMessage::ChangeSide))
            }

            ui.separator();

            let mut yolo_mode_a = self.server_config.yolo_mode_a;
            if ui.checkbox(&mut yolo_mode_a, "YOLO Mode A").changed() {
                messages.push(EngineMessage::ChangeServerConfig(
                    ChangeConfigMessage::YoloModeA(yolo_mode_a),
                ));
            }
            
            let mut yolo_mode_b = self.server_config.yolo_mode_b;
            if ui.checkbox(&mut yolo_mode_b, "YOLO Mode B").changed() {
                messages.push(EngineMessage::ChangeServerConfig(
                    ChangeConfigMessage::YoloModeB(yolo_mode_b),
                ));
            }

            ui.separator();

            ui.checkbox(&mut self.gui_state.debug_mouse, "Cursor");
            ui.checkbox(&mut self.gui_state.debug_move_paths, "Move");
            if ui
                .checkbox(&mut self.gui_state.debug_formation_positions, "Formation")
                .changed()
            {
                messages.push(EngineMessage::ChangeServerConfig(
                    ChangeConfigMessage::SendDebugPoints(self.gui_state.debug_formation_positions),
                ));
            };
            ui.checkbox(&mut self.gui_state.debug_scene_item_circles, "Soldier");
            ui.checkbox(&mut self.gui_state.debug_areas, "Areas");
            ui.checkbox(&mut self.gui_state.debug_visibilities, "Visibilities");
            ui.checkbox(&mut self.gui_state.debug_targets, "Targets");
            ui.checkbox(&mut self.gui_state.debug_physics_areas, "Physics");
            ui.checkbox(&mut self.gui_state.debug_grid, "Grid");

            ui.label(format!("FPS : {:.2}", ctx.time.fps()));
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Game Speed:");
            for speed in 1..=5 {
                if ui.button(format!("{}X", speed)).clicked() {
                    let speed_f32 = speed as f32;
                    messages.push(EngineMessage::ChangeServerConfig(ChangeConfigMessage::GameSpeed(speed_f32)));
                    self.config.target_fps = (battle_core::config::TARGET_FPS as f32 * speed_f32) as u32;
                }
            }

            ui.separator();

            ui.label("Global Volume:");
            ui.add(ggegui::egui::Slider::new(&mut self.config.global_volume, 0.0..=1.0));
            if ui.button("Reset").clicked() {
                self.config.global_volume = 0.2;
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Phase");
            for phase in Phase::iter() {
                let text = phase.to_string();
                if ui
                    .radio_value(self.battle_state.phase_mut(), phase.clone(), text)
                    .changed()
                {
                    messages.push(EngineMessage::BattleState(BattleStateMessage::SetPhase(
                        phase.clone(),
                    )));
                }
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Cursor physics");
            ui.horizontal(|ui| {
                let changes = [ui.radio_value(self.gui_state.debug_physics_mut(), DebugPhysics::None, "No")
                        .changed(),
                    ui.radio_value(
                        self.gui_state.debug_physics_mut(),
                        DebugPhysics::MosinNagantM1924GunFire,
                        "MosinNagantM1924",
                    )
                    .changed(),
                    ui.radio_value(
                        self.gui_state.debug_physics_mut(),
                        DebugPhysics::BrandtMle2731Shelling,
                        "BrandtMle2731",
                    )
                    .changed()];

                if changes.iter().any(|v| *v) {
                    messages.extend(vec![EngineMessage::GuiState(GuiStateMessage::SetControl(
                        self.physics_control(self.gui_state.debug_physics()),
                    ))]);
                }
            });
        });

        ui.separator();

        messages
    }
}
