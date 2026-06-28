use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use battle_core::config::{GuiConfig, ServerConfig};
use battle_core::game::control::MapControl;
use battle_core::game::Side;
use battle_core::message::{InputMessage, OutputMessage};
use battle_core::state::battle::BattleState;
use battle_core::types::WindowPoint;
use crossbeam_channel::{Receiver, Sender};
use self::ggegui_patch::Gui;
use ggez::event::EventHandler;
use ggez::event::MouseButton;
use ggez::graphics::{self, Canvas, Color, MeshBuilder};
use ggez::input::keyboard::KeyInput;
use ggez::GameError;
use ggez::{Context, GameResult};

use crate::audio::player::Player;
use crate::graphics::Graphics;
use crate::saves::reader::BattleSavesListBuilder;
use crate::ui::hud::builder::HudBuilder;
use crate::ui::hud::painter::HudPainter;
use crate::ui::hud::{Hud, HUD_HEIGHT};

use crate::engine::message::GuiStateMessage;

use self::debug::gui::state::DebugGuiState;
use self::message::EngineMessage;
use self::state::GuiState;

pub mod debug;
pub mod draw;
pub mod end;
pub mod event;
pub mod game;
pub mod gui;
pub mod hud;
pub mod input;
pub mod interior;
pub mod intro;
pub mod message;
pub mod network;
pub mod order;
pub mod physics;
pub mod react;
pub mod save;
pub mod state;
pub mod tick;
pub mod ui;
pub mod utils;
pub mod ggegui_patch;
pub mod ime;

pub struct Engine {
    config: GuiConfig,
    // Mirror of server config used to live debug window
    server_config: ServerConfig,
    graphics: Graphics,
    input: Receiver<Vec<OutputMessage>>,
    output: Sender<Vec<InputMessage>>,
    player: Player,
    /// The current shared state of the game. This struct is own by server and replicated on clients
    battle_state: BattleState,
    /// The current local state of the game.
    gui_state: GuiState,
    sync_required: Arc<AtomicBool>,
    stop_required: Arc<AtomicBool>,
    // Debug gui
    debug_gui: DebugGuiState,
    egui_backend: Gui,
    ///
    hud: Hud,
    a_control: MapControl,
    b_control: MapControl,
    //
    first_copy_loaded: bool,
    when_first_copy_messages: Vec<EngineMessage>,
    // [추가] 인게임 자체 한글 입력기(오토마타) 상태 머신
    pub hangul_ime: ime::HangulAutomata,
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &mut Context,
        side: &Side,
        config: GuiConfig,
        server_config: ServerConfig,
        input_sender: Sender<Vec<InputMessage>>,
        output_receiver: Receiver<Vec<OutputMessage>>,
        graphics: Graphics,
        battle_state: BattleState,
        sync_required: Arc<AtomicBool>,
        stop_required: Arc<AtomicBool>,
        a_control: MapControl,
        b_control: MapControl,
        apply: Vec<EngineMessage>,
        when_first_copy_apply: Vec<EngineMessage>,
    ) -> GameResult<Engine> {
        let mut gui_state = GuiState::new(*side, battle_state.map());
        gui_state.set_saves(
            BattleSavesListBuilder::new(battle_state.map().name())
                .build()
                .unwrap_or_default(),
        );

        let hud = HudBuilder::new(&gui_state, &battle_state).build(ctx);

        let mut engine = Engine {
            config,
            server_config,
            graphics,
            input: output_receiver, // Gui input is server output
            output: input_sender,   // Gui output is server input
            player: Player::new(ctx)?,
            battle_state,
            gui_state,
            sync_required,
            stop_required,
            debug_gui: DebugGuiState::new()?,
            egui_backend: Gui::default(),
            hud,
            a_control,
            b_control,
            first_copy_loaded: false,
            when_first_copy_messages: when_first_copy_apply,
            hangul_ime: ime::HangulAutomata::new(),
        };
        engine.react(apply, ctx)?;

        // [수정] 자체 인게임 오토마타를 사용하므로, 이벤트를 씹어먹는 OS 네이티브 IME는 강제로 꺼버립니다.
        ctx.gfx.window().set_ime_allowed(false);

        // [추가] 네이티브 렌더링용 한글 폰트를 엔진 코어에 직접 등록합니다.
        let font_data = ggez::graphics::FontData::from_slice(include_bytes!("../../../resources/fonts/GowunBatang-Regular.ttf")).unwrap();
        ctx.gfx.add_font("korean", font_data);

        Ok(engine)
    }
}

impl EventHandler<ggez::GameError> for Engine {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        let frame_i = self.gui_state.frame_i();
        puffin::profile_scope!("update", format!("frame {frame_i}"));
        puffin::GlobalProfiler::lock().new_frame();

        while ctx.time.check_update_time(self.config.target_fps) {
            // Execute "each frame" code
            self.tick(ctx)?;

            // Increment the frame counter
            self.gui_state.increment_frame_i();
        }

        // [Egui 플리커링 방지] Egui는 Immediate Mode GUI이므로 모든 UI 선언이 끝난 후
        // 마지막에 update(메쉬 및 텍스처 버퍼 생성)를 호출해야 프레임 지연 및 깜빡임이 발생하지 않습니다.
        self.update_debug_gui(ctx)?;
        self.update_intro_gui(ctx)?;
        self.update_end_gui(ctx)?;
        self.update_chat_gui(ctx)?;
        self.update_task_gui(ctx)?; // [추가] Task 리스트 업데이트 
        self.egui_backend.update(ctx); 
        self.graphics.tick(ctx);

        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let window = ctx.gfx.window().inner_size();
        
        // 창이 최소화되었을 때는 그리기 로직을 건너뛰어 백그라운드에서 서버 및 논리 업데이트만 계속 진행합니다.
        if window.width == 0 || window.height == 0 {
            return Ok(());
        }

        let mut canvas = Canvas::from_frame(ctx, Color::from((0.392, 0.584, 0.929)));
        self.hud = HudBuilder::new(&self.gui_state, &self.battle_state)
            .point(WindowPoint::new(0., window.height as f32 - HUD_HEIGHT))
            .width(window.width as f32)
            .height(HUD_HEIGHT)
            .build(ctx);

        self.graphics.clear(&self.gui_state.zoom);
        let dest = graphics::DrawParam::new().dest(self.gui_state.display_scene_offset.to_vec2());
        let scale = dest.scale(self.gui_state.zoom.to_vec2());
        let decor = self.gui_state.draw_decor;

        // Draw entire scene
        self.generate_map_sprites(self.gui_state.draw_decor)?;
        self.generate_flags_sprites()?;
        self.generate_soldiers_sprites()?;
        self.generate_vehicles_sprites()?;
        self.generate_explosion_sprites()?;
        self.generate_cannon_blasts_sprites()?;
        self.graphics
            .draw_map(&mut canvas, dest, &self.gui_state.zoom)?;
        self.draw_debug_terrain(ctx, &mut canvas, scale)?;
        self.graphics
            .draw_units(&mut canvas, dest, &self.gui_state.zoom)?;
        self.graphics
            .draw_decor(&mut canvas, decor, dest, &self.gui_state.zoom)?;
        self.graphics.draw_flags(&mut canvas, dest)?;
        self.draw_flags_names(&mut canvas, dest)?;
        self.draw_ammo_crates(&mut canvas, dest)?;
        self.draw_debug_grid(ctx, &mut canvas, dest)?;

        // Draw ui
        let mut mesh_builder = MeshBuilder::new();
        self.generate_menu_sprites()?;

        self.draw_physics(&mut mesh_builder)?;
        self.generate_debug_meshes(&mut mesh_builder)?;
        self.generate_selection_meshes(&mut mesh_builder)?;
        self.generate_display_paths_meshes(&mut mesh_builder)?;
        self.generate_game_play_meshes(&mut mesh_builder)?;
        self.generate_hud_meshes(ctx, &mut mesh_builder)?;
        self.generate_orders_sprites(&mut mesh_builder)?;
        self.generate_hud_sprites(ctx)?;

        let ui_draw_param = graphics::DrawParam::new();
        self.graphics
            .draw_ui(ctx, &mut canvas, ui_draw_param, mesh_builder)?;

        self.graphics.draw_minimap(ctx, &mut canvas, &self.hud)?;
        HudPainter::new(&self.hud, &self.gui_state).draw(ctx, &mut canvas)?;

        self.draw_egui(ctx, &mut canvas);

        if self.gui_state.display_chat_gui() {
            let window_size = ctx.gfx.window().inner_size();
            let rect_w = 400.0;
            let rect_h = 50.0;
            let rect_x = (window_size.width as f32 - rect_w) / 2.0;
            let rect_y = window_size.height as f32 - rect_h - 220.0;
            let bg_rect = ggez::graphics::Rect::new(rect_x, rect_y, rect_w, rect_h);
            let bg_mesh = ggez::graphics::Mesh::new_rectangle(
                ctx,
                ggez::graphics::DrawMode::fill(),
                bg_rect,
                ggez::graphics::Color::new(0.0, 0.0, 0.0, 0.8),
            )?;
            canvas.draw(&bg_mesh, ggez::graphics::DrawParam::default());
            let cursor_char = if self.gui_state.frame_i() % 60 < 30 { "_" } else { "" };
            let mode_text = if self.hangul_ime.is_korean_mode { "[한]" } else { "[영]" };
            let display_text = format!("{} {}{}", mode_text, self.gui_state.chat_input(), cursor_char);
            let mut text = ggez::graphics::Text::new(display_text);
            text.set_font("korean").set_scale(24.0);
            canvas.draw(
                &text,
                ggez::graphics::DrawParam::default()
                    .dest(glam::Vec2::new(rect_x + 15.0, rect_y + 12.0))
                    .color(ggez::graphics::Color::WHITE),
            );
        }

        canvas.finish(ctx)?;

        Ok(())
    }

    fn mouse_button_down_event(
        &mut self,
        ctx: &mut Context,
        button: MouseButton,
        x: f32,
        y: f32,
    ) -> Result<(), GameError> {
        // [입력 모드 Lock] 채팅창(LLM 명령)이 활성화되어 있을 때는 마우스 클릭이 게임 월드에 영향을 주지 않도록 전면 차단합니다.
        if self.gui_state.display_chat_gui() || self.egui_backend.inner.ctx().is_pointer_over_area() {
            return GameResult::Ok(());
        }

        if !self.gui_state.debug_gui_hovered {
            let messages = self.collect_mouse_down(ctx, button, x, y);
            self.react(messages, ctx)?;
        }
        GameResult::Ok(())
    }

    fn mouse_button_up_event(
        &mut self,
        ctx: &mut Context,
        button: MouseButton,
        x: f32,
        y: f32,
    ) -> Result<(), GameError> {
        // [입력 모드 Lock] 채팅창 활성화 시 월드 조작 메시지 발생을 전면 차단합니다.
        if self.gui_state.display_chat_gui() || self.egui_backend.inner.ctx().is_pointer_over_area() {
            if !self.egui_backend.inner.ctx().is_pointer_over_area() && self.gui_state.display_chat_gui() {
                let window_point = WindowPoint::new(x, y);
                let world_point = self.gui_state.world_point_from_window_point(window_point);
                
                let mut append_str = String::new();
                let mut is_squad_clicked = false;

                // 1. HUD UI(분대 카드) 클릭 검사 - 왼쪽 클릭만 활성화
                if button == MouseButton::Left {
                    if let Some(component) = self.hud.hovered_by(ctx, &[&window_point, &window_point]) {
                        if let Some(crate::ui::hud::event::HudEvent::SelectSquad(squad_uuid)) = component.event(ctx) {
                            append_str = format!("@{}분대", squad_uuid.0);
                            is_squad_clicked = true;
                        } else if let Some(crate::ui::hud::event::HudEvent::CenterMapOnSquad(squad_uuid)) = component.event(ctx) {
                            append_str = format!("@{}분대", squad_uuid.0);
                            is_squad_clicked = true;
                        } else if let Some(crate::ui::hud::event::HudEvent::SelectSoldier(soldier_index)) = component.event(ctx) {
                            let soldier = self.battle_state.soldier(soldier_index);
                            append_str = format!("@{}분대", soldier.squad_uuid().0);
                            is_squad_clicked = true;
                        }
                    }
                }

                // 2. 맵 상의 분대(병사) 클릭 검사 - 왼쪽 클릭만 활성화
                if !is_squad_clicked && button == MouseButton::Left {
                    let soldiers = self.soldiers_at_point(world_point, Some(self.gui_state.side()));
                    if let Some(soldier) = soldiers.first() {
                        append_str = format!("@{}분대", soldier.squad_uuid().0);
                        is_squad_clicked = true;
                    }
                }

                // 3. 분대가 클릭되지 않았다면 섹터(그리드) 클릭으로 처리 (기존 로직 유지)
                if !is_squad_clicked {
                    let map = self.battle_state.map();
                    let grid_size = 30;
                    let cell_width = map.tile_width() as f32 * grid_size as f32;
                    let cell_height = map.tile_height() as f32 * grid_size as f32;
                    
                    let mut offset_x = 0.0;
                    let mut offset_y = 0.0;
                    if let Some(first_flag) = map.flags().first() {
                        let flag_center = first_flag.position();
                        offset_x = flag_center.x % cell_width - (cell_width / 2.0);
                        offset_y = flag_center.y % cell_height - (cell_height / 2.0);
                    }

                    let chars: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
                    let adj_x = world_point.x - offset_x;
                    let adj_y = world_point.y - offset_y;
                    let col = (adj_x / cell_width).floor() as i32;
                    let row = (adj_y / cell_height).floor() as i32;
                    let row_idx = (row + 5).max(0) as usize;
                    let col_idx = (col + 6).max(0) as usize;
                    let letter = chars.get(row_idx % chars.len()).unwrap_or(&'?');
                    let sector_name = format!("{}{}", letter, col_idx);

                    append_str = match button {
                        MouseButton::Left => format!("&{}", sector_name),
                        MouseButton::Right => format!("#{}", sector_name),
                        _ => "".to_string(),
                    };
                }

                if !append_str.is_empty() {
                    if !self.gui_state.chat_input().is_empty() && !self.gui_state.chat_input().ends_with(' ') {
                        self.gui_state.chat_input_mut().push(' ');
                    }
                    self.gui_state.chat_input_mut().push_str(&append_str);
                    self.gui_state.chat_input_mut().push(' ');
                    
                    let query = self.gui_state.chat_input().to_string();
                    self.react(vec![crate::engine::message::EngineMessage::RequestTacticSuggestions(query)], ctx)?;
                }
            }
            return GameResult::Ok(());
        }

        if !self.gui_state.debug_gui_hovered {
            let messages = self.collect_mouse_up(ctx, button, x, y);
            self.react(messages, ctx)?;
        }
        GameResult::Ok(())
    }

    fn mouse_motion_event(
        &mut self,
        ctx: &mut Context,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    ) -> Result<(), GameError> {
        let messages = self.collect_mouse_motion(ctx, x, y, dx, dy);
        
        // [입력 모드 Lock] 채팅창 활성화 상태이거나 UI 위에 마우스가 있을 경우,
        // 순수 커서 좌표 갱신 메시지만 통과시키고, 화면 스크롤(Edge pan)이나 유닛 호버 같은 게임 이벤트를 모두 차단하여 포커스 해제를 방지합니다.
        if self.gui_state.display_chat_gui() || self.egui_backend.inner.ctx().is_pointer_over_area() {
            let filtered_messages: Vec<_> = messages
                .into_iter()
                .filter(|m| matches!(m, EngineMessage::GuiState(GuiStateMessage::SetCursorPoint(_))))
                .collect();
            self.react(filtered_messages, ctx)?;
            return GameResult::Ok(());
        }

        self.react(messages, ctx)?;
        GameResult::Ok(())
    }

    fn mouse_wheel_event(&mut self, ctx: &mut Context, x: f32, y: f32) -> Result<(), GameError> {
        // [입력 모드 Lock] 채팅창 활성화 상태이거나 UI 영역일 때 줌인/아웃 등의 휠 이벤트를 차단합니다.
        if self.gui_state.display_chat_gui() || self.egui_backend.inner.ctx().is_pointer_over_area() {
            return GameResult::Ok(());
        }

        let messages = self.collect_mouse_wheel(ctx, x, y);
        self.react(messages, ctx)?;
        GameResult::Ok(())
    }

    fn key_down_event(
        &mut self,
        ctx: &mut Context,
        input: KeyInput,
        _repeated: bool,
    ) -> Result<(), GameError> {
        // [정공법] 인게임 자체 한글 오토마타 이벤트 후킹 (kime 방식)
        if self.gui_state.display_chat_gui() {
            if let Some(keycode) = input.keycode {
                let is_shift = ctx.keyboard.is_key_pressed(ggez::winit::event::VirtualKeyCode::LShift) 
                            || ctx.keyboard.is_key_pressed(ggez::winit::event::VirtualKeyCode::RShift);
                
                // 한/영 전환 토글 (우측 Alt 또는 RWin/Kana 키 사용)
                if keycode == ggez::winit::event::VirtualKeyCode::RAlt || keycode == ggez::winit::event::VirtualKeyCode::Kana {
                    self.hangul_ime.toggle_mode();
                    println!("[IME] Korean Mode: {}", self.hangul_ime.is_korean_mode);
                    return GameResult::Ok(());
                }

                // 백스페이스 우선 가로채기
                if keycode == ggez::winit::event::VirtualKeyCode::Back {
                    if self.hangul_ime.handle_backspace(self.gui_state.chat_input_mut()) {
                        let query = self.gui_state.chat_input().to_string();
                        self.react(vec![EngineMessage::RequestTacticSuggestions(query)], ctx)?;
                        return GameResult::Ok(()); 
                    } else {
                        self.gui_state.chat_input_mut().pop();
                        let query = self.gui_state.chat_input().to_string();
                        self.react(vec![EngineMessage::RequestTacticSuggestions(query)], ctx)?;
                        return GameResult::Ok(());
                    }
                }

                // 스페이스바 처리
                if keycode == ggez::winit::event::VirtualKeyCode::Space {
                    self.hangul_ime.clear();
                    self.gui_state.chat_input_mut().push(' ');
                    let query = self.gui_state.chat_input().to_string();
                    self.react(vec![EngineMessage::RequestTacticSuggestions(query)], ctx)?;
                    return GameResult::Ok(());
                }
                
                // 엔터키 처리 (LLM에 명령어 전송)
                if keycode == ggez::winit::event::VirtualKeyCode::Return || keycode == ggez::winit::event::VirtualKeyCode::NumpadEnter {
                    self.hangul_ime.clear();
                    let cmd = self.gui_state.chat_input().to_string();
                    if !cmd.trim().is_empty() {
                        let messages = vec![EngineMessage::SendChatCommand(cmd)];
                        self.react(messages, ctx)?;
                    }
                    return GameResult::Ok(());
                }

                // 한국어 모드일 때 글자 조합 처리
                if self.hangul_ime.is_korean_mode {
                    if self.hangul_ime.process_key(keycode, is_shift, self.gui_state.chat_input_mut()) {
                        let query = self.gui_state.chat_input().to_string();
                        self.react(vec![EngineMessage::RequestTacticSuggestions(query)], ctx)?;
                        return GameResult::Ok(()); 
                    }
                }
            }
        }

        // [입력 모드 Lock] 입력 모드일 때는 포커스를 잃었더라도 키보드 이벤트(WASD 등)가 게임으로 넘어가지 않도록 강제 차단합니다.
        if self.gui_state.display_chat_gui() || self.egui_backend.inner.ctx().wants_keyboard_input() {
            return GameResult::Ok(());
        }

        let messages = self.collect_key_pressed(ctx, input);
        self.react(messages, ctx)?;
        GameResult::Ok(())
    }

    fn key_up_event(&mut self, ctx: &mut Context, input: KeyInput) -> Result<(), GameError> {
        // [수정] Tab 키 입력은 채팅창 열기/닫기 토글용이므로 최우선적으로 가로채어 처리합니다.
        if input.keycode == Some(ggez::winit::event::VirtualKeyCode::Tab) {
            self.react(vec![EngineMessage::GuiState(GuiStateMessage::ToggleChatGui)], ctx)?;
            return GameResult::Ok(());
        }

        // [입력 모드 Lock] 채팅창이 떠있을 때는 모든 단축키(명령 취소, 저장 등) 해제를 방지하기 위해 차단합니다.
        if self.gui_state.display_chat_gui() || self.egui_backend.inner.ctx().wants_keyboard_input() {
            return GameResult::Ok(());
        }

        let messages = self.collect_key_released(ctx, input);
        self.react(messages, ctx)?;
        GameResult::Ok(())
    }

    // [추가] 키보드 텍스트 입력(타이핑) 이벤트를 Egui로 전달합니다. 
    // 이 파이프라인이 있어야 Egui 텍스트 인풋 창에 글자가 정상적으로 타이핑됩니다.
    fn text_input_event(&mut self, ctx: &mut Context, character: char) -> Result<(), GameError> {
        // [수정] Egui를 버리고 ggez로 직접 그리기로 했으므로, 영문 모드일 때는 여기서 직접 버퍼에 글자를 넣습니다.
        if self.gui_state.display_chat_gui() {
            if self.hangul_ime.is_korean_mode {
                // 한글 모드일 때는 영문 알파벳(a-z, A-Z)의 침범만 차단하고, 숫자와 기호는 통과시킵니다.
                if character.is_ascii_alphabetic() {
                    return GameResult::Ok(()); 
                }
            }
            
            // 백스페이스나 엔터 등 제어 문자가 아닌 정상 알파벳(숫자, 기호 포함)만 버퍼에 추가
            if !character.is_control() {
                self.gui_state.chat_input_mut().push(character);
                let query = self.gui_state.chat_input().to_string();
                self.react(vec![EngineMessage::RequestTacticSuggestions(query)], ctx)?;
            }
            return GameResult::Ok(());
        }

        self.egui_backend.text_input_event(character);
        GameResult::Ok(())
    }

    fn quit_event(&mut self, _ctx: &mut Context) -> Result<bool, ggez::GameError> {
        self.stop_required.store(true, Ordering::Relaxed);
        Ok(false)
    }
}
