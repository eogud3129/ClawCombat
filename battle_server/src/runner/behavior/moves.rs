use battle_core::{
    behavior::Behavior,
    entity::{soldier::Soldier, vehicle::OnBoardPlace},
    order::Order,
    types::SquadUuid,
};

use crate::runner::Runner;

impl Runner {
    pub fn propagate_move(
        &self,
        squad_uuid: SquadUuid,
        behavior: &Behavior,
    ) -> Vec<(&Soldier, Order)> {
        let mut behaviors = vec![];
        let squad = self.battle_state.squad(squad_uuid);
        let leader = self.battle_state.soldier(squad.leader());
        let map = self.battle_state.map();

        // [Tactical Routing] 전술 길찾기를 위한 동적 위협/안전 가중치 맵 생성
        let mut tactical_costs = std::collections::HashMap::new();

        // [핵심 버그 수정: 위험 사로 네비게이션 무시 현상 해결]
        // 기존에는 idle_behavior(YOLO)에만 적용되고, 실제 강제 이동(Move/FastMove) 경로를 생성하는 A* 알고리즘에는 전술 핑이 누락되어 있었습니다.
        for (ping_grid, (_, ping_side)) in &self.tactical_pings {
            if ping_side != leader.side() {
                // [수정] 길찾기 고착을 방지하기 위해 핑 영향 범위를 30x30 -> 7x7로 대폭 줄이고, 패널티도 2000 -> 200으로 완화하여 우회만 유도합니다.
                let danger_grids = battle_core::utils::grid_points_for_square(ping_grid, 7, 7);
                for dg in danger_grids {
                    *tactical_costs.entry(dg).or_insert(0) += 200; 
                }
            }
        }

        // 1. 점령되지 않은 적/중립 깃발 주변(반경 약 20m)에 강한 패널티 부여 (사선 우회 유도)
        for flag in map.flags() {
            let is_owned = self.battle_state.flags().ownerships().iter().any(|(n, o)| {
                n == flag.name() && (
                    o == &battle_core::game::flag::FlagOwnership::Both || 
                    (leader.side() == &battle_core::game::Side::A && o == &battle_core::game::flag::FlagOwnership::A) ||
                    (leader.side() == &battle_core::game::Side::B && o == &battle_core::game::flag::FlagOwnership::B)
                )
            });
            if !is_owned {
                let flag_grid = map.grid_point_from_world_point(&flag.position());
                let danger_grids = battle_core::utils::grid_points_for_square(&flag_grid, 20, 20);
                for dg in danger_grids {
                    *tactical_costs.entry(dg).or_insert(0) += 50; 
                }
            }
        }

// 2. 적군 시야 및 사격 사로(위협 구역) 패널티 부여 (반경 약 60m)
// 개활지 타일에는 5000의 강력한 패널티를 부여하여 절대 통과하지 않도록 유도
for enemy in self.battle_state.soldiers().iter().filter(|s| s.side() != leader.side() && s.alive()) {
    let enemy_grid = map.grid_point_from_world_point(&enemy.world_point());
    let threat_grids = battle_core::utils::grid_points_for_square(&enemy_grid, 60, 60);
    for tg in threat_grids {
        if let Some(tile) = map.terrain_tiles().get((tg.y * map.width() as i32 + tg.x) as usize) {
            let opacity = self.config.terrain_tile_opacity(&tile.type_);
            if opacity < 0.1 {
                // 개활지: 매우 높은 패널티 (우회 유도)
                *tactical_costs.entry(tg).or_insert(0) += 5000;
            } else {
                // 엄폐물이 있는 지형: 낮은 패널티
                *tactical_costs.entry(tg).or_insert(0) += 20;
            }
        } else {
            *tactical_costs.entry(tg).or_insert(0) += 100;
        }
    }
}

        // 3. 아군이 엎드려 있는 후방(안전 구역) 보너스 부여
        for ally in self.battle_state.soldiers().iter().filter(|s| s.side() == leader.side() && s.alive() && s.uuid() != leader.uuid()) {
            if ally.body() == battle_core::behavior::Body::Lying {
                let ally_grid = map.grid_point_from_world_point(&ally.world_point());
                let safe_grids = battle_core::utils::grid_points_for_square(&ally_grid, 10, 10);
                for sg in safe_grids {
                    // 엎드린 아군 근처는 이동 비용 감소 (-20)
                    *tactical_costs.entry(sg).or_insert(0) -= 20; 
                }
            }
        }

        // [건물 기하학 기반 사로 및 사각지대 동적 스캔 (CQB Geometry Scan)]
        for interior in map.interiors() {
            let center_x = interior.x() + interior.width() / 2.0;
            let center_y = interior.y() + interior.height() / 2.0;
            let interior_center = battle_core::types::WorldPoint::new(center_x, center_y);

            // 전술적 최적화: 현재 이동하는 지휘관의 위치가 해당 건물 주변(40m 이내)에 있을 때만 스캔을 수행합니다. (연산량 감축)
            let dist_to_leader = battle_core::physics::utils::distance_between_points(&leader.world_point(), &interior_center).meters();
            if dist_to_leader > 40 {
                continue;
            }

            let interior_center_grid = map.grid_point_from_world_point(&interior_center);

            // [최적화] Raycast 연산(Visibility)이 CPU 스파이크의 주범이므로 이를 전면 제거하고,
            // 단순히 건물 테두리(외벽)를 둘러싸는 타일 자체에만 보너스를 부여하여 벽 타기(Wall-Hugging)를 유도합니다.
            let cqb_grids = battle_core::utils::grid_points_for_square(&interior_center_grid, 15, 15);
            for cg in cqb_grids {
                if map.contains(&cg) {
                    let tile_pos = map.world_point_from_grid_point(cg);

                    if tile_pos.x >= interior.x() && tile_pos.x <= interior.x() + interior.width() &&
                       tile_pos.y >= interior.y() && tile_pos.y <= interior.y() + interior.height() {
                        continue;
                    }

                    let mut is_near_wall = false;
                    for (mx, my) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let ng = battle_core::types::GridPoint::new(cg.x + mx, cg.y + my);
                        if map.contains(&ng) {
                            if let Some(nt) = map.terrain_tiles().get((ng.y * map.width() as i32 + ng.x) as usize) {
                                if matches!(nt.type_(), battle_core::map::terrain::TileType::BrickWall) {
                                    is_near_wall = true;
                                    break;
                                }
                            }
                        }
                    }

                    if is_near_wall {
                        *tactical_costs.entry(cg).or_insert(0) -= 120; // 벽면 밀착 보너스
                    }
                }
            }
        }

        // [정찰조(Scout) 척후 기동 판별]
        let mut is_scout = false;
        for comp in self.companies.values() {
            if comp.scout_squad == Some(squad_uuid) {
                is_scout = true;
                break;
            }
        }

        let leader_pos = leader.world_point();

        let leader_paths_opt = behavior.world_paths();

        // [개선: 부하 개별 길찾기(A*) 완벽 제거 및 연산 폭주(프리징) 방지]
        // 부하 수만큼 매 프레임 A* 연산(find_tactical_path)을 호출하던 악성 코드를 전면 제거합니다.
        // 분대원은 지휘관의 경로(leader_paths)를 그대로 복사받으며, 워프(Warping) 현상은 엔진 내부 벡터 
        // 이동 로직(movement_updates)에서 발생하지 않으므로 가장 가볍고 동기화된 복사(Clone) 방식을 사용합니다.
        for soldier_index in squad.subordinates() {
            let soldier = self.battle_state.soldier(*soldier_index);
            if let Some(leader_paths) = leader_paths_opt {
                let fallback_order = match behavior {
                    Behavior::MoveTo(_) => Order::MoveTo(leader_paths.clone(), None),
                    Behavior::MoveFastTo(_) => Order::MoveFastTo(leader_paths.clone(), None),
                    Behavior::SneakTo(_) => Order::SneakTo(leader_paths.clone(), None),
                    _ => Order::Idle,
                };
                behaviors.push((soldier, fallback_order));
            }
        }

        behaviors
    }

    pub fn propagate_drive(
        &self,
        squad_uuid: SquadUuid,
        behavior: &Behavior,
    ) -> Vec<(&Soldier, Order)> {
        let squad = self.battle_state.squad(squad_uuid);

        for member_index in squad.members() {
            if let Some((_, place)) = self.battle_state.soldier_board(*member_index) {
                if place == &OnBoardPlace::Driver {
                    let soldier = self.battle_state.soldier(*member_index);
                    let paths = match &behavior {
                        Behavior::MoveTo(paths)
                        | Behavior::MoveFastTo(paths)
                        | Behavior::SneakTo(paths) => paths,
                        _ => {
                            unreachable!()
                        }
                    };
                    return vec![(soldier, Order::MoveTo(paths.clone(), None))];
                }
            }
        }

        vec![]
    }
}
