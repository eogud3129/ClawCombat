use battle_core::{
    behavior::Behavior,
    state::battle::message::{BattleStateMessage, SoldierMessage},
    types::SoldierIndex,
};

use super::{message::RunnerMessage, Runner};

impl Runner {
    pub fn update_soldier(&self, i: SoldierIndex) -> Vec<RunnerMessage> {
        puffin::profile_scope!("update_soldier", format!("{}", i));
        let mut messages = vec![];
        let soldier = self.battle_state.soldier(i);

        // [기획 반영 2-3] 투척 딜레이 타이머가 종료되는 순간 정확하게 맵 상에 수류탄 폭발을 스폰(PushExplosion)합니다.
        if let battle_core::behavior::gesture::Gesture::Throwing(end_frame, target) = soldier.gesture() {
            if *self.battle_state.frame_i() >= *end_frame {
                messages.push(RunnerMessage::BattleState(BattleStateMessage::PushExplosion(
                    battle_core::physics::event::explosion::Explosion::new(
                        *target,
                        battle_core::game::explosive::ExplosiveType::FA19241927
                    )
                )));
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetGesture(battle_core::behavior::gesture::Gesture::Idle)
                )));
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier.uuid(),
                    SoldierMessage::SetLastGrenadeFrameI(*self.battle_state.frame_i())
                )));
            }
        }

        messages.extend(self.orientation_update(i));
        messages.extend(self.behavior_update(i));

        messages
    }

    fn orientation_update(&self, i: SoldierIndex) -> Vec<RunnerMessage> {
        let soldier = self.battle_state.soldier(i);
        let mut messages = vec![];

        // [Part 1: 기동 간 사격(Move & Shoot) 조준 방향 동기화]
        // 기동 중(MoveTo, MoveFastTo, SneakTo)일 때 전방 50m 이내에 교전 가능한 적이 있다면,
        // 이동하는 경로 방향이 아닌, 타겟(적군)을 향해 시선(총구)을 고정하여 조준 오차를 완벽히 상쇄합니다.
        let mut target_angle = None;
        
        if matches!(soldier.behavior(), battle_core::behavior::Behavior::MoveTo(_) | battle_core::behavior::Behavior::MoveFastTo(_) | battle_core::behavior::Behavior::SneakTo(_)) {
            if let Some(opponent) = self.soldier_find_opponent_to_target(soldier, None, &crate::runner::fight::choose::ChooseMethod::RandomFromNearest) {
                let dist = battle_core::physics::utils::distance_between_points(&soldier.world_point(), &opponent.world_point()).meters();
                if dist <= 50 {
                    target_angle = Some(battle_core::utils::angle(&opponent.world_point(), &soldier.world_point()));
                }
            }
        }

        let final_angle = target_angle.or_else(|| self.behavior_angle(soldier.behavior(), &soldier.world_point()));

        if let Some(angle_) = final_angle {
            let soldier_message = SoldierMessage::SetOrientation(angle_);
            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                i,
                soldier_message,
            )));
        }

        messages
    }

    fn behavior_update(&self, soldier_index: SoldierIndex) -> Vec<RunnerMessage> {
        let soldier = self.battle_state.soldier(soldier_index);
        let mut messages = vec![];

        messages.extend(match soldier.behavior() {
            Behavior::Idle(_) => {
                vec![]
            }
            Behavior::MoveTo(paths) | Behavior::MoveFastTo(paths) | Behavior::SneakTo(paths) => {
                self.movement_updates(soldier_index, paths)
            }
            Behavior::Defend(_) => {
                vec![]
            }
            Behavior::Hide(_) | Behavior::ScatterToCover(_) | Behavior::GatherToCover(_) => {
                vec![]
            }
            Behavior::DriveTo(paths) => self.drive_update(soldier_index, paths),
            Behavior::RotateTo(angle) => self.rotate_update(soldier_index, angle),
            Behavior::SuppressFire(_) => {
                vec![]
            }
            Behavior::EngageSoldier(target) => self.engage_update(&soldier_index, target),
            Behavior::Dead => vec![],
            Behavior::Unconscious => vec![],
            Behavior::OffMapTransit(return_frame) => self.off_map_update(soldier_index, *return_frame),
        });

        messages
    }

    fn off_map_update(&self, soldier_index: SoldierIndex, return_frame: u64) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        
        // [Operation Ghost - Part 4] 맵 밖으로 대피했던 유닛이 재배치 타이머가 만료되면 후방에서 재합류합니다.
        if *self.battle_state.frame_i() >= return_frame {
            let soldier = self.battle_state.soldier(soldier_index);
            let map = self.battle_state.map();
            
            // 아군 진영(후방)의 맵 경계선 근처로 스폰 좌표를 계산합니다.
            let spawn_x = if soldier.side() == &battle_core::game::Side::A { 
                50.0 
            } else { 
                map.visual_width() as f32 - 50.0 
            };
            let spawn_point = battle_core::types::WorldPoint::new(spawn_x, soldier.world_point().y);
            
            // 후방 배치 및 페널티 부여: 스트레스 40%(Warning 직전)
            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier_index,
                SoldierMessage::SetWorldPosition(spawn_point),
            )));
            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier_index,
                SoldierMessage::IncreaseUnderFire(80), 
            )));
            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier_index,
                SoldierMessage::SetOrder(battle_core::order::Order::Hide(battle_core::types::Angle(0.))),
            )));
            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier_index,
                SoldierMessage::SetBehavior(Behavior::Hide(battle_core::types::Angle(0.))),
            )));
        }

        messages
    }
}
