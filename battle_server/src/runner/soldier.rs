use battle_core::{
    order::Order,
    physics::path::{find_path, Direction, PathMode},
    physics::utils::distance_between_points,
    state::battle::message::{BattleStateMessage, SoldierMessage},
    types::{Angle, SoldierIndex, WorldPath, WorldPaths},
};

use super::{message::RunnerMessage, Runner};

impl Runner {
    // TODO : Soldiers in vehicles must be managed differently than ground soldiers
    pub fn tick_soldiers(&self) -> Vec<RunnerMessage> {
        puffin::profile_scope!("tick_soldiers");
        let mut messages = vec![];
        let tick_animate = self.battle_state.frame_i() % self.config.soldier_animate_freq() == 0
            && self.battle_state.phase().is_battle();
        let tick_update = self.battle_state.frame_i() % self.config.soldier_update_freq() == 0;

        // Entities animation
        if tick_animate {
            messages.extend(
                (0..self.battle_state.soldiers().len())
                    // TODO : For now, parallel iter cost more than serial
                    // .into_par_iter()
                    .flat_map(|i| self.animate_soldier(SoldierIndex(i)))
                    .collect::<Vec<RunnerMessage>>(),
            );
        }

        // Entities updates
        if tick_update {
            let soldier_messages: Vec<RunnerMessage> = (0..self.battle_state.soldiers().len())
                // TODO : For now, parallel iter cost more than serial
                // .into_par_iter()
                .flat_map(|i| self.update_soldier(SoldierIndex(i)))
                .collect();
            messages.extend(soldier_messages);
        }

        messages
    }

    pub fn tick_feeling_decreasing_soldiers(&self) -> Vec<RunnerMessage> {
        puffin::profile_scope!("tick_feeling_decreasing_soldiers");
        let mut messages = vec![];
        let tick_feeling_decreasing =
            self.battle_state.frame_i() % self.config.feeling_decreasing_freq() == 0
                && self.battle_state.phase().is_battle();

        if tick_feeling_decreasing {
            messages.extend((0..self.battle_state.soldiers().len()).flat_map(|i| {
                let soldier = self.battle_state.soldier(SoldierIndex(i));
                let mut msgs = vec![RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    SoldierIndex(i),
                    SoldierMessage::DecreaseUnderFire,
                ))];

                // [개선] 은엄폐(Hide) 중이거나 포복(Lying) 자세일 경우 스트레스 초고속 안정화
                if matches!(soldier.behavior(), battle_core::behavior::Behavior::Hide(_)) 
                    || matches!(soldier.body(), battle_core::behavior::Body::Lying) 
                {
                    msgs.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                        SoldierIndex(i),
                        // 기본 DecreaseUnderFire 외에 30을 추가로 감소시켜 약 4배 빠르게 멘탈 회복
                        SoldierMessage::RelieveStress(30),
                    )));
                }

                msgs
            }));
        }

        // [사기 지원 도박 (Support Promise) 체크 및 진입 트리거]
        // 전투 중 매 틱마다 약속이 만료되었거나, 실제 아군이 도착했는지 확인합니다.
        if self.battle_state.phase().is_battle() {
            for i in 0..self.battle_state.soldiers().len() {
                let soldier = self.battle_state.soldier(SoldierIndex(i));
                if *soldier.support_promise_end_frame_i() > 0 {
                    // 해제 트리거 (Proximity Evaluator): 반경 50m 이내에 다른 분대장이나 아군 전차가 도착했는지 스캔
                    let mut support_arrived = false;
                    
                    for other_soldier in self.battle_state.soldiers() {
                        if other_soldier.alive() && other_soldier.side() == soldier.side() && other_soldier.squad_uuid() != soldier.squad_uuid() {
                            let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &other_soldier.world_point());
                            if dist.meters() <= 50 {
                                support_arrived = true;
                                break;
                            }
                        }
                    }

                    if support_arrived {
                        // 진짜 아군 지원 도착! 타이머 즉시 해제 및 Hiding 락 해제 (도박 성공)
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            SoldierIndex(i),
                            SoldierMessage::ClearSupportPromise,
                        )));
                        // 위험 구역 고착 상태에서 풀려나 다시 기동할 수 있도록 Idle 상태로 전환합니다.
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            SoldierIndex(i),
                            SoldierMessage::RelieveStress(200),
                        )));
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            SoldierIndex(i),
                            SoldierMessage::SetOrder(battle_core::order::Order::Idle),
                        )));
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            SoldierIndex(i),
                            SoldierMessage::SetBehavior(battle_core::behavior::Behavior::Idle(battle_core::behavior::Body::StandUp)),
                        )));
                    } else if *self.battle_state.frame_i() >= *soldier.support_promise_end_frame_i() {
                        // 지원군 미도착 상태로 시간 만료 (역풍 발생)
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            SoldierIndex(i),
                            SoldierMessage::CheckSupportPromise,
                        )));
                    }
                }
            }
        }

        messages
    }

    pub fn soldier_is_squad_leader(&self, soldier_index: SoldierIndex) -> bool {
        let soldier = self.battle_state.soldier(soldier_index);
        let squad_uuid = soldier.squad_uuid();
        let squad_composition = self.battle_state.squad(squad_uuid);
        let squad_leader = squad_composition.leader();
        squad_leader == soldier_index
    }

    pub fn animate_soldier(&self, soldier_index: SoldierIndex) -> Vec<RunnerMessage> {
        puffin::profile_scope!("animate_soldier", format!("{}", soldier_index));
        let soldier = self.battle_state.soldier(soldier_index);
        if !soldier.can_be_animated() {
            return vec![];
        }

        let mut messages = vec![];

        messages.extend(self.soldier_behavior(soldier));
        messages.extend(self.soldier_gesture(soldier));

        messages
    }

    pub fn tick_update_squad_leaders(&self) -> Vec<RunnerMessage> {
        puffin::profile_scope!("tick_update_squad_leaders");
        let mut messages = vec![];
        let tick_update =
            self.battle_state.frame_i() % self.config.squad_leaders_update_freq() == 0;

        // [사망 처리 및 지휘관 교체 프리징 버그 해결]
        // 기존 120프레임(2초) 고정 주기 조건문 내부에서 분대장 생존 여부를 판별하면 전사 시 2초간 무전과 기동이 먹통이 됩니다.
        // 루프를 밖으로 빼내어, 지휘관 능력을 완전 상실했거나(!can_be_leader()) 사망(!alive()) 상태라면 주기와 관계없이 즉각(Every frame) 지휘권을 다음 병사에게 승계시킵니다.
        for squad_uuid in self.battle_state.squads().keys() {
            let squad = self.battle_state.squad(*squad_uuid);
            let leader = self.battle_state.soldier(squad.leader());

            if !leader.can_be_leader() || !leader.alive() || tick_update {
                if !leader.can_be_leader() || !leader.alive() {
                    if let Some(member) = squad
                        .subordinates()
                        .iter()
                        .map(|s| self.battle_state.soldier(**s))
                        .find(|s| s.can_be_leader() && s.alive())
                    {
                        messages.push(RunnerMessage::BattleState(
                            BattleStateMessage::SetSquadLeader(*squad_uuid, member.uuid()),
                        ))
                    }
                }
            }
        }

        // [전술 AI: 수적 열세 지원 및 후방 분대 백필(Backfill)]
        // 교전 상황을 모니터링하여 병력이 부족한 곳에 자동으로 예비대를 투입하고, 그 빈자리를 더 후방의 병력으로 채웁니다. (YOLO 모드와 무관하게 동작)
        if tick_update {
            let mut occupied_squads = std::collections::HashSet::new();

            // 모든 분대를 순회하며 교전 중인 분대 스캔
            for squad_uuid in self.battle_state.squads().keys() {
                let squad = self.battle_state.squad(*squad_uuid);
                let leader = self.battle_state.soldier(squad.leader());

                if !leader.alive() { continue; }

                // 교전 중이거나 위협을 받고 있는 상태인지 확인
                if leader.under_fire().exist() || leader.target().is_some() {
                    let mut allies_count = 0;
                    let mut enemies_count = 0;
                    let leader_pos = leader.world_point();

                    // 반경 40m 내의 피아 병력 수 계산
                    for other in self.battle_state.soldiers() {
                        if other.alive() {
                            let dist = distance_between_points(&leader_pos, &other.world_point()).meters();
                            if dist <= 40 {
                                if other.side() == leader.side() {
                                    allies_count += 1;
                                } else {
                                    enemies_count += 1;
                                }
                            }
                        }
                    }

                    // 수적 열세 판별 (적이 아군보다 많을 때 지원 요청)
                    if enemies_count > allies_count && allies_count > 0 {
                        let mut best_support = None;
                        let mut min_support_dist = std::f32::MAX;

                        // 지원해줄 가장 가까운 대기/방어/엄폐 중인 아군 분대 찾기
                        for (other_squad_uuid, other_squad) in self.battle_state.squads() {
                            if other_squad_uuid == squad_uuid || occupied_squads.contains(other_squad_uuid) { continue; }

                            let other_leader = self.battle_state.soldier(other_squad.leader());
                            if !other_leader.alive() || other_leader.side() != leader.side() { continue; }

                            if matches!(other_leader.order(), Order::Idle | Order::Defend(_) | Order::Hide(_)) {
                                // 지원 분대 주변(20m)에는 적이 없어야 함 (안전한 상태의 예비대만 차출)
                                let enemies_near_support = self.battle_state.soldiers().iter()
                                    .filter(|s| s.side() != leader.side() && s.alive())
                                    .any(|s| distance_between_points(&other_leader.world_point(), &s.world_point()).meters() <= 20);

                                if !enemies_near_support {
                                    let dist = (other_leader.world_point().to_vec2() - leader_pos.to_vec2()).length();
                                    if dist < min_support_dist {
                                        min_support_dist = dist;
                                        best_support = Some((*other_squad_uuid, other_leader.world_point(), other_leader.get_looking_direction()));
                                    }
                                }
                            }
                        }

                        // 지원 분대를 찾았다면 전방으로 급파
                        if let Some((support_squad_uuid, support_orig_pos, support_orig_dir)) = best_support {
                            occupied_squads.insert(support_squad_uuid);

                            let map = self.battle_state.map();
                            let path_mode = PathMode::Walk;
                            let start_dir = Some(Direction::from_angle(&support_orig_dir));

                            // [우회 기동(Flank) 네비게이션 생성] 적군의 사선을 피하기 위해 "후퇴 -> 측면 기동 -> 합류"의 ㄷ자 형태 경로를 생성합니다.
                            // 1. 적진 중심점(Front) 계산
                            let mut enemy_center = glam::Vec2::ZERO;
                            let mut enemy_count = 0.0;
                            for enemy in self.battle_state.soldiers() {
                                if enemy.side() != leader.side() && enemy.alive() {
                                    enemy_center += enemy.world_point().to_vec2();
                                    enemy_count += 1.0;
                                }
                            }
                            let enemy_center = if enemy_count > 0.0 {
                                enemy_center / enemy_count
                            } else {
                                leader_pos.to_vec2() + glam::Vec2::new(100.0, 0.0) // 적이 안 보이면 현재 위치 기준 임의 전방 설정
                            };

                            // 2. 후퇴(Retreat) 벡터: 적 중심에서 아군 방향으로 뻗어나가는 후방 벡터
                            let mut retreat_dir = (support_orig_pos.to_vec2() - enemy_center).normalize();
                            if retreat_dir.is_nan() { retreat_dir = glam::Vec2::new(0.0, 1.0); }

                            // 맵 클램핑 헬퍼 함수 (에지 충돌 방지)
                            let map_w = map.visual_width() as f32 - 30.0;
                            let map_h = map.visual_height() as f32 - 30.0;
                            let clamp_pt = |mut v: glam::Vec2| {
                                v.x = v.x.clamp(30.0, map_w.max(30.0));
                                v.y = v.y.clamp(30.0, map_h.max(30.0));
                                battle_core::types::WorldPoint::from_vec2(v)
                            };

                            // 웨이포인트 1 (후방 후퇴 지점): 현재 위치에서 적 반대 방향으로 약 45m 후퇴 (150 pixel)
                            let wp1 = clamp_pt(support_orig_pos.to_vec2() + retreat_dir * 150.0);
                            // 웨이포인트 2 (측면 우회 지점): 목표 아군 진지의 후방 약 45m 지점
                            let wp2 = clamp_pt(leader_pos.to_vec2() + retreat_dir * 150.0);

                            let grids = vec![
                                map.grid_point_from_world_point(&support_orig_pos),
                                map.grid_point_from_world_point(&wp1),
                                map.grid_point_from_world_point(&wp2),
                                map.grid_point_from_world_point(&leader_pos)
                            ];

                            let mut combined_world_paths = vec![];
                            let mut current_start_dir = start_dir;

                            for i in 0..grids.len()-1 {
                                let from_g = grids[i];
                                let to_g = grids[i+1];
                                if from_g != to_g {
                                    if let Some(grid_path) = find_path(
                                        &self.config, map, &from_g, &to_g, true, &path_mode, &current_start_dir,
                                    ) {
                                        if !grid_path.is_empty() {
                                            let last_p = *grid_path.last().unwrap();
                                            let d_angle = battle_core::utils::angleg(&last_p, &from_g);
                                            current_start_dir = Some(Direction::from_angle(&d_angle));
                                            
                                            let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                            combined_world_paths.push(WorldPath::new(world_path));
                                        }
                                    }
                                }
                            }

                            if !combined_world_paths.is_empty() {
                                let world_paths = WorldPaths::new(combined_world_paths);
                                let support_squad_comp = self.battle_state.squad(support_squad_uuid);
                                let support_leader_idx = support_squad_comp.leader();

                                // [Lock 강제 해제] 지원 부대원 전체의 스트레스를 초기화하고 강제 엄폐(Hide) 상태를 풀어줍니다.
                                for member_idx in support_squad_comp.members() {
                                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                        *member_idx,
                                        SoldierMessage::RelieveStress(200),
                                    )));
                                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                        *member_idx,
                                        SoldierMessage::SetOrder(Order::Idle),
                                    )));
                                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                        *member_idx,
                                        SoldierMessage::SetBehavior(battle_core::behavior::Behavior::Idle(battle_core::behavior::Body::StandUp)),
                                    )));
                                }

                                // 지원 부대에게 우회 네비게이션이 주입된 빠른 기동 명령 하달
                                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                    support_leader_idx,
                                    SoldierMessage::SetOrder(Order::MoveFastTo(world_paths, Some(Box::new(Order::Defend(Angle(0.0))))))
                                )));

                                // [백필(Backfill)] 지원 분대가 비운 자리를 메울 더 후방의 예비대 찾기
                                let mut best_backfill = None;
                                let mut min_backfill_dist = std::f32::MAX;

                                for (bf_squad_uuid, bf_squad) in self.battle_state.squads() {
                                    if bf_squad_uuid == squad_uuid || bf_squad_uuid == &support_squad_uuid || occupied_squads.contains(bf_squad_uuid) { continue; }

                                    let bf_leader = self.battle_state.soldier(bf_squad.leader());
                                    if !bf_leader.alive() || bf_leader.side() != leader.side() { continue; }

                                    if matches!(bf_leader.order(), Order::Idle | Order::Defend(_) | Order::Hide(_)) {
                                        let enemies_near_bf = self.battle_state.soldiers().iter()
                                            .filter(|s| s.side() != leader.side() && s.alive())
                                            .any(|s| distance_between_points(&bf_leader.world_point(), &s.world_point()).meters() <= 20);

                                        if !enemies_near_bf {
                                            // 현재 교전 지역(leader_pos)으로부터 더 멀리 있는(25m 이상 후방) 분대 중에서 선택
                                            let dist_to_combat = distance_between_points(&bf_leader.world_point(), &leader_pos).meters();
                                            if dist_to_combat > 25 { 
                                                let dist_to_vacant = (bf_leader.world_point().to_vec2() - support_orig_pos.to_vec2()).length();
                                                if dist_to_vacant < min_backfill_dist {
                                                    min_backfill_dist = dist_to_vacant;
                                                    best_backfill = Some((*bf_squad_uuid, bf_leader.world_point(), bf_leader.get_looking_direction()));
                                                }
                                            }
                                        }
                                    }
                                }

                                // 백필 부대를 찾았다면 지원 부대가 비운 진지로 이동시킴
                                if let Some((bf_squad_uuid, bf_orig_pos, bf_orig_dir)) = best_backfill {
                                    occupied_squads.insert(bf_squad_uuid);

                                    // 백필(예비대) 역시 사선을 피하기 위해 동일한 후방 우회 로직을 적용합니다.
                                    let bf_wp1 = clamp_pt(bf_orig_pos.to_vec2() + retreat_dir * 150.0);
                                    let bf_wp2 = clamp_pt(support_orig_pos.to_vec2() + retreat_dir * 150.0);

                                    let bf_grids = vec![
                                        map.grid_point_from_world_point(&bf_orig_pos),
                                        map.grid_point_from_world_point(&bf_wp1),
                                        map.grid_point_from_world_point(&bf_wp2),
                                        map.grid_point_from_world_point(&support_orig_pos)
                                    ];

                                    let mut bf_combined_paths = vec![];
                                    let mut bf_start_dir = Some(Direction::from_angle(&bf_orig_dir));

                                    for i in 0..bf_grids.len()-1 {
                                        let from_g = bf_grids[i];
                                        let to_g = bf_grids[i+1];
                                        if from_g != to_g {
                                            if let Some(grid_path) = find_path(
                                                &self.config, map, &from_g, &to_g, true, &path_mode, &bf_start_dir,
                                            ) {
                                                if !grid_path.is_empty() {
                                                    let last_p = *grid_path.last().unwrap();
                                                    let d_angle = battle_core::utils::angleg(&last_p, &from_g);
                                                    bf_start_dir = Some(Direction::from_angle(&d_angle));
                                                    
                                                    let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                                    bf_combined_paths.push(WorldPath::new(world_path));
                                                }
                                            }
                                        }
                                    }

                                    if !bf_combined_paths.is_empty() {
                                        let bf_world_paths = WorldPaths::new(bf_combined_paths);
                                        let bf_squad_comp = self.battle_state.squad(bf_squad_uuid);
                                        let bf_leader_idx = bf_squad_comp.leader();

                                        // [Lock 강제 해제] 백필 부대원 전체의 스트레스를 초기화하고 강제 엄폐(Hide) 상태를 풀어줍니다.
                                        for member_idx in bf_squad_comp.members() {
                                            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                                *member_idx,
                                                SoldierMessage::RelieveStress(200),
                                            )));
                                            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                                *member_idx,
                                                SoldierMessage::SetOrder(Order::Idle),
                                            )));
                                            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                                *member_idx,
                                                SoldierMessage::SetBehavior(battle_core::behavior::Behavior::Idle(battle_core::behavior::Body::StandUp)),
                                            )));
                                        }

                                        // 빈 자리로 우회 이동(MoveTo) 후 방어 진지 구축
                                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                            bf_leader_idx,
                                            SoldierMessage::SetOrder(Order::MoveTo(bf_world_paths, Some(Box::new(Order::Defend(Angle(0.0))))))
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        messages
    }
}
