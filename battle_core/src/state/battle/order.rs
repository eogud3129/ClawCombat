use crate::{
    game::Side,
    order::{marker::OrderMarker, Order},
    types::*,
};

use super::BattleState;

impl BattleState {
    // TODO : this func must clone things, this is not optimal
    // TODO : return type is too much complex
pub fn order_markers(
    &self,
    side: &Side,
) -> Vec<(Order, OrderMarker, SquadUuid, WorldPoint, OrderMarkerIndex)> {
    let mut marker_data = vec![];

    for (squad_id, order) in self.all_orders(side) {
        // 분대가 존재하지 않으면 건너뜀
        if !self.squads().contains_key(&squad_id) {
            continue;
        }
        let marker = order.marker();
        let squad = self.squad(squad_id);
        match &order {
            Order::MoveTo(world_paths, _)
            | Order::MoveFastTo(world_paths, _)
            | Order::SneakTo(world_paths, _) => {
                marker_data.extend::<Vec<(
                    Order,
                    OrderMarker,
                    SquadUuid,
                    WorldPoint,
                    OrderMarkerIndex,
                )>>(
                    world_paths
                        .paths
                        .iter()
                        .enumerate()
                        .map(|(i, wp)| {
                            (
                                order.clone(),
                                marker.clone().unwrap(),
                                squad_id,
                                wp.last_point().expect("Must have point here"),
                                OrderMarkerIndex(i),
                            )
                        })
                        .collect(),
                );
            }
            Order::Defend(_) | Order::Hide(_) => {
                let squad_leader = self.soldier(squad.leader());
                marker_data.push((
                    order.clone(),
                    marker.clone().unwrap(),
                    squad_id,
                    squad_leader.world_point(),
                    OrderMarkerIndex(0),
                ));
            }
            Order::Idle => {}
            Order::EngageSquad(squad_index) => {
                // 타겟 분대도 존재하는지 확인
                if !self.squads().contains_key(squad_index) {
                    continue;
                }
                let squad = self.squad(*squad_index);
                let leader = self.soldier(squad.leader());
                marker_data.push((
                    order.clone(),
                    marker.clone().unwrap(),
                    squad_id,
                    leader.world_point(),
                    OrderMarkerIndex(0),
                ));
            }
            Order::SuppressFire(point) => {
                marker_data.push((
                    order.clone(),
                    marker.clone().unwrap(),
                    squad_id,
                    *point,
                    OrderMarkerIndex(0),
                ));
            }
            Order::OffMapTransit(_) => {}
        }
    }

    marker_data
}
}
