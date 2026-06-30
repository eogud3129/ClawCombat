use std::collections::HashMap;

use oc_core::morale::Morale;

use crate::{
    behavior::{Behavior, Body},
    deployment::{Deployment, SquadTypes},
    entity::{soldier::Soldier, vehicle::Vehicle},
    game::{control::MapControl, flag::FlagsOwnership, Side},
    graphics::vehicle::VehicleGraphicInfos,
    map::Map,
    order::Order,
    physics::{
        event::{bullet::BulletFire, cannon_blast::CannonBlast, explosion::Explosion},
        path::{Direction, PathMode},
        utils::distance_between_points,
        visibility::Visibilities,
    },
    sync::BattleStateCopy,
    types::{
        Distance, SoldierBoard, SoldierIndex, SoldiersOnBoard, SquadComposition, SquadUuid,
        VehicleBoard, VehicleIndex, WorldPoint,
    },
    utils::{vehicle_board_from_soldiers_on_board, WorldShape},
};

use self::{
    message::{BattleStateMessage, SideEffect},
    phase::Phase,
};

pub mod builder;
pub mod message;
pub mod order;
pub mod phase;
pub mod soldier;
pub mod squad;
pub mod vehicle;
pub mod visibility;

pub struct BattleState {
    frame_i: u64,
    map: Map,
    phase: Phase,
    soldiers: Vec<Soldier>,
    vehicles: Vec<Vehicle>,
    soldier_on_board: SoldiersOnBoard,
    vehicle_board: VehicleBoard,
    squads: HashMap<SquadUuid, SquadComposition>,
    squad_types: SquadTypes,
    bullet_fires: Vec<BulletFire>,
    explosions: Vec<Explosion>,
    cannon_blasts: Vec<CannonBlast>,
    visibilities: Visibilities,
    a_connected: bool,
    b_connected: bool,
    a_ready: bool,
    b_ready: bool,
    a_morale: Morale,
    b_morale: Morale,
    flags: FlagsOwnership,
}

impl BattleState {
    pub fn new(
        frame_i: u64,
        map: Map,
        soldiers: Vec<Soldier>,
        vehicles: Vec<Vehicle>,
        soldier_on_board: SoldiersOnBoard,
        squad_types: SquadTypes,
        phase: Phase,
        flags: FlagsOwnership,
    ) -> Self {
        let vehicle_board = vehicle_board_from_soldiers_on_board(&soldier_on_board);
        Self {
            frame_i,
            map,
            phase,
            soldiers,
            vehicles,
            soldier_on_board,
            vehicle_board,
            squads: HashMap::new(),
            squad_types,
            bullet_fires: vec![],
            explosions: vec![],
            cannon_blasts: vec![],
            visibilities: Visibilities::default(),
            a_connected: false,
            b_connected: false,
            a_ready: false,
            b_ready: false,
            a_morale: Morale(1.0), // FIXME BS NOW : from context ?
            b_morale: Morale(1.0), // FIXME BS NOW : from context ?
            flags,
        }
    }

    pub fn empty(map: &Map) -> Self {
        Self {
            frame_i: 0,
            map: map.clone(),
            phase: Phase::Placement,
            soldiers: vec![],
            vehicles: vec![],
            soldier_on_board: HashMap::new(),
            vehicle_board: HashMap::new(),
            squads: HashMap::new(),
            squad_types: SquadTypes::new(),
            bullet_fires: vec![],
            explosions: vec![],
            cannon_blasts: vec![],
            visibilities: Visibilities::default(),
            a_connected: false, // TODO : should be in (server) Runner ?
            b_connected: false, // TODO : should be in (server) Runner ?
            a_ready: false,
            b_ready: false,
            a_morale: Morale(1.0),
            b_morale: Morale(1.0),
            flags: FlagsOwnership::empty(),
        }
    }

    pub fn from_copy(copy: &BattleStateCopy, map: &Map) -> Self {
        Self::new(
            copy.frame_i(),
            map.clone(),
            copy.soldiers().clone(),
            copy.vehicles().clone(),
            copy.soldier_on_board().clone(),
            copy.squad_types().clone(),
            copy.phase().clone(),
            copy.flags().clone(),
        )
    }

    pub fn resolve(&mut self) {
        // At start point, squads have not been defined. We must initialize it.
        self.update_squads();
        self.check_board_integrity()
            .expect("Error with board integrity imply programmatic error");
        self.initialize_vehicle_positions();
    }

    pub fn clean(&mut self, replaced_frame_i: Option<u64>) {
        let frame_i = replaced_frame_i.unwrap_or(self.frame_i);
        self.bullet_fires.retain(|b| !b.finished(frame_i));
        self.explosions.retain(|e| !e.finished(frame_i));
        self.cannon_blasts.retain(|b| !b.finished(frame_i));
    }

    pub fn frame_i(&self) -> &u64 {
        &self.frame_i
    }

    pub fn map(&self) -> &Map {
        &self.map
    }

    pub fn visibilities(&self) -> &Visibilities {
        &self.visibilities
    }

    pub fn soldiers(&self) -> &Vec<Soldier> {
        &self.soldiers
    }

    pub fn soldier(&self, soldier_index: SoldierIndex) -> &Soldier {
        &self.soldiers[soldier_index.0]
    }

    pub fn soldier_mut(&mut self, soldier_index: SoldierIndex) -> &mut Soldier {
        &mut self.soldiers[soldier_index.0]
    }

    pub fn vehicle(&self, vehicle_index: VehicleIndex) -> &Vehicle {
        &self.vehicles[vehicle_index.0]
    }

    pub fn vehicles(&self) -> &Vec<Vehicle> {
        &self.vehicles
    }

    pub fn vehicle_mut(&mut self, vehicle_index: VehicleIndex) -> &mut Vehicle {
        &mut self.vehicles[vehicle_index.0]
    }

    pub fn squads(&self) -> &HashMap<SquadUuid, SquadComposition> {
        &self.squads
    }

    pub fn set_squads(&mut self, squads: HashMap<SquadUuid, SquadComposition>) {
        self.squads = squads;
    }

    pub fn all_orders(&self, side: &Side) -> Vec<(SquadUuid, &Order)> {
        let mut orders: Vec<(SquadUuid, &Order)> = vec![];

        for (squad_uuid, squad_composition) in &self.squads {
            if side != &Side::All && self.squad_side(squad_uuid) != side {
                continue;
            }

            let squad_leader = self.soldier(squad_composition.leader());
            orders.push((*squad_uuid, squad_leader.order()));
        }

        orders
    }

    pub fn squad_side(&self, squad_uuid: &SquadUuid) -> &Side {
        let composition = self.squad(*squad_uuid);
        let squad_leader = self.soldier(composition.leader());
        squad_leader.side()
    }

    pub fn squad(&self, squad_uuid: SquadUuid) -> &SquadComposition {
        self.squads
            .get(&squad_uuid)
            .expect("Game shared_state should never own inconsistent squad index")
    }

    pub fn bullet_fires(&self) -> &Vec<BulletFire> {
        self.bullet_fires.as_ref()
    }

    pub fn explosions(&self) -> &Vec<Explosion> {
        self.explosions.as_ref()
    }

    pub fn cannon_blasts(&self) -> &Vec<CannonBlast> {
        self.cannon_blasts.as_ref()
    }

    pub fn soldier_on_board(&self) -> &SoldiersOnBoard {
        &self.soldier_on_board
    }

    pub fn soldier_board(&self, soldier_index: SoldierIndex) -> Option<&SoldierBoard> {
        self.soldier_on_board.get(&soldier_index)
    }

    pub fn soldier_vehicle(&self, soldier_index: SoldierIndex) -> Option<VehicleIndex> {
        if let Some(soldier_board) = self.soldier_board(soldier_index) {
            return Some(soldier_board.0);
        }

        None
    }

    pub fn squad_path_mode_and_direction(
        &self,
        squad_id: SquadUuid,
    ) -> (PathMode, Option<Direction>) {
        let squad_leader_index = self.squad(squad_id).leader();
        if let Some(vehicle_index) = self.soldier_vehicle(squad_leader_index) {
            let vehicle = self.vehicle(vehicle_index);
            (
                PathMode::Drive(*VehicleGraphicInfos::from_type(vehicle.type_()).size()),
                Some(Direction::from_angle(vehicle.chassis_orientation())),
            )
        } else {
            (PathMode::Walk, None)
        }
    }

    pub fn vehicle_board(&self) -> &VehicleBoard {
        &self.vehicle_board
    }

    pub fn react(&mut self, state_message: &BattleStateMessage, frame_i: u64) -> Vec<SideEffect> {
        match state_message {
            BattleStateMessage::IncrementFrameI => self.frame_i += 1,
            BattleStateMessage::Soldier(soldier_index, soldier_message) => {
                return self.react_soldier_message(soldier_index, soldier_message);
            }
            BattleStateMessage::Vehicle(vehicle_index, vehicle_message) => {
                return self.react_vehicle_message(vehicle_index, vehicle_message);
            }
            BattleStateMessage::PushBulletFire(bullet_fire) => {
                let mut bullet_fire = bullet_fire.clone();
                bullet_fire.init(frame_i + 1);
                self.bullet_fires.push(bullet_fire)
            }
            BattleStateMessage::PushExplosion(explosion) => {
                let mut explosion = explosion.clone();
                explosion.init(frame_i + 1);
                self.explosions.push(explosion)
            }
            BattleStateMessage::PushCannonBlast(cannon_blast) => {
                let mut cannon_blast = cannon_blast.clone();
                cannon_blast.init(frame_i + 1);
                self.cannon_blasts.push(cannon_blast)
            }
            BattleStateMessage::SetVisibilities(visibilities) => {
                self.visibilities.update(visibilities.clone())
            }
            BattleStateMessage::SetPhase(phase) => self.phase = phase.clone(),
            BattleStateMessage::SetAConnected(value) => self.a_connected = *value,
            BattleStateMessage::SetBConnected(value) => self.b_connected = *value,
            BattleStateMessage::SetAReady(value) => self.a_ready = *value,
            BattleStateMessage::SetBReady(value) => self.b_ready = *value,
            BattleStateMessage::SetFlagsOwnership(flags) => self.flags = flags.clone(),
            BattleStateMessage::SetAMorale(morale) => self.a_morale = morale.clone(),
            BattleStateMessage::SetBMorale(morale) => self.b_morale = morale.clone(),
            BattleStateMessage::SetSquadLeader(squad_uuid, soldier_index) => {
                let mut side = Side::A;
                let mut members_to_move = vec![];
                let mut has_squad = false;

                if let Some(squad_comp) = self.squads.get(squad_uuid) {
                    has_squad = true;
                    if let Some(first_member) = squad_comp.members().first() {
                        if first_member.0 < self.soldiers.len() {
                            side = *self.soldiers[first_member.0].side();
                        }
                    }
                    members_to_move = squad_comp.members().clone();
                }

                if has_squad {
                    // [Part 4 개선: 불필요한 강제 복귀 명령 주입 제거 및 명령 클리어]
                    // 스폰 지점 복귀(SneakTo)를 임의로 주입하면 합류할 분대의 명령과 충돌하여 병사가 멈춰 서는 디더링이 발생합니다.
                    // 기존 행동을 깔끔하게 지우고 새 지휘관의 명령을 온전히 받을 준비를 시킵니다.
                    for member_idx in &members_to_move {
                        if member_idx.0 < self.soldiers.len() {
                            let member = &mut self.soldiers[member_idx.0];
                            if member.alive() {
                                member.set_order(Order::Idle);
                                member.set_behavior(Behavior::Idle(Body::StandUp));
                                member.set_gesture(crate::behavior::gesture::Gesture::Idle);
                            }
                        }
                    }

                    // 2. 살아있는 다른 대상 분대를 탐색합니다. (self.squads 불변 참조)
                    let mut target_squad_uuid = None;
                    for (other_uuid, other_squad) in &self.squads {
                        if other_uuid != squad_uuid {
                            let mut has_alive_leader = false;
                            let leader_idx = other_squad.leader();
                            if leader_idx.0 < self.soldiers.len() && self.soldiers[leader_idx.0].alive() && *self.soldiers[leader_idx.0].side() == side {
                                has_alive_leader = true;
                            }
                            if has_alive_leader {
                                target_squad_uuid = Some(*other_uuid);
                                break;
                            }
                        }
                    }

                    // 3. 탐색된 대상 분대로 병합하거나 지휘권을 이양합니다.
                    if let Some(next_squad_id) = target_squad_uuid {
                        if let Some(next_squad) = self.squads.get_mut(&next_squad_id) {
                            let next_leader_order = self.soldiers[next_squad.leader().0].order().clone();
                            let next_leader_behavior = self.soldiers[next_squad.leader().0].behavior().clone();
                            for m in members_to_move {
                                if m.0 < self.soldiers.len() && self.soldiers[m.0].alive() {
                                    // [중복 병합 방지 로직 보강] 중복 검사를 거쳐 안전하게 합류
                                    if !next_squad.members().contains(&m) {
                                        next_squad.members_mut().push(m);
                                    }
                                    self.soldiers[m.0].set_squad_uuid(next_squad_id);
                                    self.soldiers[m.0].set_order(next_leader_order.clone());
                                    self.soldiers[m.0].set_behavior(next_leader_behavior.clone());
                                }
                            }
                        }
                        
                        // [Part 4 개선: 분대 병합 시 기존 분대 완전 삭제]
                        // 멤버만 비우고 맵에 남겨두는 방식은 루프 안정성과 정찰조 유령 호출을 야기합니다.
                        // 병합 완료 후 기존 깡통 분대를 squads 맵에서 완전히 제거하여 소멸시킵니다.
                        self.squads.remove(squad_uuid);
                    } else {
                        if let Some(squad_comp) = self.squads.get_mut(squad_uuid) {
                            *squad_comp.leader_mut() = *soldier_index;
                            let leader_order = self.soldiers[soldier_index.0].order().clone();
                            let leader_behavior = self.soldiers[soldier_index.0].behavior().clone();
                            let member_copies = squad_comp.members().clone();
                            for m in member_copies {
                                if m.0 < self.soldiers.len() && self.soldiers[m.0].alive() {
                                    self.soldiers[m.0].set_order(leader_order.clone());
                                    self.soldiers[m.0].set_behavior(leader_behavior.clone());
                                }
                            }
                        }
                    }
                }
            }
        };

        vec![]
    }

    pub fn inject(&mut self, deployment: &Deployment) {
        for soldier_deployment in deployment.soldiers() {
            self.soldiers.push(Soldier::from(soldier_deployment))
        }
        for vehicle_deployment in deployment.vehicles() {
            self.vehicles.push(Vehicle::from(vehicle_deployment))
        }
        self.soldier_on_board = deployment.boards().clone();
        self.squad_types = deployment.squad_types().clone();
        self.resolve();
    }

    pub fn debug_lines(&self) -> Vec<(String, String)> {
        vec![
            (
                "Soldiers (len)".to_string(),
                self.soldiers.len().to_string(),
            ),
            ("Squads (len)".to_string(), self.squads.len().to_string()),
            (
                "Vehicles (len)".to_string(),
                self.vehicles.len().to_string(),
            ),
        ]
    }

    pub fn copy(&self) -> BattleStateCopy {
        BattleStateCopy::new(
            self.frame_i,
            self.soldiers.clone(),
            self.vehicles.clone(),
            self.soldier_on_board.clone(),
            self.squad_types.clone(),
            self.phase.clone(),
            self.flags.clone(),
        )
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn phase_mut(&mut self) -> &mut Phase {
        &mut self.phase
    }

    pub fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    pub fn a_connected(&self) -> bool {
        self.a_connected
    }

    pub fn b_connected(&self) -> bool {
        self.b_connected
    }

    pub fn a_ready(&self) -> bool {
        self.a_ready
    }

    pub fn b_ready(&self) -> bool {
        self.b_ready
    }

    pub fn ready(&self, side: &Side) -> bool {
        match side {
            Side::A => self.a_ready,
            Side::B => self.b_ready,
            Side::All => panic!("Never call ready for Side::All"),
        }
    }

    pub fn update_flags_from_control(&mut self, a_control: MapControl, b_control: MapControl) {
        self.flags = FlagsOwnership::from_control(&self.map, &a_control, &b_control);
    }

    pub fn flags(&self) -> &FlagsOwnership {
        &self.flags
    }

    pub fn there_is_side_soldier_in(&self, side: &Side, shape: WorldShape) -> bool {
        self.soldiers
            .iter()
            .filter(|s| s.side() == side)
            .filter(|s| s.can_take_flag())
            .any(|s| shape.contains(&s.world_point()))
    }

    pub fn a_morale(&self) -> &Morale {
        &self.a_morale
    }

    pub fn b_morale(&self) -> &Morale {
        &self.b_morale
    }

    pub fn get_circle_side_soldiers_able_to_see(
        &self,
        side: &Side,
        point: &WorldPoint,
        distance: &Distance,
    ) -> Vec<&Soldier> {
        self.soldiers
            .iter()
            .filter(|s| s.can_seek())
            .filter(|s| s.side() == side)
            .filter(|s| {
                distance_between_points(&s.world_point(), &point).millimeters()
                    <= distance.millimeters()
            })
            .collect()
    }
}

#[derive(Debug)]
pub enum BattleStateError {
    BoardIntegrity(String),
}
