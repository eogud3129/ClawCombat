use battle_core::{
    behavior::Behavior,
    entity::{soldier::Soldier, vehicle::OnBoardPlace},
    game::cover::CoverFinder,
    order::Order,
    types::{SquadUuid, WorldPath, WorldPaths},
    utils::NewDebugPoint,
};

use crate::runner::Runner;

impl Runner {
    pub fn propagate_defend_or_hide(
        &self,
        squad_uuid: SquadUuid,
        behavior: &Behavior,
    ) -> (Vec<(&Soldier, Order)>, Vec<NewDebugPoint>) {
        let squad = self.battle_state.squad(squad_uuid);
        let leader = self.battle_state.soldier(squad.leader());
        let mut orders = vec![];

        // In case of hide and enemy in perimeter, switch to defend
        if let Behavior::Hide(angle) = behavior {
            if self.visible_soldier_in_circle(
                &leader.world_point(),
                &self.config.hide_maximum_rayon,
                &leader.side().opposite(),
            ) {
                return (vec![(leader, Order::Defend(*angle))], vec![]);
            }
        }

        // 행동(Behavior) 종류에 따라 새로 개발한 흩어짐/뭉침 알고리즘을 분기하여 호출
        let (moves, debug_points) = match behavior {
            // (Note: Behavior 에러가 난다면, core의 Enum에 ScatterToCover, GatherToCover 추가가 필요함)
            Behavior::ScatterToCover(_) => CoverFinder::new(&self.battle_state, &self.config)
                .find_scatter_cover_points(squad, leader),
            Behavior::GatherToCover(_) => CoverFinder::new(&self.battle_state, &self.config)
                .find_gather_cover_points(squad, leader),
            _ => CoverFinder::new(&self.battle_state, &self.config)
                .find_arbitrary_cover_points(squad, leader),
        };

        for (member_id, from_world_point, cover_world_point) in &moves {
            let path = WorldPaths::new(vec![WorldPath::new(vec![
                *from_world_point,
                *cover_world_point,
            ])]);

            // 이동 후 최종 상태(then_order) 결정
            let then_order = match behavior {
                Behavior::Hide(angle) | Behavior::ScatterToCover(angle) | Behavior::GatherToCover(angle) => Order::Hide(*angle),
                Behavior::Defend(angle) => Order::Defend(*angle),
                _ => unreachable!(),
            };

            // 이동하는 방식 결정 (흩어질 땐 포복, 뭉칠 땐 빠른 달리기)
            let order = match behavior {
                Behavior::Hide(_) | Behavior::ScatterToCover(_) => Order::SneakTo(path, Some(Box::new(then_order))),
                Behavior::Defend(_) | Behavior::GatherToCover(_) => Order::MoveFastTo(path, Some(Box::new(then_order))),
                _ => unreachable!(),
            };
            orders.push((self.battle_state.soldier(*member_id), order));
        }

        (orders, debug_points)
    }

    pub fn propagate_rotate(
        &self,
        squad_uuid: SquadUuid,
        behavior: &Behavior,
    ) -> (Vec<(&Soldier, Order)>, Vec<NewDebugPoint>) {
        let squad = self.battle_state.squad(squad_uuid);

        for member_index in squad.members() {
            if let Some((_, place)) = self.battle_state.soldier_board(*member_index) {
                if place == &OnBoardPlace::Driver {
                    let soldier = self.battle_state.soldier(*member_index);
                    let order = match &behavior {
                        Behavior::Defend(angle) => Order::Defend(*angle),
                        Behavior::Hide(angle) => Order::Hide(*angle),
                        _ => {
                            unreachable!()
                        }
                    };
                    return (vec![(soldier, order)], vec![]);
                }
            }
        }

        (vec![], vec![])
    }
}
