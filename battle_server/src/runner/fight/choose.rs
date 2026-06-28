use rand::seq::SliceRandom;

use battle_core::{
    entity::soldier::Soldier,
    physics::utils::distance_between_points,
    state::battle::BattleState,
    types::{Distance, SoldierIndex, SquadUuid},
};

use crate::runner::Runner;

pub const NEAR_SOLDIERS_DISTANCE_METERS: i64 = 7;

pub enum ChooseMethod {
    RandomFromNearest,
}
impl ChooseMethod {
    fn choose(&self, battle_state: &BattleState, soldiers: Vec<&Soldier>) -> Option<SoldierIndex> {
        match self {
            Self::RandomFromNearest => self.choose_random_from_nearest(battle_state, soldiers),
        }
    }

    fn choose_random_from_nearest(
        &self,
        _battle_state: &BattleState,
        soldiers: Vec<&Soldier>,
    ) -> Option<SoldierIndex> {
        if let Some(soldier) = soldiers.first() {
            let soldier_position = soldier.world_point();
            let near_soldiers: Vec<&Soldier> = soldiers
                .into_iter()
                .filter(|s| {
                    distance_between_points(&soldier_position, &s.world_point())
                        < Distance::from_meters(NEAR_SOLDIERS_DISTANCE_METERS)
                })
                .collect();

            return near_soldiers
                .choose(&mut rand::thread_rng())
                .map(|s| s.uuid());
        }

        None
    }
}

impl Runner {
    // TODO : choose soldier according to distance, weapon type, etc
    // TODO : choose soldier according to other squad targets (distribution)
    // TODO : don't make it if soldier is driver, working assistant, etc
    pub fn soldier_find_opponent_to_target(
        &self,
        soldier: &Soldier,
        squad_index: Option<&SquadUuid>,
        choose_method: &ChooseMethod,
    ) -> Option<&Soldier> {
        // [시야 확장] 기존 20m로 제한되었던 주변 아군 시야 공유 및 초기 탐지 풀 반경을 60m로 대폭 확장합니다.
        // 이를 통해 평야 지대에서 원거리 교전이 정상적으로 발생할 수 있도록 탐지 가능 풀을 확보합니다.
        let around_soldiers: Vec<SoldierIndex> = self
            .battle_state
            .get_circle_side_soldiers_able_to_see(
                soldier.side(),
                &soldier.world_point(),
                &Distance::from_meters(60),
            )
            .iter()
            .map(|s| s.uuid())
            .collect();
        let mut visibles: Vec<&Soldier> = self
            .battle_state
            .visibilities()
            // FIXME BS NOW: !!! visible by near soldiers instead of all side
            .visibles_soldiers_by_soldiers(around_soldiers)
            .iter()
            .map(|s| self.battle_state.soldier(*s))
            .collect();

        visibles.retain(|s| s.can_be_designed_as_target());

        if let Some(squad_index) = squad_index {
            visibles.retain(|s| s.squad_uuid() == *squad_index)
        }

        // Why this sort ?
        // visibles.sort_by(|a, b| {
        //     a.distance
        //         .millimeters()
        //         .partial_cmp(&b.distance.millimeters())
        //         .expect("Must be i64")
        // });

        if soldier.behavior().is_hide() {
            visibles.retain(|s| {
                distance_between_points(&soldier.world_point(), &s.world_point())
                    <= self.config.hide_maximum_rayon
            })
        }

        // [상수 버그 수정 및 가시성 독립 검증 적용]
        // 주석에는 60m 제한이라고 표기하고 실제 코드 제약을 120m로 개방하여 보병들이 지나치게 먼 거리를 사격하고 있었습니다.
        // 또한 주변 아군(20m)의 탐지 공유 정보에 전적으로 의존해 본인 시야 패널티(나무/수풀)를 무시하던 현상을 교정하기 위해
        // 60m 유효 사거리 제한 및 개별 사선 가시거리 재검증 로직을 추가 주입합니다.
        visibles.retain(|s| {
            let is_within_range = distance_between_points(&soldier.world_point(), &s.world_point()).meters() <= 60;
            if is_within_range {
                let check_vis = self.battle_state.point_is_visible_by_soldier(
                    &self.config,
                    soldier,
                    &s.world_point(),
                    self.config.visibility_by_last_frame_shoot_distance
                );
                check_vis.visible
            } else {
                false
            }
        });

        choose_method
            .choose(&self.battle_state, visibles)
            .map(|i| self.battle_state.soldier(i))
    }
}
