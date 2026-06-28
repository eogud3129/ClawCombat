use crate::{config::ServerConfig, map::Map, types::*, utils::angleg};
use pathfinding::prelude::astar;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

pub enum PathMode {
    Walk,
    Drive(VehicleSize),
}
impl PathMode {
    pub fn include_vehicles(&self) -> bool {
        match self {
            PathMode::Walk => false,
            PathMode::Drive(_) => true,
        }
    }
}

pub const COST_AHEAD: i32 = 0;
pub const COST_DIAGONAL: i32 = 10;
pub const COST_CORNER: i32 = 20;
pub const COST_BACK_CORNER: i32 = 30;
pub const COST_BACK: i32 = 50;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, EnumIter)]
pub enum Direction {
    North,
    NorthEst,
    Est,
    SouthEst,
    South,
    SouthWest,
    West,
    NorthWest,
}

impl Direction {
    pub fn from_angle(angle: &Angle) -> Self {
        let degrees = angle.0.to_degrees();
        if degrees >= 337.5 || degrees <= 22.5 {
            Self::North
        } else if degrees > 22.5 && degrees <= 67.5 {
            Self::NorthEst
        } else if degrees > 67.5 && degrees <= 112.5 {
            Self::Est
        } else if degrees > 112.5 && degrees <= 157.5 {
            Self::SouthEst
        } else if degrees > 157.5 && degrees <= 202.5 {
            Self::South
        } else if degrees > 202.5 && degrees <= 247.5 {
            Self::SouthWest
        } else if degrees > 247.5 && degrees <= 292.5 {
            Self::West
        } else {
            Self::NorthWest
        }
    }

    pub fn modifier(&self) -> (i32, i32) {
        match self {
            Direction::NorthWest => (-1, -1),
            Direction::North => (0, -1),
            Direction::NorthEst => (1, -1),
            Direction::Est => (1, 0),
            Direction::SouthEst => (1, 1),
            Direction::South => (0, 1),
            Direction::SouthWest => (-1, 1),
            Direction::West => (-1, 0),
        }
    }

    pub fn angle_cost(&self, direction: &Direction) -> i32 {
        match self {
            Direction::North => match direction {
                Direction::North => COST_AHEAD,
                Direction::NorthEst => COST_DIAGONAL,
                Direction::Est => COST_CORNER,
                Direction::SouthEst => COST_BACK_CORNER,
                Direction::South => COST_BACK,
                Direction::SouthWest => COST_BACK_CORNER,
                Direction::West => COST_CORNER,
                Direction::NorthWest => COST_DIAGONAL,
            },
            Direction::NorthEst => match direction {
                Direction::North => COST_DIAGONAL,
                Direction::NorthEst => COST_AHEAD,
                Direction::Est => COST_DIAGONAL,
                Direction::SouthEst => COST_CORNER,
                Direction::South => COST_BACK_CORNER,
                Direction::SouthWest => COST_BACK,
                Direction::West => COST_BACK_CORNER,
                Direction::NorthWest => COST_CORNER,
            },
            Direction::Est => match direction {
                Direction::North => COST_CORNER,
                Direction::NorthEst => COST_DIAGONAL,
                Direction::Est => COST_AHEAD,
                Direction::SouthEst => COST_DIAGONAL,
                Direction::South => COST_CORNER,
                Direction::SouthWest => COST_BACK_CORNER,
                Direction::West => COST_BACK,
                Direction::NorthWest => COST_BACK_CORNER,
            },
            Direction::SouthEst => match direction {
                Direction::North => COST_BACK_CORNER,
                Direction::NorthEst => COST_CORNER,
                Direction::Est => COST_DIAGONAL,
                Direction::SouthEst => COST_AHEAD,
                Direction::South => COST_DIAGONAL,
                Direction::SouthWest => COST_CORNER,
                Direction::West => COST_BACK_CORNER,
                Direction::NorthWest => COST_BACK,
            },
            Direction::South => match direction {
                Direction::North => COST_BACK,
                Direction::NorthEst => COST_BACK_CORNER,
                Direction::Est => COST_CORNER,
                Direction::SouthEst => COST_DIAGONAL,
                Direction::South => COST_AHEAD,
                Direction::SouthWest => COST_DIAGONAL,
                Direction::West => COST_CORNER,
                Direction::NorthWest => COST_BACK_CORNER,
            },
            Direction::SouthWest => match direction {
                Direction::North => COST_BACK_CORNER,
                Direction::NorthEst => COST_BACK,
                Direction::Est => COST_BACK_CORNER,
                Direction::SouthEst => COST_CORNER,
                Direction::South => COST_DIAGONAL,
                Direction::SouthWest => COST_AHEAD,
                Direction::West => COST_DIAGONAL,
                Direction::NorthWest => COST_CORNER,
            },
            Direction::West => match direction {
                Direction::North => COST_CORNER,
                Direction::NorthEst => COST_BACK_CORNER,
                Direction::Est => COST_BACK,
                Direction::SouthEst => COST_BACK_CORNER,
                Direction::South => COST_CORNER,
                Direction::SouthWest => COST_DIAGONAL,
                Direction::West => COST_AHEAD,
                Direction::NorthWest => COST_DIAGONAL,
            },
            Direction::NorthWest => match direction {
                Direction::North => COST_DIAGONAL,
                Direction::NorthEst => COST_CORNER,
                Direction::Est => COST_BACK_CORNER,
                Direction::SouthEst => COST_BACK,
                Direction::South => COST_BACK_CORNER,
                Direction::SouthWest => COST_CORNER,
                Direction::West => COST_DIAGONAL,
                Direction::NorthWest => COST_AHEAD,
            },
        }
    }
}

// TODO : When "to" is unreachable (ex. for vehicle) do not search a path (it consume all path before stop)
pub fn find_path(
    config: &ServerConfig,
    map: &Map,
    from: &GridPoint,
    to: &GridPoint,
    exclude_first: bool,
    path_mode: &PathMode,
    start_direction: &Option<Direction>,
) -> Option<Vec<GridPoint>> {
    if !map.contains(from) || !map.contains(to) {
        return None;
    }
    let start_direction = start_direction.unwrap_or(Direction::from_angle(&angleg(to, from)));

    match astar(
        &(*from, start_direction),
        |p| {
            let mut successors = map.successors(p, path_mode);
            // [Phase 2] 엄폐물 자석 효과 상시화 (Magnet Effect)
            for (succ_node, cost) in successors.iter_mut() {
                let mut near_cover_bonus = 0;
                for (mx, my) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let neighbor_grid = GridPoint::new(succ_node.0.x + mx, succ_node.0.y + my);
                    if map.contains(&neighbor_grid) {
                        if let Some(n_tile) = map.terrain_tiles().get((neighbor_grid.y * map.width() as i32 + neighbor_grid.x) as usize) {
                            match n_tile.type_() {
                                crate::map::terrain::TileType::BrickWall |
                                crate::map::terrain::TileType::Trunk |
                                crate::map::terrain::TileType::MiddleRock |
                                crate::map::terrain::TileType::Hedge => {
                                    near_cover_bonus = 5; // [수정] 무리한 우회를 막기 위해 자석 효과를 40 -> 5로 축소합니다.
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                *cost = (*cost - near_cover_bonus).max(1);
            }
            successors
        },
        |p| {
            (p.0.to_vec2().distance(to.to_vec2()) * config.path_finding_heuristic_coefficient)
                as i32
        },
        |p| p.0 == *to,
    ) {
        None => None,
        Some(path) => {
            if exclude_first {
                let new_path = path.0[1..].to_vec();
                if !new_path.is_empty() {
                    Some(new_path.iter().map(|x| x.0).collect())
                } else {
                    None
                }
            } else {
                Some(path.0.iter().map(|x| x.0).collect())
            }
        }
    }
}

pub fn find_tactical_path(
    config: &ServerConfig,
    map: &Map,
    from: &GridPoint,
    to: &GridPoint,
    exclude_first: bool,
    path_mode: &PathMode,
    start_direction: &Option<Direction>,
    tactical_costs: &std::collections::HashMap<GridPoint, i32>,
) -> Option<Vec<GridPoint>> {
    if !map.contains(from) || !map.contains(to) {
        return None;
    }
    let start_direction = start_direction.unwrap_or(Direction::from_angle(&angleg(to, from)));

    match astar(
        &(*from, start_direction),
        |p| {
            let mut successors = map.successors(p, path_mode);
            for (succ_node, cost) in successors.iter_mut() {
                // [Phase 2] 엄폐물 자석 효과 상시화 (Magnet Effect)
                let mut near_cover_bonus = 0;
                for (mx, my) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let neighbor_grid = GridPoint::new(succ_node.0.x + mx, succ_node.0.y + my);
                    if map.contains(&neighbor_grid) {
                        if let Some(n_tile) = map.terrain_tiles().get((neighbor_grid.y * map.width() as i32 + neighbor_grid.x) as usize) {
                            match n_tile.type_() {
                                crate::map::terrain::TileType::BrickWall |
                                crate::map::terrain::TileType::Trunk |
                                crate::map::terrain::TileType::MiddleRock |
                                crate::map::terrain::TileType::Hedge => {
                                    near_cover_bonus = 5; // [수정] 무리한 우회를 막기 위해 자석 효과를 40 -> 5로 축소합니다.
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                
                let mut final_cost = *cost - near_cover_bonus;

                // 전술적 위협/안전 가중치 맵을 조회하여 타일 이동 비용을 동적으로 증감합니다.
                if let Some(extra_cost) = tactical_costs.get(&succ_node.0) {
                    final_cost += extra_cost;
                }
                *cost = final_cost.max(1); // 이동 비용은 A* 무한 루프 방지를 위해 최소 1로 보정
            }
            // [위험 사로 진입 원천 차단 및 연산 지연(느려짐) 해결]
            // 맵 전체가 고착되는 것을 막기 위해 하드 블록 비용 컷을 2000에서 10000으로 올려 길 자체는 뚫릴 수 있게 허용합니다.
            successors.retain(|(_, cost)| *cost < 10000);
            successors
        },
        |p| {
            (p.0.to_vec2().distance(to.to_vec2()) * config.path_finding_heuristic_coefficient)
                as i32
        },
        |p| p.0 == *to,
    ) {
        None => None,
        Some(path) => {
            if exclude_first {
                let new_path = path.0[1..].to_vec();
                if !new_path.is_empty() {
                    Some(new_path.iter().map(|x| x.0).collect())
                } else {
                    None
                }
            } else {
                Some(path.0.iter().map(|x| x.0).collect())
            }
        }
    }
}

pub fn find_stealth_path(
    config: &ServerConfig,
    map: &Map,
    from: &GridPoint,
    to: &GridPoint,
    exclude_first: bool,
    path_mode: &PathMode,
    start_direction: &Option<Direction>,
) -> Option<Vec<GridPoint>> {
    if !map.contains(from) || !map.contains(to) {
        return None;
    }
    let start_direction = start_direction.unwrap_or(Direction::from_angle(&angleg(to, from)));

    match astar(
        &(*from, start_direction),
        |p| {
            let mut successors = map.successors(p, path_mode);
            for (succ_node, cost) in successors.iter_mut() {
                // [Phase 2] 엄폐물 자석 효과 상시화 (Magnet Effect)
                let mut near_cover_bonus = 0;
                for (mx, my) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let neighbor_grid = GridPoint::new(succ_node.0.x + mx, succ_node.0.y + my);
                    if map.contains(&neighbor_grid) {
                        if let Some(n_tile) = map.terrain_tiles().get((neighbor_grid.y * map.width() as i32 + neighbor_grid.x) as usize) {
                            match n_tile.type_() {
                                crate::map::terrain::TileType::BrickWall |
                                crate::map::terrain::TileType::Trunk |
                                crate::map::terrain::TileType::MiddleRock |
                                crate::map::terrain::TileType::Hedge => {
                                    near_cover_bonus = 40;
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                
                let tile_idx = (succ_node.0.y * map.width() as i32 + succ_node.0.x) as usize;
                let mut final_cost = *cost - near_cover_bonus;

                if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                    match tile.type_() {
                        crate::map::terrain::TileType::Underbrush |
                        crate::map::terrain::TileType::LightUnderbrush |
                        crate::map::terrain::TileType::Hedge |
                        crate::map::terrain::TileType::MiddleWoodLogs |
                        crate::map::terrain::TileType::Trunk => {
                            // [Operation Ghost - Part 3] 스텔스 우회 가중치
                            // 수풀이나 나무 타일의 비용을 1(최솟값)로 덮어씌워 마이너스 가중치(자석 효과)를 발휘합니다.
                            final_cost = 1;
                        }
                        crate::map::terrain::TileType::ShortGrass |
                        crate::map::terrain::TileType::Concrete |
                        crate::map::terrain::TileType::Dirt => {
                            // 평야 지대의 비용을 극단적으로 올려 AI가 철저히 기피하도록 설정합니다.
                            final_cost += 500;
                        }
                        _ => {}
                    }
                }
                *cost = final_cost.max(1);
            }
            successors
        },
        |p| {
            (p.0.to_vec2().distance(to.to_vec2()) * config.path_finding_heuristic_coefficient)
                as i32
        },
        |p| p.0 == *to,
    ) {
        None => None,
        Some(path) => {
            if exclude_first {
                let new_path = path.0[1..].to_vec();
                if !new_path.is_empty() {
                    Some(new_path.iter().map(|x| x.0).collect())
                } else {
                    None
                }
            } else {
                Some(path.0.iter().map(|x| x.0).collect())
            }
        }
    }
}
