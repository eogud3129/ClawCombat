use ggez::{
    graphics::{Canvas, Color, DrawMode, DrawParam, Mesh, MeshBuilder, Text, TextFragment},
    Context, GameResult,
};

use battle_core::{
    behavior::Behavior,
    game::{
        explosive::ExplosiveType,
        squad::{squad_positions, Formation},
        weapon::{Shot, Weapon},
        Side,
    },
    physics::event::{bullet::BulletFire, explosion::Explosion},
    state::battle::message::BattleStateMessage,
    types::WorldPoint,
    utils::DebugPoint,
};

use crate::{
    debug::DebugPhysics,
    utils::{BLUE, DARK_MAGENTA, GREEN, MAGENTA, RED, YELLOW},
};

use super::{message::EngineMessage, Engine};
pub mod gui;

impl Engine {
    pub fn generate_debug_mouse_meshes(&self, mesh_builder: &mut MeshBuilder) -> GameResult {
        // Draw circle where left click down
        if let Some(point) = self.gui_state.left_click_down_window_point() {
            mesh_builder.circle(DrawMode::fill(), point.to_vec2(), 2.0, 2.0, YELLOW)?;
        }

        // Draw circle at cursor position
        mesh_builder.circle(
            DrawMode::fill(),
            self.gui_state.current_cursor_window_point().to_vec2(),
            2.0,
            2.0,
            BLUE,
        )?;

        Ok(())
    }

    pub fn generate_move_paths_meshes(&self, mesh_builder: &mut MeshBuilder) -> GameResult {
        for squad_composition in self.battle_state.squads().values() {
            let squad_leader = self.battle_state.soldier(squad_composition.leader());
            if let Some(world_paths) = match squad_leader.behavior() {
                Behavior::MoveTo(world_paths)
                | Behavior::MoveFastTo(world_paths)
                | Behavior::SneakTo(world_paths)
                | Behavior::DriveTo(world_paths) => Some(world_paths),

                _ => None,
            } {
                for world_path in &world_paths.paths {
                    let last_point = self.gui_state.window_point_from_world_point(
                        world_path.last_point().expect("Must contains point"),
                    );
                    mesh_builder.circle(
                        DrawMode::Fill(Default::default()),
                        last_point.to_vec2(),
                        5.0,
                        1.0,
                        YELLOW,
                    )?;

                    for point in &world_path.points {
                        mesh_builder.circle(
                            DrawMode::Fill(Default::default()),
                            self.gui_state
                                .window_point_from_world_point(*point)
                                .to_vec2(),
                            2.0,
                            1.0,
                            BLUE,
                        )?;
                    }
                }
            }
        }

        Ok(())
    }

    pub fn generate_formation_positions_meshes(
        &mut self,
        mesh_builder: &mut MeshBuilder,
    ) -> GameResult {
        // Display selected squad formation positions
        for squad_id in &self.gui_state.selected_squads().1 {
            let squad = self.battle_state.squad(*squad_id);
            
            // [버그 수정: 단일 생존자 잔류 시 중대원 전체 1줄 나열 정렬 데드락 방지]
            // 분대에 생존한 대원이 1명 이하이거나 리더 홀로 남았을 경우 진형 벡터 공식의 가중치가 파괴되어 중대 전체가 일렬로 서는 버그가 발생합니다.
            // 인원이 혼자 남았을 때는 대형 포메이션 재정렬 연산을 생략하고 독립 개별 포지션을 유지하도록 우회 보호합니다.
            let alive_members_count = squad.members().iter().filter(|m| m.0 < self.battle_state.soldiers().len() && self.battle_state.soldier(**m).alive()).count();
            if alive_members_count <= 1 {
                continue;
            }

            let leader = self.battle_state.soldier(squad.leader());
            for (_, point) in squad_positions(squad, Formation::Line, leader, None) {
                let window_point = self.gui_state.window_point_from_world_point(point);
                mesh_builder.circle(DrawMode::fill(), window_point.to_vec2(), 2.0, 2.0, YELLOW)?;
            }
        }

        Ok(())
    }

    pub fn generate_debug_point_meshes(&mut self, mesh_builder: &mut MeshBuilder) -> GameResult {
        let mut debug_points_left = vec![];
        while let Some(debug_point) = self.gui_state.debug_points_mut().pop() {
            if debug_point.frame_i >= self.gui_state.frame_i() {
                let window_point = self
                    .gui_state
                    .window_point_from_world_point(debug_point.point);
                mesh_builder.circle(
                    DrawMode::fill(),
                    window_point.to_vec2(),
                    2.0,
                    2.0,
                    debug_point.color.into(),
                )?;
                debug_points_left.push(debug_point);
            }
        }
        self.gui_state.set_debug_points(debug_points_left);

        Ok(())
    }

    /// Draw circle on each soldier position
    pub fn generate_scene_item_circles_meshes(
        &mut self,
        mesh_builder: &mut MeshBuilder,
    ) -> GameResult {
        for soldier in self.battle_state.soldiers() {
            let color = if soldier.side() == self.gui_state.side() {
                GREEN
            } else {
                RED
            };

            let point = self
                .gui_state
                .window_point_from_world_point(soldier.world_point());
            mesh_builder.circle(DrawMode::fill(), point.to_vec2(), 2.0, 2.0, color)?;
        }

        Ok(())
    }

    /// Draw selection areas
    pub fn generate_areas_meshes(&mut self, mesh_builder: &mut MeshBuilder) -> GameResult {
        let cursor_world_point = self.gui_state.current_cursor_world_point();
        let cursor_window_point = self.gui_state.current_cursor_window_point();

        // Draw soldiers selection areas
        for soldier in self.battle_state.soldiers() {
            let rect = self
                .gui_state
                .window_rect_from_world_rect(self.graphics.soldier_selection_rect(soldier));
            mesh_builder.rectangle(DrawMode::stroke(1.0), rect, MAGENTA)?;
        }

        // Draw vehicle physics areas
        for vehicle in self.battle_state.vehicles() {
            let shape = self
                .gui_state
                .window_shape_from_world_shape(&vehicle.chassis_shape());

            mesh_builder.line(&shape.draw_points(), 1.0, MAGENTA)?;
        }

        // Draw selection area on cursor hover scene items
        for soldier in self.soldiers_at_point(cursor_world_point, Some(self.gui_state.side())) {
            let rect = self
                .gui_state
                .window_rect_from_world_rect(self.graphics.soldier_selection_rect(soldier));
            mesh_builder.rectangle(DrawMode::stroke(1.0), rect, DARK_MAGENTA)?;
        }

        // Draw selection area on all order markers
        for (order, order_marker, _, world_point, _) in self.battle_state.order_markers(&Side::All)
        {
            let shape =
                self.gui_state
                    .window_shape_from_world_shape(&self.order_marker_selection_shape(
                        &order,
                        &order_marker,
                        &world_point,
                    ));
            let color = if shape.contains(cursor_window_point) {
                DARK_MAGENTA
            } else {
                MAGENTA
            };
            mesh_builder.line(&shape.draw_points(), 1.0, color)?;
        }

        Ok(())
    }

    ///
    pub fn generate_visibilities_meshes(&mut self, mesh_builder: &mut MeshBuilder) -> GameResult {
        for squad_uuid in &self.gui_state.selected_squads().1 {
            let squad_composition = self.battle_state.squad(*squad_uuid);
            for soldier_index in squad_composition.members() {
                let from_soldier = self.battle_state.soldier(*soldier_index);
                let to_soldiers = self
                    .battle_state
                    .soldiers()
                    .iter()
                    .filter(|s| s.side() != from_soldier.side());
                for to_soldier in to_soldiers {
                    if let Some(visibility) = self
                        .battle_state
                        .visibilities()
                        .get(&(from_soldier.uuid(), to_soldier.uuid()))
                    {
                        let start_world_point = from_soldier.world_point();
                        let mut previous_point = self
                            .gui_state
                            .window_point_from_world_point(start_world_point);
                        let mut previous_opacity: f32 = 0.0;

                        for (segment_world_point, segment_new_opacity) in
                            visibility.opacity_segments.iter().skip(1)
                        {
                            let segment_point = self
                                .gui_state
                                .window_point_from_world_point(*segment_world_point);
                            let mut color_canal_value = 1.0 - previous_opacity;
                            if color_canal_value < 0.0 {
                                color_canal_value = 0.0;
                            }
                            mesh_builder.line(
                                &[previous_point.to_vec2(), segment_point.to_vec2()],
                                1.0,
                                Color {
                                    r: color_canal_value,
                                    g: color_canal_value,
                                    b: color_canal_value,
                                    a: 1.0,
                                },
                            )?;

                            previous_point = segment_point;
                            previous_opacity = *segment_new_opacity;
                        }
                    }
                }
            }
        }

        Ok(())
    }
    pub fn generate_targets_meshes(&mut self, mesh_builder: &mut MeshBuilder) -> GameResult {
        for squad_uuid in &self.gui_state.selected_squads().1 {
            let squad_composition = self.battle_state.squad(*squad_uuid);
            for soldier_index in squad_composition.members() {
                let soldier = self.battle_state.soldier(*soldier_index);
                if let Some(target_soldier) = soldier.target() {
                    let target_soldier = self.battle_state.soldier(*target_soldier);
                    let from_point = self
                        .gui_state
                        .window_point_from_world_point(soldier.world_point());
                    let to_point = self
                        .gui_state
                        .window_point_from_world_point(target_soldier.world_point());
                    mesh_builder.line(&[from_point.to_vec2(), to_point.to_vec2()], 1.0, RED)?;
                }
            }
        }

        Ok(())
    }

    pub fn generate_debug_physics(&self, from: WorldPoint, to: WorldPoint) -> Vec<EngineMessage> {
        let mut messages = vec![];

        match self.gui_state.debug_physics() {
            DebugPhysics::None => {}
            DebugPhysics::MosinNagantM1924GunFire => {
                let weapon = Weapon::MosinNagantM1924(true, None);
                messages.extend(
                    [vec![EngineMessage::BattleState(
                        BattleStateMessage::PushBulletFire(BulletFire::new(
                            0,
                            from,
                            to,
                            None,
                            weapon.ammunition(),
                            Some(weapon.gun_fire_sound_type()),
                            Shot::x1,
                        )),
                    )]]
                    .concat(),
                );
            }
            DebugPhysics::BrandtMle2731Shelling => {
                messages.push(EngineMessage::BattleState(
                    BattleStateMessage::PushExplosion(Explosion::new(
                        from,
                        ExplosiveType::FA19241927,
                    )),
                ));
            }
        };

        messages
    }

    pub fn generate_physics_areas_meshes(&self, mesh_builder: &mut MeshBuilder) -> GameResult {
        if let Some(explosive) = &self.gui_state.debug_physics().explosive() {
            let explosion = Explosion::new(
                self.gui_state.current_cursor_world_point(),
                explosive.clone(),
            );
            self.generate_explosive_areas_meshes(mesh_builder, &explosion)?;
        };

        for explosion in self.battle_state.explosions() {
            self.generate_explosive_areas_meshes(mesh_builder, explosion)?;
        }

        Ok(())
    }

    pub fn generate_explosive_areas_meshes(
        &self,
        mesh_builder: &mut MeshBuilder,
        explosion: &Explosion,
    ) -> GameResult {
        if let (
            Some(direct_death_rayons),
            Some(regressive_death_rayon),
            Some(regressive_injured_rayon),
        ) = (
            self.server_config
                .explosive_direct_death_rayon
                .get(explosion.type_()),
            self.server_config
                .explosive_regressive_death_rayon
                .get(explosion.type_()),
            self.server_config
                .explosive_regressive_injured_rayon
                .get(explosion.type_()),
        ) {
            let point = self
                .gui_state
                .window_point_from_world_point(*explosion.point());
            let direct_death_radius = self.gui_state.distance_pixels(direct_death_rayons);
            mesh_builder.circle(
                DrawMode::stroke(1.0),
                point.to_vec2(),
                direct_death_radius * self.gui_state.zoom.factor(),
                1.0,
                RED,
            )?;

            let regressive_death_radius = self.gui_state.distance_pixels(regressive_death_rayon);
            let part = regressive_death_radius / 10.;
            for i in 1..=10 {
                let radius_ = part * i as f32;
                if radius_ > direct_death_radius {
                    mesh_builder.circle(
                        DrawMode::stroke(1.0),
                        point.to_vec2(),
                        radius_ * self.gui_state.zoom.factor(),
                        1.0,
                        Color {
                            r: 1.0,
                            g: 0.1,
                            b: 0.0,
                            a: 1.0 - (i as f32 / 15.),
                        },
                    )?;
                }
            }

            let regressive_injured_radius =
                self.gui_state.distance_pixels(regressive_injured_rayon);
            let part = regressive_injured_radius / 10.;
            for i in 1..=10 {
                let radius_ = part * i as f32;
                if radius_ > direct_death_radius {
                    mesh_builder.circle(
                        DrawMode::stroke(1.0),
                        point.to_vec2(),
                        radius_ * self.gui_state.zoom.factor(),
                        1.0,
                        Color {
                            r: 1.0,
                            g: 1.0,
                            b: 0.0,
                            a: 1.0 - (i as f32 / 10.),
                        },
                    )?;
                }
            }
        }

        Ok(())
    }

    pub fn inspect_for_bullet_fire_into_debug_points(&mut self, message: &BattleStateMessage) {
        let frame_i = self.gui_state.frame_i();
        match message {
            BattleStateMessage::PushBulletFire(bullet_fire) => {
                self.gui_state.debug_points_mut().push(DebugPoint {
                    frame_i: frame_i + 30,
                    point: bullet_fire.point().clone(),
                    color: RED.into(),
                })
            }
            _ => {}
        }
    }

    pub fn draw_debug_grid(
        &self,
        ctx: &mut Context,
        canvas: &mut Canvas,
        draw_param: DrawParam,
    ) -> GameResult {
        if !self.gui_state.debug_grid {
            return Ok(());
        }

        let map = self.battle_state.map();
        let grid_size = 30; // 3배 확장: 타일 30개 단위를 1개의 전술 구역으로 지정
        let cell_width = map.tile_width() as f32 * grid_size as f32;
        let cell_height = map.tile_height() as f32 * grid_size as f32;
        
        // 깃발(Flag) 위치에 그리드 박스가 딱 맞물리도록 오프셋(Offset) 동적 계산
        let mut offset_x = 0.0;
        let mut offset_y = 0.0;
        if let Some(first_flag) = map.flags().first() {
            let flag_center = first_flag.position();
            // 깃발 중심이 박스의 정중앙에 오도록 그리드 시작점을 시프트
            offset_x = flag_center.x % cell_width - (cell_width / 2.0);
            offset_y = flag_center.y % cell_height - (cell_height / 2.0);
        }

        let mut mesh_builder = MeshBuilder::new();
        let grid_color = Color::new(1.0, 1.0, 1.0, 0.7); // 투명도 0.7로 진하게 상향

        // 맵 전역을 커버하도록 넉넉하게 반복문 범위 설정
        for x in -5..100 {
            let px = offset_x + (x as f32 * cell_width);
            if px >= 0.0 && px <= map.visual_width() as f32 {
                mesh_builder.line(
                    &[WorldPoint::new(px, 0.0).to_vec2(), WorldPoint::new(px, map.visual_height() as f32).to_vec2()],
                    2.0,
                    grid_color,
                )?;
            }
        }

        for y in -5..100 {
            let py = offset_y + (y as f32 * cell_height);
            if py >= 0.0 && py <= map.visual_height() as f32 {
                mesh_builder.line(
                    &[WorldPoint::new(0.0, py).to_vec2(), WorldPoint::new(map.visual_width() as f32, py).to_vec2()],
                    2.0,
                    grid_color,
                )?;
            }
        }

        // 섹터 선택 색상(투명 레이어) 렌더링
        let chars: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
        let chat_input = self.gui_state.chat_input();
        let move_sectors: Vec<&str> = chat_input.split_whitespace().filter(|w| w.starts_with('&')).map(|w| w.trim_start_matches('&')).collect();
        let attack_sectors: Vec<&str> = chat_input.split_whitespace().filter(|w| w.starts_with('#')).map(|w| w.trim_start_matches('#')).collect();

        for y in -5..100 {
            for x in -5..100 {
                let px = offset_x + (x as f32 * cell_width);
                let py = offset_y + (y as f32 * cell_height);
                
                if px < 0.0 || py < 0.0 || px >= map.visual_width() as f32 || py >= map.visual_height() as f32 {
                    continue;
                }

                let letter = chars.get((y + 5) as usize % chars.len()).unwrap_or(&'?');
                let number = (x + 6) as usize;
                let text_str = format!("{}{}", letter, number);

                if move_sectors.contains(&text_str.as_str()) {
                    mesh_builder.rectangle(
                        ggez::graphics::DrawMode::fill(),
                        ggez::graphics::Rect::new(px, py, cell_width, cell_height),
                        Color::new(1.0, 1.0, 1.0, 0.3),
                    )?;
                }
                
                if attack_sectors.contains(&text_str.as_str()) {
                    mesh_builder.rectangle(
                        ggez::graphics::DrawMode::fill(),
                        ggez::graphics::Rect::new(px, py, cell_width, cell_height),
                        Color::new(1.0, 0.0, 0.0, 0.3),
                    )?;
                }
            }
        }

        let mesh = Mesh::from_data(ctx, mesh_builder.build());
        canvas.draw(&mesh, draw_param);

        // A1, B2, H5 등 문자 조합 네이밍 렌더링
        for y in -5..100 {
            for x in -5..100 {
                let px = offset_x + (x as f32 * cell_width);
                let py = offset_y + (y as f32 * cell_height);
                
                if px < 0.0 || py < 0.0 || px >= map.visual_width() as f32 || py >= map.visual_height() as f32 {
                    continue;
                }

                let letter = chars.get((y + 5) as usize % chars.len()).unwrap_or(&'?');
                let number = (x + 6) as usize;
                let text_str = format!("{}{}", letter, number);
                
                let text_px = px + 15.0;
                let text_py = py + 15.0;

                // 스크롤 및 줌 상태를 반영하여 정밀하게 좌표 계산
                let dest_x = text_px * self.gui_state.zoom.factor() + self.gui_state.display_scene_offset.x;
                let dest_y = text_py * self.gui_state.zoom.factor() + self.gui_state.display_scene_offset.y;

                let mut text = Text::new(TextFragment::new(text_str).color(Color::new(1.0, 1.0, 1.0, 0.7)));
                text.set_scale(32.0 * self.gui_state.zoom.factor());
                canvas.draw(&text, DrawParam::default().dest(glam::Vec2::new(dest_x, dest_y)));
            }
        }

        Ok(())
    }
}
