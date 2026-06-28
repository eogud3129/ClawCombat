use battle_core::{
    behavior::{Behavior, Body},
    order::Order,
    state::battle::message::{BattleStateMessage, SoldierMessage},
    types::{SoldierIndex, WorldPaths},
};

use super::{message::RunnerMessage, Runner};

impl Runner {
    /// 지정된 경로(WorldPaths)를 따라갈 때 소요되는 정확한 예상 시간(프레임 단위 ETA)을 계산합니다.
    /// LLM이나 AI 시스템이 행동을 예약하고 계산값을 사전 확인할 때 호출할 수 있습니다.
    pub fn compute_path_eta_frames(&self, soldier_index: SoldierIndex, path: &WorldPaths) -> u64 {
        let soldier = self.battle_state.soldier(soldier_index);
        let velocity = self
            .config
            .behavior_velocity(soldier.behavior())
            .unwrap_or(battle_core::config::MOVE_VELOCITY); // 설정된 이동/포복 속도 가져오기

        if velocity == 0.0 {
            return 0;
        }

        let mut total_distance = 0.0;
        let mut last_point = soldier.world_point().to_vec2();

        for p in &path.paths {
            for wp in &p.points {
                total_distance += last_point.distance(wp.to_vec2());
                last_point = wp.to_vec2();
            }
        }

        // 전체 거리를 매 틱당 이동하는 velocity로 나누면 정확히 엔진이 도달에 소모하는 Frame이 나옴
        (total_distance / velocity) as u64
    }

    pub fn movement_updates(
        &self,
        soldier_index: SoldierIndex,
        path: &WorldPaths,
    ) -> Vec<RunnerMessage> {
        let mut messages = vec![];
        let soldier = self.battle_state.soldier(soldier_index);
        let point = path.next_point().expect("Must have point in path");

        // There is a next point in path, go to it
        let velocity = self
            .config
            .behavior_velocity(soldier.behavior())
            .expect("Entity behavior must have velocity when move code called");
        let vector = (point.to_vec2() - soldier.world_point().to_vec2()).normalize() * velocity;

        // Point reached
        if vector.is_nan()
            || (soldier.world_point().to_vec2() - point.to_vec2()).length() <= vector.length()
        {
            // If it is the last point, move is finished
            if path.is_last_point().expect("Must contain points") {
                let (behavior, order) = if let Some(then_order) = soldier.order().then() {
                    (
                        Behavior::from_order(&then_order, soldier, &self.battle_state),
                        then_order,
                    )
                } else {
                    (
                        Behavior::Idle(Body::from_soldier(soldier, &self.battle_state)),
                        Order::Idle,
                    )
                };

                messages.extend(vec![
                    RunnerMessage::BattleState(BattleStateMessage::Soldier(
                        soldier_index,
                        SoldierMessage::SetBehavior(behavior),
                    )),
                    RunnerMessage::BattleState(BattleStateMessage::Soldier(
                        soldier_index,
                        SoldierMessage::SetOrder(order),
                    )),
                ]);
            } else {
                messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                    soldier_index,
                    SoldierMessage::ReachBehaviorStep,
                )));
            }

            // Movement required
        } else {
            let new_point = soldier.world_point().apply(vector);
            messages.push(RunnerMessage::BattleState(BattleStateMessage::Soldier(
                soldier_index,
                SoldierMessage::SetWorldPosition(new_point),
            )));
        }

        messages
    }
}
