use battle_core::{
    behavior::{Behavior, BehaviorMode, BehaviorPropagation, Body},
    entity::soldier::Soldier,
    game::flag::FlagOwnership,
    order::Order,
    physics::path::{find_path, find_tactical_path, Direction, PathMode},
    state::{
        battle::message::{BattleStateMessage, SoldierMessage},
        client::ClientStateMessage,
    },
    types::{Angle, SquadUuid, WorldPath, WorldPaths, WorldPoint},
    utils::NewDebugPoint,
};

use super::{fight::choose::ChooseMethod, message::RunnerMessage, Runner};

mod blast;
mod bullet;
mod death;
mod defend;
mod engage;
mod moves;
mod suppress;

impl Runner {
    pub fn soldier_behavior(&self, soldier: &Soldier) -> Vec<RunnerMessage> {
        puffin::profile_scope!("soldier_behavior");
        let mut messages = vec![];
        let soldier = self.battle_state.soldier(soldier.uuid());

        // [사망자 좀비 방지] 사망한 유닛은 더 이상 어떠한 행동(Behavior)도 재계산하거나 명령을 덮어쓰지 않습니다.
        if !soldier.alive() {
            return messages;
        }

        // [기획 반영: 최초 스폰 위치를 체크포인트로 영구 고정]
        // 매번 플래그 이동 시 갱신하는 것이 아니라, 최초에 배정된 안전한 스폰(출발) 위치를 최우선 체크포인트로 기록합니다.
        if !self.checkpoints.read().unwrap().contains_key(&soldier.squad_uuid()) {
            self.checkpoints.write().unwrap().insert(soldier.squad_uuid(), soldier.world_point());
        }

        let mut current_order = soldier.order();
        #[allow(unused_assignments)]
        let mut temp_order_storage = None;

        let is_side_a = soldier.side() == &battle_core::game::Side::A;
        let is_side_b = soldier.side() == &battle_core::game::Side::B;
        let yolo_active = (is_side_a && self.config.yolo_mode_a) || (is_side_b && self.config.yolo_mode_b);

        let mut is_scout = false;
        for comp in self.companies.values() {
            if comp.scout_squad == Some(soldier.squad_uuid()) {
                is_scout = true;
                break;
            }
        }

        // [YOLO Mode Auto-Awake & Auto-Unhide] 
        // 폭발로 인해 강제 엄폐(Hide)되었거나, 게임 시작 시 배치(Placement)로 인해 방어(Defend) 중인 봇들을 깨웁니다.
        // YOLO 모드이거나 정찰조(Scout)라면, 안전한 상태일 때 즉각 기동(Idle) 상태로 전환하여 자동 진격 및 로테이션 알고리즘을 태웁니다.
        let should_awake = if yolo_active || is_scout {
            matches!(current_order, Order::Defend(_) | Order::Hide(_))
        } else {
            matches!(current_order, Order::Hide(_))
        };

        if should_awake {
            // 전술적 보정: 길찾기 실패(Trapped)로 인해 지원 대기 타이머(*soldier.support_promise_end_frame_i() > 0)가 작동 중일 때는 무조건적인 숨기 해제를 차단하여 연산 폭주를 막습니다.
            if !soldier.under_fire().is_danger() && !soldier.under_fire().is_max() && *soldier.support_promise_end_frame_i() == 0 {
                temp_order_storage = Some(Order::Idle);
                current_order = temp_order_storage.as_ref().unwrap();
                
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetOrder(Order::Idle),
                )));
            }
        }

        // [방어 진지 우선순위 반영] 방어(Defend)나 은엄폐(Hide) 상태로 진지를 사수 중일 때는 패닉 이탈과 수류탄 난사를 억제하고 사격망 유지에 집중합니다.
        let is_defending = matches!(current_order, Order::Defend(_) | Order::Hide(_));

        // [진지 돌파 위기 감지 (Breach Detection)]
        // 방어 중이더라도 적이 10m 이내로 초근접하여 진지가 돌파당할 위기라면 최후의 수단으로 수류탄 투척을 허용합니다.
        let mut defense_breached = false;
        if is_defending {
            for enemy in self.battle_state.soldiers() {
                if enemy.side() != soldier.side() && enemy.alive() {
                    let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &enemy.world_point()).meters();
                    if dist <= 10 {
                        defense_breached = true;
                        break;
                    }
                }
            }
        }

        // [Panic Defensive Grenade & Off-map Retreat]
        // 경직(Danger/Max) 상태일 때, 적의 사격 발원지(Ping) 방향으로 대충 수류탄을 던져 적을 억제하고 즉시 맵 밖으로 도주(OffMapTransit)합니다.
        // 골대 넣듯 정확한 타겟팅이 아니라, 공포에 질려 대략적인 방향의 중간 지점에 맹목적으로 던지는 현실적인 억제(Suppression) 투척입니다.
        // [추가 개선] 척후조(Scout) 분대원일 경우, 앞선 포인트맨이 공격받거나 전사하여 전술 핑(위험)이 반경 35m 내에 발생하면
        // 본인이 직접 피격당하지 않았더라도 엎드려 후퇴(OffMapTransit)하는 패닉 로직을 발동합니다.

        let map = self.battle_state.map();
        let mut nearest_ping = None;
        let mut min_dist = std::f32::MAX;

        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
            if ping_side != soldier.side() {
                let ping_world = map.world_point_from_grid_point(*ping_grid);
                let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &ping_world).meters() as f32;
                // [수류탄 락 해제] 거리가 멀어도 해당 사로 방향을 차단하기 위해 거리 제한(35m)을 해제하고 가장 가까운 위협 방향을 찾습니다.
                if dist < min_dist { 
                    min_dist = dist;
                    nearest_ping = Some(ping_world);
                }
            }
        }

        // [버그 수정] 315.0 오타를 35.0(35m)으로 정상화하여 맵 전체 사격음에 반응하는 무한 패닉을 차단합니다.
        let pointman_under_attack = is_scout && min_dist <= 35.0;

        // [Part 4: 정찰조와 본대의 상호작용 및 지원 사격망 연계]
        // 정찰조가 적을 마주쳐 패닉 후퇴/엄폐(Hide)하려 할 때, 뒤에서 본대가 지원 사격을 해주고 있다면
        // 안도감을 느끼고 후퇴 스위치를 차단하여 계속 전선을 유지하며 싸우도록 합니다.
        let mut has_support_fire = false;
        if is_scout {
            for ally in self.battle_state.soldiers() {
                if ally.side() == soldier.side() && ally.alive() && ally.uuid() != soldier.uuid() {
                    let dist_to_ally = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &ally.world_point()).meters();
                    // 후방 60m 이내 스캔
                    if dist_to_ally <= 60 {
                        let mut ally_is_scout = false;
                        for comp in self.companies.values() {
                            if comp.scout_squad == Some(ally.squad_uuid()) {
                                ally_is_scout = true;
                                break;
                            }
                        }
                        
                        // 아군이 본대(Main) 소속이며, 현재 교전(EngageSoldier) 또는 제압사격(SuppressFire) 중일 때
                        if !ally_is_scout && matches!(ally.behavior(), Behavior::SuppressFire(_) | Behavior::EngageSoldier(_)) {
                            has_support_fire = true;
                            
                            // 지원을 받으면 즉시 안도하여 스트레스를 대폭 깎고 패닉 수치를 안정화시킵니다.
                            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                soldier.uuid(),
                                SoldierMessage::RelieveStress(100),
                            )));
                            break;
                        }
                    }
                }
            }
        }

        // 진지를 사수하는 방어군(is_defending)이 아닐 때만 수류탄을 까넣고 엄폐합니다.
        // 단, 진지가 돌파당할 위기(defense_breached)라면 방어군도 예외적으로 수류탄을 투척합니다.
        // [조건 추가] 아군의 든든한 지원 사격망(has_support_fire)이 없을 때만 패닉 후퇴를 허용합니다.
        if (!is_defending || defense_breached) && !has_support_fire && (soldier.under_fire().is_danger() || soldier.under_fire().is_max() || pointman_under_attack) {
            if let Some(target_pos) = nearest_ping {
                // 1. 수류탄이 있으면 적 방향으로 투척 (거리 2.5배 상향)
                if soldier.grenades() > 0 && *self.battle_state.frame_i() > soldier.last_grenade_frame_i() + 120 {
                    // NaN 방지 및 안전한 방향 계산
                    let dir = if (target_pos.to_vec2() - soldier.world_point().to_vec2()).length() > 0.1 {
                        (target_pos.to_vec2() - soldier.world_point().to_vec2()).normalize()
                    } else {
                        glam::Vec2::new(1.0, 0.0)
                    };
                    
                    // 난사 효과: 방향 각도를 랜덤하게 비틀고(±약 25도)
                    let mut rng = rand::thread_rng();
                    let inaccurate_angle: f32 = rand::Rng::gen_range(&mut rng, -0.4..0.4); 
                    let cos_t = inaccurate_angle.cos();
                    let sin_t = inaccurate_angle.sin();
                    let blind_dir = glam::Vec2::new(
                        dir.x * cos_t - dir.y * sin_t,
                        dir.x * sin_t + dir.y * cos_t
                    );
                    
                    // [투척 거리 2.5배 상향] 기존 60~85에서 150~212.5 픽셀(약 45~63m) 지점으로 대폭 늘려 먼 거리의 적을 사전에 경직/억제시킵니다.
                    let throw_dist = rand::Rng::gen_range(&mut rng, 150.0..212.5); 
                    let blind_target = battle_core::types::WorldPoint::from_vec2(soldier.world_point().to_vec2() + blind_dir * throw_dist);

                    // [기획 반영 2-2] 즉발 폭발 스폰을 제거하고, 1~1.5초(60~90 프레임)의 무기 전환 및 투척 딜레이 상태(Throwing)로 돌입시킵니다.
                    let delay_frames = rand::Rng::gen_range(&mut rng, 60..90);
                    let throw_end_frame = *self.battle_state.frame_i() + delay_frames;

                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                        soldier.uuid(),
                        SoldierMessage::SetGesture(battle_core::behavior::gesture::Gesture::Throwing(throw_end_frame, blind_target))
                    )));
                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                        soldier.uuid(),
                        SoldierMessage::ConsumeGrenade
                    )));
                    // (PushExplosion과 SetLastGrenadeFrameI는 update.rs에서 딜레이 종료 후 처리됩니다)
                }

                // 2. [버그 수정: 정찰조 무한 증발 및 전선 고착화(프리징) 해결]
                // 기존에는 위협을 느끼면 OffMapTransit(맵 밖으로 후퇴) 명령을 내려 20초간 유닛을 아예 삭제시켰습니다.
                // 이로 인해 척후조가 총소리만 나면 맵 밖으로 도주하여 목표로 다가가지 못하는 현상이 발생했습니다.
                // 맵 이탈을 전면 폐지하고, 즉각 바닥에 엎드려(Hide) 사격을 피한 뒤 스트레스가 낮아지면 다시 진격(자동 복구)하도록 교정합니다.
                temp_order_storage = Some(Order::Hide(battle_core::types::Angle(0.0)));
                current_order = temp_order_storage.as_ref().unwrap();

                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetOrder(current_order.clone())
                )));

                let behavior = Behavior::Hide(battle_core::types::Angle(0.0));
                if self.soldier_is_squad_leader(soldier.uuid()) {
                    messages.extend(self.propagate_behavior(soldier, &behavior));
                }
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetBehavior(behavior)
                )));
                
                return messages; // Rust의 반환 타입(Vec<RunnerMessage>) 규격을 준수하며 즉시 함수를 탈출합니다.
            }
        }

        // [CQB Breach & Clear: 건물 진입 전 수류탄 선투척 (Stacking Up)]
        // 실내(Interior) 근처 20m 이내로 접근했을 때, 건물 내부에 적이 있거나 점령 목표(Flag)가 있다면
        // 진입 전 건물 입구에서 멈춰 서서 수류탄을 먼저 창문/문 너머로 까넣어 소탕(Clear)을 시도합니다.
        let soldier_grid_breach = map.grid_point_from_world_point(&soldier.world_point());
        let _current_tile_idx_breach = (soldier_grid_breach.y * map.width() as i32 + soldier_grid_breach.x) as usize;
        let is_indoor_breach = map.interiors().iter().any(|i| {
            soldier.world_point().x >= i.x() && soldier.world_point().x <= i.x() + i.width() &&
            soldier.world_point().y >= i.y() && soldier.world_point().y <= i.y() + i.height()
        });

        // [버그 수정 1: 수류탄 자판기 방지] 분대원 중 단 한 명이라도 최근 20초(1200프레임) 이내에 수류탄을 깠다면
        // 해당 분대는 이미 '소탕 절차'를 밟은 것으로 간주하여 추가 수류탄 낭비를 막고 수류탄을 보존합니다.
        let mut squad_recent_breach = false;
        for member_idx in self.battle_state.squad(soldier.squad_uuid()).members() {
            let member = self.battle_state.soldier(*member_idx);
            if *self.battle_state.frame_i() < member.last_grenade_frame_i() + 1200 {
                squad_recent_breach = true;
                break;
            }
        }

        let mut target_interior_for_breach = None;
        if !is_defending && !is_indoor_breach && !squad_recent_breach {
            for interior in map.interiors() {
                let center_x = interior.x() + interior.width() / 2.0;
                let center_y = interior.y() + interior.height() / 2.0;
                let interior_center = battle_core::types::WorldPoint::new(center_x, center_y);
                
                let dist_to_interior = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &interior_center).meters() as f32;
                
                // 건물 20m 이내 접근 시 (진입 직전 외곽 스탠바이)
                if dist_to_interior <= 20.0 {
                    // 내부에 적이 있는지 스캔
                    let mut enemy_inside = false;
                    for enemy in self.battle_state.soldiers() {
                        if enemy.side() != soldier.side() && enemy.alive() {
                            let ep = enemy.world_point();
                            if ep.x >= interior.x() && ep.x <= interior.x() + interior.width() &&
                               ep.y >= interior.y() && ep.y <= interior.y() + interior.height() {
                                enemy_inside = true;
                                break;
                            }
                        }
                    }

                    // 적이 없더라도 깃발(점령지)이 건물 안에 있으면 무조건 예방적 투척
                    let mut flag_inside = false;
                    for flag in map.flags() {
                        let is_owned = self.battle_state.flags().ownerships().iter().any(|(n, o)| {
                            n == flag.name() && (
                                o == &battle_core::game::flag::FlagOwnership::Both || 
                                (is_side_a && o == &battle_core::game::flag::FlagOwnership::A) ||
                                (is_side_b && o == &battle_core::game::flag::FlagOwnership::B)
                            )
                        });
                        if !is_owned {
                            let fp = flag.position();
                            if fp.x >= interior.x() && fp.x <= interior.x() + interior.width() &&
                               fp.y >= interior.y() && fp.y <= interior.y() + interior.height() {
                                flag_inside = true;
                                break;
                            }
                        }
                    }

                    if enemy_inside || flag_inside {
                        target_interior_for_breach = Some(interior_center);
                        break;
                    }
                }
            }
        }

        if let Some(breach_target) = target_interior_for_breach {
            // [기획 반영: 건물 진입 직전 수류탄 투척 시 고착 및 목표 상실 방지]
            // 소탕 절차 시 오더를 강제로 엎드림(Hide)으로 덮어씌워 건물 안으로 안 들어가는 버그를 수정합니다.
            // 기존의 Move/Sneak 오더를 그대로 유지시킨 채 Gesture만 Throwing으로 주입하여, 걸어가면서 수류탄을 던지고 그대로 건물로 진입하도록 만듭니다.
            let mut rng = rand::thread_rng();
            let delay_frames = rand::Rng::gen_range(&mut rng, 60..90);
            let throw_end_frame = *self.battle_state.frame_i() + delay_frames;

            if soldier.uuid().0 % 3 != 2 && soldier.grenades() > 0 {
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetGesture(battle_core::behavior::gesture::Gesture::Throwing(throw_end_frame, breach_target))
                )));
                
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::ConsumeGrenade
                )));
                
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetLastGrenadeFrameI(*self.battle_state.frame_i())
                )));
            }
            // return 없이 통과시켜 하단의 match current_order 가 정상적으로 이동 행동(Behavior)을 반환하게 둡니다.
        }

        // [YOLO Mode Auto-Grenade] 전투 시작 전 (거점/적진 반경 25m 이내 접근 시) 수류탄 투척 (3초 쿨타임 적용)
        if !is_defending && (yolo_active || is_scout) && soldier.grenades() > 0 && *self.battle_state.frame_i() > soldier.last_grenade_frame_i() + 180 {
            let map = self.battle_state.map();
            let mut target_flag = None;
            let mut min_dist_meters = std::i64::MAX;

            for flag in map.flags() {
                let is_owned = self.battle_state.flags().ownerships().iter().any(|(n, o)| {
                    n == flag.name() && (
                        o == &FlagOwnership::Both || 
                        (is_side_a && o == &FlagOwnership::A) ||
                        (is_side_b && o == &FlagOwnership::B)
                    )
                });

                if !is_owned {
                    // 픽셀이 아닌 실제 미터(meters) 단위로 정확한 거리 측정
                    let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &flag.position()).meters();
                    if dist < min_dist_meters {
                        min_dist_meters = dist;
                        target_flag = Some(flag.clone());
                    }
                }
            }

            if let Some(flag) = target_flag {
                // 기존 15픽셀(4.5m) 단위의 오계산을 실제 25m(미터) 단위로 수정하여 교전 직전 확실하게 투척
                if min_dist_meters <= 25 {
                    // [조건 추가] 타겟 깃발 주변 반경 15m 이내에 살아있는 적군이 있는지 확인하여, 허공에 수류탄을 낭비하지 않도록 방지합니다.
                    let mut enemy_near_flag = false;
                    for enemy in self.battle_state.soldiers() {
                        if enemy.side() != soldier.side() && enemy.alive() {
                            let dist_to_flag = battle_core::physics::utils::distance_between_points(&flag.position(), &enemy.world_point());
                            if dist_to_flag.meters() <= 15 {
                                enemy_near_flag = true;
                                break;
                            }
                        }
                    }

                    if enemy_near_flag {
                        let pixel_dist = (flag.position().to_vec2() - soldier.world_point().to_vec2()).length();
                        // NaN(Not a Number) 좌표 증발 오류 방지를 위한 방어 코드 및 타겟 방향 80% 지점에 안착하도록 연산
                        let direction = if pixel_dist > 0.1 {
                            (flag.position().to_vec2() - soldier.world_point().to_vec2()).normalize()
                        } else {
                            glam::Vec2::new(1.0, 0.0)
                        };

                        // [측면 투척 변환] 병사 고유 ID를 분기하여 좌측 혹은 우측으로 30도(0.523 라디안) 각도를 회전 행렬로 꺾어 던지도록 계산
                        let throw_angle: f32 = if soldier.uuid().0 % 2 == 0 { 0.523 } else { -0.523 };
                        let cos_t = throw_angle.cos();
                        let sin_t = throw_angle.sin();
                        let side_direction = glam::Vec2::new(
                            direction.x * cos_t - direction.y * sin_t,
                            direction.x * sin_t + direction.y * cos_t
                        );
                        let throw_target = battle_core::types::WorldPoint::from_vec2(soldier.world_point().to_vec2() + side_direction * (pixel_dist * 0.8));
    
                        // [기획 반영 2-2] 즉발 폭발 스폰을 제거하고, 1~1.5초(60~90 프레임)의 무기 전환 및 투척 딜레이 상태(Throwing)로 돌입시킵니다.
                        let mut rng = rand::thread_rng();
                        let delay_frames = rand::Rng::gen_range(&mut rng, 60..90);
                        let throw_end_frame = *self.battle_state.frame_i() + delay_frames;

                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::SetGesture(battle_core::behavior::gesture::Gesture::Throwing(throw_end_frame, throw_target))
                        )));
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::ConsumeGrenade
                        )));
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::SetLastGrenadeFrameI(*self.battle_state.frame_i())
                        )));

                        // [기획 반영: 정찰조 수류탄 파지 후 사격 불가 버그 수정]
                        // 투척 후 강제로 도망(MoveFastTo)가게 만들던 로직을 제거합니다. 
                        // 병사들은 현재의 기동/탐색 행동을 그대로 유지한 채 걸어가면서 수류탄만 자연스럽게 던지게 되며, 적 조우 시 즉각 엎드려 사격이 가능해집니다.
                    }
                }
            }
        }

        // [YOLO Mode Auto-Flank] 전투 중 우회 기동: 적과 교전 중(Idle 상태이면서 타겟이 있음)일 때, 일정 주기(약 5초)마다 조금씩 포복(Sneak)으로 적의 측면을 향해 기동합니다.
        if (yolo_active || is_scout) && matches!(current_order, Order::Idle) {
            if let Some(target_idx) = soldier.target() {
                // 분대원들이 동시에 움직이지 않도록 UUID 기반으로 시간차(offset)를 둡니다. (반드시 20의 배수여야 실행 주기에 맞음)
                let animate_freq = self.config.soldier_animate_freq() as u64;
                let offset = ((soldier.uuid().0 as u64 * 2) % (300 / animate_freq)) * animate_freq; 
                if (*self.battle_state.frame_i() + offset) % 300 == 0 {
                    let target_soldier = self.battle_state.soldier(*target_idx);
                    let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &target_soldier.world_point());
                    
                    // 적과의 거리가 5m ~ 60m 사이일 때만 측면 우회 기동 수행 (너무 가깝거나 멀면 제자리 사격 유지)
                    if dist.meters() > 5 && dist.meters() < 60 {
                        // 적을 바라보는 방향 벡터 (적 -> 아군)
                        let dir = (soldier.world_point().to_vec2() - target_soldier.world_point().to_vec2()).normalize();
                        
                        // 병사 고유 ID를 이용해 홀수는 우측(60도), 짝수는 좌측(-60도)으로 산개하여 우회
                        let angle: f32 = if soldier.uuid().0 % 2 == 0 { 1.047 } else { -1.047 }; 
                        let cos_a = angle.cos();
                        let sin_a = angle.sin();
                        let flank_dir = glam::Vec2::new(
                            dir.x * cos_a - dir.y * sin_a,
                            dir.x * sin_a + dir.y * cos_a
                        );
                        
                        // 현재 거리에서 20% 접근하면서 측면으로 이동하는 우회 목표점 계산
                        let current_dist = dist.meters() as f32;
                        let target_pos = target_soldier.world_point().to_vec2();
                        let flank_target_point = target_pos + flank_dir * (current_dist * 0.8);
                        
                        // 해당 목표점을 향해 1회당 약 4m씩만 포복 이동 (조금씩 이동 후 사격 재개)
                        let move_dir = (flank_target_point - soldier.world_point().to_vec2()).normalize();
                        let next_point = battle_core::types::WorldPoint::from_vec2(soldier.world_point().to_vec2() + move_dir * 4.0);
                        
                        let map = self.battle_state.map();
                        let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                        let to_grid = map.grid_point_from_world_point(&next_point);
                        
                        if from_grid != to_grid {
                            let path_mode = PathMode::Walk;
                            let start_dir = Some(Direction::from_angle(&soldier.get_looking_direction()));
                            
                            if let Some(grid_path) = find_path(
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
                                let world_paths = WorldPaths::new(vec![WorldPath::new(world_path)]);
                                
                                // 기동 명령을 주입하고 끝나면 다시 대기(Idle) 상태로 돌아와서 교전을 재개하도록 예약(Then) 설정
                                temp_order_storage = Some(Order::SneakTo(world_paths, Some(Box::new(Order::Idle))));
                                current_order = temp_order_storage.as_ref().unwrap();
                                
                                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                    soldier.uuid(),
                                    SoldierMessage::SetOrder(current_order.clone())
                                )));
                            }
                        }
                    }
                }
            }
        }

        // [CQB 진입 목표점(Destination)의 동적 수정]
        // 이동 목적지가 건물 내부(Interior)로 설정되어 있을 경우, 사선에 노출되는 건물 중앙 대신 외벽에 바짝 붙어 대기(Stacking Up)하도록 목적지를 안전한 사각지대로 동적 보정합니다.
        // [최적화] 매 프레임 연산 폭주를 방지하기 위해 개별 유닛별로 1초(60프레임)에 한 번씩만 띄엄띄엄 연산하도록 주기를 분산시킵니다.
        let animate_freq = self.config.soldier_animate_freq() as u64;
        let cqb_eval_offset = ((soldier.uuid().0 as u64 * 7) % (60 / animate_freq)) * animate_freq;
        if (*self.battle_state.frame_i() + cqb_eval_offset) % 60 == 0 {
            if let Order::MoveTo(paths, _) | Order::MoveFastTo(paths, _) | Order::SneakTo(paths, _) = current_order {
                if let Some(last_pt) = paths.paths.last().and_then(|p| p.last_point()) {
                    let mut target_interior = None;
                    for interior in map.interiors() {
                        if last_pt.x >= interior.x() && last_pt.x <= interior.x() + interior.width() &&
                           last_pt.y >= interior.y() && last_pt.y <= interior.y() + interior.height() {
                            target_interior = Some(interior);
                            break;
                        }
                    }

                    if let Some(interior) = target_interior {
                        // [버그 수정: 무한 외곽 대기(풀밭 달리기) 방지]
                        // 해당 건물 안에 점령해야 할 목표 깃발(미점령 상태)이 있다면, 
                        // 무의미한 외벽 스태킹 대기를 스킵하고 그대로 내부 점령지로 돌입하도록 예외 처리합니다.
                        let mut has_unowned_flag = false;
                        for flag in map.flags() {
                            let fp = flag.position();
                            if fp.x >= interior.x() && fp.x <= interior.x() + interior.width() &&
                               fp.y >= interior.y() && fp.y <= interior.y() + interior.height() {
                                
                                let is_owned = self.battle_state.flags().ownerships().iter().any(|(n, o)| {
                                    n == flag.name() && (
                                        o == &battle_core::game::flag::FlagOwnership::Both || 
                                        (is_side_a && o == &battle_core::game::flag::FlagOwnership::A) ||
                                        (is_side_b && o == &battle_core::game::flag::FlagOwnership::B)
                                    )
                                });
                                
                                if !is_owned {
                                    has_unowned_flag = true;
                                    break;
                                }
                            }
                        }

                        // 점령해야 할 깃발이 없거나 100% 점령 완료된 건물에 대해서만 사각지대(외곽 풀밭) 방어 대기를 수행
                        if !has_unowned_flag {
                            let center_x = interior.x() + interior.width() / 2.0;
                            let center_y = interior.y() + interior.height() / 2.0;
                            let interior_center = battle_core::types::WorldPoint::new(center_x, center_y);
                            let interior_center_grid = map.grid_point_from_world_point(&interior_center);
                        
                            let mut best_cqb_wall_point = None;
                            let mut min_dist_to_soldier = std::f32::MAX;
                            
                            // [Phase 3] 대안 사각지대 탐색 변수 추가
                            let mut best_safe_cqb_point = None;
                            let mut min_dist_safe_cqb = std::f32::MAX;

                            let cqb_grids = battle_core::utils::grid_points_for_square(&interior_center_grid, 35, 35);
                            for cg in cqb_grids {
                                if map.contains(&cg) {
                                    let tile_pos = map.world_point_from_grid_point(cg);
                                    if !(tile_pos.x >= interior.x() && tile_pos.x <= interior.x() + interior.width() &&
                                         tile_pos.y >= interior.y() && tile_pos.y <= interior.y() + interior.height()) {
                                        
                                        // [수정: 목적지 타일 자체가 이동 불가능한 벽(BrickWall)이나 장애물인지 먼저 검사]
                                        let tile_idx = (cg.y * map.width() as i32 + cg.x) as usize;
                                        let mut is_walkable = false;
                                        if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                                            if !matches!(tile.type_(), battle_core::map::terrain::TileType::BrickWall | battle_core::map::terrain::TileType::Trunk | battle_core::map::terrain::TileType::DeepWater) {
                                                is_walkable = true;
                                            }
                                        }
                                        
                                        // 목적지 타일이 이동 불가능한 곳이면 멈추지 않고 즉시 스킵합니다.
                                        if !is_walkable {
                                            continue;
                                        }

                                        let mut touches_solid_wall = false;
                                        let neighbor_directions = [(-1, 0), (1, 0), (0, -1), (0, 1)];
                                        for (mx, my) in neighbor_directions {
                                            let neighbor_grid = battle_core::types::GridPoint::new(cg.x + mx, cg.y + my);
                                            if map.contains(&neighbor_grid) {
                                                let n_tile_idx = (neighbor_grid.y * map.width() as i32 + neighbor_grid.x) as usize;
                                                if let Some(n_tile) = map.terrain_tiles().get(n_tile_idx) {
                                                    if matches!(n_tile.type_, battle_core::map::terrain::TileType::BrickWall) {
                                                        touches_solid_wall = true;
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                        
                                        if touches_solid_wall {
                                            let dist = (tile_pos.to_vec2() - soldier.world_point().to_vec2()).length();
                                            
                                            if dist < min_dist_to_soldier {
                                                min_dist_to_soldier = dist;
                                                best_cqb_wall_point = Some(tile_pos);
                                            }

                                            // [Phase 3] 풀밭이 아닌 안전한 지형(숲, 흙 등)인지 체크하여 1순위 타겟으로 업데이트
                                            if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                                                if !matches!(tile.type_(), battle_core::map::terrain::TileType::ShortGrass | battle_core::map::terrain::TileType::MiddleGrass | battle_core::map::terrain::TileType::HighGrass) {
                                                    if dist < min_dist_safe_cqb {
                                                        min_dist_safe_cqb = dist;
                                                        best_safe_cqb_point = Some(tile_pos);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            
                            // [Phase 3] 안전한 대안 사각지대가 있으면 채택하고, 주변이 온통 풀밭이라면 가장 가까운 벽면(best_cqb_wall_point) 강제 지정. 스폰 후퇴 폐지.
                            let final_target_point = best_safe_cqb_point.or(best_cqb_wall_point);

                            if let Some(cqb_wall_point) = final_target_point {
                                // [버그 수정: Move 좌표 해제 안 됨 및 무한 왕복 고착화 해결]
                                // 병사가 이미 CQB 사각지대에 도달했거나 건물 내부에 진입했다면 외곽으로 다시 튕겨내는 것을 방지하고 곧바로 진입(Breach)하게 합니다.
                                let dist_to_cqb = (soldier.world_point().to_vec2() - cqb_wall_point.to_vec2()).length();
                                let is_already_inside = soldier.world_point().x >= interior.x() && soldier.world_point().x <= interior.x() + interior.width() &&
                                                        soldier.world_point().y >= interior.y() && soldier.world_point().y <= interior.y() + interior.height();
                                
                                // [Part 3: CQB 건물 진입 전술 3단계 상태 머신 적용]
                                // 1단계 (포복 접근): 아직 건물 외벽 사각지대(15m 밖)에 도달하지 못했다면 은밀 포복(SneakTo)으로 엄폐하며 조심스럽게 접근합니다.
                                if dist_to_cqb > 15.0 && !is_already_inside {
                                    let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                                    let to_grid = map.grid_point_from_world_point(&cqb_wall_point);
                                    if from_grid != to_grid {
                                        if let Some(grid_path) = find_path(&self.config, map, &from_grid, &to_grid, true, &PathMode::Walk, &None) {
                                            let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                            let new_world_paths = WorldPaths::new(vec![WorldPath::new(world_path)]);
                                            
                                            // 2단계 (정지 및 수류탄 선투척 소탕 대기): 외벽에 닿으면 자리에 멈춰 서서 수류탄을 먼저 던지도록(Defend) 연계를 잡고,
                                            // 3단계 (돌입): 수류탄 투척 및 소탕 완료 시 건물 정중앙 목표지로 전력 돌격(MoveFastTo)하도록 순차적 상태 예약을 체인으로 구성합니다.
                                            let final_assault_order = Box::new(Order::MoveFastTo(paths.clone(), current_order.then().map(Box::new)));
                                            let intermediate_clear_order = Box::new(Order::Defend(battle_core::utils::angle(&interior_center, &cqb_wall_point)));
                                            let adjusted_order = Order::SneakTo(new_world_paths, Some(intermediate_clear_order));
                                            
                                            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                                soldier.uuid(),
                                                SoldierMessage::SetOrder(adjusted_order.clone())
                                            )));

                                            temp_order_storage = Some(adjusted_order);
                                            current_order = temp_order_storage.as_ref().unwrap();
                                        }
                                    }
                                } else {
                                    // 2단계 및 3단계 전술 전환 연산:
                                    // 이미 건물 외벽(dist_to_cqb <= 15.0)에 안전하게 붙어 스태킹을 마친 상태라면,
                                    // 현재 수류탄 투척 제스처(Throwing)가 진행 중인 경우 화망이 형성될 때까지 대기(Defend) 상태를 유지합니다.
                                    // 투척이 완전히 끝나 무기가 Idle로 돌아오거나 이미 내부에 진입했다면 3단계인 전력 돌입 명령(MoveFastTo)을 확정 하달합니다.
                                    let is_throwing = matches!(soldier.gesture(), battle_core::behavior::gesture::Gesture::Throwing(_, _));
                                    
                                    if is_throwing && !is_already_inside {
                                        let clear_order = Order::Defend(battle_core::utils::angle(&interior_center, &soldier.world_point()));
                                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                            soldier.uuid(),
                                            SoldierMessage::SetOrder(clear_order.clone())
                                        )));
                                        temp_order_storage = Some(clear_order);
                                        current_order = temp_order_storage.as_ref().unwrap();
                                    } else {
                                        let breach_order = Order::MoveFastTo(paths.clone(), current_order.then().map(Box::new));
                                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                            soldier.uuid(),
                                            SoldierMessage::SetOrder(breach_order.clone())
                                        )));
                                        temp_order_storage = Some(breach_order);
                                        current_order = temp_order_storage.as_ref().unwrap();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut behavior = match current_order {
            Order::Idle => self.idle_behavior(soldier, &mut messages),
            Order::MoveTo(paths, _) => self.move_behavior(soldier, paths),
            Order::MoveFastTo(paths, _) => self.move_fast_behavior(soldier, paths),
            Order::SneakTo(paths, _) => self.sneak_to_behavior(soldier, paths),
            Order::Defend(angle) => self.defend_behavior(soldier, angle),
            Order::Hide(angle) => self.hide_behavior(soldier, angle),
            Order::EngageSquad(squad_index) => self.engage_behavior(soldier, squad_index),
            Order::SuppressFire(point) => self.suppress_fire_behavior(soldier, point),
            Order::OffMapTransit(frame) => Behavior::OffMapTransit(*frame),
        };

        // [Part 2: 탄약 고갈 시 교전 강제 취소 및 탄약고(스폰 지점) 복귀/보급 로직]
        if matches!(behavior, Behavior::EngageSoldier(_) | Behavior::SuppressFire(_)) || matches!(current_order, Order::Idle | Order::Hide(_) | Order::Defend(_)) {
            let has_ammo = if let Some(weapon) = soldier.weapon(&battle_core::entity::soldier::WeaponClass::Main) {
                weapon.can_fire() || weapon.can_reload() || soldier.magazines().iter().any(|m| weapon.accepted_magazine(m))
            } else {
                false
            };

            if !has_ammo {
                let map = self.battle_state.map();
                if let Some(cp) = self.checkpoints.read().unwrap().get(&soldier.squad_uuid()) {
                    let dist_to_cp = battle_core::physics::utils::distance_between_points(&soldier.world_point(), cp).meters();
                    
                    // 체크포인트 반경 5m 이내에 도달하면 탄약(수류탄 포함) 즉시 보급 및 안도감(스트레스 완화)
                    if dist_to_cp <= 5 {
                        // [Part 2: 탄약고 보급 체류(Delay) 로직 도입]
                        // 보급소에 도달하면 즉시 튕겨나가지 않고, 약 3초(180프레임)간 재장전 모션(Reloading)을 취하며 재정비합니다.
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::ReplenishAmmunition
                        )));
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::RelieveStress(200)
                        )));
                        
                        let reload_end_frame = *self.battle_state.frame_i() + 180;
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::SetGesture(battle_core::behavior::gesture::Gesture::Reloading(reload_end_frame, battle_core::entity::soldier::WeaponClass::Main))
                        )));
                        
                        behavior = Behavior::Idle(Body::Crouched);
                        let new_order = Order::Idle;
                        temp_order_storage = Some(new_order.clone());
                        current_order = temp_order_storage.as_ref().unwrap();
                        
                        messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                            soldier.uuid(),
                            SoldierMessage::SetOrder(new_order)
                        )));
                        
                        // 중복 출력 방지
                        if *self.battle_state.frame_i() % 180 == 0 {
                            println!("[탄약 보급] 분대 {}가 스폰 지점(탄약고)에서 재보급을 완료하고 재정비 중입니다!", soldier.squad_uuid().0);
                        }
                    } else {
                        // 탄약이 없고 체크포인트와 멀다면 후퇴 기동(MoveFastTo)
                        let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                        let to_grid = map.grid_point_from_world_point(cp);
                        
                        if from_grid != to_grid {
                            if let Some(grid_path) = find_path(&self.config, map, &from_grid, &to_grid, true, &PathMode::Walk, &None) {
                                let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                let paths = WorldPaths::new(vec![WorldPath::new(world_path)]);
                                
                                behavior = Behavior::MoveFastTo(paths.clone());
                                let new_order = Order::MoveFastTo(paths, Some(Box::new(Order::Idle)));
                                temp_order_storage = Some(new_order.clone());
                                current_order = temp_order_storage.as_ref().unwrap();
                                
                                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                    soldier.uuid(),
                                    SoldierMessage::SetOrder(new_order)
                                )));
                            } else {
                                // 길이 막혔다면 바닥에 엎드림
                                behavior = Behavior::Hide(soldier.get_looking_direction());
                            }
                        } else {
                            behavior = Behavior::Hide(soldier.get_looking_direction());
                        }
                    }
                } else {
                    behavior = Behavior::Hide(soldier.get_looking_direction());
                }
            }
        }

        // [고착 해제] idle_behavior 내부에서 강제로 오더가 변경(temp_order_storage 주입)되었을 경우, 
        let is_order_forced_changed = temp_order_storage.is_some();

        // In case of squad leader and regularly propagation
        if self.soldier_is_squad_leader(soldier.uuid())
            && (behavior.propagation() == BehaviorPropagation::Regularly || is_order_forced_changed)
        {
            // Order must be propagated to squad members
            messages.extend(self.propagate_behavior(soldier, &behavior));
        }

        // Change behavior if computed behavior is different
        if &behavior != soldier.behavior() || is_order_forced_changed {
            // In case of squad leader and regularly propagation
            if self.soldier_is_squad_leader(soldier.uuid())
                && (behavior.propagation() == BehaviorPropagation::OnChange || is_order_forced_changed)
            {
                messages.extend(self.propagate_behavior(soldier, &behavior));
            }

            messages.extend(vec![RunnerMessage::BattleState(
                BattleStateMessage::Soldier(soldier.uuid(), SoldierMessage::SetBehavior(behavior.clone())),
            )]);
        };

        messages
    }

    pub fn propagate_behavior(&self, leader: &Soldier, behavior: &Behavior) -> Vec<RunnerMessage> {
        assert!(self.soldier_is_squad_leader(leader.uuid()));
        let mut messages = vec![];
        let mut debug_points: Vec<NewDebugPoint> = vec![];

        let orders: Vec<(&Soldier, Order)> = match behavior {
            Behavior::MoveTo(_) | Behavior::MoveFastTo(_) | Behavior::SneakTo(_) => {
                match self.battle_state.soldier_behavior_mode(leader) {
                    BehaviorMode::Ground => self.propagate_move(leader.squad_uuid(), behavior),
                    BehaviorMode::Vehicle => self.propagate_drive(leader.squad_uuid(), behavior),
                }
            }
            Behavior::Defend(_) => {
                let (orders, debug_points_) = match self.battle_state.soldier_behavior_mode(leader)
                {
                    BehaviorMode::Ground => {
                        self.propagate_defend_or_hide(leader.squad_uuid(), behavior)
                    }
                    BehaviorMode::Vehicle => self.propagate_rotate(leader.squad_uuid(), behavior),
                };
                debug_points.extend(debug_points_);
                orders
            }
            Behavior::Hide(_) | Behavior::ScatterToCover(_) | Behavior::GatherToCover(_) => {
                let (orders, debug_points_) = match self.battle_state.soldier_behavior_mode(leader)
                {
                    BehaviorMode::Ground => {
                        self.propagate_defend_or_hide(leader.squad_uuid(), behavior)
                    }
                    BehaviorMode::Vehicle => self.propagate_rotate(leader.squad_uuid(), behavior),
                };
                debug_points.extend(debug_points_);
                orders
            }
            Behavior::DriveTo(_) => todo!(),
            Behavior::RotateTo(_) => todo!(),
            Behavior::Idle(_) => {
                // [개선: 지휘관 기동 종료 시 부하 강제 정지(Separation) 방지]
                // 지휘관이 목적지에 먼저 도달했다고 해서 부하들의 이동 오더를 강제로 지워버리면,
                // 뒤따라오던 부하들이 도중에 멈춰서 분대가 분리되는 현상이 발생하므로 이를 제거합니다.
                vec![]
            }
            Behavior::Dead | Behavior::Unconscious => {
                vec![]
            }
            Behavior::OffMapTransit(frame) => {
                // [Operation Ghost - Part 4] 지휘관의 오프맵 이탈 명령을 전체 분대원에게 동일하게 전파합니다.
                let mut sq_orders = vec![];
                for member in self.battle_state.squad(leader.squad_uuid()).subordinates().iter().map(|i| self.battle_state.soldier(**i)) {
                    sq_orders.push((member, Order::OffMapTransit(*frame)));
                }
                sq_orders
            }
            Behavior::SuppressFire(point) => {
                self.propagate_suppress_fire(leader.squad_uuid(), point)
            }
            Behavior::EngageSoldier(soldier_index) => {
                self.propagate_engage_soldier(&leader.squad_uuid(), soldier_index)
            }
        };

        for (subordinate, order) in orders {
            // [억제 상태 패닉 락 (Suppression Lock)]
            // 부하가 공포 상태거나 수류탄/포격 회피용으로 강제 은폐(Hide) 중일 때는 지휘관의 분대 명령 전파를 무시합니다.
            let is_panicking = subordinate.under_fire().is_danger() || subordinate.under_fire().is_max();
            let is_dodging_blast = matches!(subordinate.order(), Order::Hide(_)) && matches!(subordinate.behavior(), Behavior::Hide(_));
            
            // [수정] 맵 밖으로 안전하게 이탈(OffMapTransit)하는 명령은 패닉 상태에서도 무조건 전파(복종)하도록 허용합니다.
            // 또한, 패닉 상태(실제 피격 공포)가 아닌 단순 전술 대기(Scout 대기, CQB 진입 대기) 중인 부하는 지휘관의 이동 명령을 무시하지 않고 즉시 복종해야 합니다.
            if (is_panicking || is_dodging_blast) && !matches!(order, Order::Hide(_) | Order::Defend(_) | Order::OffMapTransit(_)) {
                if !is_panicking && matches!(order, Order::MoveTo(_, _) | Order::MoveFastTo(_, _) | Order::SneakTo(_, _)) {
                    // 패닉이 없는 클린 상태에서 지휘관의 새로운 기동 명령이 전파되면 락을 우회하여 복종시킵니다.
                } else {
                    continue; // 실제 공포 상태이거나 실시간 폭발 회피 중이므로 명령 무시 유지
                }
            }

            // Give order only if different from subordinate current order
            if subordinate.order() != &order {
                messages.extend(vec![RunnerMessage::BattleState(
                    BattleStateMessage::Soldier(
                        subordinate.uuid(),
                        SoldierMessage::SetOrder(order),
                    ),
                )]);
            }
        }

        for debug_point in debug_points {
            messages.push(RunnerMessage::ClientsState(
                ClientStateMessage::PushDebugPoint(debug_point),
            ))
        }

        messages
    }

    pub fn idle_behavior(&self, soldier: &Soldier, messages: &mut Vec<RunnerMessage>) -> Behavior {
        let is_side_a = soldier.side() == &battle_core::game::Side::A;
        let is_side_b = soldier.side() == &battle_core::game::Side::B;
        let yolo_active = (is_side_a && self.config.yolo_mode_a) || (is_side_b && self.config.yolo_mode_b);

        let mut is_scout = false;
        let mut my_company_name = String::new();
        for comp in self.companies.values() {
            if comp.squads.contains(&soldier.squad_uuid()) {
                my_company_name = comp.id.clone();
                if comp.scout_squad == Some(soldier.squad_uuid()) {
                    is_scout = true;
                }
                break;
            }
        }

        let map = self.battle_state.map();
        let soldier_grid = map.grid_point_from_world_point(&soldier.world_point());
        let current_tile_idx = (soldier_grid.y * map.width() as i32 + soldier_grid.x) as usize;
        let in_open_field = if let Some(tile) = map.terrain_tiles().get(current_tile_idx) {
            self.config.terrain_tile_opacity(&tile.type_) < 0.1 // 투명도 0.1 미만은 뻥 뚫린 평야로 간주
        } else {
            false
        };

        let is_indoor = map.interiors().iter().any(|i| {
            soldier.world_point().x >= i.x() && soldier.world_point().x <= i.x() + i.width() &&
            soldier.world_point().y >= i.y() && soldier.world_point().y <= i.y() + i.height()
        });

        // [건물 내부 안전지대 교전 로직]
        // 건물 내부에 진입한 병사는 평야처럼 멍청하게 서서 교전하지 않고, 창문이나 내부 구조물을 활용해
        // 엎드려서(Hide) 몸을 보호하고 '숨었다가 쏘는(Peek & Shoot)' 농성 전술을 최우선으로 채택합니다.
        if is_indoor && !yolo_active {
            if let Some(opponent) = self.soldier_find_opponent_to_target(soldier, None, &ChooseMethod::RandomFromNearest) {
                // 실내에서는 즉각 사격하는 대신 일단 엎드려 방어/엄폐 태세로 사격을 준비합니다.
                return Behavior::Hide(battle_core::utils::angle(&opponent.world_point(), &soldier.world_point()));
            } else {
                return Behavior::Hide(soldier.get_looking_direction());
            }
        }

        let mut opponent_to_engage = self.soldier_find_opponent_to_target(soldier, None, &ChooseMethod::RandomFromNearest);

        // [전술 변경: 정찰조(Scout)와 본대(Main)의 교전 수칙 분리]
        // 본대(비정찰조)는 적 발견 시 최우선으로 교전합니다.
        // 정찰조는 기본적으로 교전을 회피하고 목표 기동을 우선하되, 
        // 주위(반경 60m 내)에 정찰조가 아닌 아군 본대가 지원 사격(교전/제압사격)을 해주고 있다면 물러서지 않고 함께 교전합니다.
        if let Some(opponent) = opponent_to_engage {
            if is_scout {
                let mut has_support_fire = false;
                
                for ally in self.battle_state.soldiers() {
                    if ally.side() == soldier.side() && ally.alive() && ally.uuid() != soldier.uuid() {
                        let dist_to_ally = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &ally.world_point()).meters();
                        if dist_to_ally <= 60 {
                            let mut ally_is_scout = false;
                            for comp in self.companies.values() {
                                if comp.scout_squad == Some(ally.squad_uuid()) {
                                    ally_is_scout = true;
                                    break;
                                }
                            }
                            
                            // 정찰조가 아닌 본대(Main) 소속 아군이 사격 중인지 확인
                            if !ally_is_scout {
                                if matches!(ally.behavior(), Behavior::SuppressFire(_) | Behavior::EngageSoldier(_)) {
                                    has_support_fire = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                
                // 본대의 지원 사격이 없다면 정찰조는 교전을 포기하고 본래의 엄폐/기동 목표로 돌아갑니다.
                if !has_support_fire {
                    opponent_to_engage = None;
                }
            }
        }

        if let Some(opponent) = opponent_to_engage {
            return Behavior::EngageSoldier(opponent.uuid());
        }

        // [개선] 얕은 스트레스(Warning 이하)에서는 눕지 않고 기동을 유지합니다. 
        // 오직 생명의 위협(Danger, Max)을 느낄 때만 그 자리에 숨습니다(Hide).
        if soldier.under_fire().is_danger() || soldier.under_fire().is_max() {
            // TODO : soldier angle
            Behavior::Hide(Angle(0.))
        } else {
            // [YOLO Mode] 가장 가장 가까운 미점령/적 깃발로 자동 진격하되, 적군의 사로를 피하는 전술 네비게이션 사용
            // 수정: 부하 병사들이 제멋대로 다른 목표를 잡고 이탈하는 것을 막기 위해, 오직 분대장(Leader)만이 전략적 타겟을 계산하도록 제한합니다.
            if (yolo_active || is_scout) && self.soldier_is_squad_leader(soldier.uuid()) {
                // 3. 중대(Company) 그룹핑 및 정찰조(Scout) 역할 확인
                // [하드코딩 락 전면 해제 패치: 중대별 고유 분대 UUID의 병렬 동시 구동 성립]
                // 1. 기존의 특정 분대(0분대)에만 쏠려있던 오더 전파 예외 락과 오염 조건 분기를 원천 제거합니다.
                // 2. 주 Runner에 백업 등록된 중대 맵 풀로부터 실시간으로 소속 상태를 조회하여, 해당 분대가 정찰 배정을 받았다면 고유 분대 번호와 관계없이 즉각 출격시킵니다.

                // [Phase 4: A* 네비게이션 연산 부하(CPU Load) 분산]
                // 다수의 중대가 동시에 A* 연산을 호출하여 발생하는 서버 프레임 드랍(프리징)을 방지합니다.
                // 중대 이름의 해시값을 사용하여 각 중대마다 길찾기 연산을 수행하는 틱(Tick) 오프셋을 균일하게 분산(Tick Staggering)시킵니다.
                let animate_freq = self.config.soldier_animate_freq() as u64;
                let company_hash: u64 = my_company_name.bytes().map(|b| b as u64).sum();
                let yolo_eval_offset = if my_company_name.is_empty() {
                    ((soldier.uuid().0 as u64 * 13) % (60 / animate_freq)) * animate_freq
                } else {
                    (company_hash % (60 / animate_freq)) * animate_freq
                };
                
                // [최적화] 무한루프 수준의 매 프레임 연산을 막기 위해 분대장별로 1초(60프레임)에 1번만 네비게이션을 띄엄띄엄 연산합니다.
                if (*self.battle_state.frame_i() + yolo_eval_offset) % 60 != 0 {
                    return Behavior::Idle(Body::Crouched);
                }

                let mut tactical_costs = std::collections::HashMap::new();
                
                // [Step 1: Tactical Ping] 사격이 발생했던 적 원점(위험 사로)을 접근 금지 구역으로 설정
                for (ping_grid, (_, ping_side)) in &self.tactical_pings {
                    if ping_side != soldier.side() {
                        // [수정] 길찾기 고착을 방지하기 위해 핑 영향 범위를 30x30 -> 7x7로 대폭 줄이고, 패널티도 2000 -> 200으로 완화하여 우회만 유도합니다.
                        let danger_grids = battle_core::utils::grid_points_for_square(ping_grid, 7, 7);
                        for dg in danger_grids {
                            *tactical_costs.entry(dg).or_insert(0) += 200; // 극단적 차단 대신 강한 우회 유도로 변경
                        }
                    }
                }

                // 1. 위협 섹터(Risk Sector) 색칠 및 Edge 영역(가장자리) 우회 정의
                for enemy in self.battle_state.soldiers().iter().filter(|s| s.side() != soldier.side() && s.alive()) {
                    let enemy_grid = map.grid_point_from_world_point(&enemy.world_point());
                    
                    // 중심부는 극도의 위험(Center Risk), 외곽은 우회(Edge) 영역으로 분리하여 칠함
                    let core_grids = battle_core::utils::grid_points_for_square(&enemy_grid, 15, 15);
                    let edge_grids = battle_core::utils::grid_points_for_square(&enemy_grid, 35, 35);
                    
                    for eg in edge_grids {
                        // Edge 영역은 상대적으로 낮은 우회 비용 부여 (가장자리를 타도록 유도)
                        *tactical_costs.entry(eg).or_insert(0) += 20; 
                    }
                    for cg in core_grids {
                        if let Some(tile) = map.terrain_tiles().get((cg.y * map.width() as i32 + cg.x) as usize) {
                            if self.config.terrain_tile_opacity(&tile.type_) < 0.1 {
                                *tactical_costs.entry(cg).or_insert(0) += 1000; // 중심부 평야는 절대 진입 금지(Risk Max)
                            } else {
                                *tactical_costs.entry(cg).or_insert(0) += 200; // 숲이더라도 중심부면 회피
                            }
                        }
                    }
                }

                // 2. 안전 섹터(후방) 보너스 부여
                for ally in self.battle_state.soldiers().iter().filter(|s| s.side() == soldier.side() && s.alive() && s.uuid() != soldier.uuid()) {
                    if ally.body() == battle_core::behavior::Body::Lying {
                        let ally_grid = map.grid_point_from_world_point(&ally.world_point());
                        let safe_grids = battle_core::utils::grid_points_for_square(&ally_grid, 10, 10);
                        for sg in safe_grids {
                            *tactical_costs.entry(sg).or_insert(0) -= 20; 
                        }
                    }
                }

                // [기획 반영: 비정찰조(본대)의 체크포인트 순찰 복귀]
                if !is_scout {
                    if let Some(cp) = self.checkpoints.read().unwrap().get(&soldier.squad_uuid()) {
                        let dist_to_cp = battle_core::physics::utils::distance_between_points(&soldier.world_point(), cp).meters();
                        if dist_to_cp > 15 { 
                            let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                            let to_grid = map.grid_point_from_world_point(cp);
                            if from_grid != to_grid {
                                if let Some(grid_path) = find_path(&self.config, map, &from_grid, &to_grid, true, &PathMode::Walk, &None) {
                                    let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                    let paths = WorldPaths::new(vec![WorldPath::new(world_path)]);
                                    
                                    // [Part 2: 체크포인트 신속 복귀] 포복(Sneak) 대신 전력 질주(MoveFast)로 복귀 속도 극대화
                                    let local_temp_order = Order::MoveFastTo(paths.clone(), Some(Box::new(Order::Idle)));
                                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                        soldier.uuid(),
                                        SoldierMessage::SetOrder(local_temp_order)
                                    )));
                                    
                                    if (*self.battle_state.frame_i()) % 300 == 0 {
                                        println!("[복귀] 비정찰조(분대 {})가 체크포인트로 신속 복귀 중입니다.", soldier.squad_uuid().0);
                                    }
                                    
                                    return Behavior::MoveFastTo(paths);
                                }
                            }
                        } else {
                            if (*self.battle_state.frame_i()) % 300 == 0 {
                                println!("[대기] 비정찰조(분대 {})가 체크포인트에 안착하여 대기 중입니다.", soldier.squad_uuid().0);
                            }
                            return Behavior::Idle(Body::Crouched);
                        }
                    }
                }

                let mut target_flag = None;
                let mut best_score = std::f32::MIN; 

                let mut weakest_flag = None;
                let mut lowest_risk = std::f32::MAX;

                for flag in map.flags() {
                    let is_owned = self.battle_state.flags().ownerships().iter().any(|(n, o)| {
                        n == flag.name() && (
                            o == &FlagOwnership::Both || 
                            (is_side_a && o == &FlagOwnership::A) ||
                            (is_side_b && o == &FlagOwnership::B)
                        )
                    });

                    if !is_owned {
                        let dist_to_flag = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &flag.position()).meters() as f32;
                        
                        let mut allies_at_flag = 0;
                        let mut enemies_at_flag = 0;
                        for s in self.battle_state.soldiers().iter().filter(|s| s.alive()) {
                            let d = battle_core::physics::utils::distance_between_points(&s.world_point(), &flag.position()).meters() as f32;
                            if d <= 40.0 { 
                                if s.side() == soldier.side() { allies_at_flag += 1; }
                                else { enemies_at_flag += 1; }
                            }
                        }

                        if dist_to_flag > 60.0 && allies_at_flag > 0 && allies_at_flag >= enemies_at_flag {
                            continue;
                        }

                        if dist_to_flag > 200.0 && !is_scout {
                            continue;
                        }

                        let mut risk_score = 0.0;
                        
                        for enemy in self.battle_state.soldiers().iter().filter(|s| s.side() != soldier.side() && s.alive()) {
                            let dist_to_enemy = battle_core::physics::utils::distance_between_points(&flag.position(), &enemy.world_point()).meters() as f32;
                            if dist_to_enemy <= 40.0 {
                                let flag_grid = map.grid_point_from_world_point(&flag.position());
                                let tile_idx = (flag_grid.y * map.width() as i32 + flag_grid.x) as usize;
                                if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                                    if self.config.terrain_tile_opacity(&tile.type_) < 0.1 {
                                        risk_score += 500.0; 
                                    } else {
                                        risk_score += 100.0; 
                                    }
                                }
                            }
                        }

                        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
                            if ping_side != soldier.side() {
                                let ping_world = map.world_point_from_grid_point(*ping_grid);
                                let dist_to_ping = battle_core::physics::utils::distance_between_points(&flag.position(), &ping_world).meters() as f32;
                                if dist_to_ping <= 40.0 {
                                    risk_score += 2000.0; 
                                }
                            }
                        }

                        if risk_score < lowest_risk {
                            lowest_risk = risk_score;
                            weakest_flag = Some(flag.clone());
                        }

                        let dist = (soldier.world_point().to_vec2() - flag.position().to_vec2()).length();
                        
                        let distance_penalty = if dist > 150.0 {
                            dist * 115.0 
                        } else {
                            dist
                        };

                        let priority_score = if is_scout {
                            10000.0 - distance_penalty
                        } else {
                            10000.0 - distance_penalty - risk_score
                        };

                        if priority_score > best_score {
                            best_score = priority_score;
                            target_flag = Some(flag.clone());
                        }
                    }
                }

                if best_score < 0.0 && weakest_flag.is_some() && !is_scout {
                    target_flag = weakest_flag;
                }

                if let Some(flag) = target_flag {
                    let mut final_target = flag.position();
                    let mut is_cqb_route = false;
                    
                    // [수정] 이미 실내(Interior)에 진입한 상태라면, 불필요하게 밖으로 나가서 벽(CQB)을 타지 않고 즉시 깃발로 직행합니다.
                    let is_already_inside_flag = map.interiors().iter().any(|i| {
                        soldier.world_point().x >= i.x() && soldier.world_point().x <= i.x() + i.width() &&
                        soldier.world_point().y >= i.y() && soldier.world_point().y <= i.y() + i.height()
                    });

                    if !is_already_inside_flag {
                        for interior in map.interiors() {
                            if final_target.x >= interior.x() && final_target.x <= interior.x() + interior.width() &&
                               final_target.y >= interior.y() && final_target.y <= interior.y() + interior.height() {
                                
                                let interior_center_grid = map.grid_point_from_world_point(&final_target);
                                let cqb_grids = battle_core::utils::grid_points_for_square(&interior_center_grid, 35, 35);
                                let mut best_cqb = None;
                                let mut min_dist_to_me = std::f32::MAX;
                                for cg in cqb_grids {
                                    if map.contains(&cg) {
                                        let tile_pos = map.world_point_from_grid_point(cg);
                                        if !(tile_pos.x >= interior.x() && tile_pos.x <= interior.x() + interior.width() &&
                                             tile_pos.y >= interior.y() && tile_pos.y <= interior.y() + interior.height()) {
                                            
                                            let tile_idx = (cg.y * map.width() as i32 + cg.x) as usize;
                                            let mut is_walkable = false;
                                            if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                                                if !matches!(tile.type_(), battle_core::map::terrain::TileType::BrickWall | battle_core::map::terrain::TileType::Trunk | battle_core::map::terrain::TileType::DeepWater) {
                                                    is_walkable = true;
                                                }
                                            }
                                            if !is_walkable { continue; }
                                            
                                            let mut touches_wall = false;
                                            for (mx, my) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                                                let ng = battle_core::types::GridPoint::new(cg.x + mx, cg.y + my);
                                                if map.contains(&ng) {
                                                    if let Some(nt) = map.terrain_tiles().get((ng.y * map.width() as i32 + ng.x) as usize) {
                                                        if matches!(nt.type_, battle_core::map::terrain::TileType::BrickWall) {
                                                            touches_wall = true; break;
                                                        }
                                                    }
                                                }
                                            }
                                            if touches_wall {
                                                let dist = (tile_pos.to_vec2() - soldier.world_point().to_vec2()).length();
                                                if dist < min_dist_to_me {
                                                    min_dist_to_me = dist;
                                                    best_cqb = Some(tile_pos);
                                                }
                                            }
                                        }
                                    }
                                }
                                if let Some(cqb) = best_cqb {
                                    final_target = cqb;
                                    is_cqb_route = true;
                                }
                                break;
                            }
                        }
                    }

                    let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                    let to_grid = map.grid_point_from_world_point(&final_target);
                    
                    if from_grid != to_grid {
                        let path_mode = PathMode::Walk;
                        let start_dir = Some(Direction::from_angle(&soldier.get_looking_direction()));
                        
                        if let Some(grid_path) = find_tactical_path(
                            &self.config,
                            map,
                            &from_grid,
                            &to_grid,
                            true,
                            &path_mode,
                            &start_dir,
                            &tactical_costs,
                        ) {
                            let then_order = if is_cqb_route {
                                let flag_grid = map.grid_point_from_world_point(&flag.position());
                                if let Some(flag_path) = find_tactical_path(&self.config, map, &to_grid, &flag_grid, true, &PathMode::Walk, &None, &tactical_costs) {
                                    let wp = flag_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                    Some(Box::new(Order::MoveFastTo(WorldPaths::new(vec![WorldPath::new(wp)]), None)))
                                } else { None }
                            } else { None };

                            let world_path = grid_path
                                .iter()
                                .map(|p| map.world_point_from_grid_point(*p))
                                .collect();
                            let world_paths = WorldPaths::new(vec![WorldPath::new(world_path)]);
                            
                            if is_cqb_route {
                                self.checkpoints.write().unwrap().insert(soldier.squad_uuid(), final_target);
                            }

                            let order = if is_scout {
                                Order::MoveFastTo(world_paths.clone(), then_order)
                            } else {
                                Order::SneakTo(world_paths.clone(), then_order)
                            };
                            
                            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                soldier.uuid(),
                                SoldierMessage::SetOrder(order)
                            )));

                            // [병렬 로그 버그 패치] 0분대 고정 필터를 소거하고, 주입받은 액티브 분대 UUID를 기반으로 병렬 출격을 단행시킵니다.
                            if is_scout {
                                println!("[정찰조] 분대 {}가 깃발 [{}] 점령을 위해 출격합니다!", soldier.squad_uuid().0, flag.name().0);
                                return Behavior::SneakTo(world_paths);
                            } else {
                                return Behavior::SneakTo(world_paths);
                            }
                        } else {
                            let current_frame = *self.battle_state.frame_i();

                            for (comp_name, comp) in &self.companies {
                                if comp.scout_squad == Some(soldier.squad_uuid()) {
                                    let mut sorted_squads = comp.squads.clone();
                                    sorted_squads.sort_by(|a, b| a.0.cmp(&b.0));
                                    let cluster_anchor_key = format!("{}-group-{}", comp.side, sorted_squads[0].0);

                                    let mut offsets = self.scout_turn_offsets.write().unwrap();
                                    let entry = offsets.entry(cluster_anchor_key.clone()).or_insert((0, 0));
                                    if current_frame > entry.1 + 180 {
                                        entry.0 += 1;
                                        entry.1 = current_frame;
                                        
                                        let mut history_guard = self.scouted_history.write().unwrap();
                                        let current_history = history_guard.entry(cluster_anchor_key).or_insert_with(std::collections::HashSet::new);
                                        current_history.insert(soldier.squad_uuid());
                                        
                                        // [Phase 3: 턴 패스 지역화] 글로벌 턴 패스 로직 삭제. 길이 막힌 해당 중대(Company)만 다른 정찰조를 보냅니다.
                                        println!("[로테이션 지역화] 정찰조(분대 {})가 길찾기(A*)에 실패하여 중대 {} 내부 턴을 패스합니다.", soldier.squad_uuid().0, comp_name);
                                    }
                                }
                            }
                            
                            return Behavior::Idle(Body::Crouched);
                        }
                    } else {
                        // [버그 수정] 이미 깃발 위치에 도달했다면, 무의미하게 체크포인트로 돌아가지 않고 그 자리에서 대기하며 점령합니다.
                        return Behavior::Idle(Body::Crouched);
                    }
                }
                
                if is_scout {
                    if let Some(cp) = self.checkpoints.read().unwrap().get(&soldier.squad_uuid()) {
                        let dist_to_cp = battle_core::physics::utils::distance_between_points(&soldier.world_point(), cp).meters();
                        let mut returned_to_cp = false;
                        
                        if dist_to_cp > 15 { 
                            let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                            let to_grid = map.grid_point_from_world_point(cp);
                            if from_grid != to_grid {
                                if let Some(grid_path) = find_path(&self.config, map, &from_grid, &to_grid, true, &PathMode::Walk, &None) {
                                    let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                    let paths = WorldPaths::new(vec![WorldPath::new(world_path)]);
                                    
                                    // [Part 2: 체크포인트 신속 복귀]
                                    let local_temp_order = Order::MoveFastTo(paths.clone(), Some(Box::new(Order::Idle)));
                                    messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                                        soldier.uuid(),
                                        SoldierMessage::SetOrder(local_temp_order)
                                    )));
                                    return Behavior::MoveFastTo(paths);
                                } else {
                                    self.checkpoints.write().unwrap().insert(soldier.squad_uuid(), soldier.world_point());
                                    returned_to_cp = true;
                                }
                            } else {
                                returned_to_cp = true;
                            }
                        } else {
                            returned_to_cp = true;
                        }
                        
                        if returned_to_cp {
                            let current_frame = *self.battle_state.frame_i();

                            for (comp_name, comp) in &self.companies {
                                if comp.scout_squad == Some(soldier.squad_uuid()) {
                                    let mut sorted_squads = comp.squads.clone();
                                    sorted_squads.sort_by(|a, b| a.0.cmp(&b.0));
                                    let cluster_anchor_key = format!("{}-group-{}", comp.side, sorted_squads[0].0);

                                    let mut offsets = self.scout_turn_offsets.write().unwrap();
                                    
                                    let entry = offsets.entry(cluster_anchor_key.clone()).or_insert((0, 0));
                                    if current_frame > entry.1 + 180 {
                                        entry.0 += 1;
                                        entry.1 = current_frame;
                                        
                                        let mut history_guard = self.scouted_history.write().unwrap();
                                        let current_history = history_guard.entry(cluster_anchor_key).or_insert_with(std::collections::HashSet::new);
                                        current_history.insert(soldier.squad_uuid());
                                        
                                        // [Phase 3: 턴 패스 지역화] 진영 글로벌 턴 패스 삭제
                                        println!("[로테이션 지역화] 정찰조(분대 {})가 거점 복귀 완료로 중대 {} 내부 턴을 패스합니다.", soldier.squad_uuid().0, comp_name);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Behavior::Idle(Body::Crouched)
        }
    }

    pub fn move_behavior(&self, soldier: &Soldier, paths: &WorldPaths) -> Behavior {
        let map = self.battle_state.map();
        let soldier_grid = map.grid_point_from_world_point(&soldier.world_point());
        let current_tile_idx = (soldier_grid.y * map.width() as i32 + soldier_grid.x) as usize;
        let in_open_field = if let Some(tile) = map.terrain_tiles().get(current_tile_idx) {
            self.config.terrain_tile_opacity(&tile.type_) < 0.1
        } else {
            false
        };

        let mut is_scout = false;
        for comp in self.companies.values() {
            if comp.scout_squad == Some(soldier.squad_uuid()) {
                is_scout = true;
                break;
            }
        }

        // [Part 2: 지형 및 역할별 동적 교전 거리 적용]
        // 본대(Main): 평야에서는 50m 밖에서도 즉각 교전을 시작하며, 숲/시가지(CQB)에서는 시야를 고려해 20m 이내로 제한합니다.
        // 정찰조(Scout): 적을 먼저 발견해도 쏘지 않고 잠행을 유지합니다(15m 초근접 시에만 최후 교전).
        let engage_dist_limit = if is_scout {
            15 
        } else {
            if in_open_field { 50 } else { 20 }
        };

        if let Some(opponent) =
            self.soldier_find_opponent_to_target(soldier, None, &ChooseMethod::RandomFromNearest)
        {
            let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &opponent.world_point());
            
            // 정찰조라도 이미 적에게 발각되어 공격(스트레스)을 받고 있다면, 거리 50m 이내일 경우 예외적으로 응사(교전)합니다.
            let under_attack = soldier.under_fire().is_warning() || soldier.under_fire().is_danger() || soldier.under_fire().is_max();
            if dist.meters() <= engage_dist_limit || (is_scout && under_attack && dist.meters() <= 50) {
                return Behavior::EngageSoldier(opponent.uuid());
            }
        }

        match self.battle_state.soldier_behavior_mode(soldier) {
            BehaviorMode::Ground => {
                // [개선] 스트레스 조건에서도 우회(사이드)나 후퇴 방향일 때는 일반 이동(MoveTo)을 허용합니다.
                let mut is_moving_towards_enemy = false;
                if let Some(next_point) = paths.next_point() {
                    let move_angle = battle_core::utils::angle(&next_point, &soldier.world_point()).0;
                    
                    let mut closest_enemy = None;
                    let mut min_dist = std::f32::MAX;
                    for enemy in self.battle_state.soldiers().iter().filter(|s| s.side() != soldier.side() && s.alive()) {
                        let edist = (enemy.world_point().to_vec2() - soldier.world_point().to_vec2()).length();
                        if edist < min_dist {
                            min_dist = edist;
                            closest_enemy = Some(enemy);
                        }
                    }
                    
                    if let Some(enemy) = closest_enemy {
                        let enemy_angle = battle_core::utils::angle(&enemy.world_point(), &soldier.world_point()).0;
                        let angle_diff = (move_angle - enemy_angle).abs();
                        let mut norm_diff = angle_diff % (2.0 * std::f32::consts::PI);
                        if norm_diff > std::f32::consts::PI {
                            norm_diff = 2.0 * std::f32::consts::PI - norm_diff;
                        }
                        
                        // 이동 방향과 가장 가까운 적 방향의 차이가 60도(약 1.047 라디안) 이내면 돌격으로 간주
                        if norm_diff <= std::f32::consts::PI / 13.0 {
                            is_moving_towards_enemy = true;
                        }
                    }
                }

                if (soldier.under_fire().is_warning() || soldier.under_fire().is_danger() || soldier.under_fire().is_max()) 
                    && is_moving_towards_enemy 
                {
                    Behavior::SneakTo(paths.clone())
                } else {
                    Behavior::MoveTo(paths.clone())
                }
            }
            BehaviorMode::Vehicle => Behavior::DriveTo(paths.clone()),
        }
    }

    pub fn move_fast_behavior(&self, soldier: &Soldier, paths: &WorldPaths) -> Behavior {
        // [기획 반영: 정찰조 적 조우 시 엎드려 사격 (교전 무시 버그 수정)]
        // 빠른 기동(MoveFast) 중이더라도 일정 거리 내에 적과 조우하면 무시하지 않고 즉각 교전(EngageSoldier) 상태로 전환합니다.
        let map = self.battle_state.map();
        let soldier_grid = map.grid_point_from_world_point(&soldier.world_point());
        let current_tile_idx = (soldier_grid.y * map.width() as i32 + soldier_grid.x) as usize;
        let in_open_field = if let Some(tile) = map.terrain_tiles().get(current_tile_idx) {
            self.config.terrain_tile_opacity(&tile.type_) < 0.1
        } else { false };

        let mut is_scout = false;
        for comp in self.companies.values() {
            if comp.scout_squad == Some(soldier.squad_uuid()) {
                is_scout = true;
                break;
            }
        }

        // [Part 2: 지형 및 역할별 동적 교전 거리 적용]
        let engage_dist_limit = if is_scout {
            15 // 정찰조는 지형 불문 15m 이내 초근접 시에만 대응 사격
        } else {
            // 빠른 기동(달리기) 중이므로 관측 페널티를 고려해 일반 기동의 50m보다 약간 짧은 40m로 평야 교전 거리를 설정합니다.
            if in_open_field { 40 } else { 20 }
        };

        if let Some(opponent) = self.soldier_find_opponent_to_target(soldier, None, &ChooseMethod::RandomFromNearest) {
            let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &opponent.world_point());
            
            let under_attack = soldier.under_fire().is_warning() || soldier.under_fire().is_danger() || soldier.under_fire().is_max();
            if dist.meters() <= engage_dist_limit || (is_scout && under_attack && dist.meters() <= 40) {
                return Behavior::EngageSoldier(opponent.uuid());
            }
        }

        // [개선] 우회(사이드)나 후퇴 방향일 때는 스트레스를 받아도 신속 기동(MoveFastTo)을 유지하도록 허용합니다.
        let mut is_moving_towards_enemy = false;
        if let Some(next_point) = paths.next_point() {
            let move_angle = battle_core::utils::angle(&next_point, &soldier.world_point()).0;
            
            let mut closest_enemy = None;
            let mut min_dist = std::f32::MAX;
            for enemy in self.battle_state.soldiers().iter().filter(|s| s.side() != soldier.side() && s.alive()) {
                let edist = (enemy.world_point().to_vec2() - soldier.world_point().to_vec2()).length();
                if edist < min_dist {
                    min_dist = edist;
                    closest_enemy = Some(enemy);
                }
            }
            
            if let Some(enemy) = closest_enemy {
                let enemy_angle = battle_core::utils::angle(&enemy.world_point(), &soldier.world_point()).0;
                let angle_diff = (move_angle - enemy_angle).abs();
                let mut norm_diff = angle_diff % (2.0 * std::f32::consts::PI);
                if norm_diff > std::f32::consts::PI {
                    norm_diff = 2.0 * std::f32::consts::PI - norm_diff;
                }
                
                // 이동 방향과 가장 가까운 적 방향의 차이가 60도 이내면 적진 돌격으로 간주
                if norm_diff <= std::f32::consts::PI / 13.0 {
                    is_moving_towards_enemy = true;
                }
            }
        }

        if (soldier.under_fire().is_danger() || soldier.under_fire().is_max()) && is_moving_towards_enemy {
            Behavior::SneakTo(paths.clone())
        } else {
            Behavior::MoveFastTo(paths.clone())
        }
    }

    pub fn sneak_to_behavior(&self, _soldier: &Soldier, paths: &WorldPaths) -> Behavior {
        Behavior::SneakTo(paths.clone())
    }

    pub fn defend_behavior(&self, soldier: &Soldier, angle: &Angle) -> Behavior {
        match self.battle_state.soldier_behavior_mode(soldier) {
            BehaviorMode::Ground => {
                if let Some(opponent) = self.soldier_find_opponent_to_target(
                    soldier,
                    None,
                    &ChooseMethod::RandomFromNearest,
                ) {
                    Behavior::EngageSoldier(opponent.uuid())
                } else {
                    // [방어 진지 교대 장전 (Section Reloading) 및 예방 제압 사격]
                    // 아군이 재장전 중이면, 적이 시야에 없더라도 전방 예상 접근로에 제압 사격을 가해 화력 공백을 메꿉니다.
                    let mut is_ally_reloading = false;
                    for ally_idx in self.battle_state.squad(soldier.squad_uuid()).members() {
                        let ally = self.battle_state.soldier(*ally_idx);
                        if ally.alive() && ally.uuid() != soldier.uuid() && matches!(ally.gesture(), battle_core::behavior::gesture::Gesture::Reloading(_, _)) {
                            is_ally_reloading = true;
                            break;
                        }
                    }

                    if is_ally_reloading {
                        let mut nearest_ping = None;
                        let mut min_dist = std::f32::MAX;
                        let map = self.battle_state.map();
                        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
                            if ping_side != soldier.side() {
                                let ping_world = map.world_point_from_grid_point(*ping_grid);
                                let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &ping_world).meters() as f32;
                                // 60m 이내의 가장 가까운 위험 사로(핑) 확보
                                if dist <= 60.0 && dist < min_dist {
                                    min_dist = dist;
                                    nearest_ping = Some(ping_world);
                                }
                            }
                        }

                        if let Some(ping_target) = nearest_ping {
                            // 현재 무기가 장전되어 있다면 망설임 없이 전방 사로 제압 사격(SuppressFire)
                            if let Some(weapon) = soldier.weapon(&battle_core::entity::soldier::WeaponClass::Main) {
                                if weapon.can_fire() {
                                    return Behavior::SuppressFire(ping_target);
                                }
                            }
                        }
                    }

                    Behavior::Defend(*angle)
                }
            }
            BehaviorMode::Vehicle => {
                // FIXME BS NOW : REF_ANGLE001 refactor it
                let vehicle_index = self
                    .battle_state
                    .soldier_board(soldier.uuid())
                    .expect("Must be in vehicle according to match")
                    .0;
                if !self
                    .battle_state
                    .vehicle(vehicle_index)
                    .chassis_orientation_match(angle)
                {
                    Behavior::RotateTo(*angle)
                } else {
                    Behavior::Idle(Body::Crouched)
                }
            }
        }
    }

    pub fn hide_behavior(&self, soldier: &Soldier, angle: &Angle) -> Behavior {
        match self.battle_state.soldier_behavior_mode(soldier) {
            BehaviorMode::Ground => {
                if let Some(opponent) = self.soldier_find_opponent_to_target(
                    soldier,
                    None,
                    &ChooseMethod::RandomFromNearest,
                ) {
                    Behavior::EngageSoldier(opponent.uuid())
                } else {
                    // [은엄폐 중 교대 장전 (Section Reloading)] 
                    // Hide 상태에서도 화력망 유지를 위해 동일한 교대 제압 사격 로직을 적용합니다.
                    let mut is_ally_reloading = false;
                    for ally_idx in self.battle_state.squad(soldier.squad_uuid()).members() {
                        let ally = self.battle_state.soldier(*ally_idx);
                        if ally.alive() && ally.uuid() != soldier.uuid() && matches!(ally.gesture(), battle_core::behavior::gesture::Gesture::Reloading(_, _)) {
                            is_ally_reloading = true;
                            break;
                        }
                    }

                    if is_ally_reloading {
                        let mut nearest_ping = None;
                        let mut min_dist = std::f32::MAX;
                        let map = self.battle_state.map();
                        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
                            if ping_side != soldier.side() {
                                let ping_world = map.world_point_from_grid_point(*ping_grid);
                                let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &ping_world).meters() as f32;
                                if dist <= 60.0 && dist < min_dist {
                                    min_dist = dist;
                                    nearest_ping = Some(ping_world);
                                }
                            }
                        }

                        if let Some(ping_target) = nearest_ping {
                            if let Some(weapon) = soldier.weapon(&battle_core::entity::soldier::WeaponClass::Main) {
                                if weapon.can_fire() {
                                    return Behavior::SuppressFire(ping_target);
                                }
                            }
                        }
                    }

                    Behavior::Hide(*angle)
                }
            }
            BehaviorMode::Vehicle => {
                let vehicle_index = self
                    .battle_state
                    .soldier_board(soldier.uuid())
                    .expect("Must be in vehicle according to match")
                    .0;
                if !self
                    .battle_state
                    .vehicle(vehicle_index)
                    .chassis_orientation_match(angle)
                {
                    Behavior::RotateTo(*angle)
                } else {
                    Behavior::Idle(Body::Crouched)
                }
            }
        }
    }

    pub fn engage_behavior(&self, soldier: &Soldier, squad_index: &SquadUuid) -> Behavior {
        let opponent = soldier
            .behavior()
            .opponent()
            .map(|s| self.battle_state.soldier(*s))
            .filter(|s| s.can_be_designed_as_target())
            .or_else(|| {
                self.soldier_find_opponent_to_target(
                    soldier,
                    Some(squad_index),
                    &ChooseMethod::RandomFromNearest,
                )
            });

        if let Some(opponent) = opponent {
            return Behavior::EngageSoldier(opponent.uuid());
        }

        Behavior::Idle(Body::from_soldier(soldier, &self.battle_state))
    }

    pub fn suppress_fire_behavior(&self, _soldier: &Soldier, point: &WorldPoint) -> Behavior {
        Behavior::SuppressFire(*point)
    }
}
