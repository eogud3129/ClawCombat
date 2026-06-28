use battle_core::{
    behavior::{Behavior, Body},
    game::{
        flag::{FlagOwnership, FlagsOwnership},
        Side,
    },
    order::Order,
    physics::utils::distance_between_points,
    state::battle::message::{BattleStateMessage, SoldierMessage},
};

use super::{message::RunnerMessage, Runner};

impl Runner {
    pub fn tick_flags(&self) -> Vec<RunnerMessage> {
        puffin::profile_scope!("tick_flags");
        if self.battle_state.frame_i() % self.config.flags_update_freq() == 0 {
            let mut new_ownerships = vec![];
            let mut messages = vec![];
            
            for (flag_name, ownership) in self.battle_state.flags().ownerships() {
                let flag = self.battle_state.map().flag(flag_name);
                let a_inside = self
                    .battle_state
                    .there_is_side_soldier_in(&Side::A, flag.shape());
                let b_inside = self
                    .battle_state
                    .there_is_side_soldier_in(&Side::B, flag.shape());

                let new_ownership = match (ownership, a_inside, b_inside) {
                    (FlagOwnership::Nobody, true, true) => FlagOwnership::Both,
                    (FlagOwnership::Nobody, true, false) => FlagOwnership::A,
                    (FlagOwnership::Nobody, false, true) => FlagOwnership::B,
                    (FlagOwnership::Nobody, false, false) => FlagOwnership::Nobody,
                    (FlagOwnership::A, true, true) => FlagOwnership::Both,
                    (FlagOwnership::A, true, false) => FlagOwnership::A,
                    (FlagOwnership::A, false, true) => FlagOwnership::B,
                    (FlagOwnership::A, false, false) => FlagOwnership::A,
                    (FlagOwnership::B, true, true) => FlagOwnership::Both,
                    (FlagOwnership::B, true, false) => FlagOwnership::A,
                    (FlagOwnership::B, false, true) => FlagOwnership::B,
                    (FlagOwnership::B, false, false) => FlagOwnership::B,
                    (FlagOwnership::Both, true, true) => FlagOwnership::Both,
                    (FlagOwnership::Both, true, false) => FlagOwnership::A,
                    (FlagOwnership::Both, false, true) => FlagOwnership::B,
                    (FlagOwnership::Both, false, false) => FlagOwnership::Both,
                };

                if ownership != &new_ownership {
                    if new_ownership == FlagOwnership::A || new_ownership == FlagOwnership::B {
                        let capturing_side = if new_ownership == FlagOwnership::A { Side::A } else { Side::B };
                        for soldier in self.battle_state.soldiers() {
                            if soldier.side() == &capturing_side && soldier.alive() {
                                messages.push(RunnerMessage::BattleState(
                                    BattleStateMessage::Soldier(
                                        soldier.uuid(),
                                        SoldierMessage::RelieveStress(200)
                                    )
                                ));

                                // [자동 사격 해제 로직] 해당 깃발 근처를 타겟으로 제압 사격 중이던 아군은 사격을 중지하고 대기 상태로 전환
                                if let Order::SuppressFire(target_point) = soldier.order() {
                                    let dist = distance_between_points(&flag.position(), target_point);
                                    if dist.meters() <= 30 {
                                        messages.push(RunnerMessage::BattleState(
                                            BattleStateMessage::Soldier(
                                                soldier.uuid(),
                                                SoldierMessage::SetOrder(Order::Idle)
                                            )
                                        ));
                                        messages.push(RunnerMessage::BattleState(
                                            BattleStateMessage::Soldier(
                                                soldier.uuid(),
                                                SoldierMessage::SetBehavior(Behavior::Idle(Body::Crouched))
                                            )
                                        ));
                                    }
                                }

                                // [기획 반영: 깃발 점령 완료 시 체크포인트 복귀]
                                // 거점 점령에 기여한 병사(또는 깃발 반경 40m 내에 있는 해당 진영 병사)가 
                                // 사전에 저장해 둔 체크포인트(출발선)를 가지고 있다면, 즉시 후퇴하여 복귀하도록 명령합니다.
                                let dist_to_flag = distance_between_points(&flag.position(), &soldier.world_point());
                                if dist_to_flag.meters() <= 40 {
                                    if self.soldier_is_squad_leader(soldier.uuid()) {
                                        // [Phase 3: 거점 점령 시 턴 패스 지역화] 
                                        // 진영 전체(글로벌 턴)를 넘기지 않고, 거점 점령에 기여한 '해당 중대 내부의 분대 로테이션'만 교체합니다.
                                        let current_frame = *self.battle_state.frame_i();

                                        for (comp_name, comp) in &self.companies {
                                            if comp.scout_squad == Some(soldier.squad_uuid()) {
                                                let mut sorted_squads = comp.squads.clone();
                                                sorted_squads.sort_by(|a, b| a.0.cmp(&b.0));
                                                let cluster_anchor_key = format!("{}-group-{}", comp.side, sorted_squads[0].0);

                                                let mut offsets = self.scout_turn_offsets.write().unwrap();
                                                
                                                // 중대 내부 분대 턴 패스
                                                let entry = offsets.entry(cluster_anchor_key.clone()).or_insert((0, 0));
                                                if current_frame > entry.1 + 180 {
                                                    entry.0 += 1;
                                                    entry.1 = current_frame;
                                                    
                                                    // 임무를 성공적으로 완수한 분대를 블랙리스트(history)에 등록하여 독점을 차단
                                                    let mut history_guard = self.scouted_history.write().unwrap();
                                                    let current_history = history_guard.entry(cluster_anchor_key).or_insert_with(std::collections::HashSet::new);
                                                    current_history.insert(soldier.squad_uuid());

                                                    println!("[로테이션 지역화] 깃발 점령 완료! 중대 {} 내부의 정찰조를 교체합니다.", comp_name);
                                                }
                                            }
                                        }
                                    }

                                    if let Some(checkpoint_pos) = self.checkpoints.read().unwrap().get(&soldier.squad_uuid()) {
                                        // 수정: A* 연산 폭주(프리징) 방지 및 분대 결속 유지를 위해 분대장에게만 복귀 명령을 하달합니다.
                                        if self.soldier_is_squad_leader(soldier.uuid()) {
                                            let map = self.battle_state.map();
                                            let from_grid = map.grid_point_from_world_point(&soldier.world_point());
                                            let to_grid = map.grid_point_from_world_point(checkpoint_pos);
                                            
                                            if from_grid != to_grid {
                                                if let Some(grid_path) = battle_core::physics::path::find_path(
                                                    &self.config, map, &from_grid, &to_grid, true, &battle_core::physics::path::PathMode::Walk, &None
                                                ) {
                                                    let world_path = grid_path.iter().map(|p| map.world_point_from_grid_point(*p)).collect();
                                                    let paths = battle_core::types::WorldPaths::new(vec![battle_core::types::WorldPath::new(world_path)]);
                                                    
                                                    // [버그 수정: 복귀 오더 하달 시 분대 데드락(정지) 해결]
                                                    // 여기서 Behavior를 강제로 SetBehavior 해버리면 다음 프레임에서 상태 변경(Change)이 감지되지 않아
                                                    // 부하들에게 복귀 오더가 전파(Propagate)되지 않고 지휘관만 빠져나가는 심각한 고착 버그가 발생합니다.
                                                    // 오직 SetOrder만 하달하여 엔진이 자연스럽게 부하들에게 명령을 전파하도록 수정합니다.
                                                    messages.push(RunnerMessage::BattleState(
                                                        BattleStateMessage::Soldier(
                                                            soldier.uuid(),
                                                            SoldierMessage::SetOrder(Order::MoveFastTo(paths.clone(), Some(Box::new(Order::Idle))))
                                                        )
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                new_ownerships.push((flag_name.clone(), new_ownership));
            }
            
            // [수정] 깃발의 소유권이 이전과 다르게 '실제로 변경'되었을 때만 상태 변경 이벤트를 발생시킵니다.
            // 이렇게 하면 매번 tick 마다 페이즈 폴더가 무한정 생성되는 것을 방지할 수 있습니다.
            if self.battle_state.flags().ownerships() != &new_ownerships {
                messages.push(RunnerMessage::BattleState(
                    BattleStateMessage::SetFlagsOwnership(FlagsOwnership::new(new_ownerships)),
                ));
            }
            
            return messages;
        }

        vec![]
    }
}
