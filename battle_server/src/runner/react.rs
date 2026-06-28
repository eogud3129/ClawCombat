use battle_core::{
    behavior::{Behavior, Body},
    order::Order,
    state::battle::message::SideEffect,
};

use super::{message::RunnerMessage, Runner};

impl Runner {
    pub fn react(&mut self, messages: &Vec<RunnerMessage>) {
        // TODO : Side effects should not exists : All side effects
        // should be computed when original message is produced
        let mut side_effects = vec![];

        let get_sector = |pos: battle_core::types::WorldPoint, map: &battle_core::map::Map| -> String {
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
            let adj_x = pos.x - offset_x;
            let adj_y = pos.y - offset_y;
            let col = (adj_x / cell_width).floor() as i32;
            let row = (adj_y / cell_height).floor() as i32;
            let row_idx = (row + 5).max(0) as usize;
            let col_idx = (col + 6).max(0) as usize;
            let letter = chars.get(row_idx % chars.len()).unwrap_or(&'?');
            format!("{}{}", letter, col_idx)
        };

        for message in messages {
            match message {
                RunnerMessage::BattleState(state_message) => {
                    // [옵시디언 마크다운 로거 중앙 이벤트 가로채기]
                    if self.logger.is_none() {
                        // 로거가 아직 활성화되지 않았다면, Placement에서 Battle 페이즈로 전환되는 시점에 최초 폴더(logs/YYYYMMDD'T'HHMMSS)를 생성합니다.
                        if let battle_core::state::battle::message::BattleStateMessage::SetPhase(battle_core::state::battle::phase::Phase::Battle) = state_message {
                            if self.battle_state.phase() != &battle_core::state::battle::phase::Phase::Battle {
                                self.logger = Some(crate::runner::logger::BattleLogger::new(*self.battle_state.frame_i()));
                            }
                        }
                    }

                    let current_frame = *self.battle_state.frame_i();
                    match state_message {
                        battle_core::state::battle::message::BattleStateMessage::SetFlagsOwnership(ref flags_ownership) => {
                            // 깃발 점령 변경 이벤트 발생 시 페이즈 전환 (새 폴더 및 기존 데이터 Flush)
                            if let Some(logger) = &mut self.logger {
                                logger.flush_phase(current_frame, "Flag Ownership Changed");
                            }

                            // [기획 반영: 전술 청소(Tactical Clean)] 
                            // 새로 소유권이 바뀐 거점의 중심 좌표 기준 50m 이내에 기록되어 있던 가중치 패널티(Cost 2000) 핑 데이터를 맵에서 완전히 증발시킵니다.
                            // 이를 통해 점령지 근처에서 봇들이 네비게이션 경로 탐색에 실패해 단체로 정지하는 락 현상을 완벽히 치유합니다.
                            let map = self.battle_state.map();
                            self.tactical_pings.retain(|ping_grid, _| {
                                let ping_world = map.world_point_from_grid_point(*ping_grid);
                                let mut keep = true;
                                for (flag_name, _) in flags_ownership.ownerships() {
                                    let flag = map.flag(flag_name);
                                    if battle_core::physics::utils::distance_between_points(&flag.position(), &ping_world).meters() <= 50 {
                                        keep = false;
                                        break;
                                    }
                                }
                                keep
                            });
                        }
                        battle_core::state::battle::message::BattleStateMessage::SetPhase(battle_core::state::battle::phase::Phase::End(_, reason)) => {
                            // 전투 종료 시 total.md 최종 저장
                            if let Some(logger) = &mut self.logger {
                                logger.end_game(current_frame, &reason.to_string());
                            }
                        }
                        battle_core::state::battle::message::BattleStateMessage::Soldier(idx, battle_core::state::battle::message::SoldierMessage::SetWorldPosition(new_point)) => {
                            // [버그 수정: 병사 개별 단위 로그 기록]
                            // 분대장(지휘관) 뿐만 아니라 모든 개별 병사의 이동 동선을 로깅하도록 제한을 해제합니다.
                            let old_wp_exact = self.battle_state.soldier(*idx).world_point();
                            let old_grid = self.battle_state.map().grid_point_from_world_point(&old_wp_exact);
                            let new_grid = self.battle_state.map().grid_point_from_world_point(new_point);
                            
                            if old_grid != new_grid {
                                let map = self.battle_state.map();
                                let tile_idx = (new_grid.y * map.width() as i32 + new_grid.x) as usize;
                                let terrain_str = if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                                    format!("{:?}", tile.type_())
                                } else {
                                    "Unknown".to_string()
                                };
                                
                                let is_indoor = map.interiors().iter().any(|i| {
                                    new_point.x >= i.x() && new_point.x <= i.x() + i.width() &&
                                    new_point.y >= i.y() && new_point.y <= i.y() + i.height()
                                });

                                let old_wp = map.world_point_from_grid_point(old_grid);
                                let new_wp = map.world_point_from_grid_point(new_grid);
                                let dist_m = battle_core::physics::utils::distance_between_points(&old_wp, &new_wp).millimeters() as f32 / 1000.0;

                                // GUI 디버그 그리드와 완벽하게 일치하는 섹터 연산 (예: A1, B2)
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
                                
                                // 이전 위치 섹터 계산
                                let old_adj_x = old_wp_exact.x - offset_x;
                                let old_adj_y = old_wp_exact.y - offset_y;
                                let old_col = (old_adj_x / cell_width).floor() as i32;
                                let old_row = (old_adj_y / cell_height).floor() as i32;
                                let old_row_idx = (old_row + 5).max(0) as usize;
                                let old_col_idx = (old_col + 6).max(0) as usize;
                                let old_letter = chars.get(old_row_idx % chars.len()).unwrap_or(&'?');
                                let old_sector = format!("{}{}", old_letter, old_col_idx);

                                // 새로운 위치 섹터 계산
                                let new_adj_x = new_point.x - offset_x;
                                let new_adj_y = new_point.y - offset_y;
                                let new_col = (new_adj_x / cell_width).floor() as i32;
                                let new_row = (new_adj_y / cell_height).floor() as i32;
                                let new_row_idx = (new_row + 5).max(0) as usize;
                                let new_col_idx = (new_col + 6).max(0) as usize;
                                let new_letter = chars.get(new_row_idx % chars.len()).unwrap_or(&'?');
                                let new_sector = format!("{}{}", new_letter, new_col_idx);

                                let soldier_posture = self.battle_state.soldier(*idx);
                                let posture_str = match soldier_posture.behavior() {
                                    battle_core::behavior::Behavior::Hide(_) | battle_core::behavior::Behavior::ScatterToCover(_) | battle_core::behavior::Behavior::GatherToCover(_) => "숨음",
                                    _ => match soldier_posture.body() {
                                        battle_core::behavior::Body::StandUp => "서있음",
                                        battle_core::behavior::Body::Crouched => "쪼그려앉음",
                                        battle_core::behavior::Body::Lying => "포복",
                                    }
                                };

                                if let Some(logger) = &mut self.logger {
                                    logger.log_movement(current_frame, *idx, old_sector, new_sector, &terrain_str, is_indoor, dist_m, posture_str);
                                }
                            }
                        }
                        battle_core::state::battle::message::BattleStateMessage::Soldier(idx, battle_core::state::battle::message::SoldierMessage::SetBehavior(behavior)) => {
                            // [버그 수정: 병사 개별 단위 로그 기록]
                            // 교전 대상 변경 시 지휘관 제한을 풀고 개별 병사 단위로 교전 정보를 기록합니다.
                            if let battle_core::behavior::Behavior::EngageSoldier(target_idx) = behavior {
                                let target_soldier = self.battle_state.soldier(*target_idx);
                                let target_squad_uuid = target_soldier.squad_uuid();
                                
                                let target_squad_comp = self.battle_state.squad(target_squad_uuid);
                                let alive_count = target_squad_comp.members().iter()
                                    .filter(|&&m_idx| self.battle_state.soldier(m_idx).alive())
                                    .count();
                                
                                let target_leader_idx = target_squad_comp.leader();
                                let target_leader = self.battle_state.soldier(target_leader_idx);
                                let target_pos = target_leader.world_point();
                                
                                let map = self.battle_state.map();
                                let target_grid = map.grid_point_from_world_point(&target_pos);
                                let tile_idx = (target_grid.y * map.width() as i32 + target_grid.x) as usize;
                                let terrain_str = if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                                    format!("{:?}", tile.type_())
                                } else {
                                    "Unknown".to_string()
                                };
                                
                                let is_indoor = map.interiors().iter().any(|i| {
                                    target_pos.x >= i.x() && target_pos.x <= i.x() + i.width() &&
                                    target_pos.y >= i.y() && target_pos.y <= i.y() + i.height()
                                });

                                let target_sector = get_sector(target_pos, map);

                                let soldier_posture = self.battle_state.soldier(*idx);
                                let posture_str = match soldier_posture.behavior() {
                                    battle_core::behavior::Behavior::Hide(_) | battle_core::behavior::Behavior::ScatterToCover(_) | battle_core::behavior::Behavior::GatherToCover(_) => "숨음",
                                    _ => match soldier_posture.body() {
                                        battle_core::behavior::Body::StandUp => "서있음",
                                        battle_core::behavior::Body::Crouched => "쪼그려앉음",
                                        battle_core::behavior::Body::Lying => "포복",
                                    }
                                };

                                // [개선] 교전거리 + 적군 공격량(스트레스) 합산 위협도 계산
                                let dist_m = battle_core::physics::utils::distance_between_points(&soldier_posture.world_point(), &target_pos).meters() as f32;
                                let enemy_attack_volume = *soldier_posture.under_fire().value() as f32;
                                // 거리가 짧을수록 위협이 커지도록 역산 (100m 기준) 후 공격량과 합산
                                let threat_score = (100.0 - dist_m.min(100.0)).max(0.0) * 2.0 + enemy_attack_volume;

                                if let Some(logger) = &mut self.logger {
                                    logger.log_engagement(current_frame, *idx, target_squad_uuid.0, target_grid, &target_sector, alive_count, &terrain_str, is_indoor, posture_str, threat_score);
                                }
                            }
                        }
                        battle_core::state::battle::message::BattleStateMessage::Soldier(idx, battle_core::state::battle::message::SoldierMessage::SetAlive(false)) => {
                            // 병사 사망 이벤트 (시간 순 기록)
                            let dead_soldier = self.battle_state.soldier(*idx);
                            let dead_pos = dead_soldier.world_point();
                            let map = self.battle_state.map();
                            let dead_grid = map.grid_point_from_world_point(&dead_pos);
                            
                            // [사로 인지 버그 수정] 아군이 전사한 위치(킬존)를 즉시 전술 메모리에 등록하여, 다른 분대들이 이 사로를 우회하도록 각인시킵니다.
                            let enemy_side = dead_soldier.side().opposite();
                            self.tactical_pings.insert(dead_grid, (current_frame + 3600, enemy_side));

                            let dead_tile_idx = (dead_grid.y * map.width() as i32 + dead_grid.x) as usize;
                            let dead_terrain_str = if let Some(tile) = map.terrain_tiles().get(dead_tile_idx) {
                                format!("{:?}", tile.type_())
                            } else {
                                "Unknown".to_string()
                            };
                            let dead_is_indoor = map.interiors().iter().any(|i| {
                                dead_pos.x >= i.x() && dead_pos.x <= i.x() + i.width() &&
                                dead_pos.y >= i.y() && dead_pos.y <= i.y() + i.height()
                            });

                            let mut cause_detail = "알 수 없음".to_string();

                            // 1. 총격에 의한 사망 여부 (마지막 타격 탄환 추적)
                            for bullet in self.battle_state.bullet_fires() {
                                // [수정] IncrementFrameI 처리로 인한 프레임 차이를 상쇄하기 위해 활성 범위를 유연하게 검사합니다.
                                if current_frame >= bullet.start() && current_frame <= bullet.end() {
                                    let dist = battle_core::physics::utils::distance_between_points(&dead_pos, bullet.point());
                                    // 타격 지점 반경 5m 이내면 피격 탄환으로 간주
                                    if dist.meters() <= 5 {
                                        let killer_pos = bullet.from();
                                        let killer_grid = map.grid_point_from_world_point(killer_pos);
                                        let killer_tile_idx = (killer_grid.y * map.width() as i32 + killer_grid.x) as usize;
                                        let killer_terrain = if let Some(tile) = map.terrain_tiles().get(killer_tile_idx) {
                                            format!("{:?}", tile.type_())
                                        } else {
                                            "Unknown".to_string()
                                        };
                                        let killer_is_indoor = map.interiors().iter().any(|i| {
                                            killer_pos.x >= i.x() && killer_pos.x <= i.x() + i.width() &&
                                            killer_pos.y >= i.y() && killer_pos.y <= i.y() + i.height()
                                        });
                                        let killer_env = if killer_is_indoor { "실내" } else { "실외" };
                                        let killer_sector = get_sector(*killer_pos, map);
                                        cause_detail = format!("총격 | 공격원 위치: {} (섹터: {}) (지형: {}, 환경: {})", killer_grid, killer_sector, killer_terrain, killer_env);
                                        break;
                                    }
                                }
                            }

                            // 2. 총격이 아니라면 폭발에 의한 사망 여부 추적
                            if cause_detail == "알 수 없음" {
                                for explosion in self.battle_state.explosions() {
                                    // [수정] 폭발 범위 역시 유연하게 검사합니다.
                                    if current_frame >= explosion.start() && current_frame <= explosion.end() {
                                        let dist = battle_core::physics::utils::distance_between_points(&dead_pos, explosion.point());
                                        // 폭발 반경 20m 이내 (안전 범위 고려)
                                        if dist.meters() <= 20 {
                                            let exp_grid = map.grid_point_from_world_point(explosion.point());
                                            let exp_tile_idx = (exp_grid.y * map.width() as i32 + exp_grid.x) as usize;
                                            let exp_terrain = if let Some(tile) = map.terrain_tiles().get(exp_tile_idx) {
                                                format!("{:?}", tile.type_())
                                            } else {
                                                "Unknown".to_string()
                                            };
                                            let exp_sector = get_sector(*explosion.point(), map);
                                            cause_detail = format!("폭발 | 폭발 원점: {} (섹터: {}) (지형: {})", exp_grid, exp_sector, exp_terrain);
                                            break;
                                        }
                                    }
                                }
                            }

                            // [버그 수정: 사상자 기록 유실 방지 폴백 고정]
                            // 타이밍 이슈로 인해 총격/폭발 원점 추적에 실패하여 사인이 "알 수 없음"으로 유지되더라도,
                            // 전사 사실 자체는 로그 배열에 반드시 누적되도록 강제 주입합니다.
                            if cause_detail == "알 수 없음" {
                                cause_detail = "미확인 타격으로 인한 전사 (교전 중 치명상)".to_string();
                            }

                            let side = *dead_soldier.side();
                            let dead_sector = get_sector(dead_pos, map);
                            if let Some(logger) = &mut self.logger {
                                logger.log_death(current_frame, side, *idx, dead_grid, &dead_sector, &dead_terrain_str, dead_is_indoor, &cause_detail);
                            }
                        }
                        battle_core::state::battle::message::BattleStateMessage::Soldier(idx, battle_core::state::battle::message::SoldierMessage::WeaponShot(_, shot)) => {
                            // 탄약 소모 이벤트 (Side 별로 구분)
                            let side = *self.battle_state.soldier(*idx).side();
                            let count = shot.count();
                            if let Some(logger) = &mut self.logger {
                                logger.log_ammo(side, count);
                            }
                        }
                        battle_core::state::battle::message::BattleStateMessage::PushBulletFire(ref bullet_fire) => {
                            // [Step 1: Tactical Ping] 총격 발생 시 해당 발포 원점을 30초간(1800프레임) 위험 지역으로 기억합니다.
                            let map = self.battle_state.map();
                            let origin_grid = map.grid_point_from_world_point(bullet_fire.from());
                            let mut shooter_side = battle_core::game::Side::B; 
                            for s in self.battle_state.soldiers() {
                                if battle_core::physics::utils::distance_between_points(&s.world_point(), bullet_fire.from()).meters() < 2 {
                                    shooter_side = *s.side();
                                    break;
                                }
                            }
                            self.tactical_pings.insert(origin_grid, (current_frame + 1800, shooter_side));

                            // [사로 인지 버그 수정] 발포 원점뿐만 아니라 총알이 향하는 종착점(피격 사로)도 함께 위험 지역으로 각인하여 킬존 진입을 막습니다.
                            let impact_grid = map.grid_point_from_world_point(bullet_fire.to());
                            self.tactical_pings.insert(impact_grid, (current_frame + 1800, shooter_side));
                        }
                        battle_core::state::battle::message::BattleStateMessage::PushCannonBlast(ref cannon_blast) => {
                            // [Step 1: Tactical Ping] 포격 발생 원점 또한 동일하게 위험 지역으로 각인시킵니다.
                            let map = self.battle_state.map();
                            let origin_grid = map.grid_point_from_world_point(cannon_blast.point());
                            let mut shooter_side = battle_core::game::Side::B; 
                            for s in self.battle_state.soldiers() {
                                if battle_core::physics::utils::distance_between_points(&s.world_point(), cannon_blast.point()).meters() < 2 {
                                    shooter_side = *s.side();
                                    break;
                                }
                            }
                            self.tactical_pings.insert(origin_grid, (current_frame + 1800, shooter_side));
                        }
                        _ => {}
                    }

                    side_effects.extend(
                        self.battle_state
                            .react(state_message, *self.battle_state.frame_i()),
                    );
                }
                // These messages are destined to be directly sent to clients
                RunnerMessage::ClientsState(_) | RunnerMessage::ClientState(_, _) => {}
                RunnerMessage::IncrementVisibilityIndex => {
                    self.current_visibility += 1;
                    if self.current_visibility >= self.battle_state.soldiers().len() {
                        self.current_visibility = 0;
                    }
                }
            }
        }

        for side_effect in &side_effects {
            self.side_effect(side_effect)
        }
    }

    // TODO : Side effects should not exists : All side effects
    // should be computed when original message is produced
    pub fn side_effect(&mut self, side_effect: &SideEffect) {
        match side_effect {
            SideEffect::SoldierFinishHisBehavior(soldier_index, then) => {
                let soldier = self.battle_state.soldier(*soldier_index);
                
                // [버그 수정: 사망 처리 누락 및 좀비 방지]
                // 이미 피격되어 죽은(Dead) 유닛이 과거 예약된 기동 종료 콜백을 받아 강제로 부활하는 현상을 차단합니다.
                if !soldier.alive() {
                    return;
                }
                
                let (behavior, order) = if let Some(then_order) = then {
                    (
                        Behavior::from_order(then_order, soldier, &self.battle_state),
                        then_order.clone(),
                    )
                } else {
                    (
                        Behavior::Idle(Body::from_soldier(soldier, &self.battle_state)),
                        Order::Idle,
                    )
                };
                let soldier = self.battle_state.soldier_mut(*soldier_index);
                soldier.set_behavior(behavior);
                soldier.set_order(order);
            }
            // Server ignore this side effect because concern Gui only
            SideEffect::RefreshEntityAnimation(_) => {}
        }
    }
}
