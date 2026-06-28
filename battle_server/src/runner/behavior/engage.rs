use battle_core::{
    entity::soldier::Soldier,
    game::cover::CoverFinder,
    order::Order,
    physics::path::{find_path, Direction, PathMode},
    types::{SoldierIndex, SquadUuid, WorldPath, WorldPaths},
};

use crate::runner::{fight::choose::ChooseMethod, Runner};

impl Runner {
    pub fn propagate_engage_soldier(
        &self,
        squad_uuid: &SquadUuid,
        engaged_soldier_index: &SoldierIndex,
    ) -> Vec<(&Soldier, Order)> {
        let mut orders = vec![];
        
        // [Operation Ghost - Part 2] 교전 이익 판별 및 맹목 제압 사격 (Advantage Check) 현실화
        let map = self.battle_state.map();
        let leader = self.battle_state.soldier(self.battle_state.squad(*squad_uuid).leader());
        
        let my_grid = map.grid_point_from_world_point(&leader.world_point());
        let my_tile = map.terrain_tiles().get((my_grid.y * map.width() as i32 + my_grid.x) as usize);
        let my_cover = my_tile.and_then(|t| t.type_().coverage(&battle_core::game::posture::Posture::Flat)).map(|c| c.0).unwrap_or(0.0);
        let my_firepower_raw = self.battle_state.squad(*squad_uuid).members().iter().filter(|&&m| self.battle_state.soldier(m).alive()).count() as f32;
        // [Part 1: 메가 분대 스케일링 상한선 적용] 분대 병력 수가 기형적으로 많아져도 위협 점수가 무한 폭증하지 않도록 최대 8명으로 캡(Cap)을 씌웁니다.
        let my_firepower = my_firepower_raw.min(8.0);
        
        // [Part 2: 다차원 전술 유불리 평가]
        // 1. 스트레스 페널티 반영 (Danger=150, Max=200 기준)
        let my_stress = *leader.under_fire().value() as f32;
        let my_stress_penalty = my_stress / 100.0; // 0.0 ~ 2.0 감점

        // 2. 행동(Posture/Order) 보너스 반영 (조준을 마치고 방어 중이면 유리)
        let my_posture_bonus = if matches!(leader.order(), Order::Defend(_) | Order::Hide(_)) { 1.5 } else { 0.0 };

        // 3. 아군 밀집도(용기) 보너스 반영 (반경 30m 이내 아군)
        let mut my_allies_nearby: f32 = 0.0;
        for ally in self.battle_state.soldiers() {
            if ally.side() == leader.side() && ally.alive() && ally.uuid() != leader.uuid() {
                let dist = battle_core::physics::utils::distance_between_points(&leader.world_point(), &ally.world_point()).meters() as f32;
                if dist <= 30.0 {
                    my_allies_nearby += 0.5; // 아군 1명당 0.5점 추가
                }
            }
        }
        // [Part 1: 주변 아군 용기 보너스 상한선 적용] 최대 5.0점으로 제한하여 맵 전체가 모였을 때 지나치게 용감해지는 현상 방지
        my_allies_nearby = my_allies_nearby.min(5.0);

        // 최종 아군 교전 점수 산출
        let my_score = (my_firepower * (1.0 + my_cover + my_posture_bonus)) - my_stress_penalty + my_allies_nearby;


        let target_soldier = self.battle_state.soldier(*engaged_soldier_index);
        let target_grid = map.grid_point_from_world_point(&target_soldier.world_point());
        let target_tile = map.terrain_tiles().get((target_grid.y * map.width() as i32 + target_grid.x) as usize);
        let target_cover = target_tile.and_then(|t| t.type_().coverage(&battle_core::game::posture::Posture::Flat)).map(|c| c.0).unwrap_or(0.0);
        let target_firepower_raw = self.battle_state.squad(target_soldier.squad_uuid()).members().iter().filter(|&&m| self.battle_state.soldier(m).alive()).count() as f32;
        let target_firepower = target_firepower_raw.min(8.0);
        
        let target_stress = *target_soldier.under_fire().value() as f32;
        let target_stress_penalty = target_stress / 100.0;

        let target_posture_bonus = if matches!(target_soldier.order(), Order::Defend(_) | Order::Hide(_)) { 1.5 } else { 0.0 };

        let mut target_allies_nearby: f32 = 0.0;
        for enemy_ally in self.battle_state.soldiers() {
            if enemy_ally.side() == target_soldier.side() && enemy_ally.alive() && enemy_ally.uuid() != target_soldier.uuid() {
                let dist = battle_core::physics::utils::distance_between_points(&target_soldier.world_point(), &enemy_ally.world_point()).meters() as f32;
                if dist <= 30.0 {
                    target_allies_nearby += 0.5;
                }
            }
        }
        target_allies_nearby = target_allies_nearby.min(5.0);

        // 최종 적군 교전 점수 산출
        let target_score = (target_firepower * (1.0 + target_cover + target_posture_bonus)) - target_stress_penalty + target_allies_nearby;

        // 상대방과 비교하여 불리한(Disadvantaged) 상황일 경우
        if my_score < target_score {
            let mut my_company = None;
            for comp in self.companies.values() {
                if comp.squads.contains(squad_uuid) {
                    my_company = Some(comp);
                    break;
                }
            }
            
            if let Some(comp) = my_company {
                if comp.scout_squad == Some(*squad_uuid) {
                    // [Part 4: 지원 사격망 연계] 본대 지원 사격이 있는지 스캔합니다.
                    let mut has_support_fire = false;
                    for ally in self.battle_state.soldiers() {
                        if ally.side() == leader.side() && ally.alive() && ally.squad_uuid() != *squad_uuid {
                            let dist_to_ally = battle_core::physics::utils::distance_between_points(&leader.world_point(), &ally.world_point()).meters();
                            if dist_to_ally <= 60 {
                                if comp.scout_squad != Some(ally.squad_uuid()) && matches!(ally.behavior(), battle_core::behavior::Behavior::SuppressFire(_) | battle_core::behavior::Behavior::EngageSoldier(_)) {
                                    has_support_fire = true;
                                    break;
                                }
                            }
                        }
                    }

                    // 본대 지원 사격이 있다면 불리하더라도 후퇴하지 않고 전선을 유지하며 맞서 싸웁니다.
                    if !has_support_fire {
                        println!("[Operation Ghost] 정찰조({:?}) 교전 불리 판정! (아군 점수: {:.1} < 적군 점수: {:.1}) -> 본대 맹목 제압사격(SuppressFire) 호출 및 회피", squad_uuid.0, my_score, target_score);
                        
                        let target_pos = target_soldier.world_point();
                        let retreat_dir = if (leader.world_point().to_vec2() - target_pos.to_vec2()).length() > 0.1 {
                            (leader.world_point().to_vec2() - target_pos.to_vec2()).normalize()
                        } else {
                            glam::Vec2::new(1.0, 0.0)
                        };
                        
                        let map_w = map.visual_width() as f32 - 30.0;
                        let map_h = map.visual_height() as f32 - 30.0;
                        let retreat_vec = leader.world_point().to_vec2() + retreat_dir * 150.0; // 150 픽셀(약 45m)
                        let retreat_vec_clamped = glam::Vec2::new(
                            retreat_vec.x.clamp(30.0, map_w.max(30.0)),
                            retreat_vec.y.clamp(30.0, map_h.max(30.0))
                        );
                        
                        // [기획 반영: 교전 불리 시 체크포인트 최우선 복귀]
                        // 무작정 반대 방향으로 후퇴하는 것이 아니라, 진격 전에 미리 기억해 둔 체크포인트가 존재한다면 
                        // 그곳을 가장 안전한 피신처로 간주하고 1순위로 복귀합니다.
                        let mut retreat_target = if let Some(cp) = self.checkpoints.read().unwrap().get(squad_uuid) {
                            *cp
                        } else {
                            battle_core::types::WorldPoint::from_vec2(retreat_vec_clamped)
                        };

                        // [버그 수정: 정찰조 필드 노출 사망 방지 (건물 엄폐 최우선)]
                        // 무지성으로 후퇴 벡터(필드)로 뛰어들지 않고, 근처에 안전한 건물이 있다면 건물 뒤편 사각지대로 피신 목표점을 수정합니다.
                        let mut min_dist_to_safe_building = std::f32::MAX;
                        for interior in map.interiors() {
                            let center_x = interior.x() + interior.width() / 2.0;
                            let center_y = interior.y() + interior.height() / 2.0;
                            let center_vec = glam::Vec2::new(center_x, center_y);
                            
                            let dist_to_me = (center_vec - leader.world_point().to_vec2()).length();
                            let dist_to_enemy = (center_vec - target_pos.to_vec2()).length();
                            
                            // 적보다 아군에게 더 가깝고, 도달 가능한 거리(150픽셀 이내)에 있는 건물 탐색
                            if dist_to_me < 150.0 && dist_to_enemy > dist_to_me {
                                if dist_to_me < min_dist_to_safe_building {
                                    min_dist_to_safe_building = dist_to_me;
                                    // 건물의 적 반대편(뒷벽)을 피신처로 설정
                                    let safe_radius = interior.width().max(interior.height()) / 2.0 + 15.0;
                                    let safe_vec = center_vec + retreat_dir * safe_radius;
                                    retreat_target = battle_core::types::WorldPoint::from_vec2(glam::Vec2::new(
                                        safe_vec.x.clamp(30.0, map_w.max(30.0)),
                                        safe_vec.y.clamp(30.0, map_h.max(30.0))
                                    ));
                                }
                            }
                        }
                        
                        // [Operation Ghost - Part 3] 지형 인식 스텔스 네비게이션 (Stealth Routing) 적용
                        // 평야를 버리고 덤불/나무를 자석처럼 파고들어 후퇴하는 경로를 생성합니다.
                        let from_grid = map.grid_point_from_world_point(&leader.world_point());
                        let to_grid = map.grid_point_from_world_point(&retreat_target);
                        
                        let escape_route = if let Some(grid_path) = battle_core::physics::path::find_stealth_path(
                            &self.config,
                            map,
                            &from_grid,
                            &to_grid,
                            true,
                            &battle_core::physics::path::PathMode::Walk,
                            &Some(battle_core::physics::path::Direction::from_angle(&leader.get_looking_direction())),
                        ) {
                            let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                            let world_paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(world_path)]);
                            Some((world_paths, Order::Hide(battle_core::types::Angle(0.0))))
                        } else {
                            None
                        };
                        
                        // [기획 반영: 분대 뿔뿔이 흩어짐 방지 (대형 유지 후퇴)]
                        // 1. 개별 병사에게 똑같은 목표점을 찍어주어 척력으로 인해 흩어지는 현상을 막기 위해, 
                        //    오직 지휘관(leader)에게만 후퇴 명령을 하달하고 엔진의 propagate_move를 통해 대형을 유지하며 후퇴시킵니다.
                        if leader.alive() {
                            if let Some((path, then_order)) = &escape_route {
                                orders.push((leader, Order::SneakTo(path.clone(), Some(Box::new(then_order.clone())))));
                            } else {
                                let return_frame = *self.battle_state.frame_i() + 1200; // 60fps * 20s
                                let escape_path = battle_core::types::WorldPaths::new(vec![
                                    battle_core::types::WorldPath::new(vec![
                                        leader.world_point(),
                                        retreat_target
                                    ])
                                ]);
                                orders.push((leader, Order::SneakTo(escape_path, Some(Box::new(Order::OffMapTransit(return_frame))))));
                            }
                        }
                        
                        // 2. 본대(나머지 분대) 제압 사격 명령
                        for squad_id in &comp.squads {
                            if squad_id != squad_uuid {
                                for member_idx in self.battle_state.squad(*squad_id).members() {
                                    let member = self.battle_state.soldier(*member_idx);
                                    // 무한 루프 방지를 위해 현재 Order가 아닐 때만 갱신
                                    if member.alive() && member.order() != &Order::SuppressFire(target_pos) {
                                        orders.push((member, Order::SuppressFire(target_pos)));
                                    }
                                }
                            }
                        }
                        
                        return orders;
                    }
                }
            }
        }

        let engaged_squad_index = self
            .battle_state
            .soldier(*engaged_soldier_index)
            .squad_uuid();
        let engaged_squad = self.battle_state.squad(engaged_squad_index);

        let mut after_grid_positions = vec![];
        'subordinates: for member in self
            .battle_state
            .squad(*squad_uuid)
            .subordinates()
            .iter()
            .map(|i| self.battle_state.soldier(**i))
        {
            let member_grid_point = self
                .battle_state
                .map()
                .grid_point_from_world_point(&member.world_point());
            if self
                .soldier_find_opponent_to_target(
                    member,
                    Some(&engaged_squad_index),
                    &ChooseMethod::RandomFromNearest,
                )
                .is_some()
            {
                log::debug!(
                    "Propagate engage soldier :: Member({}) :: have target",
                    member.uuid()
                );

                after_grid_positions.push(member_grid_point);
                orders.push((member, Order::EngageSquad(engaged_squad_index)));
            } else {
                // Subordinate can't targeted squad member. Try to find another place where he can
                let visible_targeted_squad_opponents: Vec<&Soldier> = engaged_squad
                    .members()
                    .iter()
                    .map(|i| self.battle_state.soldier(*i))
                    .filter(|s| {
                        self.battle_state
                            .soldier_is_visible_by_side(s, member.side())
                    })
                    .collect();

                for visible_opponent in &visible_targeted_squad_opponents {
                    if let Some(new_grid_point) = CoverFinder::new(&self.battle_state, &self.config)
                        .exclude_grid_points(after_grid_positions.clone())
                        .find_better_cover_point_from_point(
                            member,
                            &visible_opponent.world_point(),
                            true,
                        )
                    {
                        if let Some(grid_points_path) = find_path(
                            &self.config,
                            self.battle_state.map(),
                            &member_grid_point,
                            &new_grid_point,
                            true,
                            &PathMode::Walk,
                            &Some(Direction::from_angle(&member.get_looking_direction())),
                        ) {
                            after_grid_positions.push(new_grid_point);

                            let world_point_path = grid_points_path
                                .iter()
                                .map(|p| self.battle_state.map().world_point_from_grid_point(*p))
                                .collect();
                            let world_path = WorldPath::new(world_point_path);

                            log::debug!(
                                "Propagate engage soldier :: Member({}) :: no target :: Opponent({}) :: found new position ({}) :: found grid path ({:?})",
                                member.uuid(),
                                visible_opponent.uuid(),
                                new_grid_point,
                                grid_points_path,
                            );

                            orders.push((
                                member,
                                Order::MoveFastTo(WorldPaths::new(vec![world_path]), None),
                            ));
                            continue 'subordinates;
                        } else {
                            log::debug!(
                                "Propagate engage soldier :: Member({}) :: no target :: Opponent({}) :: found new position ({}) :: do not found grid path",
                                member.uuid(),
                                visible_opponent.uuid(),
                                new_grid_point,
                            );
                        };
                    } else {
                        log::debug!(
                            "Propagate engage soldier :: Member({}) :: no target :: Opponent({}) :: do not found new position",
                            member.uuid(),
                            visible_opponent.uuid(),
                        );
                    }
                }

                if visible_targeted_squad_opponents.is_empty() {
                    log::debug!(
                        "Propagate engage soldier :: Member({}) :: no target :: no visible enemy",
                        member.uuid(),
                    );
                }
            }
        }

        orders
    }
}
