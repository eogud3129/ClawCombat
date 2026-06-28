use std::collections::{HashMap, HashSet};

use bresenham::Bresenham;
use glam::Vec2;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::{
    config::{ServerConfig, VISIBILITY_FIRSTS, VISIBILITY_PIXEL_STEPS},
    entity::soldier::Soldier,
    game::Side,
    map::Map,
    types::{Distance, GridPath, SoldierIndex, WorldPoint},
};

use super::utils::distance_between_points;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Visibilities {
    visibilities: HashMap<(SoldierIndex, SoldierIndex), Visibility>,
}

impl Visibilities {
    pub fn update(&mut self, value: HashMap<(SoldierIndex, SoldierIndex), Visibility>) {
        for (k, v) in value {
            self.visibilities.insert(k, v);
        }
    }

    pub fn get(&self, soldiers: &(SoldierIndex, SoldierIndex)) -> Option<&Visibility> {
        self.visibilities.get(soldiers)
    }

    pub fn visibles_soldiers_by_side(&self, side: &Side) -> Vec<SoldierIndex> {
        self.visibilities
            .values()
            .filter(|v| v.from_side == Some(*side) && v.to_soldier.is_some() && v.visible)
            .map(|v| {
                v.to_soldier
                    .expect("Previous line must test v.to_soldier.is_some()")
            })
            .collect()
    }

    pub fn visibles_soldiers_by_soldiers(&self, soldiers: Vec<SoldierIndex>) -> Vec<SoldierIndex> {
        self.visibilities
            .values()
            .filter(|v| v.from_soldier.is_some() && v.to_soldier.is_some())
            .filter(|v| soldiers.contains(&v.from_soldier.expect("Must be filtered previous line")))
            .filter(|v| v.visible)
            .map(|v| v.to_soldier.expect("Must be filtered previous line"))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn visibles_soldiers(&self) -> Vec<&Visibility> {
        self.visibilities
            .values()
            .filter(|v| v.to_soldier.is_some() && v.visible)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.visibilities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Visibility {
    pub from: WorldPoint,
    pub from_soldier: Option<SoldierIndex>,
    pub from_side: Option<Side>,
    pub to: WorldPoint,
    pub to_soldier: Option<SoldierIndex>,
    pub altered_to: WorldPoint,
    pub path_final_opacity: f32,
    pub to_scene_item_opacity: f32,
    pub opacity_segments: Vec<(WorldPoint, f32)>,
    pub visible: bool,
    /// true if something will (probably) intercept bullets
    /// before final point (wall, trunk, etc)
    pub blocked: bool,
    pub distance: Distance,
    pub break_point: Option<WorldPoint>,
}

impl Visibility {
    pub fn between_soldiers_no(from_soldier: &Soldier, to_soldier: &Soldier) -> Self {
        let from_point = from_soldier.world_point();
        let to_point = to_soldier.world_point();
        let distance =
            distance_between_points(&from_soldier.world_point(), &to_soldier.world_point());
        Self {
            from: from_point,
            from_soldier: Some(from_soldier.uuid()),
            from_side: Some(*from_soldier.side()),
            to: to_point,
            altered_to: to_point,
            to_soldier: Some(to_soldier.uuid()),
            opacity_segments: vec![],
            path_final_opacity: 999.,
            to_scene_item_opacity: 999.,
            visible: false,
            blocked: false,
            distance,
            break_point: Some(from_point),
        }
    }

    pub fn between_soldiers(
        frame_i: u64,
        config: &ServerConfig,
        from_soldier: &Soldier,
        to_soldier: &Soldier,
        map: &Map,
    ) -> Self {
        let from_point = from_soldier.world_point();
        let to_point = to_soldier.world_point();
        let last_shoot_frame_i = to_soldier.last_shoot_frame_i();

        let mut by_behavior_modifier: f32 = config.visibility_behavior_modifier(to_soldier.behavior());

        // [평야지대 은엄폐 딜레이(지연) 시스템]
        // 평야지대(투명도 0.1 미만)에서 은엄폐(Hide/Defend)를 시도할 경우 즉시 투명화되지 않고 3초(180프레임)에 걸쳐 서서히 은폐율이 오르도록 지연시킵니다.
        let is_hiding = matches!(to_soldier.behavior(), crate::behavior::Behavior::Hide(_) | crate::behavior::Behavior::Defend(_) | crate::behavior::Behavior::ScatterToCover(_) | crate::behavior::Behavior::GatherToCover(_));
        if is_hiding {
            let to_grid = map.grid_point_from_world_point(&to_point);
            let tile_idx = (to_grid.y * map.width() as i32 + to_grid.x) as usize;
            if let Some(tile) = map.terrain_tiles().get(tile_idx) {
                if config.terrain_tile_opacity(&tile.type_) < 0.1 {
                    let hide_duration = frame_i.saturating_sub(to_soldier.last_hide_frame_i());
                    let required_delay = 180; // 3초 (TARGET_FPS * 3)
                    if hide_duration < required_delay {
                        // 0% ~ 100% 진행률 계산
                        let progress = hide_duration as f32 / required_delay as f32;
                        // 원래 모디파이어(예: -0.9)에 진행률을 곱해 서서히 목표치에 도달하도록 부드럽게 보정합니다. (초기에는 0.0에 가깝게)
                        by_behavior_modifier *= progress;
                    }
                }
            }
        }

        let exclude_lasts = if last_shoot_frame_i + config.visibility_by_last_frame_shoot >= frame_i
        {
            config.visibility_by_last_frame_shoot_distance
        } else {
            0
        };

        let (
            mut to_soldier_item_opacity,
            opacity_segments,
            path_final_opacity,
            break_point,
            blocked,
            altered_to,
        ) = Self::between_points_raw(
            config,
            &from_point,
            &to_point,
            map,
            config.visibility_firsts,
            exclude_lasts,
        );

        to_soldier_item_opacity -= by_behavior_modifier;
        let visible = to_soldier_item_opacity < config.visible_starts_at;

        let distance =
            distance_between_points(&from_soldier.world_point(), &to_soldier.world_point());
        Self {
            from: from_point,
            from_soldier: Some(from_soldier.uuid()),
            from_side: Some(*from_soldier.side()),
            to: to_point,
            to_soldier: Some(to_soldier.uuid()),
            altered_to,
            opacity_segments,
            path_final_opacity,
            to_scene_item_opacity: to_soldier_item_opacity,
            visible,
            blocked,
            distance,
            break_point,
        }
    }

    pub fn between_soldier_and_point(
        config: &ServerConfig,
        from_soldier: &Soldier,
        to_point: &WorldPoint,
        map: &Map,
        exclude_lasts: usize,
    ) -> Self {
        let from_point = from_soldier.world_point();

        let (
            to_soldier_item_opacity,
            opacity_segments,
            path_final_opacity,
            break_point,
            blocked,
            altered_to,
        ) = Self::between_points_raw(
            config,
            &from_point,
            to_point,
            map,
            VISIBILITY_FIRSTS,
            exclude_lasts,
        );

        let visible = to_soldier_item_opacity < config.visible_starts_at;
        let distance = distance_between_points(&from_point, to_point);
        Self {
            from: from_point,
            from_soldier: Some(from_soldier.uuid()),
            from_side: Some(*from_soldier.side()),
            to: *to_point,
            to_soldier: None,
            altered_to,
            opacity_segments,
            path_final_opacity,
            to_scene_item_opacity: to_soldier_item_opacity,
            visible,
            blocked,
            distance,
            break_point,
        }
    }

    pub fn between_points(
        config: &ServerConfig,
        from_point: &WorldPoint,
        to_point: &WorldPoint,
        map: &Map,
    ) -> Self {
        let (
            to_soldier_item_opacity,
            opacity_segments,
            path_final_opacity,
            break_point,
            blocked,
            altered_to,
        ) = Self::between_points_raw(config, from_point, to_point, map, VISIBILITY_FIRSTS, 0);

        let visible = to_soldier_item_opacity < config.visible_starts_at;
        let distance = distance_between_points(from_point, to_point);
        Self {
            from: *from_point,
            from_soldier: None,
            from_side: None,
            to: *to_point,
            to_soldier: None,
            altered_to,
            opacity_segments,
            path_final_opacity,
            to_scene_item_opacity: to_soldier_item_opacity,
            visible,
            blocked,
            distance,
            break_point,
        }
    }

    // TODO : Optimize performances here
    pub fn between_points_raw(
        config: &ServerConfig,
        from_point: &WorldPoint,
        to_point: &WorldPoint,
        map: &Map,
        exclude_firsts: usize,
        exclude_lasts: usize,
    ) -> (
        f32,
        Vec<(WorldPoint, f32)>,
        f32,
        Option<WorldPoint>,
        bool,
        WorldPoint,
    ) {
        let mut rng = rand::thread_rng();
        let mut opacity_segments: Vec<(WorldPoint, f32)> = vec![];
        let mut path_final_opacity: f32 = 0.0;
        let mut to_opacity: f32 = 0.0;
        let mut break_point = None;
        let mut blocked = false;
        let _visible_by_bullet_fire = false;

        // Compute line pixels
        let pixels = Bresenham::new(
            (from_point.x as isize, from_point.y as isize),
            (to_point.x as isize, to_point.y as isize),
        );

        let mut grid_path: GridPath = GridPath::new();
        let mut other: Vec<(WorldPoint, f32)> = vec![];
        for (i, (pixel_x, pixel_y)) in pixels.step_by(VISIBILITY_PIXEL_STEPS).enumerate() {
            let grid_point =
                map.grid_point_from_world_point(&WorldPoint::new(pixel_x as f32, pixel_y as f32));
            if !grid_path.contains(&grid_point) {
                let terrain_tile = match map
                    .terrain_tiles()
                    .get((grid_point.y * map.width() as i32 + grid_point.x) as usize)
                {
                    Some(tile) => tile,
                    None => {
                        continue;
                    }
                };
                let mut grid_point_opacity = if grid_path.len() <= exclude_firsts {
                    0.0
                } else {
                    config.terrain_tile_opacity(&terrain_tile.type_)
                };

                // [비대칭 가시성(Asymmetric Visibility) 개편]
                // 관측자(Shooter)가 있는 시작점(exclude_firsts 이내)의 수풀/나무 불투명도는 위에서 0.0으로 완전히 상쇄되었습니다. (안에서 밖은 잘 보임)
                // 반면, 관측자의 시야를 벗어난 외부(즉, 광선이 향하는 타겟 쪽)에 수풀이나 나무가 존재한다면, 
                // 불투명도 가중치를 기존 1.5배에서 4.0배로 극단적으로 증폭시킵니다. (밖에서 안은 들여다보기 매우 힘듦)
                // 벌목된 나무(MiddleWoodLogs)는 가시거리를 전혀 방해하지 않도록 불투명도를 0.0으로 완전 무력화합니다.
                if grid_path.len() > exclude_firsts {
                    match terrain_tile.type_ {
                        crate::map::terrain::TileType::Trunk | 
                        crate::map::terrain::TileType::Underbrush | 
                        crate::map::terrain::TileType::LightUnderbrush | 
                        crate::map::terrain::TileType::Hedge => {
                            grid_point_opacity *= 4.0;
                        }
                        crate::map::terrain::TileType::MiddleWoodLogs => {
                            grid_point_opacity = 0.0;
                        }
                        _ => {}
                    }
                }

                if i >= exclude_firsts && terrain_tile.type_().block_bullet() {
                    // FIXME BS NOW: defend and move etc. must change their order only if visible and not !blocked !
                    blocked = true
                }
                grid_path.push(grid_point);
                other.push((
                    WorldPoint::new(pixel_x as f32, pixel_y as f32),
                    grid_point_opacity,
                ));
            }
        }

        let exclude_lasts = if grid_path.len() < exclude_lasts {
            grid_path.len()
        } else {
            exclude_lasts
        };
        let exclude_opacity_starts_at = grid_path.len() - exclude_lasts;
        for (i, (_, (world_point, opacity))) in grid_path.points.iter().zip(other).enumerate() {
            // Disable to_scene_item firsts if seen because firing
            let opacity = if i < exclude_opacity_starts_at {
                opacity
            } else {
                0.
            };
            path_final_opacity += opacity;
            to_opacity += opacity;
            opacity_segments.push((world_point, path_final_opacity));
            if path_final_opacity > config.visible_starts_at && break_point.is_none() {
                break_point = Some(world_point);
            }
        }

        // Compute a target point altered by opacity
        let altered_to = {
            let range = path_final_opacity * config.target_alteration_by_opacity_factor;
            if range > 0. {
                let x_change = rng.gen_range(-range..range);
                let y_change = rng.gen_range(-range..range);
                to_point.apply(Vec2::new(x_change, y_change))
            } else {
                *to_point
            }
        };

        (
            to_opacity,
            opacity_segments,
            path_final_opacity,
            break_point,
            blocked,
            altered_to,
        )
    }
}
