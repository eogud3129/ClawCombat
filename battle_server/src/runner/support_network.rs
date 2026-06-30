use battle_core::{
    entity::soldier::AmmoState,
    game::weapon::Weapon,
    order::Order,
    physics::utils::distance_between_points,
    state::battle::message::{BattleStateMessage, SoldierMessage},
    types::{SquadUuid, WorldPoint},
};

use crate::runner::{message::RunnerMessage, Runner, SupportRequest, SupportUrgency, SupportAssignment, SupportAssignmentStatus};

pub struct SupportThreatAssessment {
    pub supporter_squad: SquadUuid,
    pub threat_level: f32,
    pub enemy_ping_count: i32,
    pub recent_impact_count: i32,
    pub safe_position: Option<WorldPoint>,
    pub should_evade: bool,
}

pub struct CompanySupportPriority {
    pub company_name: String,
    pub side: battle_core::game::Side,
    pub squads: Vec<SquadUuid>,
    pub total_threat: f32,
    pub active_requests: usize,
    pub max_concurrent_support: usize,
}

impl Runner {
    pub fn tick_support_network(&mut self) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        let current_frame = *self.battle_state.frame_i();

        self.support_requests.retain(|_, req| {
            current_frame - req.created_frame < 3600
        });

        self.support_assignments.retain(|_, assignment| {
            assignment.status != SupportAssignmentStatus::Completed
                && assignment.status != SupportAssignmentStatus::Cancelled
        });

        self.support_cooldown.retain(|_, last_frame| {
            current_frame - *last_frame < 1800
        });

        if current_frame % 120 == 0 {
            self.retry_pending_requests(current_frame);
            messages.extend(self.assess_support_squad_safety(current_frame));
            messages.extend(self.issue_evasion_orders(current_frame));
        }

        if current_frame % 60 == 0 {
            let company_priorities = self.calculate_company_priorities(current_frame);

            for side in [battle_core::game::Side::A, battle_core::game::Side::B] {
                messages.extend(self.scan_and_request_support_with_priority(side, current_frame, &company_priorities));
            }

            messages.extend(self.process_support_requests_with_company_priority(current_frame, &company_priorities));
            messages.extend(self.process_support_requests(current_frame));
        }

        if current_frame % 60 == 0 {
            messages.extend(self.update_supporting_squads(current_frame));
        }

        messages
    }

    fn retry_pending_requests(&mut self, current_frame: u64) {
        let mut to_retry = vec![];
        
        for (squad_uuid, request) in self.support_requests.iter_mut() {
            if request.retry_count < 3 && current_frame - request.last_retry_frame > 600 {
                to_retry.push(*squad_uuid);
                request.retry_count += 1;
                request.last_retry_frame = current_frame;
            }
        }
        
        for squad_uuid in to_retry {
            if let Some(request) = self.support_requests.get(&squad_uuid) {
                if current_frame % 600 == 0 {
                    println!(
                        "[지원 재시도] 분대 {} 지원 요청 재시도 ({}회차)",
                        squad_uuid.0,
                        request.retry_count
                    );
                }
            }
        }
    }

    fn assess_support_squad_safety(&mut self, current_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        let mut threat_assessments: Vec<SupportThreatAssessment> = vec![];

        for (requester_squad, assignment) in &self.support_assignments {
            // [버그 수정: 잦은 A* 연산으로 인한 프레임 저하(프리징) 방지]
            // 이미 회피(Evasion) 명령을 받고 이동 중인 분대에게 쿨타임 동안은 재평가와 A* 탐색을 생략하여 연산 폭주를 차단합니다.
            if let Some(cooldown) = self.support_cooldown.get(&assignment.supporter_squad) {
                if current_frame < *cooldown {
                    continue;
                }
            }

            let supporter = self.battle_state.soldier(
                self.battle_state.squad(assignment.supporter_squad).leader()
            );

            if !supporter.alive() {
                continue;
            }

            let supporter_pos = supporter.world_point();
            let mut threat_level = 0.0;
            let mut enemy_ping_count = 0;
            let mut recent_impact_count = 0;

            let map = self.battle_state.map();
            for (ping_grid, (ping_frame, ping_side)) in &self.tactical_pings {
                if *ping_side == supporter.side().opposite() {
                    let ping_world = map.world_point_from_grid_point(*ping_grid);
                    let dist = distance_between_points(&supporter_pos, &ping_world);
                    
                    if dist.meters() < 50 {
                        threat_level += 0.3;
                        enemy_ping_count += 1;
                    } else if dist.meters() < 100 {
                        threat_level += 0.1;
                        enemy_ping_count += 1;
                    }

                    if current_frame - ping_frame < 300 {
                        threat_level += 0.2;
                    }
                }
            }

            let stress = *supporter.under_fire().value() as f32 / 200.0;
            threat_level += stress * 0.3;

            if stress > 0.5 {
                recent_impact_count += 1;
            }

            let mut enemy_nearby = 0;
            for enemy in self.battle_state.soldiers() {
                if *enemy.side() == supporter.side().opposite() && enemy.alive() {
                    let dist = distance_between_points(&supporter_pos, &enemy.world_point());
                    if dist.meters() < 30 {
                        enemy_nearby += 1;
                        threat_level += 0.2;
                    } else if dist.meters() < 60 {
                        enemy_nearby += 1;
                        threat_level += 0.1;
                    }
                }
            }

            let should_evade = threat_level > 0.7 || enemy_nearby >= 3 || recent_impact_count >= 2;

            let safe_position = if should_evade {
                self.calculate_safe_position(&supporter_pos, &supporter.side().opposite())
            } else {
                None
            };

            threat_assessments.push(SupportThreatAssessment {
                supporter_squad: assignment.supporter_squad,
                threat_level,
                enemy_ping_count,
                recent_impact_count,
                safe_position,
                should_evade,
            });

            if current_frame % 600 == 0 && should_evade {
                println!(
                    "[카운터 배터리] 분대 {} 위협 평가: 위협도={:.2}, 핑={}개, 근접적={}명, 회피필요={}",
                    assignment.supporter_squad.0,
                    threat_level,
                    enemy_ping_count,
                    enemy_nearby,
                    should_evade
                );
            }
        }

        for assessment in &threat_assessments {
            if assessment.should_evade {
                if let Some(safe_pos) = assessment.safe_position {
                    messages.extend(self.create_evasion_order(assessment.supporter_squad, safe_pos, current_frame));
                }
            }
        }

        messages
    }

    fn calculate_safe_position(&self, current_pos: &WorldPoint, enemy_side: &battle_core::game::Side) -> Option<WorldPoint> {
        let map = self.battle_state.map();
        let mut safe_positions: Vec<(WorldPoint, f32)> = vec![];

        let mut checkpoints: Vec<WorldPoint> = vec![];
        for (_, cp) in self.checkpoints.read().unwrap().iter() {
            checkpoints.push(*cp);
        }

        if !checkpoints.is_empty() {
            let mut nearest_cp = checkpoints[0];
            let mut min_dist = std::f32::MAX;
            for cp in &checkpoints {
                let dist = (current_pos.to_vec2() - cp.to_vec2()).length();
                if dist < min_dist {
                    min_dist = dist;
                    nearest_cp = *cp;
                }
            }
            safe_positions.push((nearest_cp, 1.0));
        }

        for squad in self.battle_state.squads().values() {
            let leader = self.battle_state.soldier(squad.leader());
            if leader.side() != enemy_side && leader.alive() {
                let dist = distance_between_points(current_pos, &leader.world_point());
                if dist.meters() < 100 {
                    let dir = (leader.world_point().to_vec2() - current_pos.to_vec2()).normalize();
                    let safe_point = current_pos.to_vec2() + dir * 30.0;
                    let clamped = self.clamp_to_map_bounds(&WorldPoint::from_vec2(safe_point));
                    let score = 1.0 - (dist.meters() as f32 / 100.0) * 0.5;
                    safe_positions.push((clamped, score));
                }
            }
        }

        let map = self.battle_state.map();
        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
            if ping_side == enemy_side {
                let ping_world = map.world_point_from_grid_point(*ping_grid);
                let dist_to_ping = distance_between_points(current_pos, &ping_world);
                if dist_to_ping.meters() < 150 {
                    let dir = (current_pos.to_vec2() - ping_world.to_vec2()).normalize();
                    let safe_point = current_pos.to_vec2() + dir * 50.0;
                    let clamped = self.clamp_to_map_bounds(&WorldPoint::from_vec2(safe_point));
                    let score = 0.7;
                    safe_positions.push((clamped, score));
                }
            }
        }

        safe_positions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        if !safe_positions.is_empty() {
            Some(safe_positions[0].0)
        } else {
            let mut retreat_dir = glam::Vec2::new(0.0, -1.0);
            let mut nearest_enemy_dist = std::f32::MAX;
            for enemy in self.battle_state.soldiers() {
                if enemy.side() == enemy_side && enemy.alive() {
                    let dist = distance_between_points(current_pos, &enemy.world_point());
                    if (dist.meters() as f32) < nearest_enemy_dist {
                        nearest_enemy_dist = dist.meters() as f32;
                        retreat_dir = (current_pos.to_vec2() - enemy.world_point().to_vec2()).normalize();
                    }
                }
            }
            // [최적화] 후퇴 거리를 100m에서 50m로 단축시켜, A* 탐색 범위가 지나치게 넓어져 게임이 멈추는 현상을 방지합니다.
            let retreat_point = current_pos.to_vec2() + retreat_dir * 50.0;
            Some(self.clamp_to_map_bounds(&WorldPoint::from_vec2(retreat_point)))
        }
    }

    fn clamp_to_map_bounds(&self, point: &WorldPoint) -> WorldPoint {
        let map = self.battle_state.map();
        let map_width = map.visual_width() as f32;
        let map_height = map.visual_height() as f32;
        WorldPoint::from_vec2(glam::Vec2::new(
            point.x.clamp(10.0, map_width - 10.0),
            point.y.clamp(10.0, map_height - 10.0),
        ))
    }

    fn create_evasion_order(&mut self, supporter_squad: SquadUuid, safe_position: WorldPoint, current_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        let squad = self.battle_state.squad(supporter_squad);
        let leader = self.battle_state.soldier(squad.leader());

        if !leader.alive() {
            return messages;
        }

        let map = self.battle_state.map();
        let from_grid = map.grid_point_from_world_point(&leader.world_point());
        let to_grid = map.grid_point_from_world_point(&safe_position);

        if from_grid == to_grid {
            return messages;
        }

        let path_mode = battle_core::physics::path::PathMode::Walk;
        let start_dir = Some(battle_core::physics::path::Direction::from_angle(&leader.get_looking_direction()));

        if let Some(grid_path) = battle_core::physics::path::find_stealth_path(
            &self.config,
            map,
            &from_grid,
            &to_grid,
            true,
            &path_mode,
            &start_dir,
        ) {
            let world_path = grid_path
                .iter()
                .map(|p| map.world_point_from_grid_point(*p))
                .collect();

            let world_paths = battle_core::types::WorldPaths::new(vec![
                battle_core::types::WorldPath::new(world_path)
            ]);

            let order = Order::SneakTo(world_paths.clone(), Some(Box::new(Order::Idle)));

            messages.push(RunnerMessage::BattleState(
                BattleStateMessage::Soldier(
                    squad.leader(),
                    SoldierMessage::SetOrder(order),
                )
            ));

            for member_idx in squad.members() {
                if *member_idx != squad.leader() {
                    let member = self.battle_state.soldier(*member_idx);
                    if member.alive() {
                        messages.push(RunnerMessage::BattleState(
                            BattleStateMessage::Soldier(
                                *member_idx,
                                SoldierMessage::SetOrder(Order::SneakTo(
                                    world_paths.clone(),
                                    Some(Box::new(Order::Idle))
                                )),
                            )
                        ));
                    }
                }
            }

            if let Some(assignment) = self.support_assignments.values_mut()
                .find(|a| a.supporter_squad == supporter_squad) {
                assignment.status = SupportAssignmentStatus::Moving;
            }

            self.support_cooldown.insert(supporter_squad, current_frame + 600);

            if current_frame % 600 == 0 {
                println!(
                    "[카운터 배터리] 분대 {} 회피 이동 시작 (목표: {:.0}, {:.0})",
                    supporter_squad.0,
                    safe_position.x,
                    safe_position.y
                );
            }
        } else {
            messages.push(RunnerMessage::BattleState(
                BattleStateMessage::Soldier(
                    squad.leader(),
                    SoldierMessage::SetBehavior(battle_core::behavior::Behavior::Hide(battle_core::types::Angle(0.0))),
                )
            ));
        }

        messages
    }

    fn issue_evasion_orders(&mut self, _current_frame: u64) -> Vec<RunnerMessage> {
        vec![]
    }

    fn calculate_company_priorities(&self, _current_frame: u64) -> Vec<CompanySupportPriority> {
        let mut priorities = vec![];

        for (company_name, company) in &self.companies {
            let mut total_threat = 0.0;
            let mut active_requests = 0;

            for squad_uuid in &company.squads {
                if let Some(request) = self.support_requests.get(squad_uuid) {
                    total_threat += request.threat_level;
                    active_requests += 1;
                } else {
                    let squad = self.battle_state.squad(*squad_uuid);
                    let leader = self.battle_state.soldier(squad.leader());
                    if leader.alive() && matches!(
                        leader.behavior(),
                        battle_core::behavior::Behavior::EngageSoldier(_)
                            | battle_core::behavior::Behavior::SuppressFire(_)
                    ) {
                        let stress = *leader.under_fire().value() as f32 / 200.0;
                        total_threat += stress * 0.5;
                    }
                }
            }

            let max_concurrent_support = (company.squads.len() / 2).max(1);

            priorities.push(CompanySupportPriority {
                company_name: company_name.clone(),
                side: company.side,
                squads: company.squads.clone(),
                total_threat,
                active_requests,
                max_concurrent_support,
            });
        }

        priorities.sort_by(|a, b| b.total_threat.partial_cmp(&a.total_threat).unwrap_or(std::cmp::Ordering::Equal));

        priorities
    }

    fn scan_and_request_support_with_priority(
        &mut self,
        side: battle_core::game::Side,
        current_frame: u64,
        company_priorities: &[CompanySupportPriority]
    ) -> Vec<RunnerMessage> {
        let mut messages = vec![];

        let side_priorities: Vec<&CompanySupportPriority> = company_priorities
            .iter()
            .filter(|p| p.side == side)
            .collect();

        for priority in side_priorities {
            if priority.active_requests >= priority.max_concurrent_support {
                continue;
            }

            let mut squad_threats: Vec<(SquadUuid, f32)> = vec![];

            for squad_uuid in &priority.squads {
                let squad = self.battle_state.squad(*squad_uuid);
                let leader = self.battle_state.soldier(squad.leader());

                if !leader.alive() {
                    continue;
                }

                if self.support_requests.contains_key(squad_uuid) {
                    continue;
                }
                if self.active_support_squads.contains(squad_uuid) {
                    continue;
                }
                if self.support_cooldown.contains_key(squad_uuid) {
                    continue;
                }

                let is_engaging = matches!(
                    leader.behavior(),
                    battle_core::behavior::Behavior::EngageSoldier(_)
                        | battle_core::behavior::Behavior::SuppressFire(_)
                );

                if !is_engaging {
                    continue;
                }

                let ammo_state = leader.determine_ammo_state();
                let ammo_factor = match ammo_state {
                    AmmoState::CriticalAmmo => 0.8,
                    AmmoState::LowAmmo => 0.3,
                    _ => 0.0,
                };

                let engagement_duration = current_frame.saturating_sub(*leader.last_shoot_frame_i());
                let base_threat = self.calculate_threat_level(squad_uuid, leader, engagement_duration);
                let threat_level = (base_threat + ammo_factor).min(1.0);

                if threat_level >= 0.15 {
                    squad_threats.push((*squad_uuid, threat_level));
                }
            }

            squad_threats.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let remaining_slots = priority.max_concurrent_support - priority.active_requests;
            let to_request = squad_threats.iter().take(remaining_slots);

            for (squad_uuid, threat_level) in to_request {
                let leader = self.battle_state.soldier(self.battle_state.squad(*squad_uuid).leader());
                let target_info = self.get_engagement_target_info(squad_uuid);
                if let Some((target_position, target_squad)) = target_info {
                    let urgency = if *threat_level >= 0.8 {
                        SupportUrgency::Critical
                    } else if *threat_level >= 0.6 {
                        SupportUrgency::High
                    } else if *threat_level >= 0.3 {
                        SupportUrgency::Medium
                    } else {
                        SupportUrgency::Low
                    };

                    let request = SupportRequest {
                        requester_squad: *squad_uuid,
                        target_position,
                        target_squad,
                        threat_level: *threat_level,
                        created_frame: current_frame,
                        urgency: urgency.clone(),
                        retry_count: 0,
                        last_retry_frame: current_frame,
                        requester_ammo_state: leader.determine_ammo_state(),
                        requester_under_fire: *leader.under_fire().value(),
                    };

                    let ammo_state = request.requester_ammo_state.clone();
                    self.support_requests.insert(*squad_uuid, request);

                    if current_frame % 1800 == 0 {
                        println!(
                            "[중대 지원] 중대 {} 분대 {} 지원 요청 생성 (위협도: {:.2}, 긴급도: {:?}, 탄약: {:?})",
                            priority.company_name,
                            squad_uuid.0,
                            threat_level,
                            urgency,
                            ammo_state
                        );
                    }
                }
            }
        }

        messages
    }

    fn process_support_requests_with_company_priority(
        &mut self,
        current_frame: u64,
        company_priorities: &[CompanySupportPriority]
    ) -> Vec<RunnerMessage> {
        let mut messages = vec![];

        for priority in company_priorities {
            let mut assigned_in_company = 0;
            for (_, assignment) in &self.support_assignments {
                if priority.squads.contains(&assignment.supporter_squad) {
                    assigned_in_company += 1;
                }
            }

            if assigned_in_company >= priority.max_concurrent_support {
                continue;
            }

            let mut pending_requests: Vec<(SquadUuid, SupportRequest)> = vec![];
            for (squad_uuid, request) in &self.support_requests {
                if priority.squads.contains(squad_uuid) {
                    if !self.support_assignments.contains_key(squad_uuid) {
                        pending_requests.push((*squad_uuid, request.clone()));
                    }
                }
            }

            pending_requests.sort_by(|a, b| {
                let urgency_order = |u: &SupportUrgency| match u {
                    SupportUrgency::Critical => 4,
                    SupportUrgency::High => 3,
                    SupportUrgency::Medium => 2,
                    SupportUrgency::Low => 1,
                };
                let a_score = urgency_order(&a.1.urgency) * 100 + (a.1.threat_level * 100.0) as i32;
                let b_score = urgency_order(&b.1.urgency) * 100 + (b.1.threat_level * 100.0) as i32;
                b_score.cmp(&a_score)
            });

            let mut available_supporters: Vec<(SquadUuid, f32)> = vec![];

            for squad_uuid in &priority.squads {
                let squad = self.battle_state.squad(*squad_uuid);
                let leader = self.battle_state.soldier(squad.leader());

                if !leader.alive() {
                    continue;
                }

                if self.active_support_squads.contains(squad_uuid) {
                    continue;
                }

                if let Some(cooldown_frame) = self.support_cooldown.get(squad_uuid) {
                    if current_frame - cooldown_frame < 1800 {
                        continue;
                    }
                }

                if matches!(
                    leader.behavior(),
                    battle_core::behavior::Behavior::EngageSoldier(_)
                        | battle_core::behavior::Behavior::SuppressFire(_)
                ) {
                    continue;
                }

                if self.support_cooldown.contains_key(squad_uuid) {
                    continue;
                }

                let mut score = 0.0;
                
                let stress = *leader.under_fire().value() as f32 / 200.0;
                score += (1.0 - stress) * 0.3;

                let total_members = squad.members().len() as f32;
                let alive_members = squad.members().iter()
                    .filter(|&&idx| self.battle_state.soldier(idx).alive())
                    .count() as f32;
                let survival_score = if total_members > 0.0 {
                    alive_members / total_members
                } else {
                    0.0
                };
                score += survival_score * 0.3;

                let mut min_dist_to_request = std::f32::MAX;
                for (req_squad, _) in &pending_requests {
                    let requester = self.battle_state.squad(*req_squad);
                    let requester_leader = self.battle_state.soldier(requester.leader());
                    let dist = distance_between_points(
                        &requester_leader.world_point(),
                        &leader.world_point()
                    );
                    if (dist.meters() as f32) < min_dist_to_request {
                        min_dist_to_request = dist.meters() as f32;
                    }
                }
                if min_dist_to_request < std::f32::MAX {
                    let dist_score = 1.0 - (min_dist_to_request / 200.0).min(1.0);
                    score += dist_score * 0.4;
                }

                let ammo_state = leader.determine_ammo_state();
                let ammo_score = match ammo_state {
                    AmmoState::CombatReady => 0.3,
                    AmmoState::LowAmmo => 0.1,
                    _ => 0.0,
                };
                score += ammo_score;

                if score > 0.2 {
                    available_supporters.push((*squad_uuid, score));
                }
            }

            available_supporters.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            for (requester_squad, request) in pending_requests {
                if assigned_in_company >= priority.max_concurrent_support {
                    break;
                }

                let mut assigned = false;
                for (supporter_squad, _) in &available_supporters {
                    if self.active_support_squads.contains(supporter_squad) {
                        continue;
                    }

                    if self.support_cooldown.contains_key(supporter_squad) {
                        continue;
                    }

                    let assignment = SupportAssignment {
                        supporter_squad: *supporter_squad,
                        target_request: request.clone(),
                        assigned_frame: current_frame,
                        status: SupportAssignmentStatus::Moving,
                    };

                    self.support_assignments.insert(requester_squad, assignment);
                    self.active_support_squads.insert(*supporter_squad);
                    assigned_in_company += 1;
                    assigned = true;

                    let safe_support_position = self.calculate_safe_support_position(
                        &self.battle_state.soldier(
                            self.battle_state.squad(*supporter_squad).leader()
                        ).world_point(),
                        &request.target_position
                    );

                    messages.extend(self.issue_safe_support_order(*supporter_squad, &request, safe_support_position, current_frame));

                    if current_frame % 1800 == 0 {
                        println!(
                            "[중대 지원] 중대 {} 내 분대 {} -> 분대 {} 지원 할당 (중대 내 {}번째 할당, 긴급도: {:?})",
                            priority.company_name,
                            supporter_squad.0,
                            requester_squad.0,
                            assigned_in_company,
                            request.urgency
                        );
                    }

                    break;
                }

                if !assigned {
                    if current_frame % 1800 == 0 {
                        println!(
                            "[중대 지원] 중대 {} 내 분대 {} 지원할 분대를 찾지 못함",
                            priority.company_name,
                            requester_squad.0
                        );
                    }
                }
            }
        }

        messages
    }

    fn calculate_safe_support_position(&self, from_point: &WorldPoint, target_point: &WorldPoint) -> WorldPoint {
        let from_vec = from_point.to_vec2();
        let target_vec = target_point.to_vec2();

        let dir = (target_vec - from_vec).normalize();
        let mid_point = from_vec + dir * 30.0;

        let perp = glam::Vec2::new(-dir.y, dir.x);
        let side = if (from_point.x as i32 + from_point.y as i32) % 2 == 0 { 1.0 } else { -1.0 };
        let mut support_point = mid_point + perp * side * 15.0;

        let map = self.battle_state.map();
        let mut ping_offset = glam::Vec2::new(0.0, 0.0);
        let mut ping_count = 0;

        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
            if ping_side != self.battle_state.soldier(
                self.battle_state.squad(SquadUuid(0)).leader()
            ).side() {
                let ping_world = map.world_point_from_grid_point(*ping_grid);
                let dist_to_ping = distance_between_points(&WorldPoint::from_vec2(support_point), &ping_world);
                if dist_to_ping.meters() < 50 {
                    let away_dir = (support_point - ping_world.to_vec2()).normalize();
                    ping_offset += away_dir * 20.0;
                    ping_count += 1;
                }
            }
        }

        if ping_count > 0 {
            support_point += ping_offset / ping_count as f32;
        }

        let map_width = map.visual_width() as f32;
        let map_height = map.visual_height() as f32;
        WorldPoint::from_vec2(glam::Vec2::new(
            support_point.x.clamp(10.0, map_width - 10.0),
            support_point.y.clamp(10.0, map_height - 10.0),
        ))
    }

    fn issue_safe_support_order(&mut self, supporter_squad: SquadUuid, request: &SupportRequest, safe_position: WorldPoint, current_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        let squad = self.battle_state.squad(supporter_squad);
        let leader = self.battle_state.soldier(squad.leader());

        if !leader.alive() {
            return messages;
        }

        let remaining_ammo = leader.support_ammo_remaining();
        if remaining_ammo <= 0 {
            if current_frame % 600 == 0 {
                println!(
                    "[탄약 관리] 분대 {} 지원 탄약 고갈 (잔량: {}), 지원 중단",
                    supporter_squad.0,
                    remaining_ammo
                );
            }
            
            if let Some(assignment) = self.support_assignments.values_mut()
                .find(|a| a.supporter_squad == supporter_squad) {
                assignment.status = SupportAssignmentStatus::Cancelled;
            }
            self.active_support_squads.remove(&supporter_squad);
            
            if let Some(cp) = self.checkpoints.read().unwrap().get(&supporter_squad) {
                let map = self.battle_state.map();
                let from_grid = map.grid_point_from_world_point(&leader.world_point());
                let to_grid = map.grid_point_from_world_point(cp);
                
                if let Some(grid_path) = battle_core::physics::path::find_path(
                    &self.config,
                    map,
                    &from_grid,
                    &to_grid,
                    true,
                    &battle_core::physics::path::PathMode::Walk,
                    &None,
                ) {
                    let world_path = grid_path
                        .iter()
                        .map(|p| map.world_point_from_grid_point(*p))
                        .collect();
                    let world_paths = battle_core::types::WorldPaths::new(vec![
                        battle_core::types::WorldPath::new(world_path)
                    ]);
                    
                    messages.push(RunnerMessage::BattleState(
                        BattleStateMessage::Soldier(
                            squad.leader(),
                            SoldierMessage::SetOrder(Order::MoveFastTo(world_paths, Some(Box::new(Order::Idle)))),
                        )
                    ));
                }
            }
            
            return messages;
        }

        let ammo_cost_per_shot = self.calculate_support_ammo_cost(leader);
        let estimated_shots = ((remaining_ammo as u32) / ammo_cost_per_shot).min(5);
        
        if estimated_shots == 0 {
            if let Some(assignment) = self.support_assignments.values_mut()
                .find(|a| a.supporter_squad == supporter_squad) {
                assignment.status = SupportAssignmentStatus::Cancelled;
            }
            self.active_support_squads.remove(&supporter_squad);
            return messages;
        }

        let ammo_factor = (remaining_ammo as f32 / leader.support_ammo_limit() as f32).min(1.0);
        
        if ammo_factor < 0.3 {
            if let Some(req) = self.support_requests.get_mut(&supporter_squad) {
                if req.urgency == SupportUrgency::Critical {
                    req.urgency = SupportUrgency::High;
                } else if req.urgency == SupportUrgency::High {
                    req.urgency = SupportUrgency::Medium;
                } else if req.urgency == SupportUrgency::Medium {
                    req.urgency = SupportUrgency::Low;
                }
            }
            
            if current_frame % 600 == 0 {
                println!(
                    "[탄약 관리] 분대 {} 탄약 부족 ({:.0}%), 지원 긴급도 하향 조정",
                    supporter_squad.0,
                    ammo_factor * 100.0
                );
            }
        }

        let map = self.battle_state.map();
        let from_grid = map.grid_point_from_world_point(&leader.world_point());
        let to_grid = map.grid_point_from_world_point(&safe_position);

        if from_grid == to_grid {
            let order = Order::SuppressFire(request.target_position);
            messages.push(RunnerMessage::BattleState(
                BattleStateMessage::Soldier(
                    squad.leader(),
                    SoldierMessage::SetOrder(order),
                )
            ));
            return messages;
        }

        let path_mode = battle_core::physics::path::PathMode::Walk;
        let start_dir = Some(battle_core::physics::path::Direction::from_angle(&leader.get_looking_direction()));

        if let Some(grid_path) = battle_core::physics::path::find_stealth_path(
            &self.config,
            map,
            &from_grid,
            &to_grid,
            true,
            &path_mode,
            &start_dir,
        ) {
            let world_path = grid_path
                .iter()
                .map(|p| map.world_point_from_grid_point(*p))
                .collect();

            let world_paths = battle_core::types::WorldPaths::new(vec![
                battle_core::types::WorldPath::new(world_path)
            ]);

            let then_order = Order::SuppressFire(request.target_position);
            let order = Order::SneakTo(world_paths.clone(), Some(Box::new(then_order.clone())));

            messages.push(RunnerMessage::BattleState(
                BattleStateMessage::Soldier(
                    squad.leader(),
                    SoldierMessage::SetOrder(order),
                )
            ));

            for member_idx in squad.members() {
                if *member_idx != squad.leader() {
                    let member = self.battle_state.soldier(*member_idx);
                    if member.alive() {
                        messages.push(RunnerMessage::BattleState(
                            BattleStateMessage::Soldier(
                                *member_idx,
                                SoldierMessage::SetOrder(Order::SneakTo(
                                    world_paths.clone(),
                                    Some(Box::new(Order::SuppressFire(request.target_position).clone()))
                                )),
                            )
                        ));
                    }
                }
            }
        } else {
            let order = Order::SuppressFire(request.target_position);
            messages.push(RunnerMessage::BattleState(
                BattleStateMessage::Soldier(
                    squad.leader(),
                    SoldierMessage::SetOrder(order),
                )
            ));
        }

        messages
    }

    fn calculate_support_ammo_cost(&self, soldier: &battle_core::entity::soldier::Soldier) -> u32 {
        if let Some(weapon) = soldier.main_weapon() {
            match weapon {
                Weapon::MosinNagantM1924(_, _) | Weapon::MauserG41(_, _) => 1,
                Weapon::BrenMark2(_) => 3,
                Weapon::Mg34(_) => 5,
            }
        } else {
            0
        }
    }

    fn scan_and_request_support(&mut self, side: battle_core::game::Side, current_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];

        for (squad_uuid, squad) in self.battle_state.squads() {
            let leader = self.battle_state.soldier(squad.leader());
            
            if leader.side() != &side {
                continue;
            }

            if !leader.alive() {
                continue;
            }

            if self.support_requests.contains_key(squad_uuid) {
                continue;
            }

            if self.active_support_squads.contains(squad_uuid) {
                continue;
            }

            if self.support_cooldown.contains_key(squad_uuid) {
                continue;
            }

            let is_engaging = matches!(
                leader.behavior(),
                battle_core::behavior::Behavior::EngageSoldier(_)
                    | battle_core::behavior::Behavior::SuppressFire(_)
            );

            let is_suppressing = matches!(
                leader.behavior(),
                battle_core::behavior::Behavior::SuppressFire(_)
            );

            if !is_engaging {
                continue;
            }

            let target_info = self.get_engagement_target_info(squad_uuid);
            if target_info.is_none() {
                continue;
            }

            let (target_position, target_squad) = target_info.unwrap();

            let engagement_duration = current_frame.saturating_sub(*leader.last_shoot_frame_i());

            let ammo_state = leader.determine_ammo_state();
            let ammo_factor = match ammo_state {
                AmmoState::CriticalAmmo => 0.8,
                AmmoState::LowAmmo => 0.3,
                _ => 0.0,
            };

            let base_threat = self.calculate_threat_level(squad_uuid, leader, engagement_duration);
            let threat_level = (base_threat + ammo_factor).min(1.0);

            if threat_level < 0.15 {
                continue;
            }

            let urgency = if threat_level >= 0.8 {
                SupportUrgency::Critical
            } else if threat_level >= 0.6 {
                SupportUrgency::High
            } else if threat_level >= 0.3 {
                SupportUrgency::Medium
            } else {
                SupportUrgency::Low
            };

            if self.support_requests.contains_key(squad_uuid) {
                continue;
            }

            let request = SupportRequest {
                requester_squad: *squad_uuid,
                target_position,
                target_squad,
                threat_level,
                created_frame: current_frame,
                urgency: urgency.clone(),
                retry_count: 0,
                last_retry_frame: current_frame,
                requester_ammo_state: ammo_state,
                requester_under_fire: *leader.under_fire().value(),
            };

            self.support_requests.insert(*squad_uuid, request);

            if current_frame % 1800 == 0 {
                println!(
                    "[지원 사격망] 분대 {} ({} 진영) 지원 요청 생성 (위협도: {:.2}, 긴급도: {:?}, 탄약: {:?})",
                    squad_uuid.0,
                    side,
                    threat_level,
                    urgency,
                    ammo_state
                );
            }
        }

        messages
    }

    fn get_engagement_target_info(&self, squad_uuid: &SquadUuid) -> Option<(WorldPoint, Option<SquadUuid>)> {
        let squad = self.battle_state.squad(*squad_uuid);
        let leader = self.battle_state.soldier(squad.leader());

        match leader.behavior() {
            battle_core::behavior::Behavior::EngageSoldier(target_idx) => {
                let target = self.battle_state.soldier(*target_idx);
                Some((target.world_point(), Some(target.squad_uuid())))
            }
            battle_core::behavior::Behavior::SuppressFire(point) => {
                Some((*point, None))
            }
            _ => None,
        }
    }

    fn calculate_threat_level(&self, squad_uuid: &SquadUuid, leader: &battle_core::entity::soldier::Soldier, engagement_duration: u64) -> f32 {
        let mut threat = 0.0;

        let stress = *leader.under_fire().value() as f32 / 200.0;
        threat += stress * 0.4;

        let duration_factor = (engagement_duration as f32 / 1800.0).min(1.0);
        threat += duration_factor * 0.3;

        let squad = self.battle_state.squad(*squad_uuid);
        let total_members = squad.members().len() as f32;
        let alive_members = squad.members().iter()
            .filter(|&&idx| self.battle_state.soldier(idx).alive())
            .count() as f32;
        let casualty_rate = if total_members > 0.0 {
            1.0 - (alive_members / total_members)
        } else {
            0.0
        };
        threat += casualty_rate * 0.2;

        let mut enemy_count = 0.0;
        for enemy in self.battle_state.soldiers() {
            if *enemy.side() == leader.side().opposite() && enemy.alive() {
                let dist = distance_between_points(&leader.world_point(), &enemy.world_point());
                if dist.meters() < 50 {
                    enemy_count += 1.0;
                }
            }
        }
        let enemy_count_f32 = enemy_count;
        let threat_add = enemy_count_f32 / 10.0;
        threat += f32::min(threat_add, 0.1);

        let ammo_state = leader.determine_ammo_state();
        match ammo_state {
            AmmoState::CriticalAmmo => threat += 0.3,
            AmmoState::LowAmmo => threat += 0.1,
            _ => {}
        }

        threat.min(1.0)
    }

    fn process_support_requests(&mut self, current_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];

        let mut requests: Vec<(SquadUuid, SupportRequest)> = self.support_requests
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();

        requests.sort_by(|a, b| {
            let urgency_order = |u: &SupportUrgency| match u {
                SupportUrgency::Critical => 4,
                SupportUrgency::High => 3,
                SupportUrgency::Medium => 2,
                SupportUrgency::Low => 1,
            };
            let a_score = urgency_order(&a.1.urgency) * 100 + (a.1.threat_level * 100.0) as i32;
            let b_score = urgency_order(&b.1.urgency) * 100 + (b.1.threat_level * 100.0) as i32;
            b_score.cmp(&a_score)
        });

        for (requester_squad, request) in requests {
            if self.support_assignments.contains_key(&requester_squad) {
                continue;
            }

            let supporter = self.find_available_support_squad(&requester_squad, current_frame);

            if let Some(supporter_squad) = supporter {
                let assignment = SupportAssignment {
                    supporter_squad,
                    target_request: request.clone(),
                    assigned_frame: current_frame,
                    status: SupportAssignmentStatus::Moving,
                };

                self.support_assignments.insert(requester_squad, assignment);
                self.active_support_squads.insert(supporter_squad);

                let supporter_leader = self.battle_state.soldier(
                    self.battle_state.squad(supporter_squad).leader()
                );
                let safe_position = self.calculate_safe_support_position(
                    &supporter_leader.world_point(),
                    &request.target_position
                );

                messages.extend(self.issue_safe_support_order(supporter_squad, &request, safe_position, current_frame));

                if current_frame % 1800 == 0 {
                    println!(
                        "[지원 사격망] 분대 {} -> 분대 {} 지원 할당 완료 (긴급도: {:?})",
                        supporter_squad.0,
                        requester_squad.0,
                        request.urgency
                    );
                }
            }
        }

        messages
    }

    fn find_available_support_squad(&mut self, requester_squad: &SquadUuid, current_frame: u64) -> Option<SquadUuid> {
        let requester = self.battle_state.squad(*requester_squad);
        let requester_leader = self.battle_state.soldier(requester.leader());
        let requester_side = requester_leader.side();

        let mut candidates: Vec<(SquadUuid, f32, f32)> = vec![];

        for (squad_uuid, squad) in self.battle_state.squads() {
            if squad_uuid == requester_squad {
                continue;
            }

            let leader = self.battle_state.soldier(squad.leader());

            if leader.side() != requester_side {
                continue;
            }

            if !leader.alive() {
                continue;
            }

            if self.active_support_squads.contains(squad_uuid) {
                continue;
            }

            if self.support_cooldown.contains_key(squad_uuid) {
                continue;
            }

            if let Some(cooldown_frame) = self.support_cooldown.get(squad_uuid) {
                if current_frame - cooldown_frame < 1800 {
                    continue;
                }
            }

            if matches!(
                leader.behavior(),
                battle_core::behavior::Behavior::EngageSoldier(_)
                    | battle_core::behavior::Behavior::SuppressFire(_)
            ) {
                continue;
            }

            let dist = distance_between_points(
                &requester_leader.world_point(),
                &leader.world_point()
            );

            if dist.meters() > 200 {
                continue;
            }

            let supporter_threat = *leader.under_fire().value() as f32 / 200.0;
            if supporter_threat > 0.7 {
                continue;
            }

            let ammo_state = leader.determine_ammo_state();
            let ammo_score = match ammo_state {
                AmmoState::CombatReady => 0.3,
                AmmoState::LowAmmo => 0.1,
                _ => 0.0,
            };

            let distance_score = 1.0 - (dist.meters() as f32 / 200.0);

            let stress = *leader.under_fire().value() as f32 / 200.0;
            let stress_score = 1.0 - stress;

            let total_members = squad.members().len() as f32;
            let alive_members = squad.members().iter()
                .filter(|&&idx| self.battle_state.soldier(idx).alive())
                .count() as f32;
            let survival_score = if total_members > 0.0 {
                alive_members / total_members
            } else {
                0.0
            };

            let total_score = distance_score * 0.3 + stress_score * 0.25 + survival_score * 0.25 + ammo_score * 0.2;

            if total_score > 0.2 {
                candidates.push((*squad_uuid, total_score, dist.meters() as f32));
            }
        }

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        candidates.first().map(|(squad_uuid, _, _)| *squad_uuid)
    }

    fn issue_support_order(&mut self, supporter_squad: SquadUuid, request: &SupportRequest, current_frame: u64) -> Vec<RunnerMessage> {
        let leader = self.battle_state.soldier(
            self.battle_state.squad(supporter_squad).leader()
        );
        let safe_position = self.calculate_safe_support_position(
            &leader.world_point(),
            &request.target_position
        );
        self.issue_safe_support_order(supporter_squad, request, safe_position, current_frame)
    }

    fn calculate_support_position(&self, from_point: &WorldPoint, target_point: &WorldPoint) -> WorldPoint {
        self.calculate_safe_support_position(from_point, target_point)
    }

    fn update_supporting_squads(&mut self, current_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        let mut completed_assignments = vec![];

        let assignment_keys: Vec<SquadUuid> = self.support_assignments.keys().cloned().collect();

        for requester_squad in assignment_keys {
            let (supporter_squad, assigned_frame) = match self.support_assignments.get(&requester_squad) {
                Some(a) => (a.supporter_squad, a.assigned_frame),
                None => continue,
            };
            
            let supporter = self.battle_state.soldier(
                self.battle_state.squad(supporter_squad).leader()
            );

            if !supporter.alive() {
                if let Some(assignment) = self.support_assignments.get_mut(&requester_squad) {
                    assignment.status = SupportAssignmentStatus::Cancelled;
                }
                self.active_support_squads.remove(&supporter_squad);
                self.support_cooldown.insert(supporter_squad, current_frame);
                completed_assignments.push(requester_squad);
                continue;
            }

            if self.support_cooldown.contains_key(&supporter_squad) {
                continue;
            }

            let ammo_check_interval = 1800;
            if current_frame - supporter.last_ammo_check_frame() >= ammo_check_interval {
                let actual_ammo = self.calculate_actual_support_ammo(supporter);
                if actual_ammo < supporter.support_ammo_remaining() as u32 {
                    messages.push(RunnerMessage::BattleState(
                        BattleStateMessage::Soldier(
                            supporter.uuid(),
                            SoldierMessage::SetSupportAmmoUsed(
                                supporter.support_ammo_limit() - actual_ammo
                            ),
                        )
                    ));
                }
                
                messages.push(RunnerMessage::BattleState(
                    BattleStateMessage::Soldier(
                        supporter.uuid(),
                        SoldierMessage::SetLastAmmoCheckFrame(current_frame),
                    )
                ));
            }

            let remaining_ammo = supporter.support_ammo_remaining();
            if remaining_ammo <= 0 {
                if current_frame % 600 == 0 {
                    println!(
                        "[탄약 관리] 분대 {} 지원 탄약 고갈, 지원 중단",
                        supporter_squad.0
                    );
                }
                
                if let Some(assignment) = self.support_assignments.get_mut(&requester_squad) {
                    assignment.status = SupportAssignmentStatus::Cancelled;
                }
                self.active_support_squads.remove(&supporter_squad);
                self.support_cooldown.insert(supporter_squad, current_frame);
                completed_assignments.push(requester_squad);
                
                if let Some(cp) = self.checkpoints.read().unwrap().get(&supporter_squad) {
                    let map = self.battle_state.map();
                    let from_grid = map.grid_point_from_world_point(&supporter.world_point());
                    let to_grid = map.grid_point_from_world_point(cp);
                    
                    if let Some(grid_path) = battle_core::physics::path::find_path(
                        &self.config,
                        map,
                        &from_grid,
                        &to_grid,
                        true,
                        &battle_core::physics::path::PathMode::Walk,
                        &None,
                    ) {
                        let world_path = grid_path
                            .iter()
                            .map(|p| map.world_point_from_grid_point(*p))
                            .collect();
                        let world_paths = battle_core::types::WorldPaths::new(vec![
                            battle_core::types::WorldPath::new(world_path)
                        ]);
                        
                        messages.push(RunnerMessage::BattleState(
                            BattleStateMessage::Soldier(
                                supporter.uuid(),
                                SoldierMessage::SetOrder(Order::MoveFastTo(world_paths, Some(Box::new(Order::Idle)))),
                            )
                        ));
                    }
                }
                continue;
            }

            let requester = self.battle_state.soldier(
                self.battle_state.squad(requester_squad).leader()
            );

            if !requester.alive() {
                if let Some(assignment) = self.support_assignments.get_mut(&requester_squad) {
                    assignment.status = SupportAssignmentStatus::Completed;
                }
                self.active_support_squads.remove(&supporter_squad);
                self.support_cooldown.insert(supporter_squad, current_frame);
                completed_assignments.push(requester_squad);
                continue;
            }

            if !matches!(
                requester.behavior(),
                battle_core::behavior::Behavior::EngageSoldier(_)
                    | battle_core::behavior::Behavior::SuppressFire(_)
            ) {
                if let Some(assignment) = self.support_assignments.get_mut(&requester_squad) {
                    assignment.status = SupportAssignmentStatus::Completed;
                }
                self.active_support_squads.remove(&supporter_squad);
                self.support_cooldown.insert(supporter_squad, current_frame);
                completed_assignments.push(requester_squad);
                continue;
            }

            let mut new_status = None;

            if matches!(
                supporter.behavior(),
                battle_core::behavior::Behavior::SuppressFire(_)
            ) {
                new_status = Some(SupportAssignmentStatus::Suppressing);
                
                let squad = self.battle_state.squad(supporter_squad);
                let mut total_ammo = 0;
                for member_idx in squad.members() {
                    let member = self.battle_state.soldier(*member_idx);
                    if member.alive() {
                        total_ammo += member.support_ammo_remaining();
                    }
                }
                
                if total_ammo < 10 {
                    println!(
                        "[탄약 관리] 분대 {} 전체 탄약 부족 (총 {}발), 지원 중단",
                        supporter_squad.0,
                        total_ammo
                    );
                    new_status = Some(SupportAssignmentStatus::Cancelled);
                    self.active_support_squads.remove(&supporter_squad);
                    completed_assignments.push(requester_squad);
                }
            } else if matches!(
                supporter.behavior(),
                battle_core::behavior::Behavior::EngageSoldier(_)
            ) {
                new_status = Some(SupportAssignmentStatus::Engaged);
            } else if matches!(
                supporter.behavior(),
                battle_core::behavior::Behavior::MoveFastTo(_)
                    | battle_core::behavior::Behavior::MoveTo(_)
                    | battle_core::behavior::Behavior::SneakTo(_)
            ) {
                new_status = Some(SupportAssignmentStatus::Moving);
            }

            if current_frame - assigned_frame > 3600 {
                new_status = Some(SupportAssignmentStatus::Completed);
                self.active_support_squads.remove(&supporter_squad);
                self.support_cooldown.insert(supporter_squad, current_frame);
                if !completed_assignments.contains(&requester_squad) {
                    completed_assignments.push(requester_squad);
                }
            }

            if let Some(status) = new_status {
                if let Some(assignment) = self.support_assignments.get_mut(&requester_squad) {
                    assignment.status = status;
                }
            }
        }

        for requester in completed_assignments {
            self.support_assignments.remove(&requester);
            self.support_requests.remove(&requester);
        }

        messages
    }

    fn calculate_actual_support_ammo(&self, soldier: &battle_core::entity::soldier::Soldier) -> u32 {
        let mut total_ammo = 0;
        
        if let Some(weapon) = soldier.main_weapon() {
            if weapon.can_fire() {
                total_ammo += match weapon {
                    Weapon::MosinNagantM1924(_, _) | Weapon::MauserG41(_, _) => 1,
                    Weapon::BrenMark2(mag) => mag.as_ref().map(|m: &battle_core::game::weapon::Magazine| m.count() as u32).unwrap_or(0),
                    Weapon::Mg34(mag) => mag.as_ref().map(|m: &battle_core::game::weapon::Magazine| m.count() as u32).unwrap_or(0),
                };
            }
            
            for mag in soldier.magazines() {
                if weapon.accepted_magazine(mag) {
                    total_ammo += mag.count() as u32;
                }
            }
        }
        
        total_ammo
    }
}