use crate::{
    config::{ServerConfig, COVER_DISTANCE},
    entity::soldier::Soldier,
    map::{find_arbitrary_cover_grid_point, find_arbitrary_cover_grid_points},
    physics::visibility::Visibility,
    state::battle::BattleState,
    types::{GridPoint, SoldierIndex, SquadComposition, WorldPoint},
    utils::NewDebugPoint,
};

use super::squad::{squad_positions, Formation};

pub struct CoverFinder<'a> {
    battle_state: &'a BattleState,
    config: &'a ServerConfig,
    point: Option<WorldPoint>,
    exclude_grid_points: Vec<GridPoint>,
}

impl<'a> CoverFinder<'a> {
    pub fn new(battle_state: &'a BattleState, config: &'a ServerConfig) -> Self {
        Self {
            battle_state,
            config,
            point: None,
            exclude_grid_points: vec![],
        }
    }

    pub fn point(mut self, point: Option<WorldPoint>) -> Self {
        self.point = point;
        self
    }

    pub fn exclude_grid_points(mut self, points: Vec<GridPoint>) -> Self {
        self.exclude_grid_points = points;
        self
    }

    pub fn find_arbitrary_cover_points(
        &self,
        squad: &SquadComposition,
        leader: &Soldier,
    ) -> (
        Vec<(SoldierIndex, WorldPoint, WorldPoint)>,
        Vec<NewDebugPoint>,
    ) {
        let mut moves = vec![];
        let mut already_used_cover_grid_points: Vec<GridPoint> = self.exclude_grid_points.clone();
        let mut debug_points = vec![];

        // [버그 수정: 단일 생존자 엄폐 연산 시 가중치 오염 전파 차단]
        let alive_members_count = squad.members().iter().filter(|m| m.0 < self.battle_state.soldiers().len() && self.battle_state.soldier(**m).alive()).count();
        if alive_members_count <= 1 {
            return (moves, debug_points);
        }

        for (member_id, formation_position) in
            squad_positions(squad, Formation::Line, leader, self.point)
        {
            let soldier = self.battle_state.soldier(member_id);
            let grid_point = self
                .battle_state
                .map()
                .grid_point_from_world_point(&formation_position);
            if let Some((cover_grid_point, debug_grid_points)) = find_arbitrary_cover_grid_point(
                self.config,
                &grid_point,
                self.battle_state.map(),
                &already_used_cover_grid_points,
                COVER_DISTANCE,
            ) {
                if self.config.send_debug_points {
                    for debug_grid_point in debug_grid_points.iter() {
                        debug_points.push(NewDebugPoint {
                            point: self
                                .battle_state
                                .map()
                                .world_point_from_grid_point(*debug_grid_point),
                            color: (0, 0, 255, 255),
                        })
                    }
                }

                let from_world_point = soldier.world_point();
                let cover_world_point = self
                    .battle_state
                    .map()
                    .world_point_from_grid_point(cover_grid_point);
                already_used_cover_grid_points.push(cover_grid_point);

                moves.push((member_id, from_world_point, cover_world_point));
            }
        }

        (moves, debug_points)
    }

    pub fn find_scatter_cover_points(
        &self,
        squad: &SquadComposition,
        leader: &Soldier,
    ) -> (
        Vec<(SoldierIndex, WorldPoint, WorldPoint)>,
        Vec<NewDebugPoint>,
    ) {
        let mut moves = vec![];
        let mut already_used_cover_grid_points: Vec<GridPoint> = self.exclude_grid_points.clone();
        let mut debug_points = vec![];

        // [버그 수정: 단일 생존자 산개 연산 시 가중치 오염 전파 차단]
        let alive_members_count = squad.members().iter().filter(|m| m.0 < self.battle_state.soldiers().len() && self.battle_state.soldier(**m).alive()).count();
        if alive_members_count <= 1 {
            return (moves, debug_points);
        }

        // 흩어질 때는 탐색 반경을 넓힘 (기본 COVER_DISTANCE의 3배)
        let scatter_distance = COVER_DISTANCE * 3;

        for (member_id, formation_position) in
            squad_positions(squad, Formation::Line, leader, self.point)
        {
            let soldier = self.battle_state.soldier(member_id);
            let grid_point = self
                .battle_state
                .map()
                .grid_point_from_world_point(&formation_position);
                
            if let Some((cover_grid_point, debug_grid_points)) = find_arbitrary_cover_grid_point(
                self.config,
                &grid_point,
                self.battle_state.map(),
                &already_used_cover_grid_points,
                scatter_distance,
            ) {
                if self.config.send_debug_points {
                    for debug_grid_point in debug_grid_points.iter() {
                        debug_points.push(NewDebugPoint {
                            point: self
                                .battle_state
                                .map()
                                .world_point_from_grid_point(*debug_grid_point),
                            color: (0, 0, 255, 255),
                        })
                    }
                }

                let from_world_point = soldier.world_point();
                let cover_world_point = self
                    .battle_state
                    .map()
                    .world_point_from_grid_point(cover_grid_point);
                already_used_cover_grid_points.push(cover_grid_point);

                moves.push((member_id, from_world_point, cover_world_point));
            }
        }

        (moves, debug_points)
    }

    pub fn find_gather_cover_points(
        &self,
        squad: &SquadComposition,
        leader: &Soldier,
    ) -> (
        Vec<(SoldierIndex, WorldPoint, WorldPoint)>,
        Vec<NewDebugPoint>,
    ) {
        let mut moves = vec![];
        let mut debug_points = vec![];

        let leader_grid_point = self.battle_state.map().grid_point_from_world_point(&leader.world_point());
        
        // 지휘관 근처의 가장 좋은 엄폐물 1개만 탐색
        if let Some((target_cover_grid_point, debug_grid_points)) = find_arbitrary_cover_grid_point(
            self.config,
            &leader_grid_point,
            self.battle_state.map(),
            &self.exclude_grid_points,
            COVER_DISTANCE,
        ) {
            if self.config.send_debug_points {
                for debug_grid_point in debug_grid_points.iter() {
                    debug_points.push(NewDebugPoint {
                        point: self
                            .battle_state
                            .map()
                            .world_point_from_grid_point(*debug_grid_point),
                        color: (0, 0, 255, 255),
                    })
                }
            }

            let target_cover_world_point = self.battle_state.map().world_point_from_grid_point(target_cover_grid_point);
            
            // 모든 분대원에게 동일한 타겟 엄폐물 지점 부여 (한곳으로 뭉치기)
            for member_id in squad.members() {
                let soldier = self.battle_state.soldier(*member_id);
                moves.push((*member_id, soldier.world_point(), target_cover_world_point));
            }
        }

        (moves, debug_points)
    }

    /// Search better covered position than current soldier point according to given point.
    /// Used to search a better place to hide from some shooters or find
    /// # Arguments
    ///
    /// * `soldier` - Concerned soldier
    /// * `from_point`- Point of origin to hide from
    /// * `keep_visible` - from_point must be still visible from new found point (if soldier is engaging)
    pub fn find_better_cover_point_from_point(
        &self,
        soldier: &Soldier,
        from_point: &WorldPoint,
        keep_visible: bool,
    ) -> Option<GridPoint> {
        let soldier_position = soldier.world_point();
        let soldier_grid_point = self
            .battle_state
            .map()
            .grid_point_from_world_point(&soldier_position);
        let possible_cover_grid_points = find_arbitrary_cover_grid_points(
            self.config,
            &soldier_grid_point,
            self.battle_state.map(),
            COVER_DISTANCE,
        );

        for (possible_cover_grid_point, _) in possible_cover_grid_points.iter().rev() {
            if !self.exclude_grid_points.contains(possible_cover_grid_point) {
                let possible_cover_point = self
                    .battle_state
                    .map()
                    .world_point_from_grid_point(*possible_cover_grid_point);
                if Visibility::between_points(
                    self.config,
                    &possible_cover_point,
                    from_point,
                    self.battle_state.map(),
                )
                // FIXME BS NOW : if keep_visible is false and not hided point found, take most opaque
                .visible
                    == keep_visible
                {
                    return Some(*possible_cover_grid_point);
                }
            }
        }

        None
    }
}
