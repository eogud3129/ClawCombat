use ggez::{
    graphics::{Canvas, DrawParam},
    Context, GameResult,
};
use glam::Vec2;

use super::{message::{EngineMessage, GuiStateMessage}, Engine};

pub const EGUI_SCALE: f32 = 1.5;

impl Engine {
    pub fn draw_egui(&mut self, _ctx: &mut Context, canvas: &mut Canvas) {
        canvas.draw(
            &self.egui_backend,
            DrawParam::default().dest(Vec2::new(0., 0.)),
        );
    }

    pub fn update_chat_gui(&mut self, ctx: &mut Context) -> GameResult<()> {
        // 채팅창이 닫혀 있으면 템플릿 UI도 렌더링하지 않습니다.
        if !self.gui_state.display_chat_gui() {
            return Ok(());
        }

        let drawable_size = ctx.gfx.drawable_size();
        self.egui_backend.set_scale_factor(EGUI_SCALE, drawable_size);
        let egui_ctx = self.egui_backend.inner.ctx();
        
        // 컴파일 에러 해결을 위해 definitions() 미지원 API를 메모리 스토리지 상태 검사로 안전하게 우회
        let needs_font_init = egui_ctx.memory(|mem| {
            let is_init = mem.data.get_temp::<bool>(ggegui::egui::Id::new("korean_font_loaded")).unwrap_or(false);
            !is_init
        });

        if needs_font_init {
            let mut fonts = ggegui::egui::FontDefinitions::default();
            fonts.font_data.insert(
                "korean_font".to_owned(),
                ggegui::egui::FontData::from_static(include_bytes!("../../../resources/fonts/GowunBatang-Regular.ttf")),
            );
            fonts.families.get_mut(&ggegui::egui::FontFamily::Proportional)
                .unwrap()
                .insert(0, "korean_font".to_owned());
            fonts.families.get_mut(&ggegui::egui::FontFamily::Monospace)
                .unwrap()
                .push("korean_font".to_owned());
            egui_ctx.set_fonts(fonts);

            egui_ctx.memory_mut(|mem| {
                mem.data.insert_temp(ggegui::egui::Id::new("korean_font_loaded"), true);
            });
        }

        let mut messages = vec![];

        ggegui::egui::Window::new("전술 템플릿 제어")
            .collapsible(false)
            .resizable(false)
            // 배율 오류가 가변적인 window_size 하드코딩 pos2 연산을 전면 폐기합니다.
            // 캔버스 이탈을 원천 방지하기 위해 egui 규격 앵커 매커니즘을 사용해 안전 자리에 안착시킵니다.
            .anchor(ggegui::egui::Align2::CENTER_BOTTOM, ggegui::egui::vec2(0.0, -220.0))
            .show(&egui_ctx, |ui| {
                ui.label("현재 상황 및 상대방에 대응할 전술 템플릿을 선택하세요.");
                ui.separator();

                let chat_text = self.gui_state.chat_input().trim();

                // 입력값이 존재할 경우, 코사인 유사도 기반 자동완성 리스트 표출
                if !chat_text.is_empty() {
                    let results = &self.gui_state.tactic_suggestions;
                    if !results.is_empty() {
                        ui.label("💡 추천 전술 (자동완성):");
                        for (id, name, score) in results {
                            let btn_text = format!("{} (유사도: {:.2})", name, score);
                            // 버튼 클릭 시 해당 작전을 즉시 실행 (전술 확정)
                            if ui.button(btn_text).clicked() {
                                messages.push(EngineMessage::SendChatCommand(id.clone()));
                                messages.push(EngineMessage::GuiState(GuiStateMessage::SelectTemplate(None)));
                                messages.push(EngineMessage::GuiState(GuiStateMessage::ToggleChatGui));
                            }
                        }
                    } else {
                        ui.label("검색된 추천 전술이 없습니다.");
                    }
                } else {
                    // 입력값이 없을 경우 기존처럼 고정 프리셋 노출
                    ui.horizontal(|ui| {
                        if ui.button("즉각 대응 사격").clicked() {
                            messages.push(EngineMessage::GuiState(GuiStateMessage::SelectTemplate(Some("suppress_fire".to_string()))));
                        }
                        if ui.button("은밀 우회 기동").clicked() {
                            messages.push(EngineMessage::GuiState(GuiStateMessage::SelectTemplate(Some("sneak_flank".to_string()))));
                        }
                        if ui.button("취소").clicked() {
                            messages.push(EngineMessage::GuiState(GuiStateMessage::SelectTemplate(None)));
                        }
                    });

                    // 특정 템플릿을 수동으로 클릭(선택)했을 때만 확정 메뉴를 펼쳐줍니다.
                    if let Some(selected) = self.gui_state.selected_template_to_confirm.clone() {
                        ui.separator();
                        ui.label(format!("선택된 템플릿: [{}]", selected));
                        ui.label("적의 전술을 무력화(Counter)할 수 있는지 확인 후 확정하십시오.");
                        
                        if ui.button("✔ 전술 확정 및 실행").clicked() {
                            // 확정 시 엔진에 넘기고 상태를 초기화합니다.
                            messages.push(EngineMessage::SendChatCommand(selected.clone()));
                            messages.push(EngineMessage::GuiState(GuiStateMessage::SelectTemplate(None)));
                            messages.push(EngineMessage::GuiState(GuiStateMessage::ToggleChatGui));
                        }
                    }
                }
            });

        if !messages.is_empty() {
            self.react(messages, ctx)?;
        }

        Ok(())
    }

    pub fn update_task_gui(&mut self, ctx: &mut Context) -> GameResult<()> {
        // [자동 해제 로직] 이동 도착 완료 또는 공격 상태 종료 시 Task 목록에서 자동 해제합니다.
        let mut active_tasks = vec![];
        for task in &self.gui_state.chat_tasks {
            let mut is_active = false;
            for sq_id in &task.2 {
                if let Some(squad) = self.battle_state.squads().get(sq_id) {
                    let leader_idx = squad.leader();
                    if leader_idx.0 < self.battle_state.soldiers().len() {
                        let leader = self.battle_state.soldier(leader_idx);
                        // 이동 중이거나 공격(제압) 중이면 작전이 진행 중인 것으로 간주
                        if matches!(leader.order(), battle_core::order::Order::MoveTo(_, _) | battle_core::order::Order::MoveFastTo(_, _) | battle_core::order::Order::SneakTo(_, _) | battle_core::order::Order::EngageSquad(_) | battle_core::order::Order::SuppressFire(_)) {
                            is_active = true;
                            break;
                        }
                    }
                }
            }
            if is_active {
                active_tasks.push(task.clone());
            }
        }
        self.gui_state.chat_tasks = active_tasks;

        if self.gui_state.chat_tasks.is_empty() {
            return Ok(());
        }

        let drawable_size = ctx.gfx.drawable_size();
        self.egui_backend.set_scale_factor(EGUI_SCALE, drawable_size);
        let egui_ctx = self.egui_backend.inner.ctx();
        
        let needs_font_init = egui_ctx.memory(|mem| {
            let is_init = mem.data.get_temp::<bool>(ggegui::egui::Id::new("korean_font_loaded")).unwrap_or(false);
            !is_init
        });

        if needs_font_init {
            let mut fonts = ggegui::egui::FontDefinitions::default();
            fonts.font_data.insert(
                "korean_font".to_owned(),
                ggegui::egui::FontData::from_static(include_bytes!("../../../resources/fonts/GowunBatang-Regular.ttf")),
            );
            fonts.families.get_mut(&ggegui::egui::FontFamily::Proportional)
                .unwrap()
                .insert(0, "korean_font".to_owned());
            fonts.families.get_mut(&ggegui::egui::FontFamily::Monospace)
                .unwrap()
                .push("korean_font".to_owned());
            egui_ctx.set_fonts(fonts);

            egui_ctx.memory_mut(|mem| {
                mem.data.insert_temp(ggegui::egui::Id::new("korean_font_loaded"), true);
            });
        }

        let mut messages = vec![];

        ggegui::egui::Window::new("전술 명령 대기열 (Task List)")
            .collapsible(false)
            .resizable(false)
            .anchor(ggegui::egui::Align2::LEFT_TOP, ggegui::egui::vec2(10.0, 50.0))
            .show(&egui_ctx, |ui| {
                let mut to_remove = None;
                for (id, cmd, squads) in &self.gui_state.chat_tasks {
                    ui.horizontal(|ui| {
                        ui.label(format!("명령: {}", cmd));
                        if ui.button("❌ 취소").clicked() {
                            to_remove = Some((*id, squads.clone()));
                        }
                    });
                }
                
                if let Some((remove_id, target_squads)) = to_remove {
                    messages.push(EngineMessage::GuiState(GuiStateMessage::RemoveChatTask(remove_id)));
                    // 취소 시 해당 분대를 Idle 로 전환
                    for sq_id in target_squads {
                        let leader_idx = self.battle_state.squad(sq_id).leader();
                        messages.push(EngineMessage::BattleState(
                            battle_core::state::battle::message::BattleStateMessage::Soldier(
                                leader_idx,
                                battle_core::state::battle::message::SoldierMessage::SetOrder(battle_core::order::Order::Idle)
                            )
                        ));
                    }
                }
            });

        if !messages.is_empty() {
            self.react(messages, ctx)?;
        }

        Ok(())
    }
}