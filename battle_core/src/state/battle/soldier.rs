use crate::{behavior::BehaviorMode, entity::soldier::Soldier, types::SoldierIndex};

use super::{
    message::{SideEffect, SoldierMessage},
    BattleState,
};

impl BattleState {
    pub fn react_soldier_message(
        &mut self,
        soldier_index: &SoldierIndex,
        soldier_message: &SoldierMessage,
    ) -> Vec<SideEffect> {
        let frame_i = self.frame_i;
        let soldier = &mut self.soldier_mut(*soldier_index);
        match soldier_message {
            SoldierMessage::SetWorldPosition(new_world_point) => {
                soldier.set_world_point(*new_world_point)
            }
            SoldierMessage::SetBehavior(behavior) => {
                // [평야지대 은엄폐 딜레이] 엎드리는 행동(Hide, Defend 등)으로 전환될 때 시간을 기록합니다.
                let is_now_hiding = matches!(behavior, crate::behavior::Behavior::Hide(_) | crate::behavior::Behavior::Defend(_) | crate::behavior::Behavior::ScatterToCover(_) | crate::behavior::Behavior::GatherToCover(_));
                let was_hiding = matches!(soldier.behavior(), crate::behavior::Behavior::Hide(_) | crate::behavior::Behavior::Defend(_) | crate::behavior::Behavior::ScatterToCover(_) | crate::behavior::Behavior::GatherToCover(_));
                
                if is_now_hiding && !was_hiding {
                    soldier.set_last_hide_frame_i(frame_i);
                }

                soldier.set_behavior(behavior.clone());
                return vec![SideEffect::RefreshEntityAnimation(*soldier_index)];
            }
            SoldierMessage::SetGesture(gesture) => {
                soldier.set_gesture(gesture.clone());
            }
            SoldierMessage::SetOrientation(angle) => soldier.set_looking_direction(*angle),
            SoldierMessage::ReachBehaviorStep => {
                if soldier.order_mut().reach_step() || soldier.behavior_mut().reach_step() {
                    return vec![SideEffect::SoldierFinishHisBehavior(
                        *soldier_index,
                        soldier.order().then().clone(),
                    )];
                }
            }
            SoldierMessage::SetAlive(alive) => soldier.set_alive(*alive),
            SoldierMessage::SetUnconscious(unconscious) => soldier.set_unconscious(*unconscious),
            SoldierMessage::IncreaseUnderFire(value) => {
                soldier.set_last_shot_frame_i(frame_i);
                soldier.increase_under_fire(*value);
            }
            SoldierMessage::DecreaseUnderFire => soldier.decrease_under_fire(),
            SoldierMessage::SetOrder(order) => soldier.set_order(order.clone()),
            SoldierMessage::ReloadWeapon(class) => soldier.reload_weapon(class),
            SoldierMessage::WeaponShot(class, shot) => soldier.weapon_shot(class, shot),
            SoldierMessage::SetLastShootFrameI(frame_i) => soldier.set_last_shoot_frame_i(*frame_i),
            SoldierMessage::SetLastShotFrameI(frame_i) => soldier.set_last_shot_frame_i(*frame_i),
            SoldierMessage::PromiseSupport(duration_frames) => {
                // 지원 약속 발동: 약속 시간 설정 및 스트레스 즉각 안정화 (도박 성공을 위한 일시적 버프)
                soldier.set_support_promise_end_frame_i(frame_i + *duration_frames);
                let current_stress = *soldier.under_fire().value();
                *soldier.under_fire_mut().value_mut() = current_stress.saturating_sub(100); 
            }
            SoldierMessage::CheckSupportPromise => {
                // 매 틱마다 불리는 체크 구간: 현재 프레임이 약속 기한을 넘겼다면?
                if *soldier.support_promise_end_frame_i() > 0 && frame_i >= *soldier.support_promise_end_frame_i() {
                    // 도박 실패(역풍): 스트레스를 단숨에 최대치(200)로 올리고 타이머 초기화
                    soldier.increase_under_fire(200);
                    soldier.set_support_promise_end_frame_i(0);
                }
            }
            SoldierMessage::ClearSupportPromise => {
                // 지원군이 성공적으로 도착했을 때 타이머 해제
                soldier.set_support_promise_end_frame_i(0);
            }
            SoldierMessage::RelieveStress(amount) => {
                // 특정 지역 점령 시 안도감으로 인한 스트레스 대폭 하락
                let current_stress = *soldier.under_fire().value();
                *soldier.under_fire_mut().value_mut() = current_stress.saturating_sub(*amount);
            }
            SoldierMessage::ConsumeGrenade => {
                soldier.set_grenades(soldier.grenades().saturating_sub(1));
            }
            SoldierMessage::SetLastGrenadeFrameI(frame) => {
                soldier.set_last_grenade_frame_i(*frame);
            }
            SoldierMessage::ReplenishAmmunition => {
                soldier.replenish_ammunition();
            }
            SoldierMessage::SetPlayerControlled(value) => {
                soldier.set_player_controlled(*value);
            }
        }

        vec![]
    }

    pub fn soldier_behavior_mode(&self, soldier: &Soldier) -> BehaviorMode {
        if self.soldier_board(soldier.uuid()).is_some() {
            return BehaviorMode::Vehicle;
        }
        BehaviorMode::Ground
    }
}
